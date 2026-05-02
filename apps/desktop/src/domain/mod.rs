//! Event-sourced aggregates.
//!
//! Each aggregate is a pure data+logic module: `State`, `Command`, `Event`,
//! plus `decide` (command → events) and `apply` (event → next state). None of
//! it knows about SQLite, Tauri, or the filesystem — the infra layer wires
//! those in.

pub mod action;
pub mod project;
pub mod session;
pub mod worktree;
