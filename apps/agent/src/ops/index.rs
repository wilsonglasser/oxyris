//! In-distro symbol index.
//!
//! The agent owns the tree-sitter index for WSL worktrees: parse + SQLite run
//! here, on native ext4, so the desktop backend never pulls file contents over
//! stdio. The backend sends queries (`index.query_symbol`, …) and a rebuild
//! trigger; everything else — the full walk, incremental updates from the
//! watcher — happens locally.
//!
//! The DB lives at `<root>/.oxyris/index.db` (same in-tree location Windows
//! uses). A process-global registry keeps one open `Index` per root so the
//! watcher and the query ops share it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;
use oxyris_index::{Index, Lang};
use oxyris_ipc::ops::{IndexRebuildProgress, IndexRebuildReport};
use serde_json::Value;

use crate::ops::OpError;
use crate::protocol;

const MAX_FILE_BYTES: u64 = 1_000_000;

/// root path → open index. Process-global so `fs.watch`'s incremental updates
/// and the query ops hit the same handle.
fn registry() -> &'static StdMutex<HashMap<String, Arc<Index>>> {
    static R: OnceLock<StdMutex<HashMap<String, Arc<Index>>>> = OnceLock::new();
    R.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn db_path(root: &str) -> PathBuf {
    Path::new(root).join(".oxyris").join("index.db")
}

/// Open (or reuse) the index for `root`, registering it for the watcher.
fn get_or_open(root: &str) -> Result<Arc<Index>, OpError> {
    if let Ok(reg) = registry().lock()
        && let Some(idx) = reg.get(root)
    {
        return Ok(idx.clone());
    }
    let idx = Index::open(&db_path(root)).map_err(|e| OpError::Index(e.to_string()))?;
    let idx = Arc::new(idx);
    if let Ok(mut reg) = registry().lock() {
        // Another task may have opened it concurrently; keep the first.
        return Ok(reg.entry(root.to_owned()).or_insert(idx).clone());
    }
    Ok(idx)
}

/// Look up an already-open index without creating one. The watcher uses this so
/// it only updates indexes the backend has explicitly `ensure`d.
fn lookup(root: &str) -> Option<Arc<Index>> {
    registry().lock().ok()?.get(root).cloned()
}

// ────── ops ────────────────────────────────────────────────────────────────

/// Ensure the index is populated. Returns an empty report if it already has
/// files; otherwise does a full rebuild.
pub async fn ensure(root: String) -> Result<Value, OpError> {
    let index = get_or_open(&root)?;
    let populated = {
        let index = index.clone();
        tokio::task::spawn_blocking(move || index.stats().map(|s| s.files > 0))
            .await
            .map_err(|e| OpError::Io(std::io::Error::other(e.to_string())))?
            .map_err(|e| OpError::Index(e.to_string()))?
    };
    if populated {
        return Ok(serde_json::to_value(empty_report())?);
    }
    let report = rebuild_inner(None, root, index).await?;
    Ok(serde_json::to_value(report)?)
}

/// Full rebuild, streaming progress under `request_id`.
pub async fn rebuild(request_id: &str, root: String) -> Result<Value, OpError> {
    let index = get_or_open(&root)?;
    let report = rebuild_inner(Some(request_id.to_owned()), root, index).await?;
    Ok(serde_json::to_value(report)?)
}

pub async fn query_symbol(
    root: String,
    name: String,
    kind: Option<String>,
    limit: u32,
) -> Result<Value, OpError> {
    let index = get_or_open(&root)?;
    let kind = kind
        .as_deref()
        .and_then(oxyris_index::SymbolKind::from_label);
    let hits = tokio::task::spawn_blocking(move || index.find_symbol(&name, kind, limit))
        .await
        .map_err(|e| OpError::Io(std::io::Error::other(e.to_string())))?
        .map_err(|e| OpError::Index(e.to_string()))?;
    Ok(serde_json::to_value(hits)?)
}

