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
    pub const FS_LIST_DIR: &str = "fs.list_dir";
    pub const FS_CREATE_FILE: &str = "fs.create_file";
    pub const FS_CREATE_DIR: &str = "fs.create_dir";
    pub const FS_RENAME: &str = "fs.rename";
    pub const FS_DELETE: &str = "fs.delete";
    pub const FS_READ_BYTES: &str = "fs.read_bytes";
    pub const FS_WRITE_BYTES: &str = "fs.write_bytes";
    pub const FS_SEARCH_PATHS: &str = "fs.search_paths";
    pub const GIT_LIST_BRANCHES: &str = "git.list_branches";
    pub const GIT_LIST_WORKTREES: &str = "git.list_worktrees";
    pub const GIT_CREATE_WORKTREE: &str = "git.create_worktree";
    pub const GIT_REMOVE_WORKTREE: &str = "git.remove_worktree";
    pub const GIT_CHECKPOINT_CAPTURE: &str = "git.checkpoint_capture";
    pub const GIT_CHECKPOINT_DIFF: &str = "git.checkpoint_diff";
    pub const GIT_CHECKPOINT_REVERT: &str = "git.checkpoint_revert";
    pub const GIT_STATUS: &str = "git.status";
    pub const GIT_DIFF_FILE: &str = "git.diff_file";
    pub const GIT_STAGE: &str = "git.stage";
    pub const GIT_UNSTAGE: &str = "git.unstage";
    pub const GIT_COMMIT: &str = "git.commit";
    pub const GIT_CLONE: &str = "git.clone";
    pub const GIT_FETCH: &str = "git.fetch";
    pub const GIT_PULL: &str = "git.pull";
    pub const GIT_PUSH: &str = "git.push";
    pub const GIT_CHECKOUT: &str = "git.checkout";
    pub const GIT_BRANCH_CREATE: &str = "git.branch_create";
    pub const GIT_BRANCH_DELETE: &str = "git.branch_delete";
    pub const GIT_LOG: &str = "git.log";
    pub const GIT_GET_CONFLICT: &str = "git.get_conflict";
    pub const GIT_RESOLVE: &str = "git.resolve";
    pub const GIT_APPLY_PATCH: &str = "git.apply_patch";
    pub const GIT_STASH_LIST: &str = "git.stash_list";
    pub const GIT_STASH_SAVE: &str = "git.stash_save";
    pub const GIT_STASH_APPLY: &str = "git.stash_apply";
    pub const GIT_STASH_DROP: &str = "git.stash_drop";
    pub const GIT_TAG_LIST: &str = "git.tag_list";
    pub const GIT_TAG_CREATE: &str = "git.tag_create";
    pub const GIT_TAG_DELETE: &str = "git.tag_delete";
    pub const GIT_CHERRY_PICK: &str = "git.cherry_pick";
    pub const GIT_REVERT: &str = "git.revert";
    pub const GIT_DIFF_REVS: &str = "git.diff_revs";
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

// ────── fs.write_bytes ─────────────────────────────────────────────────────

/// Binary write. Used for attachments (pasted/dropped images) that must land
/// inside the distro so a WSL-hosted `claude` can resolve the `@path` ref.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsWriteBytesArgs {
    pub path: String,
    /// Base64-encoded file bytes (no data-URL prefix). Reuses `FsWriteResult`.
    pub bytes_b64: String,
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

// ────── fs.list_dir ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsListDirArgs {
    pub path: String,
    /// When false, hidden entries (leading-dot names on POSIX, hidden-attr on
    /// Windows) are omitted. Defaults to false.
    #[serde(default)]
    pub show_hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsListDirEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: Option<u64>,
    pub modified_secs: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsListDirResult {
    pub path: String,
    pub entries: Vec<FsListDirEntry>,
}

