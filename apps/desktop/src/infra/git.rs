//! Git facade for the desktop backend.
//!
//! Windows projects: call `oxyris-git` directly (git2 in-process).
//! WSL projects: dispatch to the per-distro agent over NDJSON; the agent
//! runs the same `oxyris-git` ops natively inside the distro so we never
//! shell out across the 9p bridge.

use oxyris_core::Environment;
pub use oxyris_git::{BranchInfo, WorktreeRef};
use oxyris_git::{GitError as InnerGitError, worktree as wt};
use oxyris_ipc::ops::{GitCreateWorktreeArgs, GitRemoveWorktreeArgs, GitRepoPathArgs, op_name};
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
