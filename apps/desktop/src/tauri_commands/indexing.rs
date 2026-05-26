//! Tauri IPC surface for the symbol index.
//!
//! Sprint 14a: read-only queries against the per-worktree tree-sitter index
//! plus an explicit `index_rebuild` trigger. Auto-rebuild on worktree
//! creation and an incremental file-watcher come in 14b/14c.
//!
//! WSL projects are not yet supported — those return
//! [`TauriIndexingError::WslNotSupported`] so the UI can show the right hint.

use oxyris_core::{AggregateId, Environment};
use oxyris_index::{ProjectMap, Symbol, SymbolHit, SymbolKind};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::app_state::AppState;
use crate::infra::indexing::{IndexingError, RebuildReport};

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriIndexingError {
    #[error("worktree not found")]
    WorktreeNotFound,
    #[error("{0}")]
    Indexing(String),
    #[error("{0}")]
    Storage(String),
}

impl From<IndexingError> for TauriIndexingError {
    fn from(e: IndexingError) -> Self {
        TauriIndexingError::Indexing(e.to_string())
    }
}

#[derive(Debug, Deserialize)]
pub struct RebuildInput {
    pub worktree_id: AggregateId,
}

#[derive(Debug, Deserialize)]
pub struct QuerySymbolInput {
    pub worktree_id: AggregateId,
    /// Required when `worktree_id` is the primary sentinel (nil UUID) — the
    /// sentinel doesn't disambiguate projects, so we resolve env+root through
    /// the project row instead.
    #[serde(default)]
    pub project_id: Option<AggregateId>,
    pub name: String,
    /// Optional kind filter — `function`, `method`, `class`, `struct`, etc.
    #[serde(default)]
    pub kind: Option<SymbolKind>,
    /// Capped server-side at 50 to avoid pathological responses.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct ListSymbolsInFileInput {
    pub worktree_id: AggregateId,
    /// Path relative to the worktree root, forward slashes.
    pub file: String,
}

#[derive(Debug, Deserialize)]
pub struct ProjectMapInput {
    pub worktree_id: AggregateId,
}

#[derive(Debug, Deserialize)]
pub struct StatsInput {
    pub worktree_id: AggregateId,
}

#[derive(Debug, Serialize)]
pub struct IndexStatsRow {
    pub files: u64,
    pub symbols: u64,
}

#[tauri::command]
pub async fn index_rebuild(
    input: RebuildInput,
    state: State<'_, AppState>,
) -> Result<RebuildReport, TauriIndexingError> {
    let ctx = lookup_worktree(&state, input.worktree_id)?;
    let report = state
        .indexing
        .rebuild(input.worktree_id, &ctx.environment, &ctx.path, None)
        .await?;
    Ok(report)
}

#[tauri::command]
pub async fn index_query_symbol(
    input: QuerySymbolInput,
    state: State<'_, AppState>,
) -> Result<Vec<SymbolHit>, TauriIndexingError> {
    let ctx = resolve_ctx(&state, input.worktree_id, input.project_id)?;
    let index = state
        .indexing
        .open_for(input.worktree_id, &ctx.environment, &ctx.path)
        .await?;
    let limit = input.limit.unwrap_or(20).min(50);
    let hits =
        tokio::task::spawn_blocking(move || index.find_symbol(&input.name, input.kind, limit))
            .await
            .map_err(|e| TauriIndexingError::Indexing(e.to_string()))?
            .map_err(|e| TauriIndexingError::Indexing(e.to_string()))?;
    Ok(hits)
}

#[tauri::command]
pub async fn index_list_symbols_in_file(
    input: ListSymbolsInFileInput,
    state: State<'_, AppState>,
) -> Result<Vec<Symbol>, TauriIndexingError> {
    let ctx = lookup_worktree(&state, input.worktree_id)?;
    let index = state
        .indexing
        .open_for(input.worktree_id, &ctx.environment, &ctx.path)
        .await?;
    let symbols = tokio::task::spawn_blocking(move || index.list_symbols_in_file(&input.file))
        .await
        .map_err(|e| TauriIndexingError::Indexing(e.to_string()))?
        .map_err(|e| TauriIndexingError::Indexing(e.to_string()))?;
    Ok(symbols)
}

#[tauri::command]
pub async fn index_project_map(
    input: ProjectMapInput,
    state: State<'_, AppState>,
) -> Result<ProjectMap, TauriIndexingError> {
    let ctx = lookup_worktree(&state, input.worktree_id)?;
    let index = state
        .indexing
        .open_for(input.worktree_id, &ctx.environment, &ctx.path)
        .await?;
    let map = tokio::task::spawn_blocking(move || index.project_map())
        .await
        .map_err(|e| TauriIndexingError::Indexing(e.to_string()))?
        .map_err(|e| TauriIndexingError::Indexing(e.to_string()))?;
    Ok(map)
}

/// Idempotent "make the worktree ready for queries" — covers projects that
/// existed before the eager warm-up shipped. Triggers an initial index walk
/// (only if the DB is empty) and pre-warms the primary LSP. All progress
/// flows through the same `indexing:progress` and `lsp:status` events the
/// auto-trigger uses, so the UI chip is identical across paths.
///
/// `project_id` is required when `worktree_id` is the synthetic primary
/// sentinel — the sentinel doesn't disambiguate between projects, so we
/// resolve the env+root through the project row instead.
#[derive(Debug, Deserialize)]
pub struct EnsureReadyInput {
    pub worktree_id: AggregateId,
    #[serde(default)]
    pub project_id: Option<AggregateId>,
}

