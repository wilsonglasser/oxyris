//! Tauri IPC for the per-worktree Docker env contract.
//!
//! - `env_template_for_worktree`  — returns whether the worktree has a
//!   `.oxyris/compose.yml` and the canonical docker-project name we'd use.
//! - `env_status_for_worktree`    — `docker ps` filtered to our project,
//!   tells the UI whether the stack is up and which services are visible.
//! - `env_up_for_worktree`        — spawns a terminal that runs
//!   `docker compose -f .oxyris/compose.yml -p <name> up -d`.
//! - `env_down_for_worktree`      — same shape, runs `down -v`.

use std::process::Stdio;

use oxyris_core::{AggregateId, Environment};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tokio::process::Command;

use crate::app_state::AppState;
use crate::infra::dotenv_render::{self, DotenvStatus, RenderOutcome};
use crate::infra::env_template::{self, EnvTemplate};
use crate::infra::pty::TerminalInfo;

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriEnvError {
    #[error("worktree not found")]
    NotFound,
    #[error("docker not available: {0}")]
    Docker(String),
    #[error("agent: {0}")]
    Agent(String),
    #[error("storage: {0}")]
    Storage(String),
    #[error("pty: {0}")]
    Pty(String),
}

impl From<crate::infra::agent_pool::AgentError> for TauriEnvError {
    fn from(e: crate::infra::agent_pool::AgentError) -> Self {
        TauriEnvError::Agent(e.to_string())
    }
}

impl From<crate::infra::projections::ProjectionError> for TauriEnvError {
    fn from(e: crate::infra::projections::ProjectionError) -> Self {
        TauriEnvError::Storage(e.to_string())
    }
}

impl From<crate::infra::pty::PtyError> for TauriEnvError {
    fn from(e: crate::infra::pty::PtyError) -> Self {
        TauriEnvError::Pty(e.to_string())
    }
}

impl From<crate::infra::dotenv_render::RenderError> for TauriEnvError {
    fn from(e: crate::infra::dotenv_render::RenderError) -> Self {
        TauriEnvError::Docker(e.to_string())
    }
}

#[derive(Debug, Deserialize)]
pub struct WorktreeIdInput {
    pub worktree_id: AggregateId,
}

#[tauri::command]
pub async fn env_template_for_worktree(
    input: WorktreeIdInput,
    state: State<'_, AppState>,
) -> Result<EnvTemplate, TauriEnvError> {
    let (env, _project_root, wt_path) = resolve_worktree(&state, input.worktree_id)?;
    let template =
        env_template::detect(&env, &state.agent_pool, input.worktree_id, &wt_path).await?;
    Ok(template)
}

#[derive(Debug, Serialize)]
pub struct EnvStatus {
    pub up: bool,
    pub services: Vec<String>,
}

#[tauri::command]
pub async fn env_status_for_worktree(
    input: WorktreeIdInput,
    state: State<'_, AppState>,
) -> Result<EnvStatus, TauriEnvError> {
    let (env, _project_root, _wt_path) = resolve_worktree(&state, input.worktree_id)?;
    let project_name = env_template::docker_project_name(input.worktree_id);
    let services = docker_ps_services(&env, &state, &project_name).await?;
    Ok(EnvStatus {
        up: !services.is_empty(),
        services,
    })
}

#[derive(Debug, Deserialize)]
pub struct WorktreeSessionInput {
    pub worktree_id: AggregateId,
    /// Active session id — needed to spawn the terminal that hosts the
    /// docker compose command. PTYs are session-scoped (see PtySupervisor).
    pub session_id: AggregateId,
}

#[tauri::command]
pub async fn env_dotenv_render_for_worktree(
    input: WorktreeIdInput,
    state: State<'_, AppState>,
) -> Result<RenderOutcome, TauriEnvError> {
    let (env, _project_root, wt_path) = resolve_worktree(&state, input.worktree_id)?;
    let outcome =
        dotenv_render::render_for_worktree(&env, &state.agent_pool, input.worktree_id, &wt_path)
            .await?;
    Ok(outcome)
}

#[tauri::command]
pub async fn env_dotenv_status_for_worktree(
    input: WorktreeIdInput,
    state: State<'_, AppState>,
) -> Result<DotenvStatus, TauriEnvError> {
    let (env, _project_root, wt_path) = resolve_worktree(&state, input.worktree_id)?;
    Ok(dotenv_render::status_for_worktree(&env, &state.agent_pool, &wt_path).await?)
}