pub async fn list_in_file(root: String, file: String) -> Result<Value, OpError> {
    let index = get_or_open(&root)?;
    let syms = tokio::task::spawn_blocking(move || index.list_symbols_in_file(&file))
        .await
        .map_err(|e| OpError::Io(std::io::Error::other(e.to_string())))?
        .map_err(|e| OpError::Index(e.to_string()))?;
    Ok(serde_json::to_value(syms)?)
}

pub async fn project_map(root: String) -> Result<Value, OpError> {
    let index = get_or_open(&root)?;
    let map = tokio::task::spawn_blocking(move || index.project_map())
        .await
        .map_err(|e| OpError::Io(std::io::Error::other(e.to_string())))?
        .map_err(|e| OpError::Index(e.to_string()))?;
    Ok(serde_json::to_value(map)?)
}

pub async fn stats(root: String) -> Result<Value, OpError> {
    let index = get_or_open(&root)?;
    let stats = tokio::task::spawn_blocking(move || index.stats())
        .await
        .map_err(|e| OpError::Io(std::io::Error::other(e.to_string())))?
        .map_err(|e| OpError::Index(e.to_string()))?;
    Ok(serde_json::to_value(stats)?)
}

/// Incremental update from the watcher. No-op unless an index is already open
/// for `root`. `abs_paths` are absolute paths inside the distro.
pub fn apply_changes(root: String, abs_paths: Vec<PathBuf>) {
    let Some(index) = lookup(&root) else {
        return;
    };
    tokio::task::spawn_blocking(move || {
        let root = Path::new(&root);
        for path in abs_paths {
            process_change(&path, root, &index);
        }
    });
}

// ────── rebuild + incremental internals ─────────────────────────────────────

async fn rebuild_inner(
    request_id: Option<String>,
    root: String,
    index: Arc<Index>,
) -> Result<IndexRebuildReport, OpError> {
    // Stream progress to a forwarder task so the blocking walk can report
    // without an async context. With no subscriber, drop the receiver so the
    // blocking walk's `tx.send`s are cheap no-ops.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<IndexRebuildProgress>();
    let pump = match request_id {
        Some(id) => Some(tokio::spawn(async move {
            let mut rx = rx;
            while let Some(p) = rx.recv().await {
                protocol::emit_event(&id, serde_json::to_value(p).unwrap_or(Value::Null)).await;
            }
        })),
        None => {
            drop(rx);
            None
        }
    };

    let root_p = PathBuf::from(&root);
    let report = tokio::task::spawn_blocking(move || rebuild_blocking(&root_p, &index, &tx))
        .await
        .map_err(|e| OpError::Io(std::io::Error::other(e.to_string())))??;

    if let Some(pump) = pump {
        let _ = pump.await;
    }
    Ok(report)
}

fn rebuild_blocking(
    root: &Path,
    index: &Index,
    tx: &tokio::sync::mpsc::UnboundedSender<IndexRebuildProgress>,
) -> Result<IndexRebuildReport, OpError> {
    let started = Instant::now();
    let mut files_indexed: u64 = 0;
    let mut symbols_extracted: u64 = 0;
    let mut files_skipped: u64 = 0;
    let mut bytes_read: u64 = 0;

    let total_files = count_indexable(root);
    let _ = tx.send(IndexRebuildProgress {
        total_files,
        files_indexed: 0,
        files_skipped: 0,
    });

    for entry in build_walker(root).build().flatten() {
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
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        match index.index_file(&relative, lang, mtime, &content) {
            Ok(n) => {
                files_indexed += 1;
                symbols_extracted += n as u64;
                bytes_read += metadata.len();
            }
            Err(e) => {
                tracing::debug!(file = %relative, error = %e, "index rebuild: file failed");
                files_skipped += 1;
            }
        }
        if (files_indexed + files_skipped).is_multiple_of(25) {
            let _ = tx.send(IndexRebuildProgress {
                total_files,
                files_indexed,
                files_skipped,
            });
        }
    }

    Ok(IndexRebuildReport {
        files_indexed,
        symbols_extracted,
        files_skipped,
        bytes_read,
        duration_ms: started.elapsed().as_millis(),
    })
}

