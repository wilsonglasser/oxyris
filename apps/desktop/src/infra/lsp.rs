//! Per-worktree LSP orchestration on the desktop side.
//!
//! Mirror of the manager that lives inside `oxyris-mcp` so that we can:
//! 1. Pre-warm language servers as soon as a worktree is created (instead
//!    of waiting for a Claude session to start), reducing cold-start pain.
//! 2. Surface status to the UI via Tauri events (`lsp:status`).
//!
//! Each worktree gets its own pool keyed by [`oxyris_lsp::LspLanguage`]. We
//! detect languages from workspace markers (`Cargo.toml`, `package.json`,
//! `composer.json`, …) and spawn lazily — except for the primary, which we
//! warm eagerly so the user's first LSP query lands without a 30–60 s wait.
//!
//! WSL is **not** supported yet (Sprint 14e.2 — needs an agent op for
//! spawning Linux LSP servers inside the distro). WSL projects skip
//! silently here and the chip stays hidden.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use oxyris_core::{AggregateId, Environment};
use oxyris_lsp::{
    LspClient, LspLanguage, detect_languages, resolve_server, rust_linked_projects_settings,
};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::infra::language_packs::{self, LanguagePacksService};

/// Shut a language server down after this long with no query. A warmed
/// rust-analyzer / tsserver holds 1–5 GB resident; across several worktrees
/// that silently balloons WSL memory. Reaped servers respawn lazily on the
/// next query (a one-time cold-start cost), so the ceiling stays bounded.
/// Held at 5 min (was 15) so an idle worktree the user tabbed away from gives
/// its memory back quickly — the respawn cost is only paid if they return.
const IDLE_TTL: Duration = Duration::from_secs(5 * 60);
/// How often the reaper sweeps the pool.
const REAPER_TICK: Duration = Duration::from_secs(60);
/// Most worktrees a single shared language server will serve as workspace
/// folders before the next worktree gets its own dedicated server instead.
/// Typical use is 3–4 worktrees per project; 8 leaves headroom while capping
/// the reindex cost of one server's linked-project set. Also the landing spot
/// for the divergent-`rust-toolchain.toml` escape hatch (§12 of the design doc)
/// — same "don't join the shared server" fallback path, not yet wired.
const FOLDER_CAP: usize = 8;

#[derive(Debug, Error)]
pub enum LspManagerError {
    #[error("language server is not installed: {0}")]
    NotInstalled(String),
    #[error("lsp client: {0}")]
    Client(String),
}

