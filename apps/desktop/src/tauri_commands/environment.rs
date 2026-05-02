//! Tauri IPC surface for environment discovery + path translation.

use oxyris_ipc::ops::op_name;
use serde::Serialize;
use tauri::State;

use crate::app_state::AppState;
use crate::infra::agent_pool::AgentError;
use crate::infra::environments::{EnvironmentEntry, environments_list};
use crate::infra::path_translator::{self, PathTranslateError};

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriEnvironmentError {
    #[error("translate: {0}")]
    Translate(String),
    #[error("agent: {0}")]
    Agent(String),
}

impl From<PathTranslateError> for TauriEnvironmentError {
    fn from(e: PathTranslateError) -> Self {
        TauriEnvironmentError::Translate(e.to_string())
    }
}

impl From<AgentError> for TauriEnvironmentError {
    fn from(e: AgentError) -> Self {
        TauriEnvironmentError::Agent(e.to_string())
    }
}

#[tauri::command]
pub fn environment_list() -> Vec<EnvironmentEntry> {
    environments_list()
}

#[tauri::command]
pub fn path_to_posix(
    distro: String,
    windows_path: String,
) -> Result<String, TauriEnvironmentError> {
    Ok(path_translator::to_posix(&distro, &windows_path)?)
}

#[tauri::command]
pub fn path_to_windows(
    distro: String,
    posix_path: String,
) -> Result<String, TauriEnvironmentError> {
    Ok(path_translator::to_windows(&distro, &posix_path)?)
}

/// Round-trip sanity check against a live WSL agent — spawn/deploy on first
/// call, then ask it for `system.info`.
#[tauri::command]
pub async fn wsl_system_info(
    distro: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, TauriEnvironmentError> {
    Ok(state
        .agent_pool
        .call(&distro, op_name::SYSTEM_INFO, serde_json::json!({}))
        .await?)
}

#[tauri::command]
pub async fn wsl_fs_stat(
    distro: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, TauriEnvironmentError> {
    Ok(state
        .agent_pool
        .call(
            &distro,
            op_name::FS_STAT,
            serde_json::json!({ "path": path }),
        )
        .await?)
}

/// Walk a directory inside WSL. Streams every entry back to the frontend as
/// `wsl-fs-walk:<request-id>` Tauri events; returns the final count + whether
/// the walk was truncated.
#[tauri::command]
pub async fn wsl_fs_walk(
    app: tauri::AppHandle,
    distro: String,
    root: String,
    ignore: Vec<String>,
    max_entries: Option<u32>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, TauriEnvironmentError> {
    use tauri::Emitter;
    let walk_id = uuid::Uuid::now_v7().to_string();
    let (mut events_rx, result) = state
        .agent_pool
        .call_streaming(
            &distro,
            op_name::FS_WALK,
            serde_json::json!({
                "root": root,
                "ignore": ignore,
                "max_entries": max_entries,
            }),
        )
        .await?;

    let event_name = format!("wsl-fs-walk:{walk_id}");
    while let Some(entry) = events_rx.recv().await {
        let _ = app.emit(&event_name, entry);
    }

    let mut result = result?;
    if let Some(obj) = result.as_object_mut() {
        obj.insert("walk_id".into(), serde_json::Value::String(walk_id));
    }
    Ok(result)
}
