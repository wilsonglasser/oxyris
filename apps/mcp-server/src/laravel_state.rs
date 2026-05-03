//! Laravel snapshot cache. Loaded lazily on the first tool call —
//! parsing all four facets is fast (<300 ms typical) so an explicit
//! pre-warm isn't worth the extra CLI knob. Refreshed on demand if a
//! tool returns "stale" results; for v1 we just accept the snapshot is
//! whatever was on disk when the cache populated.

use std::path::Path;
use std::sync::Arc;

use oxyris_laravel::{LaravelSnapshot, snapshot};
use tokio::sync::Mutex;

#[derive(Default)]
pub struct LaravelState {
    inner: Mutex<Option<Arc<LaravelSnapshot>>>,
}

impl LaravelState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `Some(snapshot)` when the workspace is Laravel; `None`
    /// otherwise (signal to drop the laravel_* tools from `tools/list`).
    /// Caches the first successful detection.
    pub async fn get(&self, workspace: &Path) -> Option<Arc<LaravelSnapshot>> {
        {
            let cache = self.inner.lock().await;
            if let Some(snap) = cache.as_ref() {
                return Some(snap.clone());
            }
        }
        let snap = match snapshot(workspace) {
            Ok(s) => Arc::new(s),
            Err(e) => {
                tracing::debug!(workspace = %workspace.display(), error = %e, "laravel: snapshot skipped");
                return None;
            }
        };
        let mut cache = self.inner.lock().await;
        *cache = Some(snap.clone());
        Some(snap)
    }

    /// Cheap detect — parses `composer.json` only, no tree-sitter. Used
    /// at `tools/list` time so we know whether to advertise the laravel_*
    /// tools without paying for the full snapshot.
    pub fn looks_like_laravel(workspace: &Path) -> bool {
        oxyris_laravel::detect(workspace).is_ok()
    }
}
