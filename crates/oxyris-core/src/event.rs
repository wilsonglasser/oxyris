use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::AggregateId;

/// A persisted event row — the durable record of one step in an aggregate's
/// history. `seq` is assigned by the store on insert; it's `None` for an event
/// that has been decided but not yet appended.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    pub aggregate: String,
    pub aggregate_id: AggregateId,
    pub version: u32,
    pub kind: String,
    pub payload: serde_json::Value,
    pub timestamp: DateTime<Utc>,
}