/// Re-index (or drop) a single changed path — the incremental counterpart to
/// the full rebuild, driven by the watcher.
fn process_change(path: &Path, root: &Path, index: &Index) {
    let Ok(rel) = path.strip_prefix(root) else {
        return;
    };
    if rel.starts_with(".oxyris") {
        return;
    }
    let relative = rel.to_string_lossy().replace('\\', "/");
    if oxyris_ipc::ops::is_generated_path(&relative) {
        return;
    }
    if !path.exists() {
        if let Err(e) = index.remove_file(&relative) {
            tracing::debug!(file = %relative, error = %e, "reindex: remove_file failed");
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
    if let Err(e) = index.index_file_if_changed(&relative, lang, mtime, &content) {
        tracing::debug!(file = %relative, error = %e, "reindex: index_file failed");
    }
}

fn count_indexable(root: &Path) -> u64 {
    let mut count: u64 = 0;
    for entry in build_walker(root).build().flatten() {
        if !entry.file_type().is_some_and(|f| f.is_file()) {
            continue;
        }
        if Lang::from_path(entry.path()).is_some() && !is_generated(entry.path(), root) {
            count += 1;
        }
    }
    count
}

/// Shared walk config: standard ignore stack, plus an explicit skip of our own
/// `.oxyris` scratch dir (where the index DB lives).
fn build_walker(root: &Path) -> WalkBuilder {
    let mut b = WalkBuilder::new(root);
    b.standard_filters(true).hidden(true).filter_entry(|e| {
        e.file_name()
            .to_str()
            .map(|n| n != ".oxyris")
            .unwrap_or(true)
    });
    b
}

fn is_generated(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .ok()
        .map(|rel| oxyris_ipc::ops::is_generated_path(&rel.to_string_lossy()))
        .unwrap_or(false)
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

fn empty_report() -> IndexRebuildReport {
    IndexRebuildReport {
        files_indexed: 0,
        symbols_extracted: 0,
        files_skipped: 0,
        bytes_read: 0,
        duration_ms: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxyris_index::{IndexStats, Symbol, SymbolHit};
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn ensure_then_query_roundtrip() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("lib.rs"),
            "pub fn greet() {}\npub struct Widget;\n",
        )
        .unwrap();
        // node_modules must be ignored by the in-distro walk too.
        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules").join("dep.js"), "function x(){}").unwrap();
        let root_s = root.to_string_lossy().to_string();

        let report: IndexRebuildReport =
            serde_json::from_value(ensure(root_s.clone()).await.unwrap()).unwrap();
        assert!(report.files_indexed >= 1, "indexed {report:?}");

        let stats: IndexStats =
            serde_json::from_value(stats(root_s.clone()).await.unwrap()).unwrap();
        assert_eq!(stats.files, 1, "node_modules must be excluded: {stats:?}");

        let hits: Vec<SymbolHit> = serde_json::from_value(
            query_symbol(root_s.clone(), "greet".to_owned(), None, 10)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(hits.iter().any(|h| h.name == "greet"), "hits {hits:?}");

        let kind_filtered: Vec<SymbolHit> = serde_json::from_value(
            query_symbol(
                root_s.clone(),
                "Widget".to_owned(),
                Some("struct".to_owned()),
                10,
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert!(kind_filtered.iter().any(|h| h.name == "Widget"));

        let in_file: Vec<Symbol> = serde_json::from_value(
            list_in_file(root_s.clone(), "lib.rs".to_owned())
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(in_file.iter().any(|s| s.name == "greet"));
    }

    #[tokio::test]
    async fn ensure_is_idempotent_on_populated_index() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.rs"), "pub fn one() {}").unwrap();
        let root_s = root.to_string_lossy().to_string();

        let first: IndexRebuildReport =
            serde_json::from_value(ensure(root_s.clone()).await.unwrap()).unwrap();
        assert!(first.files_indexed >= 1);
        // Second ensure sees a populated DB → empty report, no rework.
        let second: IndexRebuildReport =
            serde_json::from_value(ensure(root_s.clone()).await.unwrap()).unwrap();
        assert_eq!(second.files_indexed, 0);
    }
}
