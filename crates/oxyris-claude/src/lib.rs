//! Claude CLI adapter.
//!
//! Wraps `claude --print --output-format stream-json --input-format stream-json`
//! (or its WSL-hosted sibling) and exposes it as an implementation of
//! [`oxyris_provider::Provider`]. Messages are framed as NDJSON over stdio.
//!
//! The parser is defensive: unknown event types fall through as `Unknown`
//! so a minor CLI bump doesn't brick the app.

#![forbid(unsafe_code)]

mod protocol;
mod provider;

pub use protocol::{StreamEvent, parse_stream_line};
pub use provider::ClaudeProvider;
