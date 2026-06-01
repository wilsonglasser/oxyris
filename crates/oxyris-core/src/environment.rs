use serde::{Deserialize, Serialize};

/// Where a project lives. Routing is absolute: `Local` projects use native
/// host ops (Windows, macOS, or Linux — whatever the desktop app runs on),
/// `Wsl` projects use the per-distro agent. No cross-fallback. `Wsl` only ever
/// exists on a Windows host. (See `PLAN.md` §13.)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Environment {
    /// Host-native execution. Path separator and shell follow the host OS at
    /// compile time, never the variant. `alias = "windows"` keeps event logs
    /// written before the Mac/Linux port (when this variant was `Windows`)
    /// deserializable.
    #[serde(alias = "windows")]
    Local,
    Wsl {
        distro: String,
    },
}

impl Environment {
    pub fn is_wsl(&self) -> bool {
        matches!(self, Environment::Wsl { .. })
    }
}
