//! Minimal asynchronous LSP client.
//!
//! Spawns a language server as a child process, speaks JSON-RPC 2.0 over
//! stdio with `Content-Length`-framed messages, and exposes the small slice
//! of the protocol we need for the MCP bridge:
//!
//! - `initialize` / `initialized` handshake
//! - `textDocument/didOpen` / `didChange` / `didSave` notifications
//! - `textDocument/references` request
//! - `textDocument/hover` request
//! - cache of `textDocument/publishDiagnostics` notifications, queryable by
//!   file or workspace-wide
//! - `$/progress` tracking, so a caller can wait for rust-analyzer's flycheck
//!   (`cargo check`) to settle before reading diagnostics
//! - `rust-analyzer/runFlycheck` to trigger that check on demand
//! - `shutdown` / `exit` for clean teardown
//!
//! The client is intentionally narrow — anything else, layer on top.

#![forbid(unsafe_code)]

mod language;
mod transport;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use lsp_types::{
    ClientCapabilities, Diagnostic, DidChangeConfigurationParams, DidChangeTextDocumentParams,
    DidChangeWorkspaceFoldersParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams, Hover,
    HoverContents, HoverParams, InitializeParams, InitializeResult, InitializedParams, Location,
    MarkedString, MarkupContent, PartialResultParams, Position, ReferenceContext, ReferenceParams,
    TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, VersionedTextDocumentIdentifier, WindowClientCapabilities,
    WorkDoneProgressParams, WorkspaceClientCapabilities, WorkspaceFolder,
    WorkspaceFoldersChangeEvent,
};
use oxyris_procutil::HideConsole;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc, oneshot, watch};

pub use language::{LspLanguage, detect_languages, resolve_server};
pub use lsp_types;
pub use transport::{OutboundFrame, ProgressSnapshot};

#[derive(Debug, Error)]
pub enum LspError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("language server is not installed: {0}")]
    NotInstalled(String),
    #[error("language server crashed or stdio closed")]
    ServerGone,
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("server reported error code {code}: {message}")]
    Server { code: i64, message: String },
    #[error("invalid path for LSP uri: {0}")]
    InvalidPath(String),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("timeout after {0:?} on {1}")]
    Timeout(Duration, &'static str),
}

/// Generous default for cold-start operations (initial handshake, first
/// hover after rust-analyzer is still indexing). User-facing tools that
/// hit a frozen server should fail clearly instead of looking hung.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(90);

type Result<T> = std::result::Result<T, LspError>;

/// How long we wait for a triggered flycheck to *start* before concluding the
/// server isn't going to run one (not a Cargo project, check disabled, …).
const FLYCHECK_START_GRACE: Duration = Duration::from_secs(3);
/// Ceiling on a single `cargo check` we're blocking a tool call on. A cold
/// workspace can exceed this; the caller reports what it has and says so
/// rather than hanging the agent.
const FLYCHECK_TIMEOUT: Duration = Duration::from_secs(120);
/// Pause before re-asking a server that ignored our first check request because
/// it was still loading the project.
const COLD_RETRY_DELAY: Duration = Duration::from_secs(2);

pub(crate) type PendingMap = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value>>>>>;
pub(crate) type DiagnosticsMap = Arc<Mutex<HashMap<Uri, Vec<Diagnostic>>>>;

/// A document we've told the server about. LSP says the client's in-memory
/// buffer wins over disk for any open document, so once we `didOpen` a file the
/// server ignores external edits to it until we push a `didChange` — hence the
/// version counter and content hash we diff against.
struct OpenDoc {
    version: i32,
    hash: u64,
}

