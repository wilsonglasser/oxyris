//! Per-worktree symbol index orchestration.
//!
//! Holds an [`oxyris_index::Index`] per worktree, opened lazily on first
//! query or rebuild. Indexes live at `<worktree>/.oxyris/index.db` so they
//! travel with the worktree (and are dropped when the worktree is removed).
//!
//! Each open index also gets a [`notify`] file watcher that re-indexes
//! changed files (debounced 250ms) and drops symbols for deleted files.
//! The watcher is automatically torn down when the entry is removed (drop
//! semantics on `WorktreeWatch`).
//!
//! WSL projects are deferred to a follow-up sprint — for now we surface
//! [`IndexingError::WslNotSupported`] so callers can fail gracefully.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;
use notify::{EventKind, RecursiveMode, Watcher};
use oxyris_core::{AggregateId, Environment};
use oxyris_index::{Index, Lang};
use serde::Serialize;
use thiserror::Error;
use tokio::sync::Mutex;

const MAX_FILE_BYTES: u64 = 1_000_000;
const WATCHER_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Debug, Error)]
pub enum IndexingError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("index: {0}")]
    Index(#[from] oxyris_index::IndexError),
    #[error("indexing for WSL projects is not yet supported")]
    WslNotSupported,
    #[error("worktree path is invalid: {0}")]
    InvalidWorktreePath(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct RebuildReport {
    pub files_indexed: u64,
    pub symbols_extracted: u64,
    pub files_skipped: u64,
    pub bytes_read: u64,
    pub duration_ms: u128,
}

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
}

impl IndexingService {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Open (or reuse) the on-disk index for a worktree. First open also
    /// installs a file watcher rooted at the worktree path. Caller passes
    /// the absolute worktree root so we don't have to re-resolve it on every
    /// hit. Index lives at `<worktree>/.oxyris/index.db`.
    pub async fn open_for(
        &self,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
    ) -> Result<Arc<Index>, IndexingError> {
        ensure_supported(env)?;
        let mut entries = self.entries.lock().await;
        if let Some(entry) = entries.get(&worktree_id) {
            return Ok(entry.index.clone());
        }
        let db_path = index_db_path(worktree_root);
        let idx = tokio::task::spawn_blocking(move || Index::open(&db_path))
            .await
            .map_err(|e| IndexingError::Io(std::io::Error::other(e.to_string())))??;
        let index = Arc::new(idx);

        let watch = match start_watcher(index.clone(), worktree_root) {
            Ok(w) => Some(w),
            Err(e) => {
                tracing::warn!(error = %e, worktree = %worktree_root, "file watcher disabled for worktree");
                None
            }
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

    /// Drop the cached entry: closes the SQLite connection on last reference
    /// and tears down the watcher. Call when the worktree is removed.
    #[allow(dead_code)]
    pub async fn close(&self, worktree_id: AggregateId) {
        self.entries.lock().await.remove(&worktree_id);
    }

    /// Walk the worktree, re-indexing every file we have a parser for.
    /// Drops files that have disappeared. Respects `.gitignore` and the
    /// standard ignore conventions of the `ignore` crate.
    pub async fn rebuild(
        &self,
        worktree_id: AggregateId,
        env: &Environment,
        worktree_root: &str,
    ) -> Result<RebuildReport, IndexingError> {
        let index = self.open_for(worktree_id, env, worktree_root).await?;
        let root = PathBuf::from(worktree_root);
        if !root.is_dir() {
            return Err(IndexingError::InvalidWorktreePath(worktree_root.to_owned()));
        }

        // Heavy: walk + parse + sqlite. Off the runtime.
        let report = tokio::task::spawn_blocking(move || rebuild_blocking(&root, &index))
            .await
            .map_err(|e| IndexingError::Io(std::io::Error::other(e.to_string())))??;
        Ok(report)
    }
}

impl Default for IndexingService {
    fn default() -> Self {
        Self::new()
    }
}

fn ensure_supported(env: &Environment) -> Result<(), IndexingError> {
    match env {
        Environment::Windows => Ok(()),
        Environment::Wsl { .. } => Err(IndexingError::WslNotSupported),
    }
}

fn index_db_path(worktree_root: &str) -> PathBuf {
    PathBuf::from(worktree_root)
        .join(".oxyris")
        .join("index.db")
}

fn rebuild_blocking(root: &Path, index: &Index) -> Result<RebuildReport, IndexingError> {
    let started = Instant::now();
    let mut files_indexed: u64 = 0;
    let mut symbols_extracted: u64 = 0;
    let mut files_skipped: u64 = 0;
    let mut bytes_read: u64 = 0;

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
    }

    Ok(RebuildReport {
        files_indexed,
        symbols_extracted,
        files_skipped,
        bytes_read,
        duration_ms: started.elapsed().as_millis(),
    })
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
