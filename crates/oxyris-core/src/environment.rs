use serde::{Deserialize, Serialize};

/// Where a project lives. Routing is absolute: `Windows` projects use native
/// Windows ops, `Wsl` projects use the per-distro agent. No cross-fallback.
/// (See `PLAN.md` §13.)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Environment {
    Windows,
    Wsl { distro: String },
}

impl Environment {
    pub fn is_wsl(&self) -> bool {
        matches!(self, Environment::Wsl { .. })
    }
}
