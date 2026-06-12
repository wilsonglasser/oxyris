//! Per-worktree symbol index orchestration.
//!
//! Holds an [`oxyris_index::Index`] per worktree, opened lazily on first
//! query or rebuild. Incremental updates are driven by the filesystem
//! watcher, not by this module — it just exposes `apply_*_changes` sinks the
//! watcher calls (see [`crate::infra::fs_watcher`]).
//!
//! - **Windows worktrees**: index lives at `<worktree>/.oxyris/index.db` so
//!   it travels with the worktree (and is dropped when the worktree is
//!   removed). `FsWatchService`'s [`oxyris_watch::DirWatcher`] feeds
//!   [`IndexingService::apply_local_changes`].
//! - **WSL worktrees**: SQLite needs random-access local FS, so we keep the
//!   DB at `<data_dir>/wsl-index/<short>.db` on the Windows side and walk
//!   files via the agent's `fs.walk` + `fs.read` ops. The agent runs the
//!   same ignore-aware watcher *inside* the distro (native inotify, reliable
//!   unlike `notify` over 9p) and streams changes back to
//!   [`IndexingService::apply_wsl_changes`]. A full rebuild still backs the
//!   cold-start / manual `ensure_ready` path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;
use oxyris_core::{AggregateId, Environment};
use oxyris_index::{Index, IndexStats, Lang, ProjectMap, Symbol, SymbolHit, SymbolKind};
use oxyris_ipc::ops::{
    IndexListInFileArgs, IndexQuerySymbolArgs, IndexRebuildProgress, IndexRebuildReport,
    IndexRootArgs, op_name,
};
use serde::Serialize;
use thiserror::Error;
use tokio::sync::{Mutex, mpsc};

use crate::infra::agent_pool::AgentPool;

const MAX_FILE_BYTES: u64 = 1_000_000;

#[derive(Debug, Error)]
pub enum IndexingError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("index: {0}")]
    Index(#[from] oxyris_index::IndexError),
    #[error("worktree path is invalid: {0}")]
    InvalidWorktreePath(String),
    #[error("agent: {0}")]
    Agent(String),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize)]
pub struct RebuildReport {
    pub files_indexed: u64,
    pub symbols_extracted: u64,
    pub files_skipped: u64,
    pub bytes_read: u64,
    pub duration_ms: u128,
}

/// Streamed updates from `rebuild`. The `worktree_id` rides on every
/// message so the frontend can route updates to the right project tab.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum IndexingProgress {
    Started {
        worktree_id: AggregateId,
        total_files: u64,
    },
    Progress {
        worktree_id: AggregateId,
        files_indexed: u64,
        files_skipped: u64,
    },
    Done {
        worktree_id: AggregateId,
        report: RebuildReport,
    },
    Failed {
        worktree_id: AggregateId,
        error: String,
    },
}

pub type ProgressSender = mpsc::UnboundedSender<IndexingProgress>;

struct IndexEntry {
    index: Arc<Index>,
}

pub struct IndexingService {
    entries: Mutex<HashMap<AggregateId, IndexEntry>>,
    agent_pool: Arc<AgentPool>,
}

