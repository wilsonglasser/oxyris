//! Git facade for the desktop backend.
//!
//! Windows projects: call `oxyris-git` directly (git2 in-process).
//! WSL projects: dispatch to the per-distro agent over NDJSON; the agent
//! runs the same `oxyris-git` ops natively inside the distro so we never
//! shell out across the 9p bridge.

use oxyris_core::Environment;
pub use oxyris_git::{
    BranchInfo, CommitInfo, CommitResult, ConflictContents, DiffMode, FileDiff, RemoteOpResult,
    StatusReport, WorktreeRef,
};
use oxyris_git::{
    GitError as InnerGitError, branch as git_branch, conflict as git_conflict, log as git_log,
    remote as git_remote, status as git_status, worktree as wt,
};
use oxyris_ipc::ops::{
    GitBranchCreateArgs, GitBranchDeleteArgs, GitCheckoutArgs, GitCommitArgs, GitConflictPathArgs,
    GitCreateWorktreeArgs, GitDiffFileArgs, GitFetchArgs, GitLogArgs, GitPathsArgs, GitPullArgs,
    GitPushArgs, GitRemoveWorktreeArgs, GitRepoPathArgs, GitResolveArgs, op_name,
};
use thiserror::Error;

use crate::infra::agent_pool::{AgentError, AgentPool};

#[derive(Debug, Error)]
pub enum GitError {
    #[error("git: {0}")]
    Git(String),
    #[error("agent: {0}")]
    Agent(String),
    #[error("repository has no commits yet")]
    EmptyRepo,
}

impl From<InnerGitError> for GitError {
    fn from(e: InnerGitError) -> Self {
        match e {
            InnerGitError::EmptyRepo => GitError::EmptyRepo,
            other => GitError::Git(other.to_string()),
        }
    }
}

impl From<AgentError> for GitError {
    fn from(e: AgentError) -> Self {
        if let AgentError::Remote { code, .. } = &e
            && code == "empty_repo"
        {
            return GitError::EmptyRepo;
        }
        GitError::Agent(e.to_string())
    }
}

