//! Worktree-scoped git commands powering the Git panel.
//!
//! Status, single-file diff, stage/unstage, and commit. Every command takes
//! `(project_id, worktree_id)` and routes to native git2 (Windows) or the
//! per-distro agent (WSL). The worktree's path on disk is the repo we open.

use oxyris_core::AggregateId;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::app_state::AppState;
use crate::infra::fs::{self as fs_infra, FsError};
use crate::infra::git::{
    self, CommitInfo, CommitResult, ConflictContents, DiffMode, FileDiff, GitError, RemoteOpResult,
    StatusReport,
};

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriGitError {
    #[error("{0}")]
    Backend(String),
}

impl From<GitError> for TauriGitError {
    fn from(e: GitError) -> Self {
        TauriGitError::Backend(e.to_string())
    }
}

impl From<FsError> for TauriGitError {
    fn from(e: FsError) -> Self {
        TauriGitError::Backend(e.to_string())
    }
}

#[derive(Debug, Deserialize)]
pub struct GitScopeInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
}

#[tauri::command]
pub async fn git_status(
    input: GitScopeInput,
    state: State<'_, AppState>,
) -> Result<StatusReport, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::status(&env, &state.agent_pool, &root).await?)
}

#[derive(Debug, Deserialize)]
pub struct GitDiffFileInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub path: String,
    /// "working_vs_head" | "staged_vs_head" | "working_vs_staged".
    pub mode: String,
}

#[tauri::command]
pub async fn git_diff_file(
    input: GitDiffFileInput,
    state: State<'_, AppState>,
) -> Result<FileDiff, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let mode = parse_mode(&input.mode)?;
    Ok(git::diff_file(&env, &state.agent_pool, &root, &input.path, mode).await?)
}

#[derive(Debug, Deserialize)]
pub struct GitPathsInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub paths: Vec<String>,
}

#[tauri::command]
pub async fn git_stage(
    input: GitPathsInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::stage(&env, &state.agent_pool, &root, input.paths).await?;
    Ok(())
}

#[tauri::command]
pub async fn git_unstage(
    input: GitPathsInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::unstage(&env, &state.agent_pool, &root, input.paths).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct GitCommitInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub message: String,
    #[serde(default)]
    pub amend: bool,
}

#[tauri::command]
pub async fn git_commit(
    input: GitCommitInput,
    state: State<'_, AppState>,
) -> Result<CommitResult, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let trimmed = input.message.trim();
    if trimmed.is_empty() {
        return Err(TauriGitError::Backend("commit message is empty".into()));
    }
    Ok(git::commit(
        &env,
        &state.agent_pool,
        &root,
        trimmed.to_owned(),
        input.amend,
    )
    .await?)
}

#[derive(Debug, Deserialize)]
pub struct GitFetchInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    #[serde(default)]
    pub remote: Option<String>,
}

#[tauri::command]
pub async fn git_fetch(
    input: GitFetchInput,
    state: State<'_, AppState>,
) -> Result<RemoteOpResult, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::fetch(&env, &state.agent_pool, &root, input.remote).await?)
}

#[derive(Debug, Deserialize)]
pub struct GitPullInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub rebase: bool,
}

#[tauri::command]
pub async fn git_pull(
    input: GitPullInput,
    state: State<'_, AppState>,
) -> Result<RemoteOpResult, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::pull(
        &env,
        &state.agent_pool,
        &root,
        input.remote,
        input.branch,
        input.rebase,
    )
    .await?)
}

#[derive(Debug, Deserialize)]
pub struct GitPushInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub set_upstream: bool,
}

#[tauri::command]
pub async fn git_push(
    input: GitPushInput,
    state: State<'_, AppState>,
) -> Result<RemoteOpResult, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::push(
        &env,
        &state.agent_pool,
        &root,
        input.remote,
        input.branch,
        input.force,
        input.set_upstream,
    )
    .await?)
}

#[derive(Debug, Deserialize)]
pub struct GitCheckoutInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub name: String,
}

#[tauri::command]
pub async fn git_checkout(
    input: GitCheckoutInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::checkout(&env, &state.agent_pool, &root, input.name).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct GitBranchCreateInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub name: String,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub checkout: bool,
}

#[tauri::command]
pub async fn git_branch_create(
    input: GitBranchCreateInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::branch_create(
        &env,
        &state.agent_pool,
        &root,
        input.name,
        input.from,
        input.checkout,
    )
    .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct GitBranchDeleteInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub name: String,
}

#[tauri::command]
pub async fn git_branch_delete(
    input: GitBranchDeleteInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::branch_delete(&env, &state.agent_pool, &root, input.name).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct GitLogInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default)]
    pub rev: Option<String>,
}

fn default_limit() -> u32 {
    50
}

#[tauri::command]
pub async fn git_log(
    input: GitLogInput,
    state: State<'_, AppState>,
) -> Result<Vec<CommitInfo>, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::log(&env, &state.agent_pool, &root, input.limit, input.rev).await?)
}

#[derive(Debug, Deserialize)]
pub struct GitConflictPathInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub path: String,
}

