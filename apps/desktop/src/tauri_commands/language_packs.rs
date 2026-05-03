//! Tauri IPC for the language-packs registry. Frontend uses these to
//! render the Settings → Languages tab and drive install/uninstall.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::app_state::AppState;
use crate::infra::language_packs::{PackError, PackRow};

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriPackError {
    #[error("unknown language pack: {0}")]
    Unknown(String),
    #[error("install failed: {0}")]
    Install(String),
    #[error("io: {0}")]
    Io(String),
}

impl From<PackError> for TauriPackError {
    fn from(e: PackError) -> Self {
        match e {
            PackError::Unknown(id) => TauriPackError::Unknown(id),
            PackError::Io(e) => TauriPackError::Io(e.to_string()),
            other => TauriPackError::Install(other.to_string()),
        }
    }
}

#[tauri::command]
pub async fn language_packs_list(
    state: State<'_, AppState>,
) -> Result<Vec<PackRow>, TauriPackError> {
    Ok(state.language_packs.list().await)
}

#[derive(Debug, Deserialize)]
pub struct InstallInput {
    pub id: String,
}

#[tauri::command]
pub async fn language_packs_install(
    input: InstallInput,
    state: State<'_, AppState>,
) -> Result<(), TauriPackError> {
    state.language_packs.install(&input.id).await?;
    Ok(())
}

#[tauri::command]
pub async fn language_packs_uninstall(
    input: InstallInput,
    state: State<'_, AppState>,
) -> Result<(), TauriPackError> {
    state.language_packs.uninstall(&input.id).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct WslInstallInput {
    pub id: String,
    pub distro: String,
}

/// Install a language pack inside a WSL distro by spawning `wsl.exe -d
/// <distro> -- bash -lc '<one-liner>'`. Blocks until the install finishes
/// (or times out) and returns the resulting binary path on success.
#[tauri::command]
pub async fn language_packs_install_in_wsl(
    input: WslInstallInput,
    state: State<'_, AppState>,
) -> Result<String, TauriPackError> {
    let path = state
        .language_packs
        .install_in_wsl(&input.distro, &input.id)
        .await?;
    Ok(path)
}

/// List the WSL distros configured on this machine. Returns the bare
/// distro names (`Ubuntu-22.04`, `Debian`, `Alpine`, ...). Empty when
/// WSL is not installed or no distros are registered.
#[tauri::command]
pub async fn wsl_distros() -> Result<Vec<String>, TauriPackError> {
    use std::process::Command;
    let out = match Command::new("wsl.exe")
        .arg("--list")
        .arg("--quiet")
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Ok(Vec::new()),
    };
    let text = crate::infra::decode_wsl_output_for_command(&out.stdout);
    let distros = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|s| s.to_owned())
        .collect();
    Ok(distros)
}
