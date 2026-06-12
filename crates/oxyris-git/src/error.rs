use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("git2: {0}")]
    Git2(#[from] git2::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("git returned non-zero: {0}")]
    NonZero(String),
    #[error("not a repository: {0}")]
    NotARepo(String),
    #[error("checkpoint ref missing: {0}")]
    RefMissing(String),
    #[error("utf8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    /// Repo exists but HEAD points at a branch with no commit yet (`git init`
    /// state). Worktrees can't be branched off nothing — the caller needs to
    /// commit first.
    #[error("repository has no commits yet")]
    EmptyRepo,
    /// A user-supplied positional argument (url / remote / branch) began with
    /// `-`, so git would parse it as an option. Rejected to block option
    /// injection (`--upload-pack=...`, `--exec=...`).
    #[error("rejected argument (looks like an option): {0}")]
    RejectedArg(String),
}
