//! Tauri IPC for the auto-pilot. Engaging stores a mission + supervisor config
//! and starts driving the session's pure (claude) PTY off the backend
//! pure-signal stream; disengaging stops it. Decisions stream back to the UI on
//! `session:<id>:autopilot`.

use oxyris_core::AggregateId;
use oxyris_supervisor::SupervisorKind;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::app_state::AppState;
use crate::infra::autopilot::config_from_parts;

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriAutopilotError {
    #[error("{0}")]
    Engage(String),
}

#[derive(Debug, Deserialize)]
pub struct AutopilotEngageInput {
    pub session_id: AggregateId,
    pub mission: String,
    /// "multi_model" | "claude" (serde snake_case of `SupervisorKind`).
    pub supervisor: SupervisorKind,
    #[serde(default)]
    pub model: Option<String>,
    /// OpenAI-compatible base URL — required for the multi-model supervisor.
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    /// Cap on driven turns for this run; `None` = unbounded.
    #[serde(default)]
    pub max_turns: Option<u32>,
}

#[tauri::command]
pub async fn autopilot_engage(
    input: AutopilotEngageInput,
    state: State<'_, AppState>,
) -> Result<(), TauriAutopilotError> {
    let config = config_from_parts(input.supervisor, input.model, input.base_url, input.api_key)
        .map_err(TauriAutopilotError::Engage)?;
    state
        .autopilot
        .engage(input.session_id, input.mission, config, input.max_turns)
        .await
        .map_err(TauriAutopilotError::Engage)
}

#[derive(Debug, Deserialize)]
pub struct AutopilotDisengageInput {
    pub session_id: AggregateId,
}

#[tauri::command]
pub async fn autopilot_disengage(
    input: AutopilotDisengageInput,
    state: State<'_, AppState>,
) -> Result<(), TauriAutopilotError> {
    state.autopilot.disengage(input.session_id).await;
    Ok(())
}
