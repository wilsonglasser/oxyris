//! Tauri IPC handlers.
//!
//! Each handler is a thin shim that decodes input, calls into the domain /
//! infra services, and returns a serializable result. Domain logic lives in
//! `crate::domain`, infrastructure in `crate::infra`.

pub mod action;
pub mod attachments;
pub mod badge;
pub mod env;
pub mod environment;
pub mod fs;
pub mod git;
pub mod indexing;
pub mod language_packs;
pub mod project;
pub mod session;
pub mod settings;
pub mod terminal;
pub mod validate;
pub mod worktree;

/// Sprint 1 sanity check — proves the React → Rust roundtrip works.
#[tauri::command]
pub fn greet(name: &str) -> String {
    format!(
        "Hello, {name}! — Oxyris backend v{}",
        env!("CARGO_PKG_VERSION")
    )
}
