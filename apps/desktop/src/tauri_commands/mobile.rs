//! Tauri IPC for the mobile-takeover companion server. Start spins up the LAN
//! HTTP/WS server (returning the pairing URL + QR), stop tears it down, status
//! reports whether it's running. Force-release hands a takeover back to the
//! desktop when a phone vanished without releasing.

use serde::Deserialize;
use tauri::State;

use crate::app_state::AppState;
use crate::infra::mobile::{self, MobileInfo};

#[derive(Debug, serde::Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriMobileError {
    #[error("{0}")]
    Start(String),
}

/// Start the server (idempotent — returns the existing info if already running).
#[tauri::command]
pub async fn mobile_takeover_start(
    state: State<'_, AppState>,
) -> Result<MobileInfo, TauriMobileError> {
    // Already running? Return its info without restarting.
    if let Ok(guard) = state.mobile.lock()
        && let Some(server) = guard.as_ref()
    {
        return Ok(server.info());
    }

    let server = mobile::start(
        state.pty.clone(),
        state.projections.clone(),
        state.agent_pool.clone(),
        state.data_dir.clone(),
    )
    .await
    .map_err(|e| TauriMobileError::Start(e.to_string()))?;
    let info = server.info();
    if let Ok(mut guard) = state.mobile.lock() {
        // Lost a race to another start — drop ours, keep the stored one.
        if guard.is_some() {
            server.stop();
            return Ok(guard.as_ref().map(|s| s.info()).unwrap_or(info));
        }
        *guard = Some(server);
    }
    Ok(info)
}

/// Stop the server and drop any in-flight takeovers' sockets.
#[tauri::command]
pub fn mobile_takeover_stop(state: State<'_, AppState>) -> Result<MobileInfo, TauriMobileError> {
    if let Ok(mut guard) = state.mobile.lock()
        && let Some(server) = guard.take()
    {
        server.stop();
    }
    Ok(MobileInfo::stopped())
}

/// Whether the server is running, plus its pairing info if so.
#[tauri::command]
pub fn mobile_takeover_status(state: State<'_, AppState>) -> Result<MobileInfo, TauriMobileError> {
    let info = state
        .mobile
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| s.info()))
        .unwrap_or_else(MobileInfo::stopped);
    Ok(info)
}

#[derive(Debug, Deserialize)]
pub struct ForceReleaseInput {
    pub session_id: oxyris_core::AggregateId,
}

/// Desktop-side "take control back": release the takeover on this session's
/// pure terminal even if the phone is still connected (e.g. it crashed/left
/// without releasing). No-op when the session has no live pure terminal or
/// isn't under takeover.
#[tauri::command]
pub fn mobile_takeover_force_release(
    input: ForceReleaseInput,
    state: State<'_, AppState>,
) -> Result<(), TauriMobileError> {
    if let Some(term_id) = state.pty.claude_terminal_for_session(input.session_id) {
        state.pty.release_takeover(&term_id);
    }
    Ok(())
}
