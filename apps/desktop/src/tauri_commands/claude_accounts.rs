//! Tauri IPC for Claude account pools and usage.
//!
//! Every mutating command returns the rebuilt [`UsageBoard`] so the caller
//! renders the new state without waiting for the next poll; the poller's
//! `claude-usage://board` event keeps every other window (taskbar widget) in
//! sync. Nothing here ever carries a token.

use std::collections::HashSet;

use oxyris_core::{AggregateId, Environment};
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::app_state::AppState;
use crate::infra::claude_accounts::{AccountRecord, AccountsError};
use crate::infra::claude_usage::UsageBoard;
use crate::infra::usage_widget::{WidgetMetrics, WidgetSettings};

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriAccountsError {
    #[error("account not found")]
    NotFound,
    #[error("no Claude login found in this environment")]
    NoLiveLogin,
    /// Claude processes are running in the target environment; the UI asks
    /// for confirmation and retries with `force`.
    #[error("{0} Claude session(s) running in this environment")]
    SessionsRunning(usize),
    /// Carries the distro name.
    #[error("WSL distro {0} is not running")]
    WslOffline(String),
    #[error("{0}")]
    Failed(String),
}

/// Oxyris-owned `claude` processes alive in `env`: structured sessions plus
/// pure-mode PTYs. Each holds its login in memory and writes a refreshed
/// token back to `.credentials.json` — after a switch that would pair the new
/// account's identity with the old account's token.
fn running_claude_sessions(state: &AppState, env: &Environment) -> usize {
    let mut ids: HashSet<AggregateId> = state.pty.claude_session_ids().into_iter().collect();
    let mut by_session: Vec<(AggregateId, AggregateId)> = Vec::new();
    if let Ok(rows) = state.projections.list_running_sessions() {
        for row in rows {
            ids.remove(&row.id);
            by_session.push((row.id, row.project_id));
        }
    }
    for id in ids {
        if let Ok(Some(snap)) = state.projections.get_session(id) {
            by_session.push((id, snap.data.project_id));
        }
    }
    let Ok(projects) = state.projections.list_projects() else {
        return 0;
    };
    by_session
        .iter()
        .filter(|(_, project_id)| {
            projects
                .iter()
                .any(|p| &p.id == project_id && &p.environment == env)
        })
        .count()
}

impl From<AccountsError> for TauriAccountsError {
    fn from(e: AccountsError) -> Self {
        match e {
            AccountsError::NotFound => TauriAccountsError::NotFound,
            AccountsError::NoLiveLogin => TauriAccountsError::NoLiveLogin,
            AccountsError::WslOffline(distro) => TauriAccountsError::WslOffline(distro),
            other => TauriAccountsError::Failed(other.to_string()),
        }
    }
}

/// Cached board (fresh one on first call).
#[tauri::command]
pub async fn claude_usage_board(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UsageBoard, ()> {
    Ok(state.claude_usage.board(&app).await)
}

/// Re-fetch everything that isn't rate-limited right now.
#[tauri::command]
pub async fn claude_usage_refresh(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UsageBoard, ()> {
    Ok(state.claude_usage.refresh(&app, true).await)
}

/// Start monitoring an environment (the panel calls this for each distro the
/// user expands) and return the board including it.
#[tauri::command]
pub async fn claude_accounts_track(
    environment: Environment,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UsageBoard, ()> {
    state.claude_usage.track(&environment).await;
    Ok(state.claude_usage.refresh(&app, false).await)
}

/// Register the environment's current login as a reserve slot.
#[tauri::command]
pub async fn claude_accounts_save_live(
    environment: Environment,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UsageBoard, TauriAccountsError> {
    let usage = &state.claude_usage;
    usage.accounts().save_live(&environment).await?;
    usage.track(&environment).await;
    usage.invalidate(&environment).await;
    Ok(usage.refresh_after_change(&app).await)
}

/// Open a console running `claude auth login` against a fresh slot and wait
/// for it to close. `None` when the user closed it without logging in.
#[tauri::command]
pub async fn claude_accounts_login(
    environment: Environment,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<AccountRecord>, TauriAccountsError> {
    let usage = &state.claude_usage;
    let record = usage.accounts().login_new(&environment).await?;
    if record.is_some() {
        usage.track(&environment).await;
        usage.invalidate(&environment).await;
        usage.refresh_after_change(&app).await;
    }
    Ok(record)
}

/// One-click switch: make `id` the live login of `environment`.
#[tauri::command]
pub async fn claude_accounts_activate(
    environment: Environment,
    id: String,
    force: Option<bool>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UsageBoard, TauriAccountsError> {
    if !force.unwrap_or(false) {
        let running = running_claude_sessions(&state, &environment);
        if running > 0 {
            return Err(TauriAccountsError::SessionsRunning(running));
        }
    }
    let usage = &state.claude_usage;
    usage.accounts().activate(&environment, &id).await?;
    usage.invalidate(&environment).await;
    Ok(usage.refresh_after_change(&app).await)
}

#[tauri::command]
pub async fn claude_accounts_remove(
    environment: Environment,
    id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UsageBoard, TauriAccountsError> {
    let usage = &state.claude_usage;
    usage.accounts().remove(&environment, &id).await?;
    usage.invalidate(&environment).await;
    Ok(usage.refresh_after_change(&app).await)
}

#[tauri::command]
pub fn usage_widget_settings(state: State<'_, AppState>) -> WidgetSettings {
    state.usage_widget.settings()
}

#[tauri::command]
pub fn usage_widget_set_settings(
    settings: WidgetSettings,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .usage_widget
        .set_settings(settings)
        .map_err(|e| e.to_string())
}

/// Current widget geometry/theme; `None` until the widget is placed (or off
/// Windows).
#[tauri::command]
pub fn usage_widget_metrics(state: State<'_, AppState>) -> Option<WidgetMetrics> {
    state.usage_widget.metrics()
}

/// Push a rendered frame (RGBA, straight alpha, `width × height`).
#[tauri::command]
pub fn usage_widget_paint(
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.usage_widget.paint(rgba, width, height)
}