#[tauri::command]
pub async fn worktree_ensure_ready(
    input: EnsureReadyInput,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), TauriIndexingError> {
    let ctx = if input.worktree_id == crate::tauri_commands::worktree::PRIMARY_WORKTREE_SENTINEL {
        // Sentinel — resolve via the owning project so we have an env+path.
        let project_id = input
            .project_id
            .ok_or(TauriIndexingError::WorktreeNotFound)?;
        let projects = state
            .projections
            .list_projects()
            .map_err(|e| TauriIndexingError::Storage(e.to_string()))?;
        let p = projects
            .into_iter()
            .find(|p| p.id == project_id)
            .ok_or(TauriIndexingError::WorktreeNotFound)?;
        WorktreeContext {
            environment: p.environment,
            path: p.root_path,
        }
    } else {
        lookup_worktree(&state, input.worktree_id)?
    };
    let worktree_id = input.worktree_id;

    // Indexing — fire on a background task so the IPC returns immediately.
    let indexing = state.indexing.clone();
    let env_idx = ctx.environment.clone();
    let path_idx = ctx.path.clone();
    let app_for_progress = app.clone();
    let app_for_failed = app.clone();
    tauri::async_runtime::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let pump = tauri::async_runtime::spawn(async move {
            while let Some(p) = rx.recv().await {
                let _ = app_for_progress.emit("indexing:progress", p);
            }
        });
        let res = indexing
            .ensure_indexed(worktree_id, &env_idx, &path_idx, Some(tx))
            .await;
        if let Err(e) = res {
            tracing::debug!(worktree_id = %worktree_id, error = %e, "ensure_indexed skipped");
            let _ = app_for_failed.emit(
                "indexing:progress",
                crate::infra::indexing::IndexingProgress::Failed {
                    worktree_id,
                    error: e.to_string(),
                },
            );
        }
        let _ = pump.await;
    });

    // LSP — idempotent: fast-path returns existing client if already ready.
    state
        .lsp
        .warm_primary(worktree_id, ctx.environment.clone(), ctx.path.clone());

    // WSL only: kick off a background poll so symbol changes inside the
    // distro show up without manual rebuild. Idempotent — re-calls collapse
    // to the existing loop. Skipped for Windows worktrees because
    // `notify` already covers them.
    if matches!(ctx.environment, Environment::Wsl { .. }) {
        state
            .indexing
            .start_wsl_poll(worktree_id, ctx.environment, ctx.path);
    }

    Ok(())
}

#[tauri::command]
pub async fn index_stats(
    input: StatsInput,
    state: State<'_, AppState>,
) -> Result<IndexStatsRow, TauriIndexingError> {
    let ctx = lookup_worktree(&state, input.worktree_id)?;
    let index = state
        .indexing
        .open_for(input.worktree_id, &ctx.environment, &ctx.path)
        .await?;
    let stats = tokio::task::spawn_blocking(move || index.stats())
        .await
        .map_err(|e| TauriIndexingError::Indexing(e.to_string()))?
        .map_err(|e| TauriIndexingError::Indexing(e.to_string()))?;
    Ok(IndexStatsRow {
        files: stats.files,
        symbols: stats.symbols,
    })
}

// ────── helpers ────────────────────────────────────────────────────────────

struct WorktreeContext {
    environment: Environment,
    path: String,
}

/// Like [`lookup_worktree`] but understands the primary sentinel: when
/// `worktree_id` is the nil-UUID primary checkout, resolve env+root through
/// the owning project (`project_id` required in that case).
fn resolve_ctx(
    state: &AppState,
    worktree_id: AggregateId,
    project_id: Option<AggregateId>,
) -> Result<WorktreeContext, TauriIndexingError> {
    if worktree_id == crate::tauri_commands::worktree::PRIMARY_WORKTREE_SENTINEL {
        let project_id = project_id.ok_or(TauriIndexingError::WorktreeNotFound)?;
        let p = state
            .projections
            .list_projects()
            .map_err(|e| TauriIndexingError::Storage(e.to_string()))?
            .into_iter()
            .find(|p| p.id == project_id)
            .ok_or(TauriIndexingError::WorktreeNotFound)?;
        return Ok(WorktreeContext {
            environment: p.environment,
            path: p.root_path,
        });
    }
    lookup_worktree(state, worktree_id)
}

fn lookup_worktree(
    state: &AppState,
    worktree_id: AggregateId,
) -> Result<WorktreeContext, TauriIndexingError> {
    // We don't have a direct "by id" projection lookup, so go via projects
    // and their worktree lists. Fine at our scale (handful of projects per
    // user); revisit if it ever shows up in a profile.
    let projects = state
        .projections
        .list_projects()
        .map_err(|e| TauriIndexingError::Storage(e.to_string()))?;
    for p in projects {
        let worktrees = state
            .projections
            .list_worktrees(p.id, /* include_removed */ false)
            .map_err(|e| TauriIndexingError::Storage(e.to_string()))?;
        if let Some(wt) = worktrees.into_iter().find(|w| w.id == worktree_id) {
            return Ok(WorktreeContext {
                environment: p.environment,
                path: wt.path,
            });
        }
    }
    Err(TauriIndexingError::WorktreeNotFound)
}