#[tauri::command]
pub async fn git_get_conflict(
    input: GitConflictPathInput,
    state: State<'_, AppState>,
) -> Result<ConflictContents, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::get_conflict(&env, &state.agent_pool, &root, input.path).await?)
}

#[derive(Debug, Deserialize)]
pub struct GitResolveInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub path: String,
    pub content: String,
}

#[tauri::command]
pub async fn git_resolve(
    input: GitResolveInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::resolve_conflict(&env, &state.agent_pool, &root, input.path, input.content).await?;
    Ok(())
}

// ────── generate commit message via Claude CLI ─────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GitGenerateCommitMsgInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
}

#[derive(Debug, Serialize)]
pub struct GitGenerateCommitMsgOutput {
    pub message: String,
}

#[tauri::command]
pub async fn git_generate_commit_message(
    input: GitGenerateCommitMsgInput,
    state: State<'_, AppState>,
) -> Result<GitGenerateCommitMsgOutput, TauriGitError> {
    use oxyris_core::Environment;
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;

    let prompt = "Write a single concise commit message for the staged diff below. \
        Use Conventional Commits format (e.g. `feat:`, `fix:`, `chore:`). \
        Subject line under 72 chars. Add a body only if the why isn't obvious. \
        Output the commit message only — no preamble, no markdown fences.";

    let (diff, claude_path) = match env {
        Environment::Windows => {
            let repo_path = root.clone();
            let diff_out = tokio::task::spawn_blocking(move || -> Result<String, String> {
                let out = std::process::Command::new("git")
                    .args(["-C", &repo_path, "diff", "--cached"])
                    .output()
                    .map_err(|e| e.to_string())?;
                if !out.status.success() {
                    return Err(String::from_utf8_lossy(&out.stderr).into_owned());
                }
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            })
            .await
            .map_err(|e| TauriGitError::Backend(format!("join: {e}")))?
            .map_err(TauriGitError::Backend)?;

            let path = which::which("claude")
                .or_else(|_| which::which("claude.cmd"))
                .or_else(|_| which::which("claude.exe"))
                .map_err(|e| TauriGitError::Backend(format!("claude not on PATH: {e}")))?;
            (diff_out, path)
        }
        Environment::Wsl { distro } => {
            // One-shot bash invocation: cd into the repo, pipe the staged
            // diff through claude inside the distro. Uses the user's claude
            // install + auth state inside WSL — no shimming through Windows.
            let posix_repo = root.clone();
            let escaped_prompt = prompt.replace('\'', "'\\''");
            let script = format!(
                "set -euo pipefail; cd '{posix_repo}'; \
                 diff=\"$(git diff --cached)\"; \
                 if [ -z \"$diff\" ]; then echo 'NOTHING_STAGED' >&2; exit 2; fi; \
                 printf '%s' \"$diff\" | claude -p '{escaped_prompt}'"
            );
            let out = Command::new("wsl.exe")
                .args(["-d", distro.as_str(), "--", "bash", "-lc", &script])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await
                .map_err(|e| TauriGitError::Backend(format!("spawn wsl: {e}")))?;
            if !out.status.success() {
                let stderr = crate::infra::decode_wsl_output_for_command(&out.stderr);
                if stderr.contains("NOTHING_STAGED") {
                    return Err(TauriGitError::Backend("nothing staged".into()));
                }
                return Err(TauriGitError::Backend(format!(
                    "wsl claude failed: {}",
                    stderr.trim()
                )));
            }
            let message = crate::infra::decode_wsl_output_for_command(&out.stdout)
                .trim()
                .to_owned();
            return Ok(GitGenerateCommitMsgOutput { message });
        }
    };

    if diff.trim().is_empty() {
        return Err(TauriGitError::Backend("nothing staged".into()));
    }

    let is_batch = claude_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e.to_ascii_lowercase().as_str(), "cmd" | "bat"))
        .unwrap_or(false);

    let mut cmd = if is_batch {
        let mut c = Command::new("cmd.exe");
        c.arg("/C");
        c.arg(claude_path.as_os_str());
        c
    } else {
        Command::new(claude_path.as_os_str())
    };
    cmd.arg("-p")
        .arg(prompt)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| TauriGitError::Backend(format!("spawn claude: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(diff.as_bytes())
            .await
            .map_err(|e| TauriGitError::Backend(format!("write diff: {e}")))?;
        stdin
            .shutdown()
            .await
            .map_err(|e| TauriGitError::Backend(format!("close stdin: {e}")))?;
    }

    let out = child
        .wait_with_output()
        .await
        .map_err(|e| TauriGitError::Backend(format!("claude wait: {e}")))?;
    if !out.status.success() {
        return Err(TauriGitError::Backend(format!(
            "claude failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let message = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Ok(GitGenerateCommitMsgOutput { message })
}

fn parse_mode(s: &str) -> Result<DiffMode, TauriGitError> {
    match s {
        "working_vs_head" => Ok(DiffMode::WorkingVsHead),
        "staged_vs_head" => Ok(DiffMode::StagedVsHead),
        "working_vs_staged" => Ok(DiffMode::WorkingVsStaged),
        other => Err(TauriGitError::Backend(format!(
            "unknown diff mode: {other}"
        ))),
    }
}