impl IndexingService {
    pub fn new(agent_pool: Arc<AgentPool>) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            agent_pool,
        }
    }

    /// Open (or reuse) the in-process on-disk index for a **Windows** worktree
    /// (`<worktree>/.oxyris/index.db`). WSL worktrees keep their index inside
    /// the distro (the agent owns it) and must never reach here — callers
    /// branch on `env` and delegate WSL ops to the agent instead.
    ///
    /// Incremental updates are driven externally by
    /// [`FsWatchService`](crate::infra::fs_watcher::FsWatchService) via
    /// [`apply_local_changes`](Self::apply_local_changes), so opening installs
    /// no watcher of its own.
    pub async fn open_for(
        &self,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
    ) -> Result<Arc<Index>, IndexingError> {
        if !matches!(env, Environment::Local) {
            return Err(IndexingError::Agent(
                "open_for is Windows-only; WSL indexes live in the distro".to_owned(),
            ));
        }
        let mut entries = self.entries.lock().await;
        if let Some(entry) = entries.get(&worktree_id) {
            return Ok(entry.index.clone());
        }
        let db_path = self.index_db_path(worktree_root);
        let idx = tokio::task::spawn_blocking(move || Index::open(&db_path))
            .await
            .map_err(|e| IndexingError::Io(std::io::Error::other(e.to_string())))??;
        let index = Arc::new(idx);
        entries.insert(
            worktree_id,
            IndexEntry {
                index: index.clone(),
            },
        );
        Ok(index)
    }

    /// Apply a batch of changed absolute paths to a Windows worktree's index
    /// incrementally — the sink the [`FsWatchService`] watcher calls. Each path
    /// is re-indexed if its mtime moved, removed if it's gone, and skipped if
    /// it's ignored / unparseable. No-op for non-Local environments (WSL is
    /// handled inside the agent).
    ///
    /// [`FsWatchService`]: crate::infra::fs_watcher::FsWatchService
    pub async fn apply_local_changes(
        &self,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
        paths: Vec<PathBuf>,
    ) {
        if !matches!(env, Environment::Local) {
            return;
        }
        let index = match self.open_for(worktree_id, env, worktree_root).await {
            Ok(i) => i,
            Err(e) => {
                tracing::debug!(%worktree_id, error = %e, "apply_local_changes: open failed");
                return;
            }
        };
        let root = PathBuf::from(worktree_root);
        let _ = tokio::task::spawn_blocking(move || {
            for path in paths {
                process_change(&path, &root, &index);
            }
        })
        .await;
    }

    // ── symbol queries ──────────────────────────────────────────────────
    //
    // Local worktrees query the in-process `Index` directly; WSL worktrees
    // delegate to the agent (the index lives inside the distro), deserializing
    // the same `oxyris_index` result types off the wire.

    /// Find symbols by name (optionally filtered by kind), capped at `limit`.
    pub async fn query_symbol(
        &self,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
        name: String,
        kind: Option<SymbolKind>,
        limit: u32,
    ) -> Result<Vec<SymbolHit>, IndexingError> {
        match env {
            Environment::Local => {
                let index = self.open_for(worktree_id, env, worktree_root).await?;
                tokio::task::spawn_blocking(move || index.find_symbol(&name, kind, limit))
                    .await
                    .map_err(|e| IndexingError::Io(std::io::Error::other(e.to_string())))?
                    .map_err(IndexingError::Index)
            }
            Environment::Wsl { distro } => {
                let args = serde_json::to_value(IndexQuerySymbolArgs {
                    root: worktree_root.to_owned(),
                    name,
                    kind: kind.map(|k| k.as_str().to_owned()),
                    limit,
                })?;
                let v = self
                    .agent_pool
                    .call(distro, op_name::INDEX_QUERY_SYMBOL, args)
                    .await
                    .map_err(|e| IndexingError::Agent(e.to_string()))?;
                Ok(serde_json::from_value(v)?)
            }
        }
    }

    /// List the symbols declared in one file (worktree-relative path).
    pub async fn list_symbols_in_file(
        &self,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
        file: String,
    ) -> Result<Vec<Symbol>, IndexingError> {
        match env {
            Environment::Local => {
                let index = self.open_for(worktree_id, env, worktree_root).await?;
                tokio::task::spawn_blocking(move || index.list_symbols_in_file(&file))
                    .await
                    .map_err(|e| IndexingError::Io(std::io::Error::other(e.to_string())))?
                    .map_err(IndexingError::Index)
            }
            Environment::Wsl { distro } => {
                let args = serde_json::to_value(IndexListInFileArgs {
                    root: worktree_root.to_owned(),
                    file,
                })?;
                let v = self
                    .agent_pool
                    .call(distro, op_name::INDEX_LIST_IN_FILE, args)
                    .await
                    .map_err(|e| IndexingError::Agent(e.to_string()))?;
                Ok(serde_json::from_value(v)?)
            }
        }
    }

    /// Directory-rollup view of the index for the project map UI.
    pub async fn project_map(
        &self,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
    ) -> Result<ProjectMap, IndexingError> {
        match env {
            Environment::Local => {
                let index = self.open_for(worktree_id, env, worktree_root).await?;
                tokio::task::spawn_blocking(move || index.project_map())
                    .await
                    .map_err(|e| IndexingError::Io(std::io::Error::other(e.to_string())))?
                    .map_err(IndexingError::Index)
            }
            Environment::Wsl { distro } => {
                let v = self
                    .agent_wsl(distro, op_name::INDEX_PROJECT_MAP, worktree_root)
                    .await?;
                Ok(serde_json::from_value(v)?)
            }
        }
    }

    /// `(files, symbols)` counts.
    pub async fn stats(
        &self,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
    ) -> Result<IndexStats, IndexingError> {
        match env {
            Environment::Local => {
                let index = self.open_for(worktree_id, env, worktree_root).await?;
                tokio::task::spawn_blocking(move || index.stats())
                    .await
                    .map_err(|e| IndexingError::Io(std::io::Error::other(e.to_string())))?
                    .map_err(IndexingError::Index)
            }
            Environment::Wsl { distro } => {
                let v = self
                    .agent_wsl(distro, op_name::INDEX_STATS, worktree_root)
                    .await?;
                Ok(serde_json::from_value(v)?)
            }
        }
    }

    /// Call a root-scoped agent index op (`stats`, `project_map`, …).
    async fn agent_wsl(
        &self,
        distro: &str,
        op: &str,
        worktree_root: &str,
    ) -> Result<serde_json::Value, IndexingError> {
        let args = serde_json::to_value(IndexRootArgs {
            root: worktree_root.to_owned(),
        })?;
        self.agent_pool
            .call(distro, op, args)
            .await
            .map_err(|e| IndexingError::Agent(e.to_string()))
    }

    /// On-disk index location for a Windows worktree (in-tree, travels with
    /// it). WSL indexes live inside the distro and are never opened here.
    fn index_db_path(&self, worktree_root: &str) -> PathBuf {
        PathBuf::from(worktree_root)
            .join(".oxyris")
            .join("index.db")
    }

    /// Drop the cached entry: closes the SQLite connection on last reference
    /// and tears down the watcher. Call when the worktree is removed.
    #[allow(dead_code)]
    pub async fn close(&self, worktree_id: AggregateId) {
        self.entries.lock().await.remove(&worktree_id);
    }

    /// Cheap "ensure the index is populated" — if the on-disk DB already
    /// has any files indexed, return early. Otherwise do a full rebuild.
    /// Existing worktrees from before this feature shipped land here on
    /// first activation; new ones go straight through `rebuild` from
    /// `worktree_create` and never see this path.
    pub async fn ensure_indexed(
        &self,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
        progress: Option<ProgressSender>,
    ) -> Result<RebuildReport, IndexingError> {
        let populated = self
            .stats(worktree_id, env, worktree_root)
            .await
            .map(|s| s.files > 0)
            .unwrap_or(false);
        if populated {
            // Already populated — the watcher keeps it incremental from here.
            return Ok(empty_report());
        }
        self.rebuild(worktree_id, env, worktree_root, progress)
            .await
    }

    /// Walk the worktree, re-indexing every file we have a parser for.
    /// Drops files that have disappeared. Respects `.gitignore` and the
    /// standard ignore conventions of the `ignore` crate. For WSL the walk +
    /// parse run inside the distro (agent `index.rebuild`); only progress and
    /// the final report cross stdio.
    pub async fn rebuild(
        &self,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
        progress: Option<ProgressSender>,
    ) -> Result<RebuildReport, IndexingError> {
        match env {
            Environment::Local => {
                let index = self.open_for(worktree_id, env, worktree_root).await?;
                let root = PathBuf::from(worktree_root);
                if !root.is_dir() {
                    return Err(IndexingError::InvalidWorktreePath(worktree_root.to_owned()));
                }
                tokio::task::spawn_blocking(move || {
                    rebuild_blocking(worktree_id, &root, &index, progress)
                })
                .await
                .map_err(|e| IndexingError::Io(std::io::Error::other(e.to_string())))?
            }
            Environment::Wsl { distro } => {
                rebuild_wsl(
                    self.agent_pool.clone(),
                    distro.clone(),
                    worktree_id,
                    worktree_root,
                    progress,
                )
                .await
            }
        }
    }
}

