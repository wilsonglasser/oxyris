//! Shared application state injected into Tauri commands.
//!
//! Holds the event store, projections, and anything else that is process-wide
//! and must outlive individual IPC calls. Tauri hands this out via
//! [`tauri::State`] so handlers can reach the durable layer.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use oxyris_claude::ClaudeProvider;
use oxyris_core::{Aggregate, replay};
use oxyris_provider::ProviderRegistry;
use tauri::{AppHandle, Manager};
use thiserror::Error;

use crate::domain::session::{Session, SessionCommand, SessionEvent};
use crate::infra::agent_pool::AgentPool;
use crate::infra::autopilot::AutopilotManager;
use crate::infra::event_store::{EventStore, EventStoreError};
use crate::infra::fs_watcher::FsWatchService;
use crate::infra::indexing::IndexingService;
use crate::infra::language_packs::LanguagePacksService;
use crate::infra::lsp::LspManager;
use crate::infra::lsp_bridge;
use crate::infra::observability::{self, LogGuard};
use crate::infra::projections::{ProjectionError, Projections};
use crate::infra::pty::PtySupervisor;
use crate::infra::session_supervisor::SessionSupervisor;
use std::sync::Mutex as StdMutex;

pub struct AppState {
    pub event_store: Arc<EventStore>,
    pub projections: Arc<Projections>,
    pub agent_pool: Arc<AgentPool>,
    /// Held to keep adapters alive for the lifetime of the app; callers go
    /// through `session_supervisor` rather than poking the registry directly.
    #[allow(dead_code)]
    pub providers: Arc<ProviderRegistry>,
    pub session_supervisor: SessionSupervisor,
    pub pty: Arc<PtySupervisor>,
    /// Drives engaged pure sessions toward their mission. Reacts to the PTY
    /// reader's pure-signals in-process (works with the window unfocused).
    pub autopilot: Arc<AutopilotManager>,
    pub indexing: Arc<IndexingService>,
    pub fs_watcher: Arc<FsWatchService>,
    pub lsp: Arc<LspManager>,
    pub language_packs: Arc<LanguagePacksService>,
    /// TCP port the LSP bridge is listening on (`127.0.0.1:<port>`).
    /// Filled asynchronously after boot — `None` until the listener has
    /// bound. The same `Arc` is cloned into `SessionSupervisor`; this
    /// copy on `AppState` is reserved for future Tauri commands (status
    /// chip, debugging) that need to know the port.
    #[allow(dead_code)]
    pub lsp_bridge_port: Arc<StdMutex<Option<u16>>>,
    /// Held so the async file-writer for NDJSON traces stays alive.
    #[allow(dead_code)]
    pub log_guard: LogGuard,
    /// Where trace.ndjson files are written — surfaced so Settings can show
    /// the folder / offer "Open log folder".
    pub logs_dir: PathBuf,
    /// App data root — surfaced so the Settings UI can derive paths like
    /// `keybindings.json` consistently.
    pub data_dir: PathBuf,
}

