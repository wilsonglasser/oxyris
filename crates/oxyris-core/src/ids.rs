use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Strongly typed aggregate identifier.
///
/// We use UUID v7 throughout so IDs are time-ordered, which makes event-store
/// scans and projection rebuilds cheaper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AggregateId(pub Uuid);

impl AggregateId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for AggregateId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AggregateId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}