/// Status snapshots streamed to the frontend as `lsp:status` events.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum LspStatusEvent {
    Spawning {
        worktree_id: AggregateId,
        language: &'static str,
    },
    Ready {
        worktree_id: AggregateId,
        language: &'static str,
    },
    Failed {
        worktree_id: AggregateId,
        language: &'static str,
        error: String,
    },
    NotInstalled {
        worktree_id: AggregateId,
        language: &'static str,
        hint: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct LspWorktreeSummary {
    pub worktree_id: AggregateId,
    pub languages: Vec<&'static str>,
}

/// A worktree the manager knows about. Recorded on `register` / `warm_primary`
/// so a path-based query from the MCP bridge routes back to the owning
/// project's shared server, and so `close` knows which folders to detach.
struct WorktreeRec {
    project_id: AggregateId,
    root: PathBuf,
    #[allow(dead_code)]
    env: Environment,
    detected: Vec<LspLanguage>,
}

/// The shared language servers for one project. Each server serves *every* open
/// worktree of the project at once, as workspace folders. rust-analyzer keys
/// dependency crates by their (shared) registry source path, so analysing N
/// worktrees in one server dedups the dependency analysis that used to be
/// duplicated N times across N per-worktree servers — the whole point of the
/// multi-root move (see `docs/design/lsp-multi-root-per-project.md`).
#[derive(Default)]
struct ProjectServers {
    /// One shared server per language.
    shared: HashMap<LspLanguage, Arc<LspClient>>,
    /// Worktree roots currently attached to each shared server as folders.
    folders: HashMap<LspLanguage, BTreeSet<PathBuf>>,
}

#[derive(Default)]
struct Pools {
    /// worktree_id → its record (project, root, env, detected languages).
    worktrees: HashMap<AggregateId, WorktreeRec>,
    /// worktree root → worktree_id, for the bridge's path-based queries.
    by_path: HashMap<PathBuf, AggregateId>,
    /// project_id → its shared servers + folder membership.
    projects: HashMap<AggregateId, ProjectServers>,
    /// Overflow: worktrees that could not join the shared server (folder cap
    /// reached, or — future — a divergent toolchain) get a dedicated server,
    /// keyed by `(worktree_id, language)`. Behaves like the old per-worktree pool.
    dedicated: HashMap<(AggregateId, LspLanguage), Arc<LspClient>>,
}

pub struct LspManager {
    app: AppHandle,
    /// Optional — when set, we prefer binaries managed by the language
    /// packs service (under `<data_dir>/lsp/`) over PATH. Wired by
    /// `AppState` after both services exist.
    packs: Mutex<Option<Arc<LanguagePacksService>>>,
    pools: Mutex<Pools>,
}

impl LspManager {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            packs: Mutex::new(None),
            pools: Mutex::new(Pools::default()),
        }
    }

    /// Inject the language-packs service so the manager can prefer
    /// freshly-installed managed binaries over whatever's on PATH. Called
    /// once at boot from `AppState::initialize`.
    pub async fn with_language_packs(&self, packs: Arc<LanguagePacksService>) {
        *self.packs.lock().await = Some(packs);
    }

    /// Record a worktree and its owning project. Idempotent. Populates the
    /// `by_path` route so a later bridge query on this path finds the project.
    pub async fn register(
        &self,
        worktree_id: AggregateId,
        project_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
    ) -> Result<LspWorktreeSummary, LspManagerError> {
        let root = PathBuf::from(worktree_root);
        let detected = detect_languages(&root);
        let mut pools = self.pools.lock().await;
        pools.by_path.insert(root.clone(), worktree_id);
        pools.projects.entry(project_id).or_default();
        let rec = pools
            .worktrees
            .entry(worktree_id)
            .or_insert_with(|| WorktreeRec {
                project_id,
                root,
                env: env.clone(),
                detected,
            });
        Ok(LspWorktreeSummary {
            worktree_id,
            languages: rec.detected.iter().map(|l| l.id()).collect(),
        })
    }

    /// Detach a worktree when it is removed: pull its folder from every shared
    /// server it was attached to (updating the server's `linkedProjects`), and
    /// shut down any server left with no folders — plus any dedicated fallback
    /// server the worktree owned. Called from `worktree_remove` so a closed
    /// worktree doesn't keep language-server memory resident.
    pub async fn close(&self, worktree_id: AggregateId) {
        let mut to_shutdown: Vec<Arc<LspClient>> = Vec::new();
        {
            let mut pools = self.pools.lock().await;
            let Some(rec) = pools.worktrees.remove(&worktree_id) else {
                return;
            };
            pools.by_path.remove(&rec.root);

            // Dedicated fallback servers owned by this worktree.
            let ded_langs: Vec<LspLanguage> = pools
                .dedicated
                .keys()
                .filter(|(wt, _)| *wt == worktree_id)
                .map(|(_, l)| *l)
                .collect();
            for lang in ded_langs {
                if let Some(c) = pools.dedicated.remove(&(worktree_id, lang)) {
                    to_shutdown.push(c);
                }
            }

            // Detach from the project's shared servers.
            if let Some(ps) = pools.projects.get_mut(&rec.project_id) {
                let langs: Vec<LspLanguage> = ps.folders.keys().copied().collect();
                for lang in langs {
                    let removed = ps
                        .folders
                        .get_mut(&lang)
                        .map(|f| f.remove(&rec.root))
                        .unwrap_or(false);
                    if !removed {
                        continue;
                    }
                    let remaining: Vec<PathBuf> = ps
                        .folders
                        .get(&lang)
                        .map(|f| f.iter().cloned().collect())
                        .unwrap_or_default();
                    if remaining.is_empty() {
                        ps.folders.remove(&lang);
                        if let Some(c) = ps.shared.remove(&lang) {
                            to_shutdown.push(c);
                        }
                    } else if let Some(server) = ps.shared.get(&lang) {
                        let settings = (lang == LspLanguage::Rust)
                            .then(|| rust_linked_projects_settings(&remaining));
                        let _ = server.remove_folder(&rec.root, settings);
                    }
                }
                if ps.shared.is_empty() {
                    pools.projects.remove(&rec.project_id);
                }
            }
        }
        let n = to_shutdown.len();
        for c in to_shutdown {
            c.shutdown().await;
        }
        if n > 0 {
            tracing::info!(worktree_id = %worktree_id, servers = n, "lsp servers reaped on worktree remove");
        }
    }

    /// Start the background idle-reaper. Every [`REAPER_TICK`] it shuts down
    /// any language server nobody has queried in [`IDLE_TTL`], keeping the
    /// resident-memory ceiling bounded no matter how many worktrees were warmed.
    /// Idempotent to *effect* but spawn once — called from `AppState::initialize`.
    pub fn spawn_idle_reaper(self: &Arc<Self>) {
        let me = self.clone();
        // MUST be `tauri::async_runtime::spawn`, not `tokio::spawn`:
        // `AppState::initialize` runs inside Tauri's `setup()`, which is not a
        // tokio runtime thread, so a raw `tokio::spawn` panics at the call site
        // and takes startup down with it (the app "opens and closes").
        tauri::async_runtime::spawn(async move {
            let mut tick = tokio::time::interval(REAPER_TICK);
            tick.tick().await; // consume the immediate first tick
            loop {
                tick.tick().await;
                me.reap_idle().await;
            }
        });
    }

    /// One reaper sweep: shut down every server nobody has queried in
    /// [`IDLE_TTL`]. Reaping is **server-level** — a shared server stays warm
    /// while *any* of its worktrees is active (their queries bump its idle
    /// clock), so we don't churn the reindex by yanking one idle folder out of
    /// a live multi-root. When the whole project (or a dedicated server) goes
    /// quiet, the server drops; a later query respawns it and re-attaches its
    /// folders lazily. Servers are removed under the lock, then shut down after
    /// it's released (the `shutdown`/`exit` handshake awaits; the last `Arc`
    /// drop fires `kill_on_drop`).
    async fn reap_idle(&self) {
        let mut victims: Vec<(String, Arc<LspClient>)> = Vec::new();
        {
            let mut pools = self.pools.lock().await;
            // Shared project servers.
            let project_ids: Vec<AggregateId> = pools.projects.keys().copied().collect();
            for pid in project_ids {
                let Some(ps) = pools.projects.get_mut(&pid) else {
                    continue;
                };
                let idle: Vec<LspLanguage> = ps
                    .shared
                    .iter()
                    .filter(|(_, c)| c.idle_for() >= IDLE_TTL)
                    .map(|(lang, _)| *lang)
                    .collect();
                for lang in idle {
                    if let Some(c) = ps.shared.remove(&lang) {
                        ps.folders.remove(&lang);
                        victims.push((format!("project {pid} {lang:?}"), c));
                    }
                }
                if ps.shared.is_empty() {
                    pools.projects.remove(&pid);
                }
            }
            // Dedicated fallback servers.
            let ded: Vec<(AggregateId, LspLanguage)> = pools
                .dedicated
                .iter()
                .filter(|(_, c)| c.idle_for() >= IDLE_TTL)
                .map(|(k, _)| *k)
                .collect();
            for k in ded {
                if let Some(c) = pools.dedicated.remove(&k) {
                    victims.push((format!("dedicated {k:?}"), c));
                }
            }
        }
        for (label, client) in victims {
            tracing::info!(server = %label, "lsp idle-reaped (no query in {IDLE_TTL:?})");
            client.shutdown().await;
            // `client` drops here; if no other Arc is held, kill_on_drop reaps
            // the child. Any stale holder gets `ServerGone` on its next call,
            // which the bridge treats as a transient miss and re-ensures.
        }
    }

    /// Resolve a workspace path to its worktree/project and ensure the language
    /// server. Used by the LSP bridge, which knows only the path (the MCP
    /// server is a separate process with no `AggregateId`). A path warmed via
    /// `warm_primary` routes to its project's shared server; an unregistered
    /// path is registered as its own single-worktree project so repeat queries
    /// reuse the same server instead of spawning a fresh one each call.
    pub async fn ensure_at(
        &self,
        workspace_root: &Path,
        env: &Environment,
        lang: LspLanguage,
    ) -> Result<Arc<LspClient>, LspManagerError> {
        let root = workspace_root.to_string_lossy().into_owned();
        let known = {
            let pools = self.pools.lock().await;
            pools
                .by_path
                .get(workspace_root)
                .and_then(|wt| pools.worktrees.get(wt).map(|r| (*wt, r.project_id)))
        };
        let (worktree_id, project_id) = match known {
            Some(pair) => pair,
            None => {
                // Unregistered path — mint a synthetic worktree that is its own
                // project, and register it so the route exists next time.
                let id = AggregateId::new();
                let _ = self.register(id, id, env, &root).await;
                (id, id)
            }
        };
        self.ensure(worktree_id, project_id, env, &root, lang).await
    }

    /// Map a file path to its LSP language using the workspace's detected
    /// set. Returns `None` for unsupported extensions or languages we
    /// haven't detected in this workspace.
    pub fn language_for_workspace(workspace: &Path, file: &Path) -> Option<LspLanguage> {
        let detected = detect_languages(workspace);
        let ext = file.extension()?.to_str()?.to_ascii_lowercase();
        let lang = match ext.as_str() {
            "rs" => LspLanguage::Rust,
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => LspLanguage::TypeScriptJavaScript,
            "php" | "phtml" => LspLanguage::Php,
            _ => return None,
        };
        if detected.contains(&lang) {
            Some(lang)
        } else {
            None
        }
    }

    /// Ensure a language server for `(project, worktree, lang)`. A worktree
    /// joins its project's **shared** server as a workspace folder (dedup of
    /// dependency analysis); when that server is at [`FOLDER_CAP`] folders it
    /// gets a **dedicated** server instead. Emits status events on every
    /// transition so the UI can show progress.
    ///
    /// We hold the pool lock across the (slow) spawn, matching the original
    /// single-`Mutex` behaviour — spawns were already globally serialised and
    /// `warm_primary` (a background task) is the only hot spawner.
    pub async fn ensure(
        &self,
        worktree_id: AggregateId,
        project_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
        lang: LspLanguage,
    ) -> Result<Arc<LspClient>, LspManagerError> {
        let root = PathBuf::from(worktree_root);
        let mut pools = self.pools.lock().await;

        // (a) Already on a dedicated fallback server.
        if let Some(c) = pools.dedicated.get(&(worktree_id, lang)) {
            tracing::trace!(worktree_id = %worktree_id, ?lang, "lsp cache hit (dedicated)");
            return Ok(c.clone());
        }
        // (b) Shared server for the project already up — reuse or attach.
        if let Some(ps) = pools.projects.get(&project_id)
            && let Some(server) = ps.shared.get(&lang).cloned()
        {
            if ps.folders.get(&lang).is_some_and(|f| f.contains(&root)) {
                tracing::trace!(worktree_id = %worktree_id, ?lang, "lsp cache hit (shared)");
                return Ok(server);
            }
            let count = ps.folders.get(&lang).map(|f| f.len()).unwrap_or(0);
            if count < FOLDER_CAP {
                // Join the shared server as a new folder.
                let ps = pools.projects.get_mut(&project_id).expect("project entry");
                let folders = ps.folders.entry(lang).or_default();
                folders.insert(root.clone());
                let all: Vec<PathBuf> = folders.iter().cloned().collect();
                let settings =
                    (lang == LspLanguage::Rust).then(|| rust_linked_projects_settings(&all));
                let _ = server.add_folder(&root, settings);
                tracing::info!(worktree_id = %worktree_id, ?lang, folders = all.len(), "lsp worktree joined shared server");
                return Ok(server);
            }
            // Cap reached → fall through and spawn a dedicated server.
        }

        // (c) Spawn — first shared server for `(project, lang)`, or a dedicated
        // fallback when the shared server is already at the folder cap.
        let dedicated = pools
            .projects
            .get(&project_id)
            .and_then(|ps| ps.folders.get(&lang))
            .map(|f| f.len() >= FOLDER_CAP)
            .unwrap_or(false);

        let (binary, args) = match env {
            Environment::Local => match self.resolve_with_packs(lang).await {
                Ok(t) => t,
                Err(hint) => {
                    self.emit(LspStatusEvent::NotInstalled {
                        worktree_id,
                        language: lang.id(),
                        hint: hint.clone(),
                    });
                    return Err(LspManagerError::NotInstalled(hint));
                }
            },
            Environment::Wsl { .. } => Self::wsl_binary_name(lang),
        };

        self.emit(LspStatusEvent::Spawning {
            worktree_id,
            language: lang.id(),
        });

        // For WSL projects `binary` is the *Linux-side* name (spawned via
        // wsl.exe); for Windows it's the absolute path. The server is born with
        // this one root; siblings attach later via `add_folder`.
        tracing::info!(
            worktree_id = %worktree_id,
            ?lang,
            binary = %binary.display(),
            ?args,
            workspace = %root.display(),
            dedicated,
            "lsp spawn",
        );
        let init_options = init_options_for(lang, std::slice::from_ref(&root));
        let result = match env {
            Environment::Local => LspClient::spawn(&binary, &args, &root, init_options).await,
            Environment::Wsl { distro } => {
                let binary_str = binary.to_string_lossy().into_owned();
                LspClient::spawn_wsl(distro, &binary_str, &args, &root, init_options).await
            }
        };
        match result {
            Ok(client) => {
                if dedicated {
                    pools.dedicated.insert((worktree_id, lang), client.clone());
                    tracing::info!(worktree_id = %worktree_id, ?lang, "lsp dedicated server spawned (shared at folder cap)");
                } else {
                    let ps = pools.projects.entry(project_id).or_default();
                    ps.shared.insert(lang, client.clone());
                    ps.folders.entry(lang).or_default().insert(root);
                }
                self.emit(LspStatusEvent::Ready {
                    worktree_id,
                    language: lang.id(),
                });
                Ok(client)
            }
            Err(e) => {
                let msg = e.to_string();
                self.emit(LspStatusEvent::Failed {
                    worktree_id,
                    language: lang.id(),
                    error: msg.clone(),
                });
                Err(LspManagerError::Client(msg))
            }
        }
    }

    /// Eager pre-warm: register the worktree, detect the primary language, and
    /// spawn (or join) its server in the background. Failures are silent
    /// (status event already reports them). Idempotent — a worktree already
    /// attached to its project's server short-circuits before logging.
    pub fn warm_primary(
        self: &Arc<Self>,
        worktree_id: AggregateId,
        project_id: AggregateId,
        env: Environment,
        root: String,
    ) {
        let me = self.clone();
        tokio::spawn(async move {
            let summary = match me.register(worktree_id, project_id, &env, &root).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(worktree_id = %worktree_id, error = %e, "lsp register skipped");
                    return;
                }
            };
            // First detected language is the primary.
            let Some(primary_id) = summary.languages.first().copied() else {
                tracing::debug!(worktree_id = %worktree_id, "no LSP languages detected");
                return;
            };
            let lang = match primary_id {
                "rust" => LspLanguage::Rust,
                "typescript-javascript" => LspLanguage::TypeScriptJavaScript,
                "php" => LspLanguage::Php,
                _ => return,
            };
            // Skip the "warmed" log when this worktree is already attached to
            // the project's server — worktree_ensure_ready can fire warm_primary
            // multiple times (active project change, panel re-render).
            let already = {
                let pools = me.pools.lock().await;
                let root_pb = PathBuf::from(&root);
                pools.dedicated.contains_key(&(worktree_id, lang))
                    || pools.projects.get(&project_id).is_some_and(|ps| {
                        ps.folders.get(&lang).is_some_and(|f| f.contains(&root_pb))
                    })
            };
            match me.ensure(worktree_id, project_id, &env, &root, lang).await {
                Ok(_) if already => {
                    tracing::trace!(worktree_id = %worktree_id, ?lang, "lsp warm noop");
                }
                Ok(_) => {
                    tracing::info!(worktree_id = %worktree_id, ?lang, "lsp warmed");
                }
                Err(e) => {
                    tracing::debug!(worktree_id = %worktree_id, error = %e, "lsp warm failed");
                }
            }
        });
    }

    fn emit(&self, event: LspStatusEvent) {
        if let Err(e) = self.app.emit("lsp:status", &event) {
            tracing::debug!(error = %e, "lsp:status emit failed");
        }
    }

    /// Resolve the LSP binary, checking the language-packs managed dir
    /// first so a freshly installed pack wins over an older system binary.
    async fn resolve_with_packs(
        &self,
        lang: LspLanguage,
    ) -> Result<(PathBuf, Vec<&'static str>), String> {
        if let Some(packs) = self.packs.lock().await.as_ref()
            && let Some(pack) = language_packs::registry()
                .iter()
                .find(|p| p.lsp_language == lang)
            && let Some(managed) = packs.resolved_binary(pack)
        {
            return Ok((managed, default_args_for(lang)));
        }
        // Fall back to the in-crate resolver (which::which on PATH).
        resolve_server(lang)
    }

    /// Binary spec for WSL spawns. We hand wsl.exe an absolute Linux path
    /// when the install lives under `~/.local/bin/` (where
    /// `language_packs_install_in_wsl` puts it), and fall back to the bare
    /// command name for distro-managed installs reachable via PATH.
    /// Resolution happens at spawn time inside the distro because Windows
    /// can't stat the WSL filesystem cheaply — we pass both forms as a
    /// shell `command -v` chain via `LspClient::spawn_wsl`.
    fn wsl_binary_name(lang: LspLanguage) -> (PathBuf, Vec<&'static str>) {
        let (cmd, args): (&str, Vec<&'static str>) = match lang {
            LspLanguage::Rust => ("rust-analyzer", vec![]),
            LspLanguage::TypeScriptJavaScript => ("typescript-language-server", vec!["--stdio"]),
            LspLanguage::Php => ("intelephense", vec!["--stdio"]),
        };
        (PathBuf::from(cmd), args)
    }
}

