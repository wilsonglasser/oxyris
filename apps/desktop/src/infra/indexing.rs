//! Per-worktree symbol index orchestration.
//!
//! Holds an [`oxyris_index::Index`] per worktree, opened lazily on first
//! query or rebuild.
//!
//! - **Windows worktrees**: index lives at `<worktree>/.oxyris/index.db`
//!   so it travels with the worktree (and is dropped when the worktree is
//!   removed). A [`notify`] file watcher debounces changes and re-indexes
//!   modified files automatically.
//! - **WSL worktrees**: SQLite needs random-access local FS, so we keep
//!   the DB at `<data_dir>/wsl-index/<short>.db` on the Windows side and
//!   walk files via the agent's `fs.walk` + `fs.read` ops. No watcher
//!   (`notify` over 9p is unreliable); manual `ensure_ready` rebuilds.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;
use notify::{EventKind, RecursiveMode, Watcher};
use oxyris_core::{AggregateId, Environment};
use oxyris_index::{Index, Lang};
use oxyris_ipc::ops::{FsReadArgs, FsReadResult, FsWalkArgs, FsWalkEvent, op_name};
use serde::Serialize;
use thiserror::Error;
use tokio::sync::{Mutex, mpsc};

use crate::infra::agent_pool::AgentPool;
use crate::infra::env_template::short_id;

const MAX_FILE_BYTES: u64 = 1_000_000;
const WATCHER_DEBOUNCE: Duration = Duration::from_millis(250);
/// How often the WSL poll loop re-walks the worktree. `notify` over the
/// 9p mount is unreliable (missed events, dup fires), so we poll instead.
/// 60 s is a compromise between staleness and the cost of a full
/// `fs.walk` over the agent — typical projects rebuild in <1 s.
const WSL_POLL_INTERVAL: Duration = Duration::from_secs(60);

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
    /// `None` when the OS refused to install a watcher — we keep the index
    /// usable rather than fail-loud, since manual rebuild still works.
    _watch: Option<WorktreeWatch>,
}

/// RAII handle for the file watcher and its drain loop. Both are torn down
/// when this is dropped.
struct WorktreeWatch {
    /// Held to keep the watcher alive; `notify` stops watching on drop.
    _watcher: notify::RecommendedWatcher,
    drain_handle: tokio::task::JoinHandle<()>,
}

impl Drop for WorktreeWatch {
    fn drop(&mut self) {
        self.drain_handle.abort();
    }
}

pub struct IndexingService {
    entries: Mutex<HashMap<AggregateId, IndexEntry>>,
    /// Where to put SQLite caches for WSL worktrees (Windows-side, since
    /// SQLite over 9p is slow and crash-prone).
    data_dir: PathBuf,
    agent_pool: Arc<AgentPool>,
    /// Worktrees that already have a poll loop running. Prevents
    /// `start_wsl_poll` from stacking multiple loops when a session
    /// activates the same worktree several times.
    wsl_polls: Mutex<HashSet<AggregateId>>,
}

