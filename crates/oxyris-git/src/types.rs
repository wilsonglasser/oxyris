use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CheckpointPhase {
    Pre,
    Post,
}

impl CheckpointPhase {
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Pre => "pre",
            Self::Post => "post",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeRef {
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Typechange,
    Unchanged,
}

impl From<git2::Delta> for FileStatus {
    fn from(d: git2::Delta) -> Self {
        match d {
            git2::Delta::Added => FileStatus::Added,
            git2::Delta::Modified => FileStatus::Modified,
            git2::Delta::Deleted => FileStatus::Deleted,
            git2::Delta::Renamed => FileStatus::Renamed,
            git2::Delta::Copied => FileStatus::Copied,
            git2::Delta::Typechange => FileStatus::Typechange,
            _ => FileStatus::Unchanged,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub old_content: Option<String>,
    pub new_content: Option<String>,
    /// Rendered unified diff (hunks) for clients that don't want to
    /// compute their own line matching.
    pub unified: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnDiff {
    pub files: Vec<FileDiff>,
}

// ────── working-tree status ────────────────────────────────────────────────

/// Where the change lives. A single file can appear under both `staged` and
/// `unstaged` if the index and working tree both differ from HEAD (the
/// classic "staged + then edited again" case).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusBucket {
    Staged,
    Unstaged,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusEntry {
    pub path: String,
    /// Set when the change is a rename (`status == Renamed`). Present in
    /// staged renames; the unstaged side reports as a delete + add unless
    /// `status.options().renames(true)` is set, which we do.
    pub old_path: Option<String>,
    pub bucket: StatusBucket,
    pub status: FileStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatusReport {
    pub entries: Vec<StatusEntry>,
    /// Current branch shorthand or `None` for detached HEAD / empty repo.
    pub branch: Option<String>,
    /// Commits ahead/behind the upstream of `branch`. `None` when there is
    /// no upstream tracking branch.
    pub ahead_behind: Option<AheadBehind>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AheadBehind {
    pub ahead: u32,
    pub behind: u32,
}

/// Which two endpoints to diff for a single file.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiffMode {
    /// Working tree vs HEAD (the "all uncommitted" view).
    WorkingVsHead,
    /// Index vs HEAD (just the staged delta).
    StagedVsHead,
    /// Working tree vs index (just the unstaged delta).
    WorkingVsStaged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitResult {
    pub oid: String,
    pub message: String,
    pub branch: Option<String>,
}
