//! Tauri IPC surface for sessions. Wraps [`SessionSupervisor`] so the front-
//! end just sees: "start a session; push messages; listen to
//! `session:<id>:event` for updates".

use oxyris_core::{AggregateId, Environment};
use oxyris_provider::{RuntimeMode, SessionOptions, ThinkingMode};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::app_state::AppState;
use crate::infra::checkpoint::{self, TurnDiff};
use crate::infra::projections::{ProjectionError, SessionSnapshot, SessionSummaryRow};
use crate::infra::session_supervisor::SupervisorError;

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriSessionError {
    #[error("{0}")]
    Supervisor(String),
    #[error("{0}")]
    Projection(String),
    #[error("{0}")]
    Checkpoint(String),
    #[error("session or project not found")]
    NotFound,
}

impl From<SupervisorError> for TauriSessionError {
    fn from(e: SupervisorError) -> Self {
        TauriSessionError::Supervisor(e.to_string())
    }
}

impl From<ProjectionError> for TauriSessionError {
    fn from(e: ProjectionError) -> Self {
        TauriSessionError::Projection(e.to_string())
    }
}

#[derive(Debug, Deserialize)]
pub struct StartSessionInput {
    pub project_id: AggregateId,
    #[serde(default)]
    pub worktree_id: Option<AggregateId>,
    pub provider_id: String,
    pub environment: Environment,
    pub cwd: String,
    pub model: String,
    #[serde(default)]
    pub thinking: ThinkingMode,
    #[serde(default)]
    pub runtime: RuntimeMode,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub env_mode: crate::domain::session::EnvMode,
    /// `structured` (default) drives the stream-json provider; `pure` creates
    /// a metadata-only session whose conversation runs in an interactive PTY.
    #[serde(default)]
    pub kind: crate::domain::session::SessionKind,
}