#[derive(Debug, Error)]
pub enum AppStateError {
    #[error("event store: {0}")]
    EventStore(#[from] EventStoreError),
    #[error("projections: {0}")]
    Projections(#[from] ProjectionError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl AppState {
    /// Initialize the application state under the user's data directory,
    /// creating the directory if it doesn't exist yet.
    pub fn initialize(app: AppHandle, data_dir: PathBuf) -> Result<Self, AppStateError> {
        // Resolve where the bundled agent lives in the installed app —
        // Tauri stages `bundle.resources` into `<install>/resources/`, so
        // `app.path().resource_dir()` + the bundle key gives us a stable
        // production location. Falls through to the dev/env-var resolver
        // below when running unbundled.
        let bundled_agent = app
            .path()
            .resource_dir()
            .ok()
            .map(|r| r.join("agent").join("oxyris-agent"));
        std::fs::create_dir_all(&data_dir)?;
        let logs_dir = data_dir.join("logs");
        let log_guard = observability::install(&logs_dir).map_err(AppStateError::Io)?;
        let events_path = data_dir.join("events.sqlite");
        let read_model_path = data_dir.join("read-model.sqlite");

        let event_store = Arc::new(EventStore::open(&events_path)?);
        let projections = Arc::new(Projections::open(&read_model_path)?);

        // Projections are persisted — we only rebuild when the schema needs
        // it. For a full rewrite, delete `read-model.sqlite` and restart.
        //
        // Reconcile phantom-running sessions using the projection directly,
        // not by replaying every event. This keeps boot O(N_running) instead
        // of O(total_events).
        reconcile_stopped_sessions_from_projection(&event_store, &projections)?;

        // Linux musl build of the agent. Resolution order:
        //  1. `OXYRIS_AGENT_BIN_PATH` env var (always wins — dev override).
        //  2. Bundled resource path under the Tauri install dir.
        //  3. Dev fallback: `dist/agent/oxyris-agent` walked up from the exe.
        //  4. Last-resort: `<data_dir>/agent/oxyris-agent` (legacy).
        let default_agent_path = bundled_agent.unwrap_or_else(|| {
            data_dir
                .parent()
                .map(|p| p.join("agent").join("oxyris-agent"))
                .unwrap_or_else(|| data_dir.join("oxyris-agent"))
        });
        let host_agent_path = AgentPool::resolve_host_agent_path(default_agent_path);
        let agent_pool = Arc::new(AgentPool::new(host_agent_path));

        // Providers. Claude is the only ships-for-MVP impl but the registry
        // is the extension point — drop new adapters here and nothing else
        // changes.
        let mut registry = ProviderRegistry::new();
        registry.register(Box::new(ClaudeProvider));
        let providers = Arc::new(registry);

        let app_for_cleanup = app.clone();
        let app_for_lsp = app.clone();
        let app_for_autopilot = app.clone();
        let lsp_bridge_port: Arc<StdMutex<Option<u16>>> = Arc::new(StdMutex::new(None));
        let session_supervisor = SessionSupervisor::new(
            providers.clone(),
            event_store.clone(),
            projections.clone(),
            agent_pool.clone(),
            app,
            lsp_bridge_port.clone(),
        );

        // Daily checkpoint GC on a background task — keeps the hidden
        // `refs/oxyris/cp/**` namespace from growing forever.
        spawn_checkpoint_gc(projections.clone());

        // Boot-time prune of orphan oxyris-managed docker stacks (containers,
        // volumes, networks tied to worktrees that no longer exist). Runs
        // off the boot path so a hung docker daemon doesn't block startup.
        spawn_docker_cleanup(projections.clone(), app_for_cleanup);

        // Boot-time prune of orphaned `pending-*` attachment buckets (pastes
        // that never got linked to a session). Session-id buckets are cleaned
        // on thread delete; this catches the ones delete can't reach.
        spawn_pending_attachment_sweep(data_dir.clone());

        let indexing = Arc::new(IndexingService::new(data_dir.clone(), agent_pool.clone()));
        let fs_watcher = Arc::new(FsWatchService::new());
        let app_for_packs = app_for_lsp.clone();
        let lsp = Arc::new(LspManager::new(app_for_lsp));
        let language_packs = Arc::new(LanguagePacksService::new(app_for_packs, data_dir.clone()));
        // Wire packs into LSP so freshly-installed managed binaries win
        // over older PATH entries. Done off the hot path so AppState
        // construction stays sync-friendly.
        {
            let lsp_for_wire = lsp.clone();
            let packs_for_wire = language_packs.clone();
            tauri::async_runtime::spawn(async move {
                lsp_for_wire.with_language_packs(packs_for_wire).await;
            });
        }
        // LSP TCP bridge — single shared rust-analyzer / tsserver / etc.
        // for every Claude session. MCP server proxies via TCP rather than
        // spawning duplicates. Port is captured async after binding so
        // `infra::mcp` can read it when generating the next session's
        // `mcp.json`.
        {
            let lsp_for_bridge = lsp.clone();
            let port_slot = lsp_bridge_port.clone();
            tauri::async_runtime::spawn(async move {
                match lsp_bridge::serve(lsp_for_bridge).await {
                    Ok(port) => {
                        if let Ok(mut slot) = port_slot.lock() {
                            *slot = Some(port);
                        }
                        tracing::info!(port, "lsp_bridge: ready");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "lsp_bridge: bind failed; MCP will spawn its own LSPs");
                    }
                }
            });
        }

        // Auto-pilot: the PTY supervisor pushes pure-signal notices into a
        // channel the manager drains. Wire the sink before any PTY can spawn so
        // every reader thread captures it.
        let pty = Arc::new(PtySupervisor::new());
        let (sig_tx, sig_rx) = tokio::sync::mpsc::unbounded_channel();
        pty.set_signal_sink(sig_tx);
        let autopilot = Arc::new(AutopilotManager::new(pty.clone(), app_for_autopilot));
        {
            let manager = autopilot.clone();
            tauri::async_runtime::spawn(async move { manager.run(sig_rx).await });
        }

        Ok(Self {
            event_store,
            projections,
            agent_pool,
            providers,
            session_supervisor,
            pty,
            autopilot,
            indexing,
            fs_watcher,
            lsp,
            language_packs,
            lsp_bridge_port,
            log_guard,
            logs_dir,
            data_dir,
        })
    }
}

fn spawn_checkpoint_gc(projections: Arc<Projections>) {
    use crate::infra::checkpoint;
    use std::time::Duration;

    // `tauri::async_runtime::spawn` goes onto the runtime Tauri itself uses,
    // which is available from `setup()` — plain `tokio::spawn` panics here
    // because we're not yet inside that reactor's context.
    tauri::async_runtime::spawn(async move {
        // First pass runs ~30s after boot to avoid competing with startup I/O.
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            let projects = match projections.list_projects() {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::warn!(error = %e, "checkpoint gc: list_projects failed");
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    continue;
                }
            };
            for p in projects {
                let root_path = p.root_path.clone();
                let project_id = p.id;
                // `spawn_blocking` needs a runtime handle too — go through
                // Tauri's so we don't assume a plain tokio context.
                let res = tauri::async_runtime::spawn_blocking(move || {
                    checkpoint::gc(std::path::Path::new(&root_path), 30)
                })
                .await;
                match res {
                    Ok(Ok(n)) if n > 0 => {
                        tracing::info!(%project_id, removed = n, "checkpoint gc");
                    }
                    Ok(Err(e)) => {
                        tracing::debug!(%project_id, error = %e, "checkpoint gc: project skipped");
                    }
                    _ => {}
                }
            }
            // Every 24h afterwards.
            tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
        }
    });
}

