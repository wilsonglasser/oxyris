//! Checkpoint facade for the desktop backend.
//!
//! Same dispatch pattern as `infra/git.rs`: Windows runs the pure git2 ops
//! from `oxyris-git` in `spawn_blocking`; WSL forwards to the per-distro
//! agent over NDJSON. Both sides share the same checkpoint code so behavior
//! and ref naming match exactly.

use std::path::Path;

use oxyris_core::Environment;
use oxyris_git::{checkpoint as cp, types::CheckpointPhase};
use oxyris_ipc::ops::{
    GitCheckpointCaptureArgs, GitCheckpointCaptureResult, GitCheckpointTurnArgs, op_name,
};
use thiserror::Error;

use crate::infra::agent_pool::{AgentError, AgentPool};

pub use oxyris_git::types::TurnDiff;

#[derive(Debug, Clone, Copy)]
pub enum Phase {
    Pre,
    Post,
}

impl Phase {
    fn as_inner(self) -> CheckpointPhase {
        match self {
            Phase::Pre => CheckpointPhase::Pre,
            Phase::Post => CheckpointPhase::Post,
        }
    }
    fn as_wire(self) -> &'static str {
        match self {
            Phase::Pre => "pre",
            Phase::Post => "post",
        }
    }
}

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("git: {0}")]
    Git(String),
    #[error("agent: {0}")]
    Agent(String),
    #[error("checkpoint ref missing: {0}")]
    RefMissing(String),
}

impl From<oxyris_git::GitError> for CheckpointError {
    fn from(e: oxyris_git::GitError) -> Self {
        match e {
            oxyris_git::GitError::RefMissing(r) => CheckpointError::RefMissing(r),
            other => CheckpointError::Git(other.to_string()),
        }
    }
}

impl From<AgentError> for CheckpointError {
    fn from(e: AgentError) -> Self {
        CheckpointError::Agent(e.to_string())
    }
}

pub async fn capture(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    session_id: &str,
    turn_id: &str,
    phase: Phase,
) -> Result<Option<String>, CheckpointError> {
    match env {
        Environment::Windows => {
            let path = repo_path.to_owned();
            let sid = session_id.to_owned();
            let tid = turn_id.to_owned();
            let p = phase.as_inner();
            tokio::task::spawn_blocking(move || cp::capture(&path, &sid, &tid, p))
                .await
                .map_err(|e| CheckpointError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::GIT_CHECKPOINT_CAPTURE,
                    serde_json::to_value(GitCheckpointCaptureArgs {
                        repo_path: repo_path.to_owned(),
                        session_id: session_id.to_owned(),
                        turn_id: turn_id.to_owned(),
                        phase: phase.as_wire().to_owned(),
                    })
                    .map_err(|e| CheckpointError::Agent(e.to_string()))?,
                )
                .await?;
            let result: GitCheckpointCaptureResult =
                serde_json::from_value(value).map_err(|e| CheckpointError::Agent(e.to_string()))?;
            Ok(result.ref_name)
        }
    }
}

pub async fn diff(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    session_id: &str,
    turn_id: &str,
) -> Result<TurnDiff, CheckpointError> {
    match env {
        Environment::Windows => {
            let path = repo_path.to_owned();
            let sid = session_id.to_owned();
            let tid = turn_id.to_owned();
            tokio::task::spawn_blocking(move || cp::diff(&path, &sid, &tid))
                .await
                .map_err(|e| CheckpointError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::GIT_CHECKPOINT_DIFF,
                    serde_json::to_value(GitCheckpointTurnArgs {
                        repo_path: repo_path.to_owned(),
                        session_id: session_id.to_owned(),
                        turn_id: turn_id.to_owned(),
                    })
                    .map_err(|e| CheckpointError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| CheckpointError::Agent(e.to_string()))
        }
    }
}

pub async fn revert_to_pre(
    env: &Environment,
    agent_pool: &AgentPool,
    repo_path: &str,
    session_id: &str,
    turn_id: &str,
) -> Result<(), CheckpointError> {
    match env {
        Environment::Windows => {
            let path = repo_path.to_owned();
            let sid = session_id.to_owned();
            let tid = turn_id.to_owned();
            tokio::task::spawn_blocking(move || cp::revert_to_pre(&path, &sid, &tid))
                .await
                .map_err(|e| CheckpointError::Git(format!("join: {e}")))?
                .map_err(Into::into)
        }
        Environment::Wsl { distro } => {
            agent_pool
                .call(
                    distro,
                    op_name::GIT_CHECKPOINT_REVERT,
                    serde_json::to_value(GitCheckpointTurnArgs {
                        repo_path: repo_path.to_owned(),
                        session_id: session_id.to_owned(),
                        turn_id: turn_id.to_owned(),
                    })
                    .map_err(|e| CheckpointError::Agent(e.to_string()))?,
                )
                .await?;
            Ok(())
        }
    }
}

/// GC of stale checkpoint refs (Windows-only currently — WSL agent could
/// schedule it on its own clock if needed).
#[allow(dead_code)]
pub fn gc(repo_path: &Path, older_than_days: i64) -> Result<usize, CheckpointError> {
    cp::gc(repo_path, older_than_days).map_err(Into::into)
}