/// WSL-side rebuild: the walk + tree-sitter parse + SQLite write all run inside
/// the distro (agent `index.rebuild`). The backend just forwards the streamed
/// progress to the UI and maps the final report — no file contents cross stdio.
async fn rebuild_wsl(
    agent: Arc<AgentPool>,
    distro: String,
    worktree_id: AggregateId,
    worktree_root: &str,
    progress: Option<ProgressSender>,
) -> Result<RebuildReport, IndexingError> {
    let args = serde_json::to_value(IndexRootArgs {
        root: worktree_root.to_owned(),
    })?;
    let (mut events_rx, final_result) = agent
        .call_streaming(&distro, op_name::INDEX_REBUILD, args)
        .await
        .map_err(|e| IndexingError::Agent(e.to_string()))?;

    // `call_streaming` resolves once the op is done, so every progress event is
    // already buffered. Forward them: the first carries the file total
    // (→ Started), the rest are running counts (→ Progress).
    let mut started_sent = false;
    while let Some(event) = events_rx.recv().await {
        let Ok(p) = serde_json::from_value::<IndexRebuildProgress>(event) else {
            continue;
        };
        if let Some(tx) = progress.as_ref() {
            if !started_sent {
                let _ = tx.send(IndexingProgress::Started {
                    worktree_id,
                    total_files: p.total_files,
                });
                started_sent = true;
            } else {
                let _ = tx.send(IndexingProgress::Progress {
                    worktree_id,
                    files_indexed: p.files_indexed,
                    files_skipped: p.files_skipped,
                });
            }
        }
    }

    let value = final_result.map_err(|e| IndexingError::Agent(e.to_string()))?;
    let wire: IndexRebuildReport = serde_json::from_value(value)?;
    let report = RebuildReport {
        files_indexed: wire.files_indexed,
        symbols_extracted: wire.symbols_extracted,
        files_skipped: wire.files_skipped,
        bytes_read: wire.bytes_read,
        duration_ms: wire.duration_ms,
    };
    if let Some(tx) = progress.as_ref() {
        let _ = tx.send(IndexingProgress::Done {
            worktree_id,
            report: report.clone(),
        });
    }
    Ok(report)
}