/// Cheap content fingerprint for "did this file change since we last synced".
/// Not cryptographic — collisions here only cost a skipped re-sync, and any
/// real edit changes the length or bytes enough for `DefaultHasher` to notice.
fn content_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Handle to one running language server. Drop the client to send `shutdown`
/// + `exit` and reap the child.
pub struct LspClient {
    tx: mpsc::UnboundedSender<OutboundFrame>,
    pending: PendingMap,
    diagnostics: DiagnosticsMap,
    next_id: AtomicI64,
    /// The server's *primary* root — the first workspace it was spawned for.
    /// Used as the (deprecated) `root_uri` at `initialize` and as the base for
    /// the first workspace folder's display name. Additional roots (other
    /// worktrees of the same project) live in `roots` and are added at runtime.
    root: PathBuf,
    /// Every workspace folder this server currently serves. Seeded with `root`
    /// at spawn; grows/shrinks via [`LspClient::add_folder`] /
    /// [`LspClient::remove_folder`] as worktrees of the same project open and
    /// close. One rust-analyzer serving N worktrees dedups their shared
    /// dependency analysis (same registry source paths → one crate in RA's
    /// global graph), which is the whole point of the multi-root move.
    roots: std::sync::Mutex<Vec<PathBuf>>,
    /// Files we've already sent `didOpen` for, with the version and content
    /// hash of what the server currently believes. Keeps `ensure_open`
    /// idempotent and lets [`LspClient::sync_from_disk`] push only real edits.
    opened: Arc<Mutex<HashMap<PathBuf, OpenDoc>>>,
    /// Latest `$/progress` state. Written by the reader task, watched by
    /// [`LspClient::wait_for_flycheck`].
    progress: watch::Sender<ProgressSnapshot>,
    /// `initializationOptions` sent on the `initialize` handshake. `None`
    /// leaves the server on its defaults. For rust-analyzer this carries the
    /// lean, memory-bounded config (see `LspLanguage::initialization_options`).
    init_options: Option<Value>,
    /// Wall-clock of the last request/notification we sent to the server.
    /// Read by the manager's idle reaper to shut down language servers nobody
    /// has queried in a while — a warmed rust-analyzer is ~1–5 GB resident, so
    /// leaving idle ones alive across many worktrees balloons WSL memory. Only
    /// *our* traffic bumps it; server-pushed diagnostics don't count as use.
    last_activity: std::sync::Mutex<Instant>,
}

impl LspClient {
    /// Spawn the language server binary and complete the `initialize`
    /// handshake. Returns a ready-to-use client.
    pub async fn spawn<S: AsRef<std::ffi::OsStr>>(
        binary: &Path,
        args: &[S],
        workspace_root: &Path,
        init_options: Option<Value>,
    ) -> Result<Arc<Self>> {
        let mut cmd = Command::new(binary);
        for a in args {
            cmd.arg(a);
        }
        cmd.current_dir(workspace_root);
        Self::spawn_with_command(cmd, workspace_root.to_owned(), init_options).await
    }

    /// WSL variant: spawn the LSP server inside `distro` via
    /// `wsl.exe -d <distro> --cd <posix_workspace> -- bash -lc 'exec <binary> <args>'`.
    /// The login-shell wrapper is what picks up `~/.local/bin` (where
    /// `language_packs_install_in_wsl` puts rust-analyzer) — without it
    /// the `wsl.exe -- <binary>` form only sees the system PATH and
    /// misses per-user installs.
    /// `workspace_root` here is the **POSIX** path inside the distro (the
    /// LSP server doesn't see Windows paths). Use the desktop's
    /// `path_translator::to_posix` to convert before calling.
    pub async fn spawn_wsl<S: AsRef<std::ffi::OsStr>>(
        distro: &str,
        binary: &str,
        args: &[S],
        posix_workspace: &Path,
        init_options: Option<Value>,
    ) -> Result<Arc<Self>> {
        let mut shell_cmd = format!("exec {}", shell_escape(binary));
        for a in args {
            shell_cmd.push(' ');
            let s = a.as_ref().to_string_lossy();
            shell_cmd.push_str(&shell_escape(&s));
        }
        let mut cmd = Command::new("wsl.exe");
        cmd.args(["-d", distro]);
        cmd.arg("--cd");
        cmd.arg(posix_workspace);
        cmd.arg("--");
        cmd.arg("bash").arg("-lc").arg(&shell_cmd);
        Self::spawn_with_command(cmd, posix_workspace.to_owned(), init_options).await
    }