impl IndexingService {
    pub fn new(data_dir: PathBuf, agent_pool: Arc<AgentPool>) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            data_dir,
            agent_pool,
            wsl_polls: Mutex::new(HashSet::new()),
        }
    }

    /// Spin up a periodic re-index loop for a WSL worktree. Idempotent —
    /// re-calling for the same worktree is a no-op. The loop exits silently
    /// when individual rebuild attempts fail; transient agent issues
    /// recover on the next tick. There's no clean shutdown handle today;
    /// loops live for the desktop process lifetime (acceptable: each is
    /// cheap when nothing has changed).
    pub fn start_wsl_poll(
        self: &Arc<Self>,
        worktree_id: AggregateId,
        env: Environment,
        worktree_root: String,
    ) {
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            {
                let mut started = me.wsl_polls.lock().await;
                if !started.insert(worktree_id) {
                    return;
                }
            }
            tracing::info!(worktree_id = %worktree_id, "wsl indexing poll started");
            let mut tick = tokio::time::interval(WSL_POLL_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.tick().await; // skip initial immediate tick
            loop {
                tick.tick().await;
                // No progress channel — these reindexes are background hygiene,
                // not user-initiated. Errors get debug-logged so a flaky agent
                // doesn't fill the trace.
                match me.rebuild(worktree_id, &env, &worktree_root, None).await {
                    Ok(report) if report.files_indexed > 0 => {
                        tracing::debug!(
                            worktree_id = %worktree_id,
                            files = report.files_indexed,
                            "wsl poll reindex"
                        );
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::debug!(worktree_id = %worktree_id, error = %e, "wsl poll skipped");
                    }
                }
            }
        });
    }

    /// Open (or reuse) the on-disk index for a worktree. For Windows
    /// worktrees the DB lives in-tree at `<worktree>/.oxyris/index.db`
    /// and gets a `notify` watcher. For WSL worktrees the DB is cached
    /// in `<data_dir>/wsl-index/<short>.db` on the Windows side and has
    /// no watcher (manual rebuild only).
    pub async fn open_for(
        &self,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
    ) -> Result<Arc<Index>, IndexingError> {
        let mut entries = self.entries.lock().await;
        if let Some(entry) = entries.get(&worktree_id) {
            return Ok(entry.index.clone());
        }
        let db_path = self.index_db_path(worktree_id, env, worktree_root);
        let idx = tokio::task::spawn_blocking(move || Index::open(&db_path))
            .await
            .map_err(|e| IndexingError::Io(std::io::Error::other(e.to_string())))??;
        let index = Arc::new(idx);

        // Watcher only for Windows — `notify` over 9p (\\wsl.localhost)
        // either misses events or fires duplicates depending on the
        // backend. Skip cleanly for WSL; rebuild covers updates.
        let watch = if matches!(env, Environment::Windows) {
            match start_watcher(index.clone(), worktree_root) {
                Ok(w) => Some(w),
                Err(e) => {
                    tracing::warn!(error = %e, worktree = %worktree_root, "file watcher disabled for worktree");
                    None
                }
            }
        } else {
            None
        };

        entries.insert(
            worktree_id,
            IndexEntry {
                index: index.clone(),
                _watch: watch,
            },
        );
        Ok(index)
    }

    fn index_db_path(
        &self,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
    ) -> PathBuf {
        match env {
            Environment::Windows => PathBuf::from(worktree_root)
                .join(".oxyris")
                .join("index.db"),
            Environment::Wsl { .. } => self
                .data_dir
                .join("wsl-index")
                .join(format!("{}.db", short_id(worktree_id))),
        }
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
        let index = self.open_for(worktree_id, env, worktree_root).await?;
        let stats = {
            let idx = index.clone();
            tokio::task::spawn_blocking(move || idx.stats())
                .await
                .map_err(|e| IndexingError::Io(std::io::Error::other(e.to_string())))??
        };
        if stats.files > 0 {
            // Already populated — incremental updates are handled by the
            // file watcher started in `open_for`. Nothing to do.
            return Ok(RebuildReport {
                files_indexed: 0,
                symbols_extracted: 0,
                files_skipped: 0,
                bytes_read: 0,
                duration_ms: 0,
            });
        }
        self.rebuild(worktree_id, env, worktree_root, progress)
            .await
    }

    /// Walk the worktree, re-indexing every file we have a parser for.
    /// Drops files that have disappeared. Respects `.gitignore` and the
    /// standard ignore conventions of the `ignore` crate.
    pub async fn rebuild(
        &self,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
        progress: Option<ProgressSender>,
    ) -> Result<RebuildReport, IndexingError> {
        let index = self.open_for(worktree_id, env, worktree_root).await?;
        match env {
            Environment::Windows => {
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
                    index,
                    progress,
                )
                .await
            }
        }
    }
}

