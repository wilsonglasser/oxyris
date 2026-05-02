//! Tauri IPC surface for the Project aggregate.
//!
//! Handlers here orchestrate one write cycle:
//!
//! 1. load current state via `replay` on the event log,
//! 2. call the pure `decide`,
//! 3. append the produced events under the current version (optimistic
//!    concurrency),
//! 4. apply each stored event to the projection so the UI's next `project_list`
//!    sees it without waiting for a rebuild.

use chrono::Utc;
use oxyris_core::{Aggregate, AggregateId, Environment, replay};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::app_state::AppState;
use crate::domain::project::{Project, ProjectCommand, ProjectError, ProjectEvent, ProjectState};
use crate::infra::event_store::EventStoreError;
use crate::infra::projections::{ProjectRow, ProjectionError};

#[derive(Debug, Serialize)]
pub struct ProjectCreateResponse {
    pub id: AggregateId,
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectInput {
    pub name: String,
    pub environment: Environment,
    pub root_path: String,
}

#[derive(Debug, Deserialize)]
pub struct RenameProjectInput {
    pub id: AggregateId,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteProjectInput {
    pub id: AggregateId,
}

/// A Tauri-facing error. We flatten our internal errors into a discriminated
/// string + optional message so the web can show friendly copy via i18n keys.
#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriProjectError {
    #[error("domain: {0}")]
    Domain(String),
    #[error("concurrency")]
    Concurrency,
    #[error("storage: {0}")]
    Storage(String),
    #[error("projection: {0}")]
    Projection(String),
}

impl From<ProjectError> for TauriProjectError {
    fn from(e: ProjectError) -> Self {
        TauriProjectError::Domain(e.to_string())
    }
}

impl From<EventStoreError> for TauriProjectError {
    fn from(e: EventStoreError) -> Self {
        match e {
            EventStoreError::Concurrency { .. } => TauriProjectError::Concurrency,
            other => TauriProjectError::Storage(other.to_string()),
        }
    }
}

impl From<ProjectionError> for TauriProjectError {
    fn from(e: ProjectionError) -> Self {
        TauriProjectError::Projection(e.to_string())
    }
}

fn load_state(state: &AppState, id: AggregateId) -> Result<(ProjectState, u32), TauriProjectError> {
    let stored = state.event_store.load(Project::KIND, id)?;
    let mut typed = Vec::with_capacity(stored.len());
    for s in &stored {
        let event: ProjectEvent = serde_json::from_value(s.payload.clone())
            .map_err(|e| TauriProjectError::Storage(format!("payload decode: {e}")))?;
        typed.push(event);
    }
    let version = stored.last().map(|s| s.version).unwrap_or(0);
    Ok((replay::<Project>(&typed), version))
}

fn dispatch(
    state: &AppState,
    id: AggregateId,
    current_version: u32,
    events: Vec<ProjectEvent>,
) -> Result<(), TauriProjectError> {
    if events.is_empty() {
        return Ok(());
    }
    let stored = state
        .event_store
        .append(Project::KIND, id, current_version, &events)?;
    for s in &stored {
        state.projections.apply(s)?;
    }
    Ok(())
}

#[tauri::command]
pub fn project_create(
    input: CreateProjectInput,
    state: State<'_, AppState>,
) -> Result<ProjectCreateResponse, TauriProjectError> {
    let id = AggregateId::new();
    let cmd = ProjectCommand::Create {
        id,
        name: input.name,
        environment: input.environment,
        root_path: input.root_path,
        now: Utc::now(),
    };
    // A fresh aggregate starts at version 0.
    let events = Project::decide(&ProjectState::default(), cmd)?;
    dispatch(&state, id, 0, events)?;
    Ok(ProjectCreateResponse { id })
}

#[tauri::command]
pub fn project_rename(
    input: RenameProjectInput,
    state: State<'_, AppState>,
) -> Result<(), TauriProjectError> {
    let (project_state, version) = load_state(&state, input.id)?;
    let events = Project::decide(
        &project_state,
        ProjectCommand::Rename {
            new_name: input.new_name,
        },
    )?;
    dispatch(&state, input.id, version, events)
}

#[tauri::command]
pub fn project_delete(
    input: DeleteProjectInput,
    state: State<'_, AppState>,
) -> Result<(), TauriProjectError> {
    let (project_state, version) = load_state(&state, input.id)?;
    let events = Project::decide(&project_state, ProjectCommand::Delete)?;
    dispatch(&state, input.id, version, events)
}

#[tauri::command]
pub fn project_list(state: State<'_, AppState>) -> Result<Vec<ProjectRow>, TauriProjectError> {
    Ok(state.projections.list_projects()?)
}
