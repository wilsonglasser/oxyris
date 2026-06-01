//! IPC surface for the Action aggregate. Frontend lists / upserts / deletes;
//! the reactor for `auto_run_on_worktree_create` lives on the frontend side
//! (it listens for worktree-created events and spawns actions that opt in).

use chrono::Utc;
use oxyris_core::{Aggregate, AggregateId, Environment};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

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
    #[serde(default = "default_icon")]
    pub icon: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default = "default_show")]
    pub show_in_sidebar: bool,
}

fn default_icon() -> String {
    "Terminal".into()
}

fn default_kind() -> String {
    "terminal_command".into()
}

fn default_show() -> bool {
    true
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
                    icon: input.icon,
                    kind: input.kind,
                    show_in_sidebar: input.show_in_sidebar,
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
                    icon: input.icon,
                    kind: input.kind,
                    show_in_sidebar: input.show_in_sidebar,
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

// ────── action_run (streaming output) ─────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ActionRunInput {
    pub action_id: AggregateId,
    pub project_id: AggregateId,
    /// Optional worktree to run inside. `None` falls back to project root.
    #[serde(default)]
    pub worktree_id: Option<AggregateId>,
}

#[derive(Debug, Serialize)]
pub struct ActionRunOutput {
    /// Stream id — frontend listens to `action:output:<run_id>` events.
    pub run_id: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActionStreamChunk {
    pub stream: ActionStream,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionStreamLine {
    /// Coalesced batch of output lines. Readers feed individual lines into a
    /// channel; a flusher drains them every ~50ms (or on a 512-line burst) and
    /// emits a single event. Without this, `cargo run` floods the WebView IPC
    /// with thousands of events/sec and freezes the whole app.
    Batch {
        lines: Vec<ActionStreamChunk>,
    },
    Exit {
        code: i32,
        success: bool,
    },
    Error {
        message: String,
    },
}

#[tauri::command]
pub async fn action_run(
    input: ActionRunInput,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ActionRunOutput, TauriActionError> {
    let (action_state, _) = load_action(&state, input.action_id)?;
    let action = action_state
        .inner
        .as_ref()
        .ok_or(TauriActionError::NotFound)?;
    let project = state
        .projections
        .list_projects()
        .map_err(|e| TauriActionError::Projection(e.to_string()))?
        .into_iter()
        .find(|p| p.id == input.project_id)
        .ok_or_else(|| TauriActionError::Domain("project not found".into()))?;

    let cwd = if let Some(wt_id) = input.worktree_id {
        state
            .projections
            .list_worktrees(input.project_id, false)
            .map_err(|e| TauriActionError::Projection(e.to_string()))?
            .into_iter()
            .find(|w| w.id == wt_id)
            .map(|w| w.path)
            .unwrap_or(project.root_path.clone())
    } else {
        project.root_path.clone()
    };

    let mut command = action.command.clone();
    if action.kind == "github_workflow" && !command.starts_with("gh ") {
        command = format!("gh workflow run {command}");
    }

    let run_id = format!("run-{}", uuid::Uuid::now_v7());
    spawn_streaming(app, project.environment, cwd, command, run_id.clone());

    Ok(ActionRunOutput { run_id })
}

fn spawn_streaming(app: AppHandle, env: Environment, cwd: String, command: String, run_id: String) {
    use oxyris_procutil::HideConsole;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command;
    use tokio::sync::mpsc;
    use tokio::time::{Duration, interval};

    /// Flush when a burst piles this many lines before the timer fires, so a
    /// single emitted payload stays bounded.
    const MAX_BATCH: usize = 512;

    tauri::async_runtime::spawn(async move {
        let event_name = format!("action:output:{run_id}");
        let spawn_result = match env {
            Environment::Local => {
                let (sh, pre) = oxyris_procutil::host_shell();
                Command::new(sh)
                    .args(pre)
                    .arg(&command)
                    .current_dir(&cwd)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .hide_console()
                    .spawn()
            }
            Environment::Wsl { ref distro } => Command::new("wsl.exe")
                .args([
                    "-d",
                    distro.as_str(),
                    "--cd",
                    cwd.as_str(),
                    "--",
                    "bash",
                    "-lc",
                    &command,
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .hide_console()
                .spawn(),
        };
        let mut child = match spawn_result {
            Ok(c) => c,
            Err(e) => {
                let _ = app.emit(
                    &event_name,
                    ActionStreamLine::Error {
                        message: e.to_string(),
                    },
                );
                return;
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Readers push individual lines into one channel; the flusher coalesces
        // them into batched events. `tx` is dropped once both readers finish so
        // the flusher sees the channel close and drains the remainder.
        let (tx, mut rx) = mpsc::unbounded_channel::<ActionStreamChunk>();

        let stdout_task = stdout.map(|s| {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(s).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = tx.send(ActionStreamChunk {
                        stream: ActionStream::Stdout,
                        text: line,
                    });
                }
            })
        });
        let stderr_task = stderr.map(|s| {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(s).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = tx.send(ActionStreamChunk {
                        stream: ActionStream::Stderr,
                        text: line,
                    });
                }
            })
        });
        drop(tx);

        let flusher = {
            let app = app.clone();
            let event_name = event_name.clone();
            tokio::spawn(async move {
                let mut buf: Vec<ActionStreamChunk> = Vec::new();
                let mut tick = interval(Duration::from_millis(50));
                loop {
                    tokio::select! {
                        maybe = rx.recv() => match maybe {
                            Some(chunk) => {
                                buf.push(chunk);
                                if buf.len() >= MAX_BATCH {
                                    let lines = std::mem::take(&mut buf);
                                    let _ = app.emit(&event_name, ActionStreamLine::Batch { lines });
                                }
                            }
                            None => {
                                if !buf.is_empty() {
                                    let lines = std::mem::take(&mut buf);
                                    let _ = app.emit(&event_name, ActionStreamLine::Batch { lines });
                                }
                                break;
                            }
                        },
                        _ = tick.tick() => {
                            if !buf.is_empty() {
                                let lines = std::mem::take(&mut buf);
                                let _ = app.emit(&event_name, ActionStreamLine::Batch { lines });
                            }
                        }
                    }
                }
            })
        };

        let exit = child.wait().await;
        if let Some(t) = stdout_task {
            let _ = t.await;
        }
        if let Some(t) = stderr_task {
            let _ = t.await;
        }
        // All `tx` clones now dropped → flusher drains and exits. Awaiting it
        // guarantees every output batch is emitted before the exit event.
        let _ = flusher.await;
        let line = match exit {
            Ok(status) => ActionStreamLine::Exit {
                code: status.code().unwrap_or(-1),
                success: status.success(),
            },
            Err(e) => ActionStreamLine::Error {
                message: e.to_string(),
            },
        };
        let _ = app.emit(&event_name, line);
    });
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