// ────── fs.create_file / create_dir / rename / delete ─────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsPathArgs {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsCreateFileArgs {
    pub path: String,
    /// Initial contents; empty by default. Useful for templates.
    #[serde(default)]
    pub contents: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsRenameArgs {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsDeleteArgs {
    pub path: String,
    /// When true, recursively delete a directory. Required for non-empty
    /// dirs; ignored for files.
    #[serde(default)]
    pub recursive: bool,
}

// ────── fs.read_bytes (binary preview) ─────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsReadBytesArgs {
    pub path: String,
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsReadBytesResult {
    pub path: String,
    /// Base64-encoded contents.
    pub bytes_b64: String,
    pub bytes_read: u64,
    pub truncated: bool,
}

// ────── fs.search_paths (Ctrl+P quick open) ────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsSearchPathsArgs {
    pub root: String,
    pub query: String,
    #[serde(default = "default_search_limit")]
    pub limit: u32,
}

fn default_search_limit() -> u32 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsSearchHit {
    /// Path relative to `root`.
    pub rel_path: String,
    /// Match score (lower = better; bare substring index of last matched
    /// character in the basename — simple but effective).
    pub score: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsSearchPathsResult {
    pub hits: Vec<FsSearchHit>,
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

// ────── git.status ─────────────────────────────────────────────────────────

// Result type re-uses `oxyris_git::StatusReport` via a `Value` since this
// crate is git2-free; the agent serializes it directly and the desktop
// deserializes back into the typed shape.

// ────── git.diff_file ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiffFileArgs {
    pub repo_path: String,
    pub path: String,
    /// "working_vs_head" | "staged_vs_head" | "working_vs_staged"
    pub mode: String,
}

// ────── git.stage / git.unstage ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitPathsArgs {
    pub repo_path: String,
    pub paths: Vec<String>,
}

// ────── git.commit ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommitArgs {
    pub repo_path: String,
    pub message: String,
    #[serde(default)]
    pub amend: bool,
}

// ────── git.clone ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCloneArgs {
    pub url: String,
    /// Absolute target dir (inside the distro for WSL). Result reuses
    /// `RemoteOpResult` from `oxyris-git`.
    pub target_dir: String,
}

// ────── git.fetch / pull / push ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFetchArgs {
    pub repo_path: String,
    #[serde(default)]
    pub remote: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitPullArgs {
    pub repo_path: String,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub rebase: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitPushArgs {
    pub repo_path: String,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub set_upstream: bool,
}

// ────── git.checkout / branch_create / branch_delete ───────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCheckoutArgs {
    pub repo_path: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitBranchCreateArgs {
    pub repo_path: String,
    pub name: String,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub checkout: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitBranchDeleteArgs {
    pub repo_path: String,
    pub name: String,
}

// ────── git.log ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLogArgs {
    pub repo_path: String,
    #[serde(default = "default_log_limit")]
    pub limit: u32,
    #[serde(default)]
    pub rev: Option<String>,
}

fn default_log_limit() -> u32 {
    50
}

// ────── git.get_conflict / resolve ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConflictPathArgs {
    pub repo_path: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitResolveArgs {
    pub repo_path: String,
    pub path: String,
    pub content: String,
}

// ────── git.apply_patch (hunk-level stage / unstage) ───────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitApplyPatchArgs {
    pub repo_path: String,
    pub patch: String,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default)]
    pub cached: bool,
}

// ────── git.stash_* ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStashSaveArgs {
    pub repo_path: String,
    pub message: String,
    #[serde(default)]
    pub include_untracked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStashApplyArgs {
    pub repo_path: String,
    pub index: u32,
    #[serde(default)]
    pub drop_after: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStashIndexArgs {
    pub repo_path: String,
    pub index: u32,
}

// ────── git.tag_* ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitTagCreateArgs {
    pub repo_path: String,
    pub name: String,
    /// Revision (commit SHA, branch name, or `None` for HEAD).
    #[serde(default)]
    pub target: Option<String>,
    /// Annotation message; lightweight tag when empty / missing.
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitTagNameArgs {
    pub repo_path: String,
    pub name: String,
}

// ────── git.cherry_pick / revert ───────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommitOidArgs {
    pub repo_path: String,
    pub oid: String,
}

// ────── git.diff_revs ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiffRevsArgs {
    pub repo_path: String,
    /// Source revision. Pass `"WORKTREE"` for the working tree + index.
    pub from: String,
    /// Destination revision. Pass `"WORKTREE"` for the working tree + index.
    pub to: String,
    #[serde(default = "default_true")]
    pub find_renames: bool,
}

fn default_true() -> bool {
    true
}
