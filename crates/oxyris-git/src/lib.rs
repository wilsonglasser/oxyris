//! Pure git2-based operations shared between the desktop Windows backend and
//! the WSL agent. Neither platform-specific IO nor IPC — callers wrap these
//! in whatever transport they need.
//!
//! The crate is deliberately `no_std`-adjacent in spirit: types serialize over
//! NDJSON so they match 1:1 between both sides without bespoke conversion.

#![forbid(unsafe_code)]

pub mod checkpoint;
pub mod dotenv;
pub mod error;
pub mod types;
pub mod worktree;

pub use error::GitError;
pub use types::{BranchInfo, FileDiff, FileStatus, TurnDiff, WorktreeRef};