#[tauri::command]
pub async fn env_up_for_worktree(
    input: WorktreeSessionInput,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TerminalInfo, TauriEnvError> {
    // Make sure the .env.local is in sync with the template before docker
    // compose reads it (ignored if no template / manual override).
    let (env, _project_root, wt_path) = resolve_worktree(&state, input.worktree_id)?;
    let _ =
        dotenv_render::render_for_worktree(&env, &state.agent_pool, input.worktree_id, &wt_path)
            .await;
    spawn_compose_command(input, app, state, "up", &["-d"]).await
}

#[tauri::command]
pub async fn env_down_for_worktree(
    input: WorktreeSessionInput,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TerminalInfo, TauriEnvError> {
    spawn_compose_command(input, app, state, "down", &["-v"]).await
}

async fn spawn_compose_command(
    input: WorktreeSessionInput,
    app: AppHandle,
    state: State<'_, AppState>,
    sub: &str,
    extra: &[&str],
) -> Result<TerminalInfo, TauriEnvError> {
    let (env, _project_root, wt_path) = resolve_worktree(&state, input.worktree_id)?;
    let template =
        env_template::detect(&env, &state.agent_pool, input.worktree_id, &wt_path).await?;
    if !template.has_template {
        return Err(TauriEnvError::Docker(
            "this worktree has no .oxyris/compose.yml".into(),
        ));
    }
    let project_name = template.docker_project.clone();
    let compose_path = template
        .template_path
        .clone()
        .unwrap_or_else(|| ".oxyris/compose.yml".to_owned());

    let mut command = format!(
        "docker compose -f {} -p {} {}",
        shell_escape(&compose_path),
        shell_escape(&project_name),
        sub
    );
    for arg in extra {
        command.push(' ');
        command.push_str(arg);
    }

    // Spawn a terminal in the worktree dir so the user can see output and
    // tail subsequent compose subcommands.
    let info = state
        .pty
        .spawn(app, &env, input.session_id, &wt_path, 100, 30)?;
    state
        .pty
        .write(&info.id, &format!("{command}\r"))
        .map_err(TauriEnvError::from)?;
    Ok(info)
}

fn resolve_worktree(
    state: &AppState,
    worktree_id: AggregateId,
) -> Result<(Environment, String, String), TauriEnvError> {
    let projects = state.projections.list_projects()?;
    for p in &projects {
        let wts = state.projections.list_worktrees(p.id, false)?;
        if let Some(wt) = wts.into_iter().find(|w| w.id == worktree_id) {
            return Ok((p.environment.clone(), p.root_path.clone(), wt.path));
        }
    }
    Err(TauriEnvError::NotFound)
}

async fn docker_ps_services(
    env: &Environment,
    _state: &State<'_, AppState>,
    project_name: &str,
) -> Result<Vec<String>, TauriEnvError> {
    // Always ask the host's docker daemon (Docker Desktop on Windows; same
    // engine inside WSL via the integration). One quick `docker ps` scoped
    // to the compose project label gives us the live service list.
    let label = format!("label=com.docker.compose.project={project_name}");
    let mut cmd = match env {
        Environment::Windows => {
            let mut c = Command::new("docker");
            c.args([
                "ps",
                "--filter",
                &label,
                "--format",
                "{{.Names}}\t{{.State}}",
            ]);
            c
        }
        Environment::Wsl { distro } => {
            let mut c = Command::new("wsl.exe");
            c.args([
                "-d",
                distro,
                "--",
                "docker",
                "ps",
                "--filter",
                &label,
                "--format",
                "{{.Names}}\t{{.State}}",
            ]);
            c
        }
    };
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let out = cmd
        .output()
        .await
        .map_err(|e| TauriEnvError::Docker(e.to_string()))?;
    if !out.status.success() {
        return Err(TauriEnvError::Docker(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let services: Vec<String> = stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let name = parts.next()?.trim();
            if name.is_empty() {
                None
            } else {
                Some(name.to_owned())
            }
        })
        .collect();
    Ok(services)
}

fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | ':' | '\\'))
    {
        s.to_owned()
    } else {
        format!("\"{}\"", s.replace('"', "\\\""))
    }
}
