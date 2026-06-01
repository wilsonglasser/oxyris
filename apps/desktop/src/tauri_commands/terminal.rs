//! Tauri IPC for terminals. Writes/resizes/kills are sync since portable-pty
//! is sync; event streaming back to the UI happens via `terminal:<id>:output`
//! and `terminal:<id>:exit` Tauri events emitted from the supervisor's reader
//! thread.

use oxyris_core::{AggregateId, Environment};
use oxyris_ipc::ops::op_name;
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
    /// Extra system-prompt text (e.g. the response-language directive). Only
    /// honored by `claude_pty_spawn`; ignored for plain shell terminals. Merged
    /// ahead of the MCP tool nudge.
    #[serde(default)]
    pub system_prompt: Option<String>,
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
    let (mcp_config_path, mcp_nudge) = if matches!(env, Environment::Local) {
        let lsp_port = state.session_supervisor.lsp_bridge_port();
        match crate::infra::mcp::prepare_for_worktree(&env, &cwd, lsp_port) {
            Ok(Some(setup)) => (Some(setup.config_path), Some(setup.system_prompt_nudge)),
            _ => (None, None),
        }
    } else {
        (None, None)
    };

    // Response-language directive (from the UI) goes first, then the MCP tool
    // nudge — same ordering the structured provider uses in `augment_with_mcp`.
    let language = input.system_prompt.filter(|s| !s.trim().is_empty());
    let system_prompt = match (language, mcp_nudge) {
        (Some(lang), Some(mcp)) => Some(format!("{lang}\n\n{mcp}")),
        (Some(lang), None) => Some(lang),
        (None, mcp) => mcp,
    };

    // If claude already wrote a transcript under this id (resumed session, e.g.
    // after an app restart), spawn with `--resume` instead of `--session-id` —
    // the latter is rejected as "already in use".
    let resume = claude_transcript_exists(&env, &state.agent_pool, input.session_id).await;

    let opts = crate::infra::pty::ClaudePtyOpts {
        session_id: input.session_id.to_string(),
        resume,
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

#[derive(Debug, Deserialize)]
pub struct PureTitleInput {
    pub session_id: AggregateId,
}

/// Best-effort auto-title for a pure-mode session, derived from claude's own
/// transcript. Pure sessions have no turn-event stream to title from (that's
/// structured mode), so the frontend calls this after a turn settles. Because
/// we spawn claude with `--session-id <session>`, its transcript is named
/// `<session>.jsonl`; we locate it under `~/.claude/projects/`, read the head,
/// and prefer claude's own summary, falling back to the first user message.
///
/// Returns the applied title, or `None` when the session is already titled, no
/// transcript exists yet, or nothing usable was found. Never clobbers a title
/// that's already set (auto or manual).
#[tauri::command]
pub async fn claude_pure_refresh_title(
    input: PureTitleInput,
    state: State<'_, AppState>,
) -> Result<Option<String>, TauriTerminalError> {
    let session_id = input.session_id;
    let snap = state
        .projections
        .get_session(session_id)
        .map_err(|e| TauriTerminalError::Pty(e.to_string()))?
        .ok_or(TauriTerminalError::NotFound)?;
    if snap.data.title.is_some() {
        return Ok(None);
    }
    let projects = state
        .projections
        .list_projects()
        .map_err(|e| TauriTerminalError::Pty(e.to_string()))?;
    let project = projects
        .into_iter()
        .find(|p| p.id == snap.data.project_id)
        .ok_or(TauriTerminalError::NotFound)?;

    let Some(text) =
        read_claude_transcript(&project.environment, &state.agent_pool, session_id).await
    else {
        return Ok(None);
    };
    let Some(title) = derive_title_from_transcript(&text) else {
        return Ok(None);
    };

    state
        .session_supervisor
        .rename_session(session_id, title.clone())
        .await
        .map_err(|e| TauriTerminalError::Pty(e.to_string()))?;
    Ok(Some(title))
}

/// Whether claude has already written a transcript for `session_id`. Same
/// lookup as `read_claude_transcript` (Windows: scan one-level project dirs;
/// WSL: agent path search) but stops at first hit and reads no content — it
/// only decides `--resume` vs `--session-id` at spawn. Any failure → `false`
/// (treat as fresh; worst case claude itself reports the collision).
async fn claude_transcript_exists(
    env: &Environment,
    agent: &crate::infra::agent_pool::AgentPool,
    session_id: AggregateId,
) -> bool {
    let filename = format!("{session_id}.jsonl");
    match env {
        Environment::Local => {
            let Some(home) = oxyris_procutil::home_dir() else {
                return false;
            };
            let projects = home.join(".claude").join("projects");
            let Ok(entries) = std::fs::read_dir(&projects) else {
                return false;
            };
            entries.flatten().any(|entry| {
                entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    && entry.path().join(&filename).is_file()
            })
        }
        Environment::Wsl { distro } => {
            let Ok(info) = agent
                .call(distro, op_name::SYSTEM_INFO, serde_json::json!({}))
                .await
            else {
                return false;
            };
            let Some(home) = info.get("home").and_then(|v| v.as_str()) else {
                return false;
            };
            let root = format!("{}/.claude/projects", home.trim_end_matches('/'));
            let Ok(found) = agent
                .call(
                    distro,
                    op_name::FS_SEARCH_PATHS,
                    serde_json::json!({ "root": root, "query": filename, "limit": 5 }),
                )
                .await
            else {
                return false;
            };
            found
                .get("hits")
                .and_then(|h| h.as_array())
                .map(|hits| {
                    hits.iter().any(|h| {
                        h.get("rel_path")
                            .and_then(|p| p.as_str())
                            .map(|p| p.ends_with(&filename))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        }
    }
}

/// Read the head of claude's transcript JSONL for `session_id`. The file is
/// `~/.claude/projects/<cwd-slug>/<session_id>.jsonl`; we don't reconstruct the
/// slug — we just look for the uniquely-named file (Windows: scan the one-level
/// project dirs; WSL: the agent's path search). Only the head is read — the
/// summary and first user message both live near the top.
async fn read_claude_transcript(
    env: &Environment,
    agent: &crate::infra::agent_pool::AgentPool,
    session_id: AggregateId,
) -> Option<String> {
    const HEAD_CAP: u64 = 512 * 1024;
    let filename = format!("{session_id}.jsonl");
    match env {
        Environment::Local => {
            let home = oxyris_procutil::home_dir()?;
            let projects = home.join(".claude").join("projects");
            for entry in std::fs::read_dir(&projects).ok()?.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let candidate = entry.path().join(&filename);
                    if candidate.is_file() {
                        return read_head(&candidate, HEAD_CAP);
                    }
                }
            }
            None
        }
        Environment::Wsl { distro } => {
            let info = agent
                .call(distro, op_name::SYSTEM_INFO, serde_json::json!({}))
                .await
                .ok()?;
            let home = info
                .get("home")
                .and_then(|v| v.as_str())?
                .trim_end_matches('/');
            let root = format!("{home}/.claude/projects");
            let found = agent
                .call(
                    distro,
                    op_name::FS_SEARCH_PATHS,
                    serde_json::json!({ "root": root, "query": filename, "limit": 5 }),
                )
                .await
                .ok()?;
            let rel = found.get("hits")?.as_array()?.iter().find_map(|h| {
                let p = h.get("rel_path")?.as_str()?;
                p.ends_with(&filename).then(|| p.to_owned())
            })?;
            let full = format!("{root}/{rel}");
            let read = agent
                .call(
                    distro,
                    op_name::FS_READ,
                    serde_json::json!({ "path": full, "max_bytes": HEAD_CAP }),
                )
                .await
                .ok()?;
            read.get("content")?.as_str().map(|s| s.to_owned())
        }
    }
}

fn read_head(path: &std::path::Path, cap: u64) -> Option<String> {
    use std::io::Read as _;
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(cap)
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Pull a title out of a claude transcript. Prefers claude's own `summary`
/// entry (what `--resume` shows); falls back to the first user prompt.
fn derive_title_from_transcript(text: &str) -> Option<String> {
    let mut summary: Option<String> = None;
    let mut first_user: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("summary") if summary.is_none() => {
                if let Some(s) = v.get("summary").and_then(|s| s.as_str())
                    && !s.trim().is_empty()
                {
                    summary = Some(s.trim().to_owned());
                }
            }
            Some("user") if first_user.is_none() => {
                if let Some(txt) = extract_user_text(&v)
                    && !txt.trim().is_empty()
                {
                    first_user = Some(txt.trim().to_owned());
                }
            }
            _ => {}
        }
    }
    let raw = summary.or(first_user)?;
    let title = normalize_title(&raw);
    (!title.is_empty()).then_some(title)
}

/// A user record's prompt text — `message.content` is either a bare string or
/// an array of content blocks; take the first text block.
fn extract_user_text(v: &serde_json::Value) -> Option<String> {
    let content = v.get("message")?.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_owned());
    }
    content.as_array()?.iter().find_map(|block| {
        (block.get("type").and_then(|t| t.as_str()) == Some("text"))
            .then(|| {
                block
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_owned())
            })
            .flatten()
    })
}

/// First non-empty line, trimmed, capped at 60 chars (ellipsised if longer).
fn normalize_title(text: &str) -> String {
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut out: String = first.chars().take(60).collect();
    if first.chars().count() > 60 {
        out.push('…');
    }
    out
}
