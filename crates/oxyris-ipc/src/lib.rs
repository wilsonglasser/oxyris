//! NDJSON protocol shared between the desktop backend and the WSL agent.
//!
//! Every frame is one JSON object per line. Frames have a `kind` discriminator:
//!
//! - `request` — backend → agent. Has an `id` the agent must echo back in
//!   every response frame, an `op` string identifying the operation, and an
//!   `args` object whose shape depends on the op.
//! - `event` — agent → backend. Streams intermediate results for a long-
//!   running op (`fs.walk` is the canonical example). Terminated by a
//!   `result` or `error` frame with the same `request_id`.
//! - `result` — agent → backend. Final success frame; payload is op-specific.
//! - `error` — agent → backend. Final failure frame.
//!
//! See `PLAN.md` §5 for the full vocabulary and rationale.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub mod ops;

/// Every frame that can travel across the stdio channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Frame {
    Request(RequestFrame),
    Event(EventFrame),
    Result(ResultFrame),
    Error(ErrorFrame),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestFrame {
    pub id: String,
    pub op: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFrame {
    pub request_id: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultFrame {
    pub request_id: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorFrame {
    pub request_id: String,
    pub code: String,
    pub message: String,
}