    /// Common spawn pipeline: wire stdio, install reaper + reader/writer
    /// tasks, drive `initialize`. Both `spawn` and `spawn_wsl` go through
    /// here so the protocol behaviour is identical.
    async fn spawn_with_command(
        mut cmd: Command,
        workspace_root: PathBuf,
        init_options: Option<Value>,
    ) -> Result<Arc<Self>> {
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);
        cmd.hide_console();

        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError::Protocol("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError::Protocol("no stdout".into()))?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    tracing::warn!(target: "oxyris_lsp::stderr", "{line}");
                }
            });
        }

        // Reap the child so it doesn't zombie.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let diagnostics: DiagnosticsMap = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = mpsc::unbounded_channel::<OutboundFrame>();

        let (progress, _) = watch::channel(ProgressSnapshot::default());

        tokio::spawn(transport::writer_loop(stdin, rx));
        tokio::spawn(transport::reader_loop(
            stdout,
            pending.clone(),
            diagnostics.clone(),
            progress.clone(),
            tx.clone(),
        ));

        let client = Arc::new(Self {
            tx,
            pending,
            diagnostics,
            next_id: AtomicI64::new(1),
            roots: std::sync::Mutex::new(vec![workspace_root.clone()]),
            root: workspace_root,
            opened: Arc::new(Mutex::new(HashMap::new())),
            progress,
            init_options,
            last_activity: std::sync::Mutex::new(Instant::now()),
        });

        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&self) -> Result<()> {
        let root_uri = path_to_uri(&self.root)?;
        let workspace_folders = self
            .roots
            .lock()
            .map(|roots| roots.iter().filter_map(|r| workspace_folder(r)).collect())
            .unwrap_or_default();
        #[allow(deprecated)]
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(root_uri),
            workspace_folders: Some(workspace_folders),
            initialization_options: self.init_options.clone(),
            capabilities: ClientCapabilities {
                window: Some(WindowClientCapabilities {
                    // Not optional for us: rust-analyzer suppresses **all**
                    // `$/progress` unless the client opts in here, and without
                    // progress there is no way to know when its `cargo check`
                    // finished — [`LspClient::wait_for_flycheck`] would always
                    // conclude "no check ran".
                    work_done_progress: Some(true),
                    ..Default::default()
                }),
                // Advertise workspace-folder support so the server honours the
                // runtime `didChangeWorkspaceFolders` we send when a sibling
                // worktree opens/closes, and configuration pushes for updated
                // `linkedProjects`.
                workspace: Some(WorkspaceClientCapabilities {
                    workspace_folders: Some(true),
                    configuration: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            client_info: Some(lsp_types::ClientInfo {
                name: "oxyris-mcp".into(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
            ..Default::default()
        };
        let _: InitializeResult = self
            .request_with_timeout("initialize", params, INITIALIZE_TIMEOUT)
            .await?;
        self.notify("initialized", InitializedParams {})?;
        Ok(())
    }

    /// Send `textDocument/didOpen` for a file once. Subsequent calls for
    /// the same path are no-ops — saves the caller from threading state.
    /// Does **not** refresh a document that changed on disk since it was
    /// opened; use [`LspClient::sync_from_disk`] for that.
    pub async fn ensure_open(&self, path: &Path) -> Result<()> {
        {
            let opened = self.opened.lock().await;
            if opened.contains_key(path) {
                return Ok(());
            }
        }
        let text = tokio::fs::read_to_string(path).await?;
        let hash = content_hash(&text);
        let uri = path_to_uri(path)?;
        let language_id = language::language_id_for(path).unwrap_or("plaintext");
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id: language_id.into(),
                version: 1,
                text,
            },
        };
        self.notify("textDocument/didOpen", params)?;
        self.opened
            .lock()
            .await
            .insert(path.to_owned(), OpenDoc { version: 1, hash });
        Ok(())
    }

    /// Reconcile the server's view of `path` with what is on disk right now.
    ///
    /// This is the fix for a specific staleness trap: our own agent edits files
    /// through its file tools, not through this client, and LSP gives the
    /// client's buffer precedence for any document it has opened. Without this
    /// the server keeps answering about the text we opened minutes ago.
    ///
    /// Opens the file if new; otherwise pushes a full-text `didChange` plus a
    /// `didSave` when the content differs. The `didSave` matters as much as the
    /// change: rust-analyzer's flycheck (`cargo check`) is save-triggered.
    ///
    /// Returns `true` when something was actually sent.
    pub async fn sync_from_disk(&self, path: &Path) -> Result<bool> {
        let text = tokio::fs::read_to_string(path).await?;
        let hash = content_hash(&text);
        let uri = path_to_uri(path)?;

        let version = {
            let mut opened = self.opened.lock().await;
            match opened.get_mut(path) {
                Some(doc) if doc.hash == hash => return Ok(false),
                Some(doc) => {
                    doc.version += 1;
                    doc.hash = hash;
                    doc.version
                }
                None => {
                    // Not open yet — a plain didOpen carries the fresh text.
                    let language_id = language::language_id_for(path).unwrap_or("plaintext");
                    self.notify(
                        "textDocument/didOpen",
                        DidOpenTextDocumentParams {
                            text_document: TextDocumentItem {
                                uri,
                                language_id: language_id.into(),
                                version: 1,
                                text,
                            },
                        },
                    )?;
                    opened.insert(path.to_owned(), OpenDoc { version: 1, hash });
                    return Ok(true);
                }
            }
        };

        self.notify(
            "textDocument/didChange",
            DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                // Full-document sync. Ranged edits would be cheaper, but we
                // only ever see before/after snapshots of someone else's edit.
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text,
                }],
            },
        )?;
        self.notify(
            "textDocument/didSave",
            DidSaveTextDocumentParams {
                text_document: TextDocumentIdentifier { uri },
                text: None,
            },
        )?;
        Ok(true)
    }

    /// [`LspClient::sync_from_disk`] for every document we have open. Files
    /// that vanished are dropped from the open set (the server learns of the
    /// deletion from its own file watching). Returns how many were re-synced.
    pub async fn sync_open_from_disk(&self) -> usize {
        let paths: Vec<PathBuf> = self.opened.lock().await.keys().cloned().collect();
        let mut changed = 0;
        for path in paths {
            match self.sync_from_disk(&path).await {
                Ok(true) => changed += 1,
                Ok(false) => {}
                Err(LspError::Io(_)) => {
                    self.opened.lock().await.remove(&path);
                }
                Err(e) => {
                    tracing::debug!(path = %path.display(), error = %e, "lsp: sync failed");
                }
            }
        }
        changed
    }

    /// Ask rust-analyzer to run `cargo check` now — `None` for the whole
    /// workspace, `Some(path)` for the package owning that file. This is the
    /// `rust-analyzer/runFlycheck` extension, so it is a no-op notification on
    /// any other server.
    ///
    /// Cargo reads the files from disk, so the check reflects on-disk truth
    /// regardless of what documents we have open.
    pub fn run_flycheck(&self, path: Option<&Path>) -> Result<()> {
        self.notify("rust-analyzer/runFlycheck", flycheck_params(path)?)
    }

    /// Trigger the check layer and block until it settles — the one call a
    /// caller needs to make diagnostics trustworthy.
    ///
    /// Retries once because a cold server drops the request: rust-analyzer has
    /// nothing to check until it has loaded the Cargo workspace, and our first
    /// `runFlycheck` can easily land before that. Returns whether a check
    /// actually completed.
    pub async fn run_check_and_wait(&self, path: Option<&Path>) -> Result<bool> {
        for attempt in 0..2 {
            if attempt > 0 {
                tokio::time::sleep(COLD_RETRY_DELAY).await;
            }
            self.run_flycheck(path)?;
            if self.wait_for_flycheck().await? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Block until the server's flycheck is idle, so a diagnostics read that
    /// follows sees the finished `cargo check` rather than the previous run's
    /// leftovers.
    ///
    /// Returns `Ok(true)` when a check ran to completion, `Ok(false)` when none
    /// started within [`FLYCHECK_START_GRACE`] (nothing to wait for — the
    /// server has no check layer, or it is disabled), and
    /// [`LspError::Timeout`] when one started but outlived
    /// [`FLYCHECK_TIMEOUT`].
    pub async fn wait_for_flycheck(&self) -> Result<bool> {
        let mut rx = self.progress.subscribe();
        let baseline = rx.borrow_and_update().flycheck_completions;
        let start = Instant::now();

        // Phase 1 — wait for a check to be in flight. It may already be, or it
        // may have begun *and* ended before we first looked (fast incremental
        // check), which the completion counter catches.
        loop {
            let (running, completed) = {
                let snap = rx.borrow_and_update();
                (
                    snap.flycheck_running(),
                    snap.flycheck_completions != baseline,
                )
            };
            if running {
                break;
            }
            if completed {
                return Ok(true);
            }
            let Some(remaining) = FLYCHECK_START_GRACE.checked_sub(start.elapsed()) else {
                return Ok(false);
            };
            if tokio::time::timeout(remaining, rx.changed()).await.is_err() {
                return Ok(false);
            }
        }

        // Phase 2 — wait for it to finish.
        loop {
            if !rx.borrow_and_update().flycheck_running() {
                return Ok(true);
            }
            let Some(remaining) = FLYCHECK_TIMEOUT.checked_sub(start.elapsed()) else {
                return Err(LspError::Timeout(FLYCHECK_TIMEOUT, "flycheck"));
            };
            if tokio::time::timeout(remaining, rx.changed()).await.is_err() {
                return Err(LspError::Timeout(FLYCHECK_TIMEOUT, "flycheck"));
            }
        }
    }

    pub async fn find_references(
        &self,
        path: &Path,
        line: u32,
        column: u32,
        include_declaration: bool,
    ) -> Result<Vec<Location>> {
        self.ensure_open(path).await?;
        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: path_to_uri(path)?,
                },
                position: Position {
                    line,
                    character: column,
                },
            },
            context: ReferenceContext {
                include_declaration,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let result: Option<Vec<Location>> = self.request("textDocument/references", params).await?;
        Ok(result.unwrap_or_default())
    }

    pub async fn hover(&self, path: &Path, line: u32, column: u32) -> Result<Option<String>> {
        self.ensure_open(path).await?;
        let params = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: path_to_uri(path)?,
                },
                position: Position {
                    line,
                    character: column,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let result: Option<Hover> = self.request("textDocument/hover", params).await?;
        Ok(result.and_then(|h| flatten_hover(h.contents)))
    }

    /// Snapshot of diagnostics the server has pushed for `path` (if any).
    /// Diagnostics are gathered from `publishDiagnostics` notifications, so
    /// this requires the file to have been opened and the server to have
    /// finished its first analysis pass — pre-warm the language at session
    /// start to avoid empty answers on the first call.
    pub async fn diagnostics_for(&self, path: &Path) -> Result<Vec<Diagnostic>> {
        let uri = path_to_uri(path)?;
        let cache = self.diagnostics.lock().await;
        Ok(cache.get(&uri).cloned().unwrap_or_default())
    }

    /// Every diagnostic the server has published, keyed by document URI. This
    /// is the workspace-wide view: an edit in one crate surfaces errors the
    /// server publishes against *other* files, which a per-file query can never
    /// find. Pair with [`LspClient::wait_for_flycheck`] and it is the
    /// equivalent of reading a finished `cargo check --workspace`.
    pub async fn all_diagnostics(&self) -> Vec<(Uri, Vec<Diagnostic>)> {
        let cache = self.diagnostics.lock().await;
        let mut out: Vec<(Uri, Vec<Diagnostic>)> =
            cache.iter().map(|(u, d)| (u.clone(), d.clone())).collect();
        out.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        out
    }

    /// How long since we last sent this server a request or notification.
    /// The manager's idle reaper compares this against its TTL.
    pub fn idle_for(&self) -> Duration {
        self.last_activity
            .lock()
            .map(|t| t.elapsed())
            .unwrap_or_default()
    }

    /// Stamp "used now". Called on every outbound request/notify so an actively
    /// queried server is never idle-reaped.
    fn touch(&self) {
        if let Ok(mut t) = self.last_activity.lock() {
            *t = Instant::now();
        }
    }

    /// The workspace folders this server currently serves.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.roots.lock().map(|r| r.clone()).unwrap_or_default()
    }

    /// Add a workspace folder at runtime — a sibling worktree of the same
    /// project opening. Idempotent: re-adding a folder already served is a
    /// no-op. Sends `workspace/didChangeWorkspaceFolders` (added) and, when
    /// `settings` is `Some`, a `workspace/didChangeConfiguration` so the
    /// server picks up config that depends on the folder set (for
    /// rust-analyzer that's the refreshed `linkedProjects`). The caller owns
    /// the settings shape — the client stays language-agnostic.
    pub fn add_folder(&self, root: &Path, settings: Option<Value>) -> Result<()> {
        {
            let mut roots = self.roots.lock().map_err(|_| LspError::ServerGone)?;
            if roots.iter().any(|r| r == root) {
                return Ok(());
            }
            roots.push(root.to_owned());
        }
        let Some(folder) = workspace_folder(root) else {
            return Ok(());
        };
        self.notify(
            "workspace/didChangeWorkspaceFolders",
            DidChangeWorkspaceFoldersParams {
                event: WorkspaceFoldersChangeEvent {
                    added: vec![folder],
                    removed: vec![],
                },
            },
        )?;
        if let Some(settings) = settings {
            self.notify(
                "workspace/didChangeConfiguration",
                DidChangeConfigurationParams { settings },
            )?;
        }
        Ok(())
    }

    /// Remove a workspace folder at runtime — a worktree closing (or idle-
    /// reaped). Idempotent: removing a folder not served is a no-op. Mirror of
    /// [`LspClient::add_folder`]. When the last folder is removed the caller
    /// should shut the whole client down (see the manager's reaper); this
    /// method does not self-terminate.
    pub fn remove_folder(&self, root: &Path, settings: Option<Value>) -> Result<()> {
        {
            let mut roots = self.roots.lock().map_err(|_| LspError::ServerGone)?;
            let before = roots.len();
            roots.retain(|r| r != root);
            if roots.len() == before {
                return Ok(());
            }
        }
        let Some(folder) = workspace_folder(root) else {
            return Ok(());
        };
        self.notify(
            "workspace/didChangeWorkspaceFolders",
            DidChangeWorkspaceFoldersParams {
                event: WorkspaceFoldersChangeEvent {
                    added: vec![],
                    removed: vec![folder],
                },
            },
        )?;
        if let Some(settings) = settings {
            self.notify(
                "workspace/didChangeConfiguration",
                DidChangeConfigurationParams { settings },
            )?;
        }
        Ok(())
    }

    /// Best-effort clean shutdown. Failures are swallowed since the child
    /// will be killed on drop anyway.
    pub async fn shutdown(&self) {
        let _: Result<Value> = self.request("shutdown", serde_json::Value::Null).await;
        let _ = self.notify("exit", serde_json::Value::Null);
    }

    async fn request<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &'static str,
        params: P,
    ) -> Result<R> {
        self.request_with_timeout(method, params, REQUEST_TIMEOUT)
            .await
    }

    async fn request_with_timeout<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &'static str,
        params: P,
        timeout: Duration,
    ) -> Result<R> {
        self.touch();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (resp_tx, resp_rx) = oneshot::channel();
        self.pending.lock().await.insert(id, resp_tx);

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.tx
            .send(OutboundFrame(frame))
            .map_err(|_| LspError::ServerGone)?;

        let outcome = tokio::time::timeout(timeout, resp_rx).await;
        let value = match outcome {
            Ok(Ok(value)) => value?,
            Ok(Err(_)) => return Err(LspError::ServerGone),
            Err(_) => {
                // Timed out — drop the pending entry so the map doesn't
                // grow when a slow server eventually replies.
                self.pending.lock().await.remove(&id);
                return Err(LspError::Timeout(timeout, method));
            }
        };
        if value.is_null() {
            // `null` deserializes into `Option<_>` as `None`; into a non-
            // optional R it would fail, so callers should declare R as
            // `Option<...>` whenever the server may legally return null.
            return serde_json::from_value::<R>(Value::Null).map_err(LspError::Serde);
        }
        serde_json::from_value(value).map_err(LspError::Serde)
    }

    fn notify<P: Serialize>(&self, method: &str, params: P) -> Result<()> {
        self.touch();
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.tx
            .send(OutboundFrame(frame))
            .map_err(|_| LspError::ServerGone)
    }
}