fn spawn_docker_cleanup(projections: Arc<Projections>, app: AppHandle) {
    use crate::infra::docker_cleanup;
    use std::time::Duration;
    use tauri::Emitter;

    tauri::async_runtime::spawn(async move {
        // Wait a few seconds before touching docker so we don't race the
        // daemon coming up alongside Oxyris.
        tokio::time::sleep(Duration::from_secs(5)).await;
        let report = docker_cleanup::prune_orphans_for_all(&projections).await;
        // Only ping the UI when something was actually pruned — silent
        // boots stay silent.
        if !report.orphan_projects.is_empty() {
            let _ = app.emit("docker:cleanup", &report);
        }
    });
}

fn spawn_pending_attachment_sweep(data_dir: std::path::PathBuf) {
    tauri::async_runtime::spawn(async move {
        // std::fs work — keep it off the async runtime's worker thread.
        let _ = tauri::async_runtime::spawn_blocking(move || {
            crate::tauri_commands::attachments::sweep_stale_pending(&data_dir);
        })
        .await;
    });
}

fn reconcile_stopped_sessions_from_projection(
    event_store: &EventStore,
    projections: &Projections,
) -> Result<(), AppStateError> {
    // Only touch sessions the projection says are running — everything else
    // is already at rest.
    let summaries = projections
        .list_running_sessions()
        .map_err(EventStoreError::from_projection)?;

    let now = Utc::now();
    for summary in summaries {
        let session_id = summary.id;
        // Fold just this session's events to get its current version/state.
        let events = event_store.load(Session::KIND, session_id)?;
        let mut typed = Vec::with_capacity(events.len());
        for s in &events {
            let event: SessionEvent = serde_json::from_value(s.payload.clone())
                .map_err(EventStoreError::Serialization)?;
            typed.push(event);
        }
        let state = replay::<Session>(&typed);
        let Some(data) = state.inner.as_ref() else {
            continue;
        };
        if !matches!(data.status, crate::domain::session::SessionStatus::Running) {
            continue;
        }
        let new_events = match Session::decide(&state, SessionCommand::Stop { now }) {
            Ok(ev) => ev,
            Err(e) => {
                tracing::warn!(error = %e, %session_id, "reconcile: decide failed");
                continue;
            }
        };
        if new_events.is_empty() {
            continue;
        }
        let current_version = events.last().map(|s| s.version).unwrap_or(0);
        let stored_new =
            event_store.append(Session::KIND, session_id, current_version, &new_events)?;
        for s in &stored_new {
            projections.apply(s)?;
        }
        tracing::info!(%session_id, "reconciled phantom-running session to stopped");
    }
    Ok(())
}
