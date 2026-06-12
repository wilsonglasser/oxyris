//! Ignore-aware, per-directory filesystem watcher.
//!
//! `notify`'s `RecursiveMode::Recursive` installs one OS watch that covers the
//! whole subtree — including `node_modules`, `target`, `dist`, and every other
//! vendored/build dir. On Linux (inotify) that means **one watch descriptor per
//! directory**, so a repo with a fat `node_modules` exhausts kernel memory and
//! the watcher floods CPU on every build write. On Windows it's a single handle
//! but the events for those dirs still cross into the process and get discarded
//! — wasted CPU during a `npm install` / `cargo build` storm.
//!
//! This watcher instead enumerates only the **non-ignored** directories (via the
//! `ignore` crate, same `.gitignore` semantics the indexer's walk uses) and
//! installs a **non-recursive** watch on each. Ignored dirs are never watched,
//! so the kernel never reports their churn — zero descriptors, zero CPU for
//! `node_modules`. New directories created at runtime are picked up from their
//! parent's watch: when one appears, we walk it (recovering any files born
//! before the watch was armed) and watch its non-ignored subtree.
//!
//! One watcher feeds many consumers: it hands debounced batches of changed
//! absolute paths to a sink. The desktop backend points that sink at both the
//! file-tree refresh event and the symbol index; the WSL agent points it at the
//! NDJSON event stream and its in-distro index. Same code, both sides.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Decides whether a path should be excluded from watching/indexing. Combines
/// three sources so the watcher stays in lockstep with the indexer's walk:
/// the project's root `.gitignore`, the shared [`is_generated_path`] list
/// (`node_modules`, `dist`, `*.min.js`, …), and an unconditional skip of our
/// own scratch dir (`.oxyris`) and `.git`.
///
/// [`is_generated_path`]: oxyris_ipc::ops::is_generated_path
pub struct IgnoreMatcher {
    root: PathBuf,
    gitignore: Gitignore,
}

impl IgnoreMatcher {
    /// Build a matcher rooted at `root`, loading `root/.gitignore` if present.
    /// Nested `.gitignore` files deeper in the tree are honored by the initial
    /// [`WalkBuilder`] enumeration; this matcher is the runtime fast-path for
    /// classifying newly-created directories, where the root rules suffice.
    pub fn new(root: &Path) -> Self {
        let mut builder = GitignoreBuilder::new(root);
        // `add` returns Some(err) on a malformed file; ignore it — a broken
        // .gitignore shouldn't disable watching, it just means fewer excludes.
        let _ = builder.add(root.join(".gitignore"));
        let gitignore = builder.build().unwrap_or_else(|_| Gitignore::empty());
        Self {
            root: root.to_path_buf(),
            gitignore,
        }
    }

    /// True if `abs` (an absolute path under the watched root) should be
    /// excluded. `is_dir` selects directory-vs-file gitignore semantics.
    pub fn is_ignored(&self, abs: &Path, is_dir: bool) -> bool {
        let rel = match abs.strip_prefix(&self.root) {
            Ok(r) => r,
            // Outside the root — not ours to watch.
            Err(_) => return true,
        };
        if rel.as_os_str().is_empty() {
            // The root itself is never ignored.
            return false;
        }
        // Our scratch dir and git internals: always skip, regardless of any
        // .gitignore. (`.oxyris` holds the index DB + WAL; watching it would
        // self-trigger on every write.)
        if rel
            .components()
            .any(|c| matches!(c.as_os_str().to_str(), Some(".git") | Some(".oxyris")))
        {
            return true;
        }
        let rel_str = rel.to_string_lossy();
        if oxyris_ipc::ops::is_generated_path(&rel_str) {
            return true;
        }
        // `matched_path_or_any_parents` walks up so a file under an ignored
        // dir (`build/` → `build/out.o`) is excluded, not just the dir itself.
        self.gitignore
            .matched_path_or_any_parents(rel, is_dir)
            .is_ignore()
    }
}

/// A live watcher. Dropping it aborts the drain task, which drops the owned
/// `notify` watcher and tears down every OS watch.
pub struct DirWatcher {
    drain: tokio::task::JoinHandle<()>,
}

impl Drop for DirWatcher {
    fn drop(&mut self) {
        self.drain.abort();
    }
}

impl DirWatcher {
    /// Start watching the non-ignored subtree of `root`. `debounce` coalesces
    /// bursts; once per interval the accumulated set of changed absolute paths
    /// is handed to `sink` (never called with an empty batch).
    ///
    /// `sink` runs on a Tokio task — keep it cheap and non-blocking (forward to
    /// a channel / emit an event); offload heavy work elsewhere.
    pub fn start<S>(root: PathBuf, debounce: Duration, sink: S) -> Result<DirWatcher, notify::Error>
    where
        S: FnMut(Vec<PathBuf>) + Send + 'static,
    {
        let pending: Arc<Mutex<HashSet<PathBuf>>> = Arc::new(Mutex::new(HashSet::new()));
        let pending_cb = pending.clone();

        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let Ok(event) = res else { return };
                if !matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) {
                    return;
                }
                if let Ok(mut p) = pending_cb.lock() {
                    p.extend(event.paths);
                }
            })?;

        let matcher = IgnoreMatcher::new(&root);
        let mut watched: HashSet<PathBuf> = HashSet::new();
        // Initial enumeration: watch every non-ignored dir. No emit — cold-start
        // indexing is a separate full rebuild; we only arm the watches here.
        watch_subtree(&mut watcher, &mut watched, &matcher, &root, None);

        let drain = tokio::spawn(drain_loop(
            watcher, watched, matcher, pending, root, debounce, sink,
        ));
        Ok(DirWatcher { drain })
    }
}

