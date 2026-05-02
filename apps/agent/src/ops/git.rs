//! Git op handlers — all powered by `oxyris-git` (git2 under the hood).

use oxyris_git::checkpoint;
use oxyris_git::types::CheckpointPhase;
use oxyris_git::worktree;
use oxyris_ipc::ops::{
    GitCheckpointCaptureArgs, GitCheckpointCaptureResult, GitCheckpointTurnArgs,
    GitCreateWorktreeArgs, GitRemoveWorktreeArgs, GitRepoPathArgs,
};

use super::OpError;

impl From<oxyris_git::GitError> for OpError {
    fn from(e: oxyris_git::GitError) -> Self {
        match e {
            oxyris_git::GitError::EmptyRepo => OpError::EmptyRepo,
            other => OpError::Git(other.to_string()),
        }
    }
}

pub fn list_branches(args: GitRepoPathArgs) -> Result<serde_json::Value, OpError> {
    let rows = worktree::list_branches(&args.repo_path)?;
    Ok(serde_json::to_value(rows)?)
}

pub fn list_worktrees(args: GitRepoPathArgs) -> Result<serde_json::Value, OpError> {
    let rows = worktree::list_worktrees(&args.repo_path)?;
    Ok(serde_json::to_value(rows)?)
}

pub fn create_worktree(args: GitCreateWorktreeArgs) -> Result<serde_json::Value, OpError> {
    let row =
        worktree::create_worktree(&args.repo_path, &args.name, &args.branch, &args.target_dir)?;
    Ok(serde_json::to_value(row)?)
}

pub fn remove_worktree(args: GitRemoveWorktreeArgs) -> Result<serde_json::Value, OpError> {
    worktree::remove_worktree(&args.repo_path, &args.name)?;
    Ok(serde_json::Value::Null)
}

pub fn checkpoint_capture(args: GitCheckpointCaptureArgs) -> Result<serde_json::Value, OpError> {
    let phase = match args.phase.as_str() {
        "pre" => CheckpointPhase::Pre,
        "post" => CheckpointPhase::Post,
        other => return Err(OpError::Git(format!("unknown phase: {other}"))),
    };
    let ref_name = checkpoint::capture(&args.repo_path, &args.session_id, &args.turn_id, phase)?;
    Ok(serde_json::to_value(GitCheckpointCaptureResult {
        ref_name,
    })?)
}

pub fn checkpoint_diff(args: GitCheckpointTurnArgs) -> Result<serde_json::Value, OpError> {
    let diff = checkpoint::diff(&args.repo_path, &args.session_id, &args.turn_id)?;
    Ok(serde_json::to_value(diff)?)
}

pub fn checkpoint_revert(args: GitCheckpointTurnArgs) -> Result<serde_json::Value, OpError> {
    checkpoint::revert_to_pre(&args.repo_path, &args.session_id, &args.turn_id)?;
    Ok(serde_json::Value::Null)
}
