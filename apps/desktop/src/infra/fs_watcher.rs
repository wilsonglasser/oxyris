//! Worktree-scoped filesystem watcher that emits `fs:changed` events to the
//! frontend so the file tree can refresh affected directories without the
//! user manually clicking refresh.
//!
//! Windows projects only — `notify` over the WSL 9p bridge is unreliable
//! enough that we skip it and let WSL projects rely on the manual refresh
//! button (same tradeoff `IndexingService` makes; see its doc comment).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};
use oxyris_core::{AggregateId, Environment};
use oxyris_ipc::ops::{FsWatchEvent, op_name};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::infra::agent_pool::AgentPool;

const DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Serialize)]
pub struct FsChangedEvent {
    pub worktree_id: AggregateId,
    /// Worktree-relative paths that changed. Frontend resolves the parent
    /// directory of each and refreshes only those nodes.
    pub paths: Vec<String>,
}

struct WatchHandle {
    _watcher: notify::RecommendedWatcher,
    _drain: tokio::task::JoinHandle<()>,
}

pub struct FsWatchService {
    inner: Mutex<HashMap<AggregateId, Arc<WatchHandle>>>,
}

impl FsWatchService {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Start a recursive watcher for `worktree_id` rooted at `worktree_root`.
    /// Idempotent — calling twice for the same worktree is a no-op. WSL
    /// projects are silently skipped.
    pub async fn ensure(
        &self,
        app: AppHandle,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: String,
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
        match start_watcher(app, worktree_id, worktree_root) {
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

fn start_watcher(
    app: AppHandle,
    worktree_id: AggregateId,
    worktree_root: String,
) -> Result<WatchHandle, notify::Error> {
    let root = PathBuf::from(&worktree_root);
    let pending: Arc<StdMutex<HashSet<PathBuf>>> = Arc::new(StdMutex::new(HashSet::new()));
    let pending_for_cb = pending.clone();

    let mut watcher =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(event) => {
                if !matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) {
                    return;
                }
                let mut p = match pending_for_cb.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                for path in event.paths {
                    p.insert(path);
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "fs_watcher: notify error");
            }
        })?;
    watcher.watch(&root, RecursiveMode::Recursive)?;

    let drain = tokio::spawn(drain_loop(pending, app, worktree_id, root));

    Ok(WatchHandle {
        _watcher: watcher,
        _drain: drain,
    })
}

async fn drain_loop(
    pending: Arc<StdMutex<HashSet<PathBuf>>>,
    app: AppHandle,
    worktree_id: AggregateId,
    root: PathBuf,
) {
    let mut tick = tokio::time::interval(DEBOUNCE);
    tick.tick().await; // skip the immediate first tick

    loop {
        tick.tick().await;
        let batch: Vec<PathBuf> = {
            let mut p = match pending.lock() {
                Ok(g) => g,
                Err(_) => continue,
            };
            if p.is_empty() {
                continue;
            }
            p.drain().collect()
        };

        let mut rels: Vec<String> = Vec::with_capacity(batch.len());
        for path in batch {
            let rel = match path.strip_prefix(&root) {
                Ok(r) => r,
                Err(_) => continue,
            };
            // Skip our own scratch dir + git internals so we don't
            // re-render the tree on every WAL write.
            if first_segment_is(rel, ".oxyris") || first_segment_is(rel, ".git") {
                continue;
            }
            rels.push(rel.to_string_lossy().replace('\\', "/"));
        }
        if rels.is_empty() {
            continue;
        }

        let event = FsChangedEvent {
            worktree_id,
            paths: rels,
        };
        if let Err(e) = app.emit("fs:changed", &event) {
            tracing::debug!(error = %e, "fs_watcher: emit failed");
        }
    }
}

fn first_segment_is(rel: &Path, name: &str) -> bool {
    rel.components()
        .next()
        .map(|c| c.as_os_str() == name)
        .unwrap_or(false)
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
