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
