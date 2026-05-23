//! Tauri IPC for terminals. Writes/resizes/kills are sync since portable-pty
//! is sync; event streaming back to the UI happens via `terminal:<id>:output`
//! and `terminal:<id>:exit` Tauri events emitted from the supervisor's reader
//! thread.

use oxyris_core::{AggregateId, Environment};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::app_state::AppState;
use crate::infra::pty::{PtyError, TerminalAttachSnapshot, TerminalInfo};

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriTerminalError {
    #[error("{0}")]
    Pty(String),
    #[error("session or project not found")]
    NotFound,
}

impl From<PtyError> for TauriTerminalError {
    fn from(e: PtyError) -> Self {
        TauriTerminalError::Pty(e.to_string())
    }
}

#[derive(Debug, Deserialize)]
pub struct TerminalSpawnInput {
    pub session_id: AggregateId,
    pub cols: u16,
    pub rows: u16,
}

#[tauri::command]
pub async fn terminal_spawn(
    input: TerminalSpawnInput,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TerminalInfo, TauriTerminalError> {
    // The session's worktree dictates the terminal's cwd so each session
    // operates in its own working tree. We fall back to the project root
    // when the session has no explicit worktree.
    let snap = state
        .projections
        .get_session(input.session_id)
        .map_err(|e| TauriTerminalError::Pty(e.to_string()))?
        .ok_or(TauriTerminalError::NotFound)?;
    let projects = state
        .projections
        .list_projects()
        .map_err(|e| TauriTerminalError::Pty(e.to_string()))?;
    let project = projects
        .into_iter()
        .find(|p| p.id == snap.data.project_id)
        .ok_or(TauriTerminalError::NotFound)?;

    let (cwd, worktree_id) = if let Some(wt_id) = snap.data.worktree_id {
        let wts = state
            .projections
            .list_worktrees(snap.data.project_id, false)
            .map_err(|e| TauriTerminalError::Pty(e.to_string()))?;
        let path = wts
            .into_iter()
            .find(|w| w.id == wt_id)
            .map(|w| w.path)
            .unwrap_or(project.root_path.clone());
        (path, Some(wt_id))
    } else {
        (project.root_path.clone(), None)
    };

    let env: Environment = project.environment;

    // If this session is in worktree env mode AND the worktree actually has a
    // template, inject the OXYRIS_* env vars so any docker/compose command
    // the user runs picks them up automatically.
    let mut extra_env: Vec<(String, String)> = Vec::new();
    if matches!(
        snap.data.env_mode,
        crate::domain::session::EnvMode::Worktree
    ) && let Some(wt_id) = worktree_id
    {
        match crate::infra::env_template::detect(&env, &state.agent_pool, wt_id, &cwd).await {
            Ok(tpl) if tpl.has_template => {
                extra_env =
                    crate::infra::env_template::env_vars(wt_id, tpl.template_path.as_deref());
            }
            _ => {}
        }
    }

    Ok(state.pty.spawn_with_env(
        app,
        &env,
        input.session_id,
        &cwd,
        input.cols,
        input.rows,
        &extra_env,
    )?)
}

/// Spawn the interactive `claude` TUI in a PTY for a Pure-mode session — the
/// "Claude Code puro" mode. Resolves cwd/env/worktree exactly like
/// `terminal_spawn`, then injects the per-worktree MCP config + system-prompt
/// nudge + the session's model/runtime so the pure session keeps our index,
/// LSP bridge and workspace controls.
#[tauri::command]
pub async fn claude_pty_spawn(
    input: TerminalSpawnInput,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TerminalInfo, TauriTerminalError> {
    let snap = state
        .projections
        .get_session(input.session_id)
        .map_err(|e| TauriTerminalError::Pty(e.to_string()))?
        .ok_or(TauriTerminalError::NotFound)?;
    let projects = state
        .projections
        .list_projects()
        .map_err(|e| TauriTerminalError::Pty(e.to_string()))?;
    let project = projects
        .into_iter()
        .find(|p| p.id == snap.data.project_id)
        .ok_or(TauriTerminalError::NotFound)?;

    let (cwd, worktree_id) = if let Some(wt_id) = snap.data.worktree_id {
        let wts = state
            .projections
            .list_worktrees(snap.data.project_id, false)
            .map_err(|e| TauriTerminalError::Pty(e.to_string()))?;
        let path = wts
            .into_iter()
            .find(|w| w.id == wt_id)
            .map(|w| w.path)
            .unwrap_or(project.root_path.clone());
        (path, Some(wt_id))
    } else {
        (project.root_path.clone(), None)
    };

    let env: Environment = project.environment;

    let mut extra_env: Vec<(String, String)> = Vec::new();
    if matches!(
        snap.data.env_mode,
        crate::domain::session::EnvMode::Worktree
    ) && let Some(wt_id) = worktree_id
    {
        match crate::infra::env_template::detect(&env, &state.agent_pool, wt_id, &cwd).await {
            Ok(tpl) if tpl.has_template => {
                extra_env =
                    crate::infra::env_template::env_vars(wt_id, tpl.template_path.as_deref());
            }
            _ => {}
        }
    }

    // Best-effort MCP config so the pure session gets the index + LSP bridge.
    // Windows only: the `oxyris-mcp` binary is a Windows exe and the config
    // lands at a Windows path — neither is reachable from claude running
    // inside a WSL distro, so we don't wire MCP there yet (matches the
    // structured provider's WSL limitation). Missing binary just means claude
    // runs without it.
    let (mcp_config_path, system_prompt) = if matches!(env, Environment::Windows) {
        let lsp_port = state.session_supervisor.lsp_bridge_port();
        match crate::infra::mcp::prepare_for_worktree(&env, &cwd, lsp_port) {
            Ok(Some(setup)) => (Some(setup.config_path), Some(setup.system_prompt_nudge)),
            _ => (None, None),
        }
    } else {
        (None, None)
    };

    let opts = crate::infra::pty::ClaudePtyOpts {
        model: snap.data.model.clone(),
        permission_mode: runtime_to_permission_mode(snap.data.runtime).to_owned(),
        mcp_config_path,
        system_prompt,
    };

    Ok(state.pty.spawn_claude(
        app,
        &env,
        input.session_id,
        &cwd,
        input.cols,
        input.rows,
        &extra_env,
        opts,
    )?)
}

fn runtime_to_permission_mode(runtime: oxyris_provider::RuntimeMode) -> &'static str {
    use oxyris_provider::RuntimeMode;
    match runtime {
        RuntimeMode::FullAccess => "bypassPermissions",
        RuntimeMode::AcceptEdits => "acceptEdits",
        RuntimeMode::Supervised => "default",
        RuntimeMode::Plan => "plan",
    }
}

#[derive(Debug, Deserialize)]
pub struct TerminalListInput {
    pub session_id: AggregateId,
}

#[tauri::command]
pub fn terminal_list(
    input: TerminalListInput,
    state: State<'_, AppState>,
) -> Result<Vec<TerminalInfo>, TauriTerminalError> {
    Ok(state.pty.list_for_session(input.session_id))
}

#[derive(Debug, Deserialize)]
pub struct TerminalRenameInput {
    pub id: String,
    pub title: String,
}

#[tauri::command]
pub fn terminal_rename(
    input: TerminalRenameInput,
    state: State<'_, AppState>,
) -> Result<(), TauriTerminalError> {
    state.pty.rename(&input.id, &input.title)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct TerminalWriteInput {
    pub id: String,
    pub data: String,
}

#[tauri::command]
pub fn terminal_write(
    input: TerminalWriteInput,
    state: State<'_, AppState>,
) -> Result<(), TauriTerminalError> {
    state.pty.write(&input.id, &input.data)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct TerminalResizeInput {
    pub id: String,
    pub cols: u16,
    pub rows: u16,
}

#[tauri::command]
pub fn terminal_resize(
    input: TerminalResizeInput,
    state: State<'_, AppState>,
) -> Result<(), TauriTerminalError> {
    state.pty.resize(&input.id, input.cols, input.rows)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct TerminalKillInput {
    pub id: String,
}

#[tauri::command]
pub fn terminal_kill(
    input: TerminalKillInput,
    state: State<'_, AppState>,
) -> Result<(), TauriTerminalError> {
    state.pty.kill(&input.id)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct TerminalAttachInput {
    pub id: String,
}

/// Returns a snapshot of everything the reader has emitted so far on this
/// terminal. The frontend calls this right after registering its `output`
/// listener so it can replay the early shell banner that was emitted before
/// `listen()` finished registering — and then deduplicate live events by seq.
#[tauri::command]
pub fn terminal_attach(
    input: TerminalAttachInput,
    state: State<'_, AppState>,
) -> Result<TerminalAttachSnapshot, TauriTerminalError> {
    Ok(state.pty.attach_snapshot(&input.id)?)
}