#[derive(Debug, Serialize)]
pub struct StartSessionResponse {
    pub session_id: AggregateId,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageInput {
    pub session_id: AggregateId,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct SendMessageResponse {
    pub turn_id: String,
}

#[derive(Debug, Deserialize)]
pub struct InterruptInput {
    pub session_id: AggregateId,
    pub turn_id: String,
}

#[derive(Debug, Deserialize)]
pub struct StopSessionInput {
    pub session_id: AggregateId,
}

#[tauri::command]
pub async fn session_start(
    input: StartSessionInput,
    state: State<'_, AppState>,
) -> Result<StartSessionResponse, TauriSessionError> {
    // The frontend may pass the sentinel id when the user picks the
    // synthetic primary card from the empty state. That doesn't map to a
    // persisted worktree, so we translate it back to "no worktree" here.
    let worktree_id = match input.worktree_id {
        Some(id) if id == crate::tauri_commands::worktree::PRIMARY_WORKTREE_SENTINEL => None,
        other => other,
    };
    let opts = SessionOptions {
        environment: input.environment,
        cwd: input.cwd,
        model: input.model,
        thinking: input.thinking,
        runtime: input.runtime,
        system_prompt: input.system_prompt,
        resume_session_id: None,
        mcp_config_path: None,
    };
    let session_id = state
        .session_supervisor
        .start_session(
            input.project_id,
            worktree_id,
            input.provider_id,
            input.env_mode,
            input.kind,
            opts,
        )
        .await?;
    Ok(StartSessionResponse { session_id })
}

#[tauri::command]
pub async fn session_send_message(
    input: SendMessageInput,
    state: State<'_, AppState>,
) -> Result<SendMessageResponse, TauriSessionError> {
    let turn_id = state
        .session_supervisor
        .send_user_message(input.session_id, input.text)
        .await?;
    Ok(SendMessageResponse { turn_id })
}

#[tauri::command]
pub async fn session_interrupt(
    input: InterruptInput,
    state: State<'_, AppState>,
) -> Result<(), TauriSessionError> {
    state
        .session_supervisor
        .interrupt(input.session_id, input.turn_id)
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn session_stop(
    input: StopSessionInput,
    state: State<'_, AppState>,
) -> Result<(), TauriSessionError> {
    state
        .session_supervisor
        .stop_session(input.session_id)
        .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct RenameSessionInput {
    pub session_id: AggregateId,
    pub title: String,
}

#[tauri::command]
pub async fn session_rename(
    input: RenameSessionInput,
    state: State<'_, AppState>,
) -> Result<(), TauriSessionError> {
    state
        .session_supervisor
        .rename_session(input.session_id, input.title)
        .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct ResumeSessionInput {
    pub session_id: AggregateId,
}

#[tauri::command]
pub async fn session_resume(
    input: ResumeSessionInput,
    state: State<'_, AppState>,
) -> Result<(), TauriSessionError> {
    state
        .session_supervisor
        .resume_session(input.session_id)
        .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct SetEnvModeInput {
    pub session_id: AggregateId,
    pub mode: crate::domain::session::EnvMode,
}

#[tauri::command]
pub async fn session_set_env_mode(
    input: SetEnvModeInput,
    state: State<'_, AppState>,
) -> Result<(), TauriSessionError> {
    state
        .session_supervisor
        .set_env_mode(input.session_id, input.mode)
        .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct DeleteSessionInput {
    pub session_id: AggregateId,
}

#[tauri::command]
pub async fn session_delete(
    input: DeleteSessionInput,
    state: State<'_, AppState>,
) -> Result<(), TauriSessionError> {
    state
        .session_supervisor
        .delete_session(input.session_id)
        .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct TogglePinInput {
    pub session_id: AggregateId,
}

#[tauri::command]
pub async fn session_toggle_pin(
    input: TogglePinInput,
    state: State<'_, AppState>,
) -> Result<(), TauriSessionError> {
    state
        .session_supervisor
        .toggle_pin(input.session_id)
        .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct ListSessionsInput {
    pub project_id: AggregateId,
}

#[tauri::command]
pub fn session_list(
    input: ListSessionsInput,
    state: State<'_, AppState>,
) -> Result<Vec<SessionSummaryRow>, TauriSessionError> {
    Ok(state.projections.list_sessions(input.project_id)?)
}

#[derive(Debug, Deserialize)]
pub struct GetSessionInput {
    pub session_id: AggregateId,
}

#[tauri::command]
pub fn session_get(
    input: GetSessionInput,
    state: State<'_, AppState>,
) -> Result<Option<SessionSnapshot>, TauriSessionError> {
    Ok(state.projections.get_session(input.session_id)?)
}

#[derive(Debug, Deserialize)]
pub struct TurnDiffInput {
    pub session_id: AggregateId,
    pub turn_id: String,
}

#[derive(Debug, Deserialize)]
pub struct TurnRevertInput {
    pub session_id: AggregateId,
    pub turn_id: String,
}

#[tauri::command]
pub async fn session_turn_revert(
    input: TurnRevertInput,
    state: State<'_, AppState>,
) -> Result<(), TauriSessionError> {
    let snap = state
        .projections
        .get_session(input.session_id)?
        .ok_or(TauriSessionError::NotFound)?;
    let projects = state.projections.list_projects()?;
    let project = projects
        .into_iter()
        .find(|p| p.id == snap.data.project_id)
        .ok_or(TauriSessionError::NotFound)?;

    let session_str = input.session_id.to_string();
    let turn_id = input.turn_id.clone();
    checkpoint::revert_to_pre(
        &project.environment,
        &state.agent_pool,
        &project.root_path,
        &session_str,
        &turn_id,
    )
    .await
    .map_err(|e| TauriSessionError::Checkpoint(e.to_string()))?;
    Ok(())
}

#[tauri::command]
pub async fn session_turn_diff(
    input: TurnDiffInput,
    state: State<'_, AppState>,
) -> Result<TurnDiff, TauriSessionError> {
    // Resolve the project for this session to get root_path + env.
    let snap = state
        .projections
        .get_session(input.session_id)?
        .ok_or(TauriSessionError::NotFound)?;
    let projects = state.projections.list_projects()?;
    let project = projects
        .into_iter()
        .find(|p| p.id == snap.data.project_id)
        .ok_or(TauriSessionError::NotFound)?;

    let session_str = input.session_id.to_string();
    let turn_id = input.turn_id.clone();
    checkpoint::diff(
        &project.environment,
        &state.agent_pool,
        &project.root_path,
        &session_str,
        &turn_id,
    )
    .await
    .map_err(|e| TauriSessionError::Checkpoint(e.to_string()))
}
