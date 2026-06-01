//! Path-validation helpers for the project-creation UI. Answers "does this
//! path exist? is it a directory? is it a git repo?" — enough to decorate the
//! form inputs with a green check or a red note.

use std::path::Path;

use oxyris_core::Environment;
use oxyris_ipc::ops::op_name;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::app_state::AppState;

#[derive(Debug, Deserialize)]
pub struct ValidatePathInput {
    pub environment: Environment,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PathValidation {
    pub exists: bool,
    pub is_dir: bool,
    pub is_git_repo: bool,
    /// Warning string when the path is suspicious but not necessarily wrong.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriValidateError {
    #[error("{0}")]
    Agent(String),
}

impl From<crate::infra::agent_pool::AgentError> for TauriValidateError {
    fn from(e: crate::infra::agent_pool::AgentError) -> Self {
        TauriValidateError::Agent(e.to_string())
    }
}

#[tauri::command]
pub async fn project_validate_path(
    input: ValidatePathInput,
    state: State<'_, AppState>,
) -> Result<PathValidation, TauriValidateError> {
    match input.environment {
        Environment::Local => Ok(validate_windows(&input.path)),
        Environment::Wsl { distro } => validate_wsl(&state, &distro, &input.path).await,
    }
}

fn validate_windows(path: &str) -> PathValidation {
    let p = Path::new(path);
    let meta = std::fs::metadata(p).ok();
    let exists = meta.is_some();
    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
    let is_git_repo = is_dir && p.join(".git").exists();
    PathValidation {
        exists,
        is_dir,
        is_git_repo,
        warning: None,
    }
}

async fn validate_wsl(
    state: &AppState,
    distro: &str,
    path: &str,
) -> Result<PathValidation, TauriValidateError> {
    // Refuse `/mnt/*` paths — those are interop-mounted Windows drives seen
    // from inside WSL and using them as WSL projects defeats the whole
    // routing model.
    let warning = if path.starts_with("/mnt/") {
        Some(
            "Path looks like a Windows drive mounted inside WSL. Use a Windows \
             project instead."
                .to_owned(),
        )
    } else {
        None
    };

    let root = state
        .agent_pool
        .call(
            distro,
            op_name::FS_STAT,
            serde_json::json!({ "path": path }),
        )
        .await?;
    let exists = root
        .get("exists")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let is_dir = root
        .get("is_dir")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let is_git_repo = if is_dir {
        let git = state
            .agent_pool
            .call(
                distro,
                op_name::FS_STAT,
                serde_json::json!({ "path": format!("{}/.git", path.trim_end_matches('/')) }),
            )
            .await?;
        git.get("exists").and_then(|v| v.as_bool()).unwrap_or(false)
    } else {
        false
    };

    Ok(PathValidation {
        exists,
        is_dir,
        is_git_repo,
        warning,
    })
}
