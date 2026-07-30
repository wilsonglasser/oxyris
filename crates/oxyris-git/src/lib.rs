//! Pure git2-based operations shared between the desktop Windows backend and
//! the WSL agent. Neither platform-specific IO nor IPC — callers wrap these
//! in whatever transport they need.
//!
//! The crate is deliberately `no_std`-adjacent in spirit: types serialize over
//! NDJSON so they match 1:1 between both sides without bespoke conversion.

#![forbid(unsafe_code)]

pub mod branch;
pub mod checkpoint;
pub mod cherry;
pub mod conflict;
pub mod dotenv;
pub mod error;
pub mod log;
pub mod merge;
pub mod remote;
pub mod stash;
pub mod status;
pub mod tag;
pub mod types;
pub mod worktree;

pub use branch::BranchDetail;
pub use conflict::ConflictContents;
pub use error::GitError;
pub use log::CommitInfo;
pub use merge::{MergeOutcome, RebaseOutcome};
pub use remote::RemoteOpResult;
pub use stash::StashEntry;
pub use tag::TagInfo;
pub use types::{
    AheadBehind, BranchInfo, CommitResult, DiffMode, FileDiff, FileStatus, RepoState, StatusBucket,
    StatusEntry, StatusReport, TurnDiff, WorktreeRef,
};