fn rebuild_blocking(
    worktree_id: AggregateId,
    root: &Path,
    index: &Index,
    progress: Option<ProgressSender>,
) -> Result<RebuildReport, IndexingError> {
    let started = Instant::now();
    let mut files_indexed: u64 = 0;
    let mut symbols_extracted: u64 = 0;
    let mut files_skipped: u64 = 0;
    let mut bytes_read: u64 = 0;

    // First pass: count indexable files cheaply so the UI can render a
    // determinate progress bar instead of a spinner. ~ms even for big repos.
    let total_files = if progress.is_some() {
        count_indexable(root)
    } else {
        0
    };
    if let Some(tx) = progress.as_ref() {
        let _ = tx.send(IndexingProgress::Started {
            worktree_id,
            total_files,
        });
    }

    let walker = WalkBuilder::new(root)
        .standard_filters(true)
        .hidden(true)
        // Don't crawl `.oxyris/` itself — that's where our DB lives.
        .filter_entry(|e| {
            e.file_name()
                .to_str()
                .map(|n| n != ".oxyris")
                .unwrap_or(true)
        })
        .build();

    for result in walker {
        let entry = match result {
            Ok(e) => e,
            Err(err) => {
                tracing::debug!(error = %err, "indexing: walker entry error");
                continue;
            }
        };
        if !entry.file_type().is_some_and(|f| f.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(lang) = Lang::from_path(path) else {
            continue;
        };
        if is_generated(path, root) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => {
                files_skipped += 1;
                continue;
            }
        };
        if metadata.len() > MAX_FILE_BYTES {
            files_skipped += 1;
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => {
                files_skipped += 1;
                continue;
            }
        };
        let mtime = mtime_secs(&metadata);
        let relative = match path.strip_prefix(root) {
            Ok(p) => p.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        match index.index_file(&relative, lang, mtime, &content) {
            Ok(n) => {
                files_indexed += 1;
                symbols_extracted += n as u64;
                bytes_read += metadata.len();
            }
            Err(e) => {
                tracing::debug!(file = %relative, error = %e, "indexing: file failed");
                files_skipped += 1;
            }
        }
        // Emit progress every 25 files so the UI moves smoothly without
        // flooding Tauri events.
        if let Some(tx) = progress.as_ref()
            && (files_indexed + files_skipped).is_multiple_of(25)
        {
            let _ = tx.send(IndexingProgress::Progress {
                worktree_id,
                files_indexed,
                files_skipped,
            });
        }
    }

    let report = RebuildReport {
        files_indexed,
        symbols_extracted,
        files_skipped,
        bytes_read,
        duration_ms: started.elapsed().as_millis(),
    };
    if let Some(tx) = progress.as_ref() {
        let _ = tx.send(IndexingProgress::Done {
            worktree_id,
            report: report.clone(),
        });
    }
    Ok(report)
}

/// Cheap pre-walk that just counts files we'd index. Single pass over the
/// directory tree using the same ignore filter as the real walk, no I/O on
/// content. Returns 0 on error so the caller can still proceed.
fn count_indexable(root: &Path) -> u64 {
    let walker = WalkBuilder::new(root)
        .standard_filters(true)
        .hidden(true)
        .filter_entry(|e| {
            e.file_name()
                .to_str()
                .map(|n| n != ".oxyris")
                .unwrap_or(true)
        })
        .build();
    let mut count: u64 = 0;
    for result in walker.flatten() {
        if !result.file_type().is_some_and(|f| f.is_file()) {
            continue;
        }
        if Lang::from_path(result.path()).is_some() && !is_generated(result.path(), root) {
            count += 1;
        }
    }
    count
}

/// Skip minified/bundled/vendored files so the symbol index isn't polluted
/// with garbage (single-letter symbols from `*.min.js`, etc.). Mirrors the
/// path-search filter via the shared [`oxyris_ipc::ops::is_generated_path`].
fn is_generated(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .ok()
        .map(|rel| oxyris_ipc::ops::is_generated_path(&rel.to_string_lossy()))
        .unwrap_or(false)
}

fn empty_report() -> RebuildReport {
    RebuildReport {
        files_indexed: 0,
        symbols_extracted: 0,
        files_skipped: 0,
        bytes_read: 0,
        duration_ms: 0,
    }
}

fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        })
}

