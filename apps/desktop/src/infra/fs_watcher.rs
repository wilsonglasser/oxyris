//! Worktree-scoped filesystem watcher that emits `fs:changed` events to the
//! frontend (so the file tree refreshes without a manual click) **and** drives
//! the symbol index incrementally — one watch feeding both consumers.
//!
//! Windows projects use a native [`oxyris_watch::DirWatcher`] (ignore-aware,
//! per-directory, so `node_modules`/`target` are never watched). WSL projects
//! delegate to the per-distro agent (see [`WslFsWatchService`]), which runs the
//! same watcher inside the distro on native inotify — `notify` over the 9p
//! `\\wsl.localhost` bridge is unreliable, so we never watch it from Windows.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use oxyris_core::{AggregateId, Environment};
use oxyris_ipc::ops::{FsWatchEvent, op_name};
use oxyris_watch::DirWatcher;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::infra::agent_pool::AgentPool;
use crate::infra::indexing::IndexingService;

const DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Serialize)]
pub struct FsChangedEvent {
    pub worktree_id: AggregateId,
    /// Worktree-relative paths that changed. Frontend resolves the parent
    /// directory of each and refreshes only those nodes.
    pub paths: Vec<String>,
}

pub struct FsWatchService {
    inner: Mutex<HashMap<AggregateId, Arc<DirWatcher>>>,
}

impl FsWatchService {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Start the watcher for `worktree_id` rooted at `worktree_root`. Idempotent
    /// — calling twice for the same worktree is a no-op. WSL projects are
    /// silently skipped (the agent-side [`WslFsWatchService`] covers them).
    ///
    /// The watcher's sink does two things per debounced batch: emit `fs:changed`
    /// to the frontend, and hand the changed paths to [`IndexingService`] for an
    /// incremental re-index. That replaces the old standalone index watcher, so
    /// each worktree now has exactly one OS watch instead of two.
    pub async fn ensure(
        &self,
        app: AppHandle,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: String,
        indexing: Arc<IndexingService>,
    ) {
        if !matches!(env, Environment::Local) {
            return;
        }
        {
            let map = self.inner.lock().await;
            if map.contains_key(&worktree_id) {
                return;
            }
        }

        let root = PathBuf::from(&worktree_root);
        let root_for_rel = root.clone();
        let root_str = worktree_root.clone();
        let wid = worktree_id;
        let sink = move |batch: Vec<PathBuf>| {
            // 1. Tree refresh — worktree-relative, forward slashes. Ignored
            //    paths (`node_modules`, `.oxyris`, …) never reach here; the
            //    DirWatcher filters them before the sink.
            let rels: Vec<String> = batch
                .iter()
                .filter_map(|p| {
                    p.strip_prefix(&root_for_rel)
                        .ok()
                        .map(|r| r.to_string_lossy().replace('\\', "/"))
                })
                .collect();
            if !rels.is_empty() {
                let event = FsChangedEvent {
                    worktree_id: wid,
                    paths: rels,
                };
                if let Err(e) = app.emit("fs:changed", &event) {
                    tracing::debug!(error = %e, "fs_watcher: emit failed");
                }
            }
            // 2. Incremental index update — offloaded so the sink stays cheap.
            let indexing = indexing.clone();
            let root_str = root_str.clone();
            tokio::spawn(async move {
                indexing
                    .apply_local_changes(wid, &Environment::Local, &root_str, batch)
                    .await;
            });
        };

        match DirWatcher::start(root, DEBOUNCE, sink) {
            Ok(handle) => {
                let mut map = self.inner.lock().await;
                map.insert(worktree_id, Arc::new(handle));
            }
            Err(e) => {
                tracing::warn!(%worktree_id, error = %e, "fs_watcher: failed to install");
            }
        }
    }
}

impl Default for FsWatchService {
    fn default() -> Self {
        Self::new()
    }
}

/// WSL counterpart to [`FsWatchService`]. The Windows `notify` watcher can't
/// see into a distro reliably (9p over `\\wsl.localhost`), so instead we ask
/// the per-distro agent to run a native inotify watcher *inside* the distro and
/// stream change batches back over stdio. We re-emit them as the very same
/// `fs:changed` event Windows projects use, so the frontend needs no special
/// handling for WSL.
pub struct WslFsWatchService {
    pool: Arc<AgentPool>,
    /// Worktrees with a live watch. Guards against re-arming on every dir list.
    armed: Arc<Mutex<HashSet<AggregateId>>>,
}

impl WslFsWatchService {
    pub fn new(pool: Arc<AgentPool>) -> Self {
        Self {
            pool,
            armed: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Start watching `worktree_root` inside the distro (idempotent per
    /// worktree). No-op for non-WSL projects. If the agent later dies, the
    /// stream ends and the worktree is disarmed so the next dir listing
    /// re-arms it against a fresh agent.
    ///
    /// Emits a `fs:changed` event per change batch for the file tree. The
    /// symbol index is kept fresh by the agent itself — the same in-distro
    /// watcher updates the in-distro index directly, so nothing index-related
    /// crosses stdio here.
    pub async fn ensure(
        &self,
        app: AppHandle,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: String,
    ) {
        let Environment::Wsl { distro } = env else {
            return;
        };
        {
            // `insert` returns false when already present → already armed.
            let mut armed = self.armed.lock().await;
            if !armed.insert(worktree_id) {
                return;
            }
        }

        let distro = distro.clone();
        let pool = self.pool.clone();
        let armed = self.armed.clone();
        tokio::spawn(async move {
            let args = serde_json::json!({ "root": worktree_root });
            let (mut rx, watch_id) = match pool
                .call_open_stream(&distro, op_name::FS_WATCH, args)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(%worktree_id, error = %e, "wsl_fs_watcher: failed to start");
                    armed.lock().await.remove(&worktree_id);
                    return;
                }
            };
            tracing::debug!(%worktree_id, %distro, %watch_id, "wsl_fs_watcher: armed");

            while let Some(payload) = rx.recv().await {
                match serde_json::from_value::<FsWatchEvent>(payload) {
                    Ok(ev) => {
                        if ev.paths.is_empty() {
                            continue;
                        }
                        let event = FsChangedEvent {
                            worktree_id,
                            paths: ev.paths,
                        };
                        if let Err(e) = app.emit("fs:changed", &event) {
                            tracing::debug!(error = %e, "wsl_fs_watcher: emit failed");
                        }
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "wsl_fs_watcher: bad event payload");
                    }
                }
            }

            // Stream closed (agent died or was cancelled) — disarm so a later
            // dir listing can re-arm.
            armed.lock().await.remove(&worktree_id);
            tracing::debug!(%worktree_id, "wsl_fs_watcher: stream ended");
        });
    }
}
