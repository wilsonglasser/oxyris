//! Persisted app-wide default supervisor config for the auto-pilot.
//!
//! The interactive thread popover passes a full config into `autopilot_engage`,
//! but an **MCP-driven** engage — Claude handing the wheel to the pilot via the
//! `oxyris_autopilot_engage` tool — originates in the backend with no frontend
//! in the loop, so it has nothing to read. This module persists the app-wide
//! default (mirrored from the Settings UI) to `<data_dir>/autopilot-defaults.json`
//! so [`crate::infra::autopilot_bridge`] can build a [`SupervisorConfig`] on its
//! own. Mirror, not source of truth: the frontend `appSettingsStore` still owns
//! the UI; this is the copy the bridge consults.

use std::path::{Path, PathBuf};

use oxyris_supervisor::SupervisorKind;
use serde::{Deserialize, Serialize};

use crate::infra::autopilot::{SupervisorConfig, config_from_parts};

/// Mirrors the frontend `AutopilotSettings` plus the default supervisor kind.
/// Field names are snake_case to match the Tauri IPC payload from the web app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutopilotDefaults {
    /// Which backend to engage with when the caller doesn't pick one.
    #[serde(default)]
    pub supervisor: SupervisorKind,
    /// Model id for the multi-model (OpenAI-compatible) supervisor.
    #[serde(default)]
    pub model: String,
    /// OpenAI-compatible base URL for the multi-model supervisor.
    #[serde(default)]
    pub base_url: String,
    /// Bearer key for the multi-model supervisor.
    #[serde(default)]
    pub api_key: String,
    /// Model id for the Claude-CLI supervisor (blank = account default).
    #[serde(default)]
    pub claude_model: String,
    /// Turn budget; `None` = unlimited.
    #[serde(default = "default_max_turns")]
    pub max_turns: Option<u32>,
}

fn default_max_turns() -> Option<u32> {
    Some(30)
}

impl Default for AutopilotDefaults {
    fn default() -> Self {
        Self {
            supervisor: SupervisorKind::MultiModel,
            model: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            claude_model: String::new(),
            max_turns: default_max_turns(),
        }
    }
}

impl AutopilotDefaults {
    /// Build the [`SupervisorConfig`] for an engage, picking the right model
    /// field for the chosen backend. Returns the config + turn budget. Errors
    /// (e.g. multi-model with no base URL configured) bubble up so the MCP tool
    /// call surfaces "configure the supervisor in Settings first" rather than
    /// silently engaging a broken pilot.
    pub fn to_config(&self) -> Result<(SupervisorConfig, Option<u32>), String> {
        let model = match self.supervisor {
            SupervisorKind::MultiModel => self.model.clone(),
            SupervisorKind::Claude => self.claude_model.clone(),
        };
        let config = config_from_parts(
            self.supervisor,
            Some(model),
            Some(self.base_url.clone()),
            Some(self.api_key.clone()),
        )?;
        Ok((config, self.max_turns))
    }
}

fn path_in(data_dir: &Path) -> PathBuf {
    data_dir.join("autopilot-defaults.json")
}

/// Load the persisted defaults, falling back to [`AutopilotDefaults::default`]
/// when the file is missing or corrupt — a fresh install just runs with empties
/// until the user configures them.
pub fn load(data_dir: &Path) -> AutopilotDefaults {
    match std::fs::read(path_in(data_dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => AutopilotDefaults::default(),
    }
}

/// Persist the defaults as pretty JSON next to the rest of the app data.
pub fn save(data_dir: &Path, defaults: &AutopilotDefaults) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(defaults)?;
    std::fs::write(path_in(data_dir), bytes)
}
