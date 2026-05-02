//! Settings + provider-discovery surface for the UI.

use tauri::State;

use crate::app_state::AppState;
use crate::infra::environments::environments_list;
use crate::infra::provider_discovery::{DiscoveredInstall, discover_claude};

#[tauri::command]
pub async fn settings_provider_discover() -> Vec<DiscoveredInstall> {
    let envs = environments_list();
    let mut out = Vec::with_capacity(envs.len());
    for entry in envs {
        out.push(discover_claude(entry.environment).await);
    }
    out
}

#[tauri::command]
pub fn settings_logs_dir(state: State<'_, AppState>) -> String {
    state.logs_dir.display().to_string()
}

#[tauri::command]
pub fn settings_keybindings_path(state: State<'_, AppState>) -> String {
    state
        .data_dir
        .join("keybindings.json")
        .display()
        .to_string()
}

const DEFAULT_KEYBINDINGS: &str = "{\n  \"$schema\": \"https://oxyris.dev/keybindings.schema.json\",\n  \"new_thread\": \"Ctrl+Shift+N\",\n  \"interrupt\": \"Escape\",\n  \"toggle_terminal\": \"Ctrl+`\",\n  \"focus_search\": \"Ctrl+K\"\n}\n";

#[tauri::command]
pub fn settings_keybindings_read(state: State<'_, AppState>) -> Result<String, String> {
    let path = state.data_dir.join("keybindings.json");
    if !path.exists() {
        return Ok(DEFAULT_KEYBINDINGS.to_owned());
    }
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn settings_keybindings_write(
    state: State<'_, AppState>,
    contents: String,
) -> Result<(), String> {
    // Validate JSON shape before persisting so we don't write garbage that
    // breaks the next boot.
    serde_json::from_str::<serde_json::Value>(&contents)
        .map_err(|e| format!("invalid JSON: {e}"))?;
    let path = state.data_dir.join("keybindings.json");
    std::fs::write(&path, contents).map_err(|e| e.to_string())
}
