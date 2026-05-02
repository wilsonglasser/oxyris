//! Typed op contracts. Each op has a constant op name, a request args type,
//! and a result type. The agent handles by matching on the op string; the
//! backend calls typed helpers that deserialize the untyped `data` payload.

use serde::{Deserialize, Serialize};

/// Reusable binding for "op" strings so both ends stay in sync on the wire.
pub mod op_name {
    pub const SYSTEM_INFO: &str = "system.info";
    pub const FS_STAT: &str = "fs.stat";
    pub const FS_READ: &str = "fs.read";
    pub const FS_WRITE: &str = "fs.write";
    pub const FS_WALK: &str = "fs.walk";
    pub const GIT_LIST_BRANCHES: &str = "git.list_branches";
    pub const GIT_LIST_WORKTREES: &str = "git.list_worktrees";
    pub const GIT_CREATE_WORKTREE: &str = "git.create_worktree";
    pub const GIT_REMOVE_WORKTREE: &str = "git.remove_worktree";
    pub const GIT_CHECKPOINT_CAPTURE: &str = "git.checkpoint_capture";
    pub const GIT_CHECKPOINT_DIFF: &str = "git.checkpoint_diff";
    pub const GIT_CHECKPOINT_REVERT: &str = "git.checkpoint_revert";
}

// ────── system.info ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemInfoArgs {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfoResult {
    pub agent_version: String,
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub hostname: String,
    pub cwd: String,
    pub home: String,
    pub user: String,
}

// ────── fs.stat ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsStatArgs {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsStatResult {
    pub path: String,
    pub exists: bool,
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    pub size: Option<u64>,
    /// Last-modified time as seconds since the Unix epoch. `None` when the
    /// file doesn't exist or the platform doesn't expose it.
    #[serde(default)]
    pub modified_secs: Option<i64>,
}

// ────── fs.read ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsReadArgs {
    pub path: String,
    /// Optional cap on bytes to read; `None` = entire file. A small limit
    /// keeps the agent from OOM-ing on giant binaries the UI accidentally
    /// pointed at.
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsReadResult {
    pub path: String,
    pub content: String,
    pub bytes_read: u64,
    pub truncated: bool,
}

// ────── fs.write ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsWriteArgs {
    pub path: String,
    /// File contents as a UTF-8 string. Binary writes go through a future
    /// `fs.write_base64` op once we need them.
    pub contents: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsWriteResult {
    pub path: String,
    pub bytes_written: u64,
}

// ────── fs.walk (streaming) ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsWalkArgs {
    pub root: String,
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Cap on emitted entries — a safety belt against walking huge trees.
    #[serde(default)]
    pub max_entries: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsWalkEvent {
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsWalkResult {
    pub count: u32,
    pub truncated: bool,
}

// ────── git.* ──────────────────────────────────────────────────────────────
//
// Every git op carries `repo_path` (absolute, inside the distro). Result
// types are re-exported from `oxyris-git` via `serde_json::Value` to keep
// this crate free of the git2 dependency. See `oxyris-git::types` for the
// shapes.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRepoPathArgs {
    pub repo_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCreateWorktreeArgs {
    pub repo_path: String,
    pub name: String,
    pub branch: String,
    pub target_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRemoveWorktreeArgs {
    pub repo_path: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCheckpointCaptureArgs {
    pub repo_path: String,
    pub session_id: String,
    pub turn_id: String,
    /// "pre" or "post" — matches `oxyris_git::types::CheckpointPhase` serde.
    pub phase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCheckpointCaptureResult {
    /// `None` when there was nothing to checkpoint (empty repo, uninit).
    pub ref_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCheckpointTurnArgs {
    pub repo_path: String,
    pub session_id: String,
    pub turn_id: String,
}
