//! IPC surface for the Action aggregate. Frontend lists / upserts / deletes;
//! the reactor for `auto_run_on_worktree_create` lives on the frontend side
//! (it listens for worktree-created events and spawns actions that opt in).

use chrono::Utc;
use oxyris_core::{Aggregate, AggregateId};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::app_state::AppState;
use crate::domain::action::{Action, ActionCommand, ActionEvent, ActionState};
use crate::infra::event_store::EventStoreError;
use crate::infra::projections::{ActionRow, ProjectionError};

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriActionError {
    #[error("{0}")]
    Domain(String),
    #[error("{0}")]
    Storage(String),
    #[error("{0}")]
    Projection(String),
    #[error("action not found")]
    NotFound,
}

impl From<EventStoreError> for TauriActionError {
    fn from(e: EventStoreError) -> Self {
        TauriActionError::Storage(e.to_string())
    }
}
impl From<ProjectionError> for TauriActionError {
    fn from(e: ProjectionError) -> Self {
        TauriActionError::Projection(e.to_string())
    }
}

#[derive(Debug, Deserialize)]
pub struct ActionUpsertInput {
    /// None = register new action; Some = update existing.
    #[serde(default)]
    pub id: Option<AggregateId>,
    pub project_id: AggregateId,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub keybinding: Option<String>,
    #[serde(default)]
    pub auto_run_on_worktree_create: bool,
}

#[derive(Debug, Deserialize)]
pub struct ActionListInput {
    pub project_id: AggregateId,
}

#[derive(Debug, Deserialize)]
pub struct ActionDeleteInput {
    pub id: AggregateId,
}

#[tauri::command]
pub fn action_list(
    input: ActionListInput,
    state: State<'_, AppState>,
) -> Result<Vec<ActionRow>, TauriActionError> {
    Ok(state.projections.list_actions(input.project_id)?)
}

#[tauri::command]
pub fn action_upsert(
    input: ActionUpsertInput,
    state: State<'_, AppState>,
) -> Result<ActionRow, TauriActionError> {
    let now = Utc::now();
    let (id, events, version) = match input.id {
        None => {
            let id = AggregateId::new();
            let events = Action::decide(
                &ActionState::default(),
                ActionCommand::Register {
                    id,
                    project_id: input.project_id,
                    name: input.name,
                    command: input.command,
                    keybinding: input.keybinding,
                    auto_run_on_worktree_create: input.auto_run_on_worktree_create,
                    now,
                },
            )
            .map_err(|e| TauriActionError::Domain(e.to_string()))?;
            (id, events, 0)
        }
        Some(id) => {
            let (state_snapshot, version) = load_action(&state, id)?;
            let events = Action::decide(
                &state_snapshot,
                ActionCommand::Update {
                    name: input.name,
                    command: input.command,
                    keybinding: input.keybinding,
                    auto_run_on_worktree_create: input.auto_run_on_worktree_create,
                    now,
                },
            )
            .map_err(|e| TauriActionError::Domain(e.to_string()))?;
            (id, events, version)
        }
    };

    let stored = state
        .event_store
        .append(Action::KIND, id, version, &events)?;
    for s in &stored {
        state.projections.apply(s)?;
    }

    state
        .projections
        .list_actions(input.project_id)?
        .into_iter()
        .find(|row| row.id == id)
        .ok_or_else(|| TauriActionError::Projection("row missing after upsert".into()))
}

#[tauri::command]
pub fn action_delete(
    input: ActionDeleteInput,
    state: State<'_, AppState>,
) -> Result<(), TauriActionError> {
    let (action_state, version) = load_action(&state, input.id)?;
    if action_state.inner.is_none() {
        return Err(TauriActionError::NotFound);
    }
    let events = Action::decide(&action_state, ActionCommand::Remove { now: Utc::now() })
        .map_err(|e| TauriActionError::Domain(e.to_string()))?;
    let stored = state
        .event_store
        .append(Action::KIND, input.id, version, &events)?;
    for s in &stored {
        state.projections.apply(s)?;
    }
    Ok(())
}

fn load_action(state: &AppState, id: AggregateId) -> Result<(ActionState, u32), TauriActionError> {
    let stored = state.event_store.load(Action::KIND, id)?;
    let mut typed = Vec::with_capacity(stored.len());
    for s in &stored {
        let event: ActionEvent = serde_json::from_value(s.payload.clone())
            .map_err(|e| TauriActionError::Storage(format!("payload decode: {e}")))?;
        typed.push(event);
    }
    let version = stored.last().map(|s| s.version).unwrap_or(0);
    Ok((oxyris_core::replay::<Action>(&typed), version))
}
