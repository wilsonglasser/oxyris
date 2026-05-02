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
use tauri::State;

use crate::app_state::AppState;
use crate::infra::indexing::{IndexingError, RebuildReport};

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriIndexingError {
    #[error("worktree not found")]
    WorktreeNotFound,
    #[error("indexing for WSL projects is not yet supported")]
    WslNotSupported,
    #[error("{0}")]
    Indexing(String),
    #[error("{0}")]
    Storage(String),
}

impl From<IndexingError> for TauriIndexingError {
    fn from(e: IndexingError) -> Self {
        match e {
            IndexingError::WslNotSupported => TauriIndexingError::WslNotSupported,
            other => TauriIndexingError::Indexing(other.to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RebuildInput {
    pub worktree_id: AggregateId,
}

#[derive(Debug, Deserialize)]
pub struct QuerySymbolInput {
    pub worktree_id: AggregateId,
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
        .rebuild(input.worktree_id, &ctx.environment, &ctx.path)
        .await?;
    Ok(report)
}

#[tauri::command]
pub async fn index_query_symbol(
    input: QuerySymbolInput,
    state: State<'_, AppState>,
) -> Result<Vec<SymbolHit>, TauriIndexingError> {
    let ctx = lookup_worktree(&state, input.worktree_id)?;
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
