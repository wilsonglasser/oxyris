//! Tauri IPC surface for the Worktree aggregate.
//!
//! One `worktree_create` call does both halves of "add a worktree":
//!   1. Ask git (via `infra::git`) to actually put a new working tree on disk.
//!   2. Persist `WorktreeCreated` + update the projection so the UI sees it.
//!
//! The caller just needs to know the project id and branch — everything else
//! (target directory, primary-branch detection) is derived here.

use chrono::{DateTime, Utc};
use oxyris_core::{Aggregate, AggregateId, Environment};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

/// Synthetic id we attach to the project's primary checkout (the repo root).
/// The primary isn't a real git worktree — we surface it as one so the UI
/// can offer it as a card alongside actual worktrees. Backend translates
/// this id back to "no worktree" (i.e., run at the project root) when a
/// session is started with it.
pub const PRIMARY_WORKTREE_SENTINEL: AggregateId = AggregateId(Uuid::nil());

use crate::app_state::AppState;
use crate::domain::worktree::{Worktree, WorktreeCommand, WorktreeEvent, WorktreeState};
use crate::infra::event_store::EventStoreError;
use crate::infra::git;
use crate::infra::projections::{ProjectionError, WorktreeRow};

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriWorktreeError {
    #[error("{0}")]
    Domain(String),
    #[error("{0}")]
    Git(String),
    #[error("{0}")]
    Storage(String),
    #[error("project not found")]
    ProjectNotFound,
    #[error("{0}")]
    Projection(String),
    #[error("repository has no commits yet")]
    EmptyRepo,
}

impl From<git::GitError> for TauriWorktreeError {
    fn from(e: git::GitError) -> Self {
        match e {
            git::GitError::EmptyRepo => TauriWorktreeError::EmptyRepo,
            other => TauriWorktreeError::Git(other.to_string()),
        }
    }
}
impl From<EventStoreError> for TauriWorktreeError {
    fn from(e: EventStoreError) -> Self {
        TauriWorktreeError::Storage(e.to_string())
    }
}
impl From<ProjectionError> for TauriWorktreeError {
    fn from(e: ProjectionError) -> Self {
        TauriWorktreeError::Projection(e.to_string())
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateWorktreeInput {
    pub project_id: AggregateId,
    pub branch: String,
    /// Optional human-friendly name. Defaults to a slug of `branch`.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RemoveWorktreeInput {
    pub id: AggregateId,
}

#[derive(Debug, Deserialize)]
pub struct ListWorktreesInput {
    pub project_id: AggregateId,
    #[serde(default)]
    pub include_removed: bool,
}

#[tauri::command]
pub async fn worktree_create(
    input: CreateWorktreeInput,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<WorktreeRow, TauriWorktreeError> {
    let project = find_project(&state, input.project_id)?;
    let name = input
        .name
        .unwrap_or_else(|| slugify(&input.branch))
        .trim()
        .to_owned();
    if name.is_empty() {
        return Err(TauriWorktreeError::Domain(
            "worktree name must be non-empty".into(),
        ));
    }
    let target_dir = worktree_target_path(&project.root_path, &project.environment, &name);

    let created = git::create_worktree(
        &project.environment,
        &state.agent_pool,
        &project.root_path,
        &name,
        &input.branch,
        &target_dir,
        /* create_branch */ true,
    )
    .await?;

    let id = AggregateId::new();
    let events = Worktree::decide(
        &WorktreeState::default(),
        WorktreeCommand::Create {
            id,
            project_id: input.project_id,
            name: created.name.clone(),
            branch: created.branch.clone().unwrap_or(input.branch),
            path: created.path.clone(),
            is_primary: created.is_primary,
            now: Utc::now(),
        },
    )
    .map_err(|e| TauriWorktreeError::Domain(e.to_string()))?;

    let stored = state.event_store.append(Worktree::KIND, id, 0, &events)?;
    for s in &stored {
        state.projections.apply(s)?;
    }

    // Kick off the initial symbol-index walk in the background — Claude
    // sessions started before it finishes will still work, they just won't
    // see Oxyris tool results until the rebuild completes. WSL projects
    // skip silently (handled inside `IndexingService::rebuild`). Progress
    // is forwarded as `indexing:progress` Tauri events for the UI chip.
    let indexing = state.indexing.clone();
    let env = project.environment.clone();
    let path = created.path.clone();
    let app_for_progress = app.clone();
    tauri::async_runtime::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let pump = tauri::async_runtime::spawn(async move {
            while let Some(p) = rx.recv().await {
                let _ = app_for_progress.emit("indexing:progress", p);
            }
        });
        let result = indexing.rebuild(id, &env, &path, Some(tx)).await;
        match result {
            Ok(report) => {
                tracing::info!(
                    worktree_id = %id,
                    files = report.files_indexed,
                    symbols = report.symbols_extracted,
                    skipped = report.files_skipped,
                    duration_ms = report.duration_ms,
                    "auto-indexed new worktree",
                );
            }
            Err(e) => {
                tracing::debug!(worktree_id = %id, error = %e, "auto-index skipped");
                let _ = app.emit(
                    "indexing:progress",
                    crate::infra::indexing::IndexingProgress::Failed {
                        worktree_id: id,
                        error: e.to_string(),
                    },
                );
            }
        }
        // Drain pump — channel closed when tx drops.
        let _ = pump.await;
    });

    // Pre-warm the primary language's LSP in parallel so the user's first
    // semantic query (hover / find references / diagnostics) doesn't pay
    // the full cold-start cost. Status events flow through `lsp:status`
    // for the UI chip.
    state
        .lsp
        .warm_primary(id, project.environment.clone(), created.path.clone());

    // Return the freshly projected row.
    let rows = state.projections.list_worktrees(input.project_id, true)?;
    rows.into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| TauriWorktreeError::Projection("row missing after insert".into()))
}