/// WSL-side rebuild: enumerate files via agent `fs.walk`, fetch contents
/// via agent `fs.read`, parse with tree-sitter on the Windows side, write
/// SQLite locally. Skips files larger than `MAX_FILE_BYTES` (signalled by
/// `fs.walk` event size when present) and any unreadable entries.
async fn rebuild_wsl(
    agent: Arc<AgentPool>,
    distro: String,
    worktree_id: AggregateId,
    worktree_root: &str,
    index: Arc<Index>,
    progress: Option<ProgressSender>,
) -> Result<RebuildReport, IndexingError> {
    let started = Instant::now();
    // Stream all paths first so we can hand off to a blocking parse loop
    // without holding the agent's mpsc channel hostage. Agent walker
    // already filters via .gitignore (it uses the `ignore` crate).
    let walk_args = serde_json::to_value(FsWalkArgs {
        root: worktree_root.to_owned(),
        ignore: vec![".oxyris".to_owned()],
        max_entries: None,
    })?;
    let (mut events_rx, _final) = agent
        .call_streaming(&distro, op_name::FS_WALK, walk_args)
        .await
        .map_err(|e| IndexingError::Agent(e.to_string()))?;

    let mut indexable_files: Vec<(String, u64)> = Vec::new();
    while let Some(event) = events_rx.recv().await {
        let entry: FsWalkEvent = match serde_json::from_value(event) {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!(error = %e, "indexing: bad fs.walk event");
                continue;
            }
        };
        if entry.is_dir {
            continue;
        }
        let path = Path::new(&entry.path);
        if Lang::from_path(path).is_none() {
            continue;
        }
        let size = entry.size.unwrap_or(0);
        if size > MAX_FILE_BYTES {
            continue;
        }
        indexable_files.push((entry.path, size));
    }

    let total_files = indexable_files.len() as u64;
    if let Some(tx) = progress.as_ref() {
        let _ = tx.send(IndexingProgress::Started {
            worktree_id,
            total_files,
        });
    }

    // Pull each file via agent fs.read, parse + write SQLite locally.
    let mut files_indexed: u64 = 0;
    let mut symbols_extracted: u64 = 0;
    let mut files_skipped: u64 = 0;
    let mut bytes_read: u64 = 0;

    for (file_path, _size) in indexable_files {
        let read_args = serde_json::to_value(FsReadArgs {
            path: file_path.clone(),
            max_bytes: Some(MAX_FILE_BYTES),
        })?;
        let result = match agent.call(&distro, op_name::FS_READ, read_args).await {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!(file = %file_path, error = %e, "indexing wsl: fs.read failed");
                files_skipped += 1;
                continue;
            }
        };
        let read: FsReadResult = match serde_json::from_value(result) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(file = %file_path, error = %e, "indexing wsl: bad fs.read result");
                files_skipped += 1;
                continue;
            }
        };
        let path = Path::new(&file_path);
        let Some(lang) = Lang::from_path(path) else {
            files_skipped += 1;
            continue;
        };
        let relative = match path.strip_prefix(worktree_root) {
            Ok(p) => p.to_string_lossy().replace('\\', "/"),
            Err(_) => file_path.replace('\\', "/"),
        };
        // No mtime from fs.read — use 0 so subsequent runs always
        // re-index. WSL rebuild is rare enough that this is fine.
        let idx = index.clone();
        let content = read.content;
        let mtime = 0i64;
        let n = match tokio::task::spawn_blocking(move || {
            idx.index_file(&relative, lang, mtime, &content)
        })
        .await
        {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                tracing::debug!(file = %file_path, error = %e, "indexing wsl: index_file failed");
                files_skipped += 1;
                continue;
            }
            Err(e) => {
                tracing::warn!(file = %file_path, error = %e, "indexing wsl: blocking task failed");
                files_skipped += 1;
                continue;
            }
        };
        files_indexed += 1;
        symbols_extracted += n as u64;
        bytes_read += read.bytes_read;
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
        if Lang::from_path(result.path()).is_some() {
            count += 1;
        }
    }
    count
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

// ────── file watcher ───────────────────────────────────────────────────────

fn start_watcher(index: Arc<Index>, worktree_root: &str) -> Result<WorktreeWatch, notify::Error> {
    let root = PathBuf::from(worktree_root);
    let pending: Arc<StdMutex<HashSet<PathBuf>>> = Arc::new(StdMutex::new(HashSet::new()));
    let pending_for_cb = pending.clone();

    let mut watcher =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(event) => {
                let interesting = matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                );
                if !interesting {
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
                tracing::debug!(error = %e, "notify watcher error");
            }
        })?;
    watcher.watch(&root, RecursiveMode::Recursive)?;

    let drain_handle = tokio::spawn(drain_loop(pending, index, root));

    Ok(WorktreeWatch {
        _watcher: watcher,
        drain_handle,
    })
}

async fn drain_loop(pending: Arc<StdMutex<HashSet<PathBuf>>>, index: Arc<Index>, root: PathBuf) {
    let mut tick = tokio::time::interval(WATCHER_DEBOUNCE);
    // First tick fires immediately; skip it so we don't process before any
    // events arrive.
    tick.tick().await;
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
        let index_clone = index.clone();
        let root_clone = root.clone();
        let _ = tokio::task::spawn_blocking(move || {
            for path in batch {
                process_change(&path, &root_clone, &index_clone);
            }
        })
        .await;
    }
}

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
    if let Err(e) = index.index_file(&relative, lang, mtime, &content) {
        tracing::debug!(file = %relative, error = %e, "watcher: index_file failed");
    }
}