// ────── incremental re-index ────────────────────────────────────────────────

/// Re-index (or drop) a single changed path. Called for each path in a watcher
/// batch via [`IndexingService::apply_local_changes`]. Skips `.oxyris/` and
/// generated/vendored paths, removes vanished files, and re-indexes the rest
/// only when the mtime actually moved.
fn process_change(path: &Path, root: &Path, index: &Index) {
    let rel = match path.strip_prefix(root) {
        Ok(r) => r,
        Err(_) => return,
    };
    // Skip `.oxyris/` so our own SQLite WAL writes don't trigger a re-index
    // storm. (The watcher fires on the WAL file too.)
    if rel.starts_with(".oxyris") {
        return;
    }
    let relative = rel.to_string_lossy().replace('\\', "/");
    if oxyris_ipc::ops::is_generated_path(&relative) {
        return;
    }

    let exists = path.exists();
    if !exists {
        if let Err(e) = index.remove_file(&relative) {
            tracing::debug!(file = %relative, error = %e, "watcher: remove_file failed");
        }
        return;
    }
    if !path.is_file() {
        return;
    }
    let Some(lang) = Lang::from_path(path) else {
        return;
    };
    let metadata = match path.metadata() {
        Ok(m) => m,
        Err(_) => return,
    };
    if metadata.len() > MAX_FILE_BYTES {
        return;
    }
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let mtime = mtime_secs(&metadata);
    // `if_changed`: a watcher can fire on a touch/metadata-only event; skip the
    // tree-sitter reparse when the mtime hasn't moved.
    if let Err(e) = index.index_file_if_changed(&relative, lang, mtime, &content) {
        tracing::debug!(file = %relative, error = %e, "reindex: index_file failed");
    }
}