pub async fn list_branches(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
) -> Result<Vec<BranchInfo>, GitError> {
    match env {
        Environment::Windows => {
            let path = repo_path.to_owned();
            tokio::task::spawn_blocking(move || wt::list_branches(&path))
                .await
                .map_err(|e| GitError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::GIT_LIST_BRANCHES,
                    serde_json::to_value(GitRepoPathArgs {
                        repo_path: repo_path.to_owned(),
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| GitError::Agent(e.to_string()))
        }
    }
}

pub async fn list_worktrees(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
) -> Result<Vec<WorktreeRef>, GitError> {
    match env {
        Environment::Windows => {
            let path = repo_path.to_owned();
            tokio::task::spawn_blocking(move || wt::list_worktrees(&path))
                .await
                .map_err(|e| GitError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::GIT_LIST_WORKTREES,
                    serde_json::to_value(GitRepoPathArgs {
                        repo_path: repo_path.to_owned(),
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| GitError::Agent(e.to_string()))
        }
    }
}

pub async fn create_worktree(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    name: &str,
    branch: &str,
    target_dir: &str,
    _create_branch: bool,
) -> Result<WorktreeRef, GitError> {
    match env {
        Environment::Windows => {
            let path = repo_path.to_owned();
            let name_owned = name.to_owned();
            let branch_owned = branch.to_owned();
            let target = target_dir.to_owned();
            tokio::task::spawn_blocking(move || {
                wt::create_worktree(&path, &name_owned, &branch_owned, &target)
            })
            .await
            .map_err(|e| GitError::Git(format!("join: {e}")))?
            .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::GIT_CREATE_WORKTREE,
                    serde_json::to_value(GitCreateWorktreeArgs {
                        repo_path: repo_path.to_owned(),
                        name: name.to_owned(),
                        branch: branch.to_owned(),
                        target_dir: target_dir.to_owned(),
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| GitError::Agent(e.to_string()))
        }
    }
}

pub async fn status(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
) -> Result<StatusReport, GitError> {
    match env {
        Environment::Windows => {
            let path = repo_path.to_owned();
            tokio::task::spawn_blocking(move || git_status::status(&path))
                .await
                .map_err(|e| GitError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::GIT_STATUS,
                    serde_json::to_value(GitRepoPathArgs {
                        repo_path: repo_path.to_owned(),
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| GitError::Agent(e.to_string()))
        }
    }
}

pub async fn diff_file(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    path: &str,
    mode: DiffMode,
) -> Result<FileDiff, GitError> {
    match env {
        Environment::Windows => {
            let repo = repo_path.to_owned();
            let p = path.to_owned();
            tokio::task::spawn_blocking(move || git_status::diff_file(&repo, &p, mode))
                .await
                .map_err(|e| GitError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::GIT_DIFF_FILE,
                    serde_json::to_value(GitDiffFileArgs {
                        repo_path: repo_path.to_owned(),
                        path: path.to_owned(),
                        mode: serialize_mode(mode).to_owned(),
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| GitError::Agent(e.to_string()))
        }
    }
}

pub async fn stage(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    paths: Vec<String>,
) -> Result<(), GitError> {
    match env {
        Environment::Windows => {
            let repo = repo_path.to_owned();
            tokio::task::spawn_blocking(move || git_status::stage(&repo, &paths))
                .await
                .map_err(|e| GitError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            agent_pool
                .call(
                    distro,
                    op_name::GIT_STAGE,
                    serde_json::to_value(GitPathsArgs {
                        repo_path: repo_path.to_owned(),
                        paths,
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            Ok(())
        }
    }
}

pub async fn unstage(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    paths: Vec<String>,
) -> Result<(), GitError> {
    match env {
        Environment::Windows => {
            let repo = repo_path.to_owned();
            tokio::task::spawn_blocking(move || git_status::unstage(&repo, &paths))
                .await
                .map_err(|e| GitError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            agent_pool
                .call(
                    distro,
                    op_name::GIT_UNSTAGE,
                    serde_json::to_value(GitPathsArgs {
                        repo_path: repo_path.to_owned(),
                        paths,
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            Ok(())
        }
    }
}

pub async fn commit(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    message: String,
    amend: bool,
) -> Result<CommitResult, GitError> {
    match env {
        Environment::Windows => {
            let repo = repo_path.to_owned();
            tokio::task::spawn_blocking(move || git_status::commit(&repo, &message, amend))
                .await
                .map_err(|e| GitError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::GIT_COMMIT,
                    serde_json::to_value(GitCommitArgs {
                        repo_path: repo_path.to_owned(),
                        message,
                        amend,
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| GitError::Agent(e.to_string()))
        }
    }
}

fn serialize_mode(mode: DiffMode) -> &'static str {
    match mode {
        DiffMode::WorkingVsHead => "working_vs_head",
        DiffMode::StagedVsHead => "staged_vs_head",
        DiffMode::WorkingVsStaged => "working_vs_staged",
    }
}

pub async fn fetch(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    remote: Option<String>,
) -> Result<RemoteOpResult, GitError> {
    match env {
        Environment::Windows => {
            let repo = repo_path.to_owned();
            tokio::task::spawn_blocking(move || git_remote::fetch(&repo, remote.as_deref()))
                .await
                .map_err(|e| GitError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::GIT_FETCH,
                    serde_json::to_value(GitFetchArgs {
                        repo_path: repo_path.to_owned(),
                        remote,
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| GitError::Agent(e.to_string()))
        }
    }
}

pub async fn pull(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    remote: Option<String>,
    branch: Option<String>,
    rebase: bool,
) -> Result<RemoteOpResult, GitError> {
    match env {
        Environment::Windows => {
            let repo = repo_path.to_owned();
            let r = remote.clone();
            let b = branch.clone();
            tokio::task::spawn_blocking(move || {
                git_remote::pull(&repo, r.as_deref(), b.as_deref(), rebase)
            })
            .await
            .map_err(|e| GitError::Git(format!("join: {e}")))?
            .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::GIT_PULL,
                    serde_json::to_value(GitPullArgs {
                        repo_path: repo_path.to_owned(),
                        remote,
                        branch,
                        rebase,
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| GitError::Agent(e.to_string()))
        }
    }
}

pub async fn push(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    remote: Option<String>,
    branch: Option<String>,
    force: bool,
    set_upstream: bool,
) -> Result<RemoteOpResult, GitError> {
    match env {
        Environment::Windows => {
            let repo = repo_path.to_owned();
            let r = remote.clone();
            let b = branch.clone();
            tokio::task::spawn_blocking(move || {
                git_remote::push(&repo, r.as_deref(), b.as_deref(), force, set_upstream)
            })
            .await
            .map_err(|e| GitError::Git(format!("join: {e}")))?
            .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::GIT_PUSH,
                    serde_json::to_value(GitPushArgs {
                        repo_path: repo_path.to_owned(),
                        remote,
                        branch,
                        force,
                        set_upstream,
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| GitError::Agent(e.to_string()))
        }
    }
}

pub async fn checkout(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    name: String,
) -> Result<(), GitError> {
    match env {
        Environment::Windows => {
            let repo = repo_path.to_owned();
            tokio::task::spawn_blocking(move || git_branch::checkout(&repo, &name))
                .await
                .map_err(|e| GitError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            agent_pool
                .call(
                    distro,
                    op_name::GIT_CHECKOUT,
                    serde_json::to_value(GitCheckoutArgs {
                        repo_path: repo_path.to_owned(),
                        name,
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            Ok(())
        }
    }
}

pub async fn branch_create(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    name: String,
    from: Option<String>,
    checkout_after: bool,
) -> Result<(), GitError> {
    match env {
        Environment::Windows => {
            let repo = repo_path.to_owned();
            let n = name.clone();
            let f = from.clone();
            tokio::task::spawn_blocking(move || {
                git_branch::create_branch(&repo, &n, f.as_deref(), checkout_after)
            })
            .await
            .map_err(|e| GitError::Git(format!("join: {e}")))?
            .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            agent_pool
                .call(
                    distro,
                    op_name::GIT_BRANCH_CREATE,
                    serde_json::to_value(GitBranchCreateArgs {
                        repo_path: repo_path.to_owned(),
                        name,
                        from,
                        checkout: checkout_after,
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            Ok(())
        }
    }
}

pub async fn branch_delete(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    name: String,
) -> Result<(), GitError> {
    match env {
        Environment::Windows => {
            let repo = repo_path.to_owned();
            tokio::task::spawn_blocking(move || git_branch::delete_branch(&repo, &name))
                .await
                .map_err(|e| GitError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            agent_pool
                .call(
                    distro,
                    op_name::GIT_BRANCH_DELETE,
                    serde_json::to_value(GitBranchDeleteArgs {
                        repo_path: repo_path.to_owned(),
                        name,
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            Ok(())
        }
    }
}

pub async fn log(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    limit: u32,
    rev: Option<String>,
) -> Result<Vec<CommitInfo>, GitError> {
    match env {
        Environment::Windows => {
            let repo = repo_path.to_owned();
            let r = rev.clone();
            tokio::task::spawn_blocking(move || git_log::log(&repo, limit as usize, r.as_deref()))
                .await
                .map_err(|e| GitError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::GIT_LOG,
                    serde_json::to_value(GitLogArgs {
                        repo_path: repo_path.to_owned(),
                        limit,
                        rev,
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| GitError::Agent(e.to_string()))
        }
    }
}

pub async fn get_conflict(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    path: String,
) -> Result<ConflictContents, GitError> {
    match env {
        Environment::Windows => {
            let repo = repo_path.to_owned();
            tokio::task::spawn_blocking(move || git_conflict::get_conflict(&repo, &path))
                .await
                .map_err(|e| GitError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::GIT_GET_CONFLICT,
                    serde_json::to_value(GitConflictPathArgs {
                        repo_path: repo_path.to_owned(),
                        path,
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| GitError::Agent(e.to_string()))
        }
    }
}

pub async fn resolve_conflict(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    path: String,
    content: String,
) -> Result<(), GitError> {
    match env {
        Environment::Windows => {
            let repo = repo_path.to_owned();
            tokio::task::spawn_blocking(move || git_conflict::resolve(&repo, &path, &content))
                .await
                .map_err(|e| GitError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            agent_pool
                .call(
                    distro,
                    op_name::GIT_RESOLVE,
                    serde_json::to_value(GitResolveArgs {
                        repo_path: repo_path.to_owned(),
                        path,
                        content,
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            Ok(())
        }
    }
}

pub async fn remove_worktree(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    name: &str,
    _target_dir: &str,
) -> Result<(), GitError> {
    match env {
        Environment::Windows => {
            let path = repo_path.to_owned();
            let name_owned = name.to_owned();
            tokio::task::spawn_blocking(move || wt::remove_worktree(&path, &name_owned))
                .await
                .map_err(|e| GitError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            agent_pool
                .call(
                    distro,
                    op_name::GIT_REMOVE_WORKTREE,
                    serde_json::to_value(GitRemoveWorktreeArgs {
                        repo_path: repo_path.to_owned(),
                        name: name.to_owned(),
                    })
                    .map_err(|e| GitError::Agent(e.to_string()))?,
                )
                .await?;
            Ok(())
        }
    }
}