/// Walk `dir`'s non-ignored subtree, installing a non-recursive watch on each
/// directory we don't already watch. When `emit` is `Some`, every non-ignored
/// file encountered is pushed to it — used when a brand-new directory appears
/// at runtime so files created before the watch armed are still reported.
fn watch_subtree(
    watcher: &mut RecommendedWatcher,
    watched: &mut HashSet<PathBuf>,
    matcher: &IgnoreMatcher,
    dir: &Path,
    mut emit: Option<&mut Vec<PathBuf>>,
) {
    // `WalkBuilder` applies the full standard ignore stack (root + nested
    // .gitignore, .ignore, hidden, git excludes), so ignored subtrees are
    // skipped here and never get a watch.
    let walker = WalkBuilder::new(dir)
        .standard_filters(true)
        .filter_entry(|e| !matches!(e.file_name().to_str(), Some(".git") | Some(".oxyris")))
        .build();
    for entry in walker.flatten() {
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        let path = entry.path();
        if is_dir {
            if watched.insert(path.to_path_buf()) {
                // Non-recursive: each dir is watched individually so creating a
                // new ignored dir later never pulls its subtree in.
                let _ = watcher.watch(path, RecursiveMode::NonRecursive);
            }
        } else if let Some(e) = emit.as_deref_mut()
            && !matcher.is_ignored(path, false)
        {
            e.push(path.to_path_buf());
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn drain_loop<S>(
    mut watcher: RecommendedWatcher,
    mut watched: HashSet<PathBuf>,
    matcher: IgnoreMatcher,
    pending: Arc<Mutex<HashSet<PathBuf>>>,
    root: PathBuf,
    debounce: Duration,
    mut sink: S,
) where
    S: FnMut(Vec<PathBuf>) + Send + 'static,
{
    let mut tick = tokio::time::interval(debounce);
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

        let mut emit: Vec<PathBuf> = Vec::new();
        for path in batch {
            // A directory that exists and isn't watched yet is a freshly
            // created dir (or one just un-ignored). Arm its subtree and emit
            // any files already inside it.
            if path.is_dir() {
                if !watched.contains(&path) && !matcher.is_ignored(&path, true) {
                    watch_subtree(&mut watcher, &mut watched, &matcher, &path, Some(&mut emit));
                }
                // The directory node itself isn't an indexable file.
                continue;
            }
            // File created/modified/removed. (A removed path reports
            // `is_dir() == false`; emitting it lets the index drop the row.)
            if path.strip_prefix(&root).is_err() {
                continue;
            }
            if matcher.is_ignored(&path, false) {
                watched.remove(&path);
                continue;
            }
            emit.push(path);
        }

        if !emit.is_empty() {
            sink(emit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;
    use tempfile::tempdir;

    fn touch(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn matcher_skips_git_oxyris_and_generated() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(".gitignore"), "build/\n").unwrap();
        let m = IgnoreMatcher::new(root);

        assert!(m.is_ignored(&root.join(".git").join("HEAD"), false));
        assert!(m.is_ignored(&root.join(".oxyris").join("index.db"), false));
        assert!(m.is_ignored(&root.join("node_modules").join("x.js"), false));
        assert!(m.is_ignored(&root.join("build").join("out.o"), false));
        assert!(m.is_ignored(&root.join("build"), true));

        assert!(!m.is_ignored(&root.join("src").join("main.rs"), false));
        assert!(!m.is_ignored(root, true)); // the root itself
    }

    async fn collect_changes(root: PathBuf, action: impl FnOnce(&Path)) -> Vec<PathBuf> {
        let seen: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_cb = seen.clone();
        let _w = DirWatcher::start(root.clone(), Duration::from_millis(80), move |batch| {
            seen_cb.lock().unwrap().extend(batch);
        })
        .unwrap();
        // Let the initial watch arm.
        tokio::time::sleep(Duration::from_millis(150)).await;
        action(&root);
        // Wait past a couple debounce cycles for the event to land + drain.
        tokio::time::sleep(Duration::from_millis(500)).await;
        seen.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn reports_file_write_in_root() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let changes = collect_changes(root.clone(), |r| {
            touch(&r.join("hello.rs"), "fn main() {}");
        })
        .await;
        assert!(
            changes.iter().any(|p| p.ends_with("hello.rs")),
            "expected hello.rs in {changes:?}"
        );
    }

    #[tokio::test]
    async fn ignores_node_modules_writes() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        // Pre-create node_modules so it exists before the watch arms.
        fs::create_dir_all(root.join("node_modules")).unwrap();
        let changes = collect_changes(root.clone(), |r| {
            touch(&r.join("node_modules").join("dep.js"), "module.exports={}");
        })
        .await;
        assert!(
            !changes
                .iter()
                .any(|p| p.to_string_lossy().contains("node_modules")),
            "node_modules write should be ignored, got {changes:?}"
        );
    }

    #[tokio::test]
    async fn picks_up_files_in_newly_created_dir() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let changes = collect_changes(root.clone(), |r| {
            // A new nested dir with a file — the watch on the new dir is armed
            // from its parent's event, and the walk recovers the child file.
            touch(&r.join("sub").join("deep").join("mod.rs"), "pub fn f() {}");
        })
        .await;
        assert!(
            changes.iter().any(|p| p.ends_with("mod.rs")),
            "expected mod.rs from newly-created dir in {changes:?}"
        );
    }
}