fn default_args_for(lang: LspLanguage) -> Vec<&'static str> {
    match lang {
        LspLanguage::Rust => vec![],
        LspLanguage::TypeScriptJavaScript => vec!["--stdio"],
        LspLanguage::Php => vec!["--stdio"],
    }
}

/// Build the `initializationOptions` for a server born with `roots`. Starts
/// from the language's lean base config ([`LspLanguage::initialization_options`])
/// and, for rust, injects `linkedProjects` for the initial root set so
/// rust-analyzer treats each worktree's `Cargo.toml` as a linked Cargo
/// workspace from the first handshake. `add_folder` later pushes an updated
/// `linkedProjects` via `didChangeConfiguration` as siblings attach. Keys are
/// prefix-less here (initializationOptions shape), unlike the section-prefixed
/// `rust_linked_projects_settings` used for config *pushes*.
fn init_options_for(lang: LspLanguage, roots: &[PathBuf]) -> Option<Value> {
    let mut base = lang.initialization_options()?;
    if lang == LspLanguage::Rust
        && let Some(obj) = base.as_object_mut()
    {
        let linked: Vec<String> = roots
            .iter()
            .map(|r| r.join("Cargo.toml").to_string_lossy().replace('\\', "/"))
            .collect();
        obj.insert("linkedProjects".into(), Value::from(linked));
    }
    Some(base)
}
