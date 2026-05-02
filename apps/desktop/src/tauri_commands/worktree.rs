//! Tauri IPC surface for the Worktree aggregate.
//!
//! One `worktree_create` call does both halves of "add a worktree":
//!   1. Ask git (via `infra::git`) to actually put a new working tree on disk.
//!   2. Persist `WorktreeCreated` + update the projection so the UI sees it.
//!
//! The caller just needs to know the project id and branch — everything else
//! (target directory, primary-branch detection) is derived here.

use chrono::Utc;
use oxyris_core::{Aggregate, AggregateId, Environment};
use serde::{Deserialize, Serialize};
use tauri::State;

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
    // see oxyris_* tool results until the rebuild completes. WSL projects
    // skip silently (handled inside `IndexingService::rebuild`).
    let indexing = state.indexing.clone();
    let env = project.environment.clone();
    let path = created.path.clone();
    tauri::async_runtime::spawn(async move {
        match indexing.rebuild(id, &env, &path).await {
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
            }
        }
    });

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
pub fn worktree_list(
    input: ListWorktreesInput,
    state: State<'_, AppState>,
) -> Result<Vec<WorktreeRow>, TauriWorktreeError> {
    Ok(state
        .projections
        .list_worktrees(input.project_id, input.include_removed)?)
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
