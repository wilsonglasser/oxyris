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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxyris_core::{AggregateId, Environment};
use oxyris_lsp::{LspClient, LspLanguage, detect_languages, resolve_server};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::infra::language_packs::{self, LanguagePacksService};

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

struct WorktreeEntry {
    workspace: PathBuf,
    detected: Vec<LspLanguage>,
    clients: HashMap<LspLanguage, Arc<LspClient>>,
}

pub struct LspManager {
    app: AppHandle,
    /// Optional — when set, we prefer binaries managed by the language
    /// packs service (under `<data_dir>/lsp/`) over PATH. Wired by
    /// `AppState` after both services exist.
    packs: Mutex<Option<Arc<LanguagePacksService>>>,
    entries: Mutex<HashMap<AggregateId, WorktreeEntry>>,
}

impl LspManager {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            packs: Mutex::new(None),
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Inject the language-packs service so the manager can prefer
    /// freshly-installed managed binaries over whatever's on PATH. Called
    /// once at boot from `AppState::initialize`.
    pub async fn with_language_packs(&self, packs: Arc<LanguagePacksService>) {
        *self.packs.lock().await = Some(packs);
    }

    /// Walk the workspace markers, register the entry. Idempotent.
    pub async fn register(
        &self,
        worktree_id: AggregateId,
        _env: &Environment,
        worktree_root: &str,
    ) -> Result<LspWorktreeSummary, LspManagerError> {
        let mut entries = self.entries.lock().await;
        let entry = entries.entry(worktree_id).or_insert_with(|| WorktreeEntry {
            workspace: PathBuf::from(worktree_root),
            detected: detect_languages(Path::new(worktree_root)),
            clients: HashMap::new(),
        });
        Ok(LspWorktreeSummary {
            worktree_id,
            languages: entry.detected.iter().map(|l| l.id()).collect(),
        })
    }

    /// Drop a worktree's pool — the kill_on_drop on every spawned LSP
    /// reaps its child. Call when the worktree is removed.
    #[allow(dead_code)]
    pub async fn close(&self, worktree_id: AggregateId) {
        self.entries.lock().await.remove(&worktree_id);
    }

    /// Find or create an entry for a workspace path. Used by the LSP
    /// bridge so the MCP server (a separate process) can reuse clients
    /// already spawned via worktree warm-up. Reuses the existing entry
    /// when its `workspace` matches; otherwise creates one with a
    /// synthetic id (the bridge has no AggregateId — only the path).
    pub async fn ensure_at(
        &self,
        workspace_root: &Path,
        env: &Environment,
        lang: LspLanguage,
    ) -> Result<Arc<LspClient>, LspManagerError> {
        let existing = {
            let entries = self.entries.lock().await;
            entries
                .iter()
                .find(|(_, e)| e.workspace == workspace_root)
                .map(|(id, _)| *id)
        };
        let id = existing.unwrap_or_else(AggregateId::new);
        let root = workspace_root.to_string_lossy().into_owned();
        self.ensure(id, env, &root, lang).await
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

    /// Spawn (or reuse) the LSP for `lang` on this worktree. Emits status
    /// events on every transition so the UI can show progress.
    pub async fn ensure(
        &self,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
        lang: LspLanguage,
    ) -> Result<Arc<LspClient>, LspManagerError> {
        // Fast path — already up. Cache hit, return without re-emitting
        // status (caller's "warmed" log only fires once per real spawn).
        {
            let entries = self.entries.lock().await;
            if let Some(entry) = entries.get(&worktree_id)
                && let Some(client) = entry.clients.get(&lang)
            {
                tracing::trace!(worktree_id = %worktree_id, ?lang, "lsp cache hit");
                return Ok(client.clone());
            }
        }

        let (binary, args) = match env {
            Environment::Windows => match self.resolve_with_packs(lang).await {
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

        let mut entries = self.entries.lock().await;
        let entry = entries.entry(worktree_id).or_insert_with(|| WorktreeEntry {
            workspace: PathBuf::from(worktree_root),
            detected: detect_languages(Path::new(worktree_root)),
            clients: HashMap::new(),
        });
        if let Some(client) = entry.clients.get(&lang) {
            return Ok(client.clone());
        }

        let workspace = entry.workspace.clone();
        // For WSL projects, the binary string is the *Linux-side* binary
        // name (e.g. `rust-analyzer`) and we spawn via wsl.exe. For
        // Windows projects, `binary` is the absolute Windows path.
        tracing::info!(
            worktree_id = %worktree_id,
            ?lang,
            binary = %binary.display(),
            ?args,
            workspace = %workspace.display(),
            "lsp spawn",
        );
        let result = match env {
            Environment::Windows => LspClient::spawn(&binary, &args, &workspace).await,
            Environment::Wsl { distro } => {
                let binary_str = binary.to_string_lossy().into_owned();
                LspClient::spawn_wsl(distro, &binary_str, &args, &workspace).await
            }
        };
        match result {
            Ok(client) => {
                entry.clients.insert(lang, client.clone());
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

    /// Eager pre-warm: detect the primary language and spawn its LSP in
    /// the background. Failures are silent (status event already
    /// reports them) so callers don't have to handle the error path.
    /// Idempotent — concurrent calls collapse to the same in-flight spawn
    /// via the entries Mutex; cache-hit short-circuits before logging.
    pub fn warm_primary(
        self: &Arc<Self>,
        worktree_id: AggregateId,
        env: Environment,
        root: String,
    ) {
        let me = self.clone();
        tokio::spawn(async move {
            let summary = match me.register(worktree_id, &env, &root).await {
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
            // Skip the "warmed" log when the client was already cached —
            // worktree_ensure_ready can fire warm_primary multiple times
            // (active project change, panel re-render) and we don't want
            // log spam for cache hits.
            let already_cached = {
                let entries = me.entries.lock().await;
                entries
                    .get(&worktree_id)
                    .is_some_and(|e| e.clients.contains_key(&lang))
            };
            match me.ensure(worktree_id, &env, &root, lang).await {
                Ok(_) if already_cached => {
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