#[tauri::command]
pub async fn worktree_remove(
    input: RemoveWorktreeInput,
    state: State<'_, AppState>,
) -> Result<(), TauriWorktreeError> {
    let (wt_state, version) = load_worktree(&state, input.id)?;
    let data = wt_state
        .inner
        .as_ref()
        .ok_or_else(|| TauriWorktreeError::Domain("worktree not found".into()))?;
    if data.is_primary {
        return Err(TauriWorktreeError::Domain(
            "cannot remove primary worktree".into(),
        ));
    }

    let project = find_project(&state, data.project_id)?;
    git::remove_worktree(
        &project.environment,
        &state.agent_pool,
        &project.root_path,
        &data.name,
        &data.path,
    )
    .await?;

    let events = Worktree::decide(&wt_state, WorktreeCommand::Remove { now: Utc::now() })
        .map_err(|e| TauriWorktreeError::Domain(e.to_string()))?;
    let stored = state
        .event_store
        .append(Worktree::KIND, input.id, version, &events)?;
    for s in &stored {
        state.projections.apply(s)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn worktree_list(
    input: ListWorktreesInput,
    state: State<'_, AppState>,
) -> Result<Vec<WorktreeRow>, TauriWorktreeError> {
    let mut rows = state
        .projections
        .list_worktrees(input.project_id, input.include_removed)?;
    // Always prepend a synthetic primary so the UI has something to scope
    // sessions to even on projects that the user hasn't created any
    // worktrees on. Best-effort: if git can't tell us the current branch
    // (empty repo, broken state) we still emit a card with an empty branch.
    if let Ok(primary) = synthesize_primary(&state, input.project_id).await {
        rows.insert(0, primary);
    }
    Ok(rows)
}

/// Build the synthetic primary row from project metadata + git's view of
/// the repo. Returns the row directly, never persists it.
async fn synthesize_primary(
    state: &AppState,
    project_id: AggregateId,
) -> Result<WorktreeRow, TauriWorktreeError> {
    let project = find_project(state, project_id)?;
    // Try to pull the actual primary branch from git; fall back to a blank
    // when the repo is empty or unreadable so the card still renders.
    let primary_from_git =
        git::list_worktrees(&project.environment, &state.agent_pool, &project.root_path)
            .await
            .ok()
            .and_then(|ws| ws.into_iter().find(|w| w.is_primary));
    let branch = primary_from_git
        .as_ref()
        .and_then(|w| w.branch.clone())
        .unwrap_or_default();
    let path = primary_from_git
        .map(|w| w.path)
        .unwrap_or_else(|| project.root_path.clone());

    Ok(WorktreeRow {
        id: PRIMARY_WORKTREE_SENTINEL,
        project_id,
        name: "primary".into(),
        branch,
        path,
        is_primary: true,
        created_at: epoch(),
        removed_at: None,
    })
}

fn epoch() -> DateTime<Utc> {
    DateTime::from_timestamp(0, 0).expect("epoch always parses")
}

/// Enumerate git branches for a project, hitting git directly (not the
/// projection). Needed by the worktree-creation UI.
#[tauri::command]
pub async fn git_list_branches(
    project_id: AggregateId,
    state: State<'_, AppState>,
) -> Result<Vec<git::BranchInfo>, TauriWorktreeError> {
    let project = find_project(&state, project_id)?;
    Ok(git::list_branches(&project.environment, &state.agent_pool, &project.root_path).await?)
}

/// Enumerate git worktrees on disk for a project, bypassing the projection.
/// Useful to reconcile what git thinks vs. what Oxyris has recorded.
#[tauri::command]
pub async fn git_list_worktrees(
    project_id: AggregateId,
    state: State<'_, AppState>,
) -> Result<Vec<git::WorktreeRef>, TauriWorktreeError> {
    let project = find_project(&state, project_id)?;
    Ok(git::list_worktrees(&project.environment, &state.agent_pool, &project.root_path).await?)
}

// ────── helpers ────────────────────────────────────────────────────────────

struct ProjectLite {
    root_path: String,
    environment: Environment,
}

fn find_project(
    state: &AppState,
    project_id: AggregateId,
) -> Result<ProjectLite, TauriWorktreeError> {
    let projects = state
        .projections
        .list_projects()
        .map_err(TauriWorktreeError::from)?;
    let p = projects
        .into_iter()
        .find(|p| p.id == project_id)
        .ok_or(TauriWorktreeError::ProjectNotFound)?;
    Ok(ProjectLite {
        root_path: p.root_path,
        environment: p.environment,
    })
}

fn load_worktree(
    state: &AppState,
    id: AggregateId,
) -> Result<(WorktreeState, u32), TauriWorktreeError> {
    let stored = state.event_store.load(Worktree::KIND, id)?;
    let mut typed = Vec::with_capacity(stored.len());
    for s in &stored {
        let event: WorktreeEvent = serde_json::from_value(s.payload.clone())
            .map_err(|e| TauriWorktreeError::Storage(format!("payload decode: {e}")))?;
        typed.push(event);
    }
    let version = stored.last().map(|s| s.version).unwrap_or(0);
    Ok((oxyris_core::replay::<Worktree>(&typed), version))
}

fn worktree_target_path(root: &str, env: &Environment, name: &str) -> String {
    match env {
        Environment::Windows => {
            // Use backslashes. `root` should already be canonical.
            let sep = if root.ends_with('\\') || root.ends_with('/') {
                ""
            } else {
                "\\"
            };
            format!("{root}{sep}.oxyris\\worktrees\\{name}")
        }
        Environment::Wsl { .. } => {
            let sep = if root.ends_with('/') { "" } else { "/" };
            format!("{root}{sep}.oxyris/worktrees/{name}")
        }
    }
}

fn slugify(s: &str) -> String {
    let s = s.trim().to_ascii_lowercase();
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}