/// POSIX-shell-quote for safe interpolation into a `bash -lc` payload.
/// Bare alphanumeric + a few common URL/path chars pass through; anything
/// else is wrapped in single quotes with embedded single quotes escaped
/// via the standard `'\''` trick.
fn shell_escape(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@%+=:,./-_".contains(&b))
    {
        return s.to_owned();
    }
    let mut out = String::from("'");
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// True when `s` (slashes already normalized to `/`) names a `X:/...` Windows
/// drive path. Checked explicitly because `Path::is_absolute` returns false for
/// such paths on non-Windows hosts (e.g. Linux CI).
fn has_windows_drive_prefix(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.next() == Some(':')
        && chars.next() == Some('/')
}

/// Params for `rust-analyzer/runFlycheck`. `None` → `{"textDocument": null}`,
/// which rust-analyzer reads as "check the whole workspace".
fn flycheck_params(path: Option<&Path>) -> Result<Value> {
    let text_document = match path {
        Some(p) => serde_json::json!({ "uri": path_to_uri(p)?.to_string() }),
        None => Value::Null,
    };
    Ok(serde_json::json!({ "textDocument": text_document }))
}

fn path_to_uri(path: &Path) -> Result<Uri> {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let absolute = if normalized.starts_with('/') || has_windows_drive_prefix(&normalized) {
        normalized
    } else {
        let mut base = std::env::current_dir()?
            .to_string_lossy()
            .replace('\\', "/");
        if !base.ends_with('/') {
            base.push('/');
        }
        format!("{base}{normalized}")
    };
    let prefixed = if absolute.starts_with('/') {
        format!("file://{absolute}")
    } else {
        format!("file:///{absolute}")
    };
    prefixed
        .parse::<Uri>()
        .map_err(|e| LspError::InvalidPath(format!("{absolute}: {e}")))
}

/// Build an LSP [`WorkspaceFolder`] for a root path. `None` when the path
/// can't be expressed as a `file:` URI (returned unchanged so callers can skip
/// that folder rather than fail the whole operation). The folder name is the
/// last path component so a multi-root server's folders are distinguishable in
/// server logs.
fn workspace_folder(root: &Path) -> Option<WorkspaceFolder> {
    let uri = path_to_uri(root).ok()?;
    let name = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("workspace")
        .to_owned();
    Some(WorkspaceFolder { uri, name })
}

/// Build the `workspace/didChangeConfiguration` `settings` payload that tells
/// rust-analyzer which Cargo workspaces to treat as linked projects — one
/// `Cargo.toml` per served root. Pushed whenever the folder set changes so RA
/// re-derives its crate graph across exactly the open worktrees. Shaped with
/// the `rust-analyzer` section key (how the server reads pushed settings),
/// unlike `initializationOptions` which omits the prefix.
pub fn rust_linked_projects_settings(roots: &[PathBuf]) -> Value {
    let linked: Vec<String> = roots
        .iter()
        .map(|r| r.join("Cargo.toml").to_string_lossy().replace('\\', "/"))
        .collect();
    serde_json::json!({ "rust-analyzer": { "linkedProjects": linked } })
}

fn flatten_hover(contents: HoverContents) -> Option<String> {
    match contents {
        HoverContents::Scalar(MarkedString::String(s)) if !s.trim().is_empty() => Some(s),
        HoverContents::Scalar(MarkedString::LanguageString(ls)) if !ls.value.trim().is_empty() => {
            Some(format!("```{}\n{}\n```", ls.language, ls.value))
        }
        HoverContents::Array(items) => {
            let mut out = String::new();
            for item in items {
                match item {
                    MarkedString::String(s) if !s.trim().is_empty() => {
                        out.push_str(&s);
                        out.push('\n');
                    }
                    MarkedString::LanguageString(ls) if !ls.value.trim().is_empty() => {
                        out.push_str(&format!("```{}\n{}\n```\n", ls.language, ls.value));
                    }
                    _ => {}
                }
            }
            if out.trim().is_empty() {
                None
            } else {
                Some(out)
            }
        }
        HoverContents::Markup(MarkupContent { value, .. }) if !value.trim().is_empty() => {
            Some(value)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_markdown_hover() {
        let h = HoverContents::Markup(MarkupContent {
            kind: lsp_types::MarkupKind::Markdown,
            value: "**fn** `add`".into(),
        });
        assert_eq!(flatten_hover(h).as_deref(), Some("**fn** `add`"));
    }

    #[test]
    fn flattens_array_with_language_strings() {
        let h = HoverContents::Array(vec![
            MarkedString::String("hello".into()),
            MarkedString::LanguageString(lsp_types::LanguageString {
                language: "rust".into(),
                value: "fn add(a: i32, b: i32) -> i32".into(),
            }),
        ]);
        let out = flatten_hover(h).expect("some");
        assert!(out.contains("hello"));
        assert!(out.contains("```rust"));
        assert!(out.contains("fn add"));
    }

    #[test]
    fn empty_hover_is_none() {
        let h = HoverContents::Scalar(MarkedString::String("".into()));
        assert!(flatten_hover(h).is_none());
    }

    #[test]
    fn path_to_uri_round_trips_windows_drive() {
        let uri = path_to_uri(Path::new("C:\\Users\\wilson\\proj\\src\\lib.rs")).expect("uri");
        let s = uri.to_string();
        assert!(
            s.starts_with("file:///C:/") || s.starts_with("file:///C%3A/"),
            "got {s}"
        );
    }

    #[test]
    fn workspace_folder_names_by_last_component() {
        let f = workspace_folder(Path::new("/home/w/repo/wt-feature")).expect("folder");
        assert_eq!(f.name, "wt-feature");
        assert!(f.uri.to_string().ends_with("wt-feature"));
    }

    #[test]
    fn content_hash_tracks_edits() {
        let before = content_hash("fn main() {}\n");
        assert_eq!(before, content_hash("fn main() {}\n"));
        assert_ne!(before, content_hash("fn main() { todo!() }\n"));
        // Whitespace-only change still counts — it can move every diagnostic
        // range in the file.
        assert_ne!(before, content_hash("fn main() {}\n\n"));
    }

    #[test]
    fn flycheck_params_null_means_whole_workspace() {
        let p = flycheck_params(None).expect("params");
        assert!(p["textDocument"].is_null());
    }

    #[test]
    fn flycheck_params_scoped_to_file_carries_uri() {
        let p = flycheck_params(Some(Path::new("/home/w/repo/src/lib.rs"))).expect("params");
        let uri = p["textDocument"]["uri"].as_str().expect("uri string");
        assert!(uri.starts_with("file:///home/w/repo/"), "got {uri}");
        assert!(uri.ends_with("src/lib.rs"), "got {uri}");
    }

    #[test]
    fn progress_snapshot_recognises_flycheck_tokens() {
        let mut snap = ProgressSnapshot::default();
        assert!(!snap.flycheck_running());
        // Indexing/cache-priming must not be mistaken for a check — waiting on
        // those would block a diagnostics read for the whole cold start.
        snap.active.insert("rustAnalyzer/Indexing".into());
        assert!(!snap.flycheck_running());
        snap.active.insert("rustAnalyzer/Flycheck".into());
        assert!(snap.flycheck_running());
        snap.active.remove("rustAnalyzer/Flycheck");
        assert!(!snap.flycheck_running());
    }

    #[test]
    fn linked_projects_settings_one_cargo_per_root() {
        let roots = vec![
            PathBuf::from("/home/w/repo/main"),
            PathBuf::from("/home/w/repo/wt-a"),
        ];
        let v = rust_linked_projects_settings(&roots);
        let linked = v["rust-analyzer"]["linkedProjects"]
            .as_array()
            .expect("array");
        assert_eq!(linked.len(), 2);
        // POSIX-normalised, one Cargo.toml per root, section-prefixed for a
        // config *push* (not the prefix-less initializationOptions shape).
        assert_eq!(linked[0], "/home/w/repo/main/Cargo.toml");
        assert_eq!(linked[1], "/home/w/repo/wt-a/Cargo.toml");
    }
}
