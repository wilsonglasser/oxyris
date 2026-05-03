//! Git op handlers — all powered by `oxyris-git` (git2 under the hood).

use oxyris_git::branch as git_branch;
use oxyris_git::checkpoint;
use oxyris_git::conflict as git_conflict;
use oxyris_git::log as git_log;
use oxyris_git::remote;
use oxyris_git::status as git_status;
use oxyris_git::types::{CheckpointPhase, DiffMode};
use oxyris_git::worktree;
use oxyris_ipc::ops::{
    GitApplyPatchArgs, GitBranchCreateArgs, GitBranchDeleteArgs, GitCheckoutArgs,
    GitCheckpointCaptureArgs, GitCheckpointCaptureResult, GitCheckpointTurnArgs, GitCommitArgs,
    GitConflictPathArgs, GitCreateWorktreeArgs, GitDiffFileArgs, GitFetchArgs, GitLogArgs,
    GitPathsArgs, GitPullArgs, GitPushArgs, GitRemoveWorktreeArgs, GitRepoPathArgs, GitResolveArgs,
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

pub fn status(args: GitRepoPathArgs) -> Result<serde_json::Value, OpError> {
    let report = git_status::status(&args.repo_path)?;
    Ok(serde_json::to_value(report)?)
}

pub fn diff_file(args: GitDiffFileArgs) -> Result<serde_json::Value, OpError> {
    let mode = parse_mode(&args.mode)?;
    let diff = git_status::diff_file(&args.repo_path, &args.path, mode)?;
    Ok(serde_json::to_value(diff)?)
}

pub fn stage(args: GitPathsArgs) -> Result<serde_json::Value, OpError> {
    git_status::stage(&args.repo_path, &args.paths)?;
    Ok(serde_json::Value::Null)
}

pub fn unstage(args: GitPathsArgs) -> Result<serde_json::Value, OpError> {
    git_status::unstage(&args.repo_path, &args.paths)?;
    Ok(serde_json::Value::Null)
}

pub fn commit(args: GitCommitArgs) -> Result<serde_json::Value, OpError> {
    let result = git_status::commit(&args.repo_path, &args.message, args.amend)?;
    Ok(serde_json::to_value(result)?)
}

pub fn fetch(args: GitFetchArgs) -> Result<serde_json::Value, OpError> {
    let result = remote::fetch(&args.repo_path, args.remote.as_deref())?;
    Ok(serde_json::to_value(result)?)
}

pub fn pull(args: GitPullArgs) -> Result<serde_json::Value, OpError> {
    let result = remote::pull(
        &args.repo_path,
        args.remote.as_deref(),
        args.branch.as_deref(),
        args.rebase,
    )?;
    Ok(serde_json::to_value(result)?)
}

pub fn push(args: GitPushArgs) -> Result<serde_json::Value, OpError> {
    let result = remote::push(
        &args.repo_path,
        args.remote.as_deref(),
        args.branch.as_deref(),
        args.force,
        args.set_upstream,
    )?;
    Ok(serde_json::to_value(result)?)
}

pub fn checkout(args: GitCheckoutArgs) -> Result<serde_json::Value, OpError> {
    git_branch::checkout(&args.repo_path, &args.name)?;
    Ok(serde_json::Value::Null)
}

pub fn branch_create(args: GitBranchCreateArgs) -> Result<serde_json::Value, OpError> {
    git_branch::create_branch(
        &args.repo_path,
        &args.name,
        args.from.as_deref(),
        args.checkout,
    )?;
    Ok(serde_json::Value::Null)
}

pub fn branch_delete(args: GitBranchDeleteArgs) -> Result<serde_json::Value, OpError> {
    git_branch::delete_branch(&args.repo_path, &args.name)?;
    Ok(serde_json::Value::Null)
}

pub fn log(args: GitLogArgs) -> Result<serde_json::Value, OpError> {
    let entries = git_log::log(&args.repo_path, args.limit as usize, args.rev.as_deref())?;
    Ok(serde_json::to_value(entries)?)
}

pub fn get_conflict(args: GitConflictPathArgs) -> Result<serde_json::Value, OpError> {
    let c = git_conflict::get_conflict(&args.repo_path, &args.path)?;
    Ok(serde_json::to_value(c)?)
}

pub fn resolve(args: GitResolveArgs) -> Result<serde_json::Value, OpError> {
    git_conflict::resolve(&args.repo_path, &args.path, &args.content)?;
    Ok(serde_json::Value::Null)
}

pub fn apply_patch(args: GitApplyPatchArgs) -> Result<serde_json::Value, OpError> {
    git_status::apply_patch(&args.repo_path, &args.patch, args.reverse, args.cached)?;
    Ok(serde_json::Value::Null)
}

fn parse_mode(s: &str) -> Result<DiffMode, OpError> {
    match s {
        "working_vs_head" => Ok(DiffMode::WorkingVsHead),
        "staged_vs_head" => Ok(DiffMode::StagedVsHead),
        "working_vs_staged" => Ok(DiffMode::WorkingVsStaged),
        other => Err(OpError::Git(format!("unknown diff mode: {other}"))),
    }
}
