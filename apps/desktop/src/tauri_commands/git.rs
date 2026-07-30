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
    self, BranchDetail, CommitInfo, CommitResult, ConflictContents, DiffMode, FileDiff, GitError,
    MergeOutcome, RebaseOutcome, RemoteOpResult, StashEntry, StatusReport, TagInfo,
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

/// Branch rows for the branch manager popup — worktree-scoped so `is_current`
/// and `checked_out_in` reflect the tree the panel is actually looking at.
#[tauri::command]
pub async fn git_branch_list(
    input: GitScopeInput,
    state: State<'_, AppState>,
) -> Result<Vec<BranchDetail>, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::branch_list_detailed(&env, &state.agent_pool, &root).await?)
}

#[derive(Debug, Deserialize)]
pub struct GitBranchRenameInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub old: String,
    pub new: String,
    #[serde(default)]
    pub force: bool,
}

#[tauri::command]
pub async fn git_branch_rename(
    input: GitBranchRenameInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::branch_rename(
        &env,
        &state.agent_pool,
        &root,
        input.old,
        input.new,
        input.force,
    )
    .await?;
    Ok(())
}

/// Drop a remote-tracking ref locally (`origin/x`). The branch on the remote
/// is untouched — that is `git_push_delete`.
#[tauri::command]
pub async fn git_branch_delete_remote(
    input: GitBranchDeleteInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::branch_delete_remote_tracking(&env, &state.agent_pool, &root, input.name).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct GitCheckoutRemoteInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub remote_ref: String,
    #[serde(default)]
    pub local: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GitCheckoutRemoteOutput {
    /// Local branch that ended up checked out.
    pub local: String,
}

#[tauri::command]
pub async fn git_checkout_remote(
    input: GitCheckoutRemoteInput,
    state: State<'_, AppState>,
) -> Result<GitCheckoutRemoteOutput, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let local = git::checkout_remote(
        &env,
        &state.agent_pool,
        &root,
        input.remote_ref,
        input.local,
    )
    .await?;
    Ok(GitCheckoutRemoteOutput { local })
}

#[derive(Debug, Deserialize)]
pub struct GitPushDeleteInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    #[serde(default = "default_origin")]
    pub remote: String,
    pub branch: String,
}

fn default_origin() -> String {
    "origin".to_owned()
}

#[tauri::command]
pub async fn git_push_delete(
    input: GitPushDeleteInput,
    state: State<'_, AppState>,
) -> Result<RemoteOpResult, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::push_delete(&env, &state.agent_pool, &root, input.remote, input.branch).await?)
}

// ────── merge / rebase ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GitMergeInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    /// Branch / tag / commit-ish merged into the current HEAD.
    pub name: String,
    #[serde(default)]
    pub no_ff: bool,
}

#[tauri::command]
pub async fn git_merge(
    input: GitMergeInput,
    state: State<'_, AppState>,
) -> Result<MergeOutcome, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::merge(&env, &state.agent_pool, &root, input.name, input.no_ff).await?)
}

#[tauri::command]
pub async fn git_merge_abort(
    input: GitScopeInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::merge_abort(&env, &state.agent_pool, &root).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct GitRebaseInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    /// Branch the current HEAD's commits are replayed onto.
    pub upstream: String,
}

#[tauri::command]
pub async fn git_rebase(
    input: GitRebaseInput,
    state: State<'_, AppState>,
) -> Result<RebaseOutcome, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::rebase(&env, &state.agent_pool, &root, input.upstream).await?)
}

#[tauri::command]
pub async fn git_rebase_continue(
    input: GitScopeInput,
    state: State<'_, AppState>,
) -> Result<RebaseOutcome, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::rebase_continue(&env, &state.agent_pool, &root).await?)
}

#[tauri::command]
pub async fn git_rebase_abort(
    input: GitScopeInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::rebase_abort(&env, &state.agent_pool, &root).await?;
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
    /// When set, restricts the log to commits touching this worktree-relative
    /// path (file history).
    #[serde(default)]
    pub path: Option<String>,
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
    Ok(git::log(
        &env,
        &state.agent_pool,
        &root,
        input.limit,
        input.rev,
        input.path,
    )
    .await?)
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

#[derive(Debug, Deserialize)]
pub struct GitApplyPatchInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub patch: String,
    /// If true, applies the patch in reverse (used to unstage a hunk).
    #[serde(default)]
    pub reverse: bool,
    /// If true, applies to the index (`git apply --cached`). Always true
    /// for stage/unstage flows; available as a knob in case we ever apply
    /// to the workdir.
    #[serde(default = "default_true")]
    pub cached: bool,
}

fn default_true() -> bool {
    true
}

#[tauri::command]
pub async fn git_apply_patch(
    input: GitApplyPatchInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::apply_patch(
        &env,
        &state.agent_pool,
        &root,
        input.patch,
        input.reverse,
        input.cached,
    )
    .await?;
    Ok(())
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
    use oxyris_procutil::HideConsole;
    use std::process::Stdio;
    use tokio::process::Command;

    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;

    let prompt = "Write a single concise commit message for the staged diff below. \
        Use Conventional Commits format (e.g. `feat:`, `fix:`, `chore:`). \
        Subject line under 72 chars. Add a body only if the why isn't obvious. \
        Output the commit message only — no preamble, no markdown fences.";

    // Compute the diff through git2 (`diff_revs`) — the SAME engine that backs
    // `git_status`. The old path shelled out to `git diff --cached`, which could
    // disagree with the panel: a stray `GIT_DIR`/`GIT_WORK_TREE` in the app's
    // environment, an external diff driver, or any libgit2-vs-CLI index quirk
    // made the CLI report an empty diff while the panel showed staged files —
    // surfacing the spurious "nothing staged" error. Sourcing the diff from
    // git2 guarantees the two never disagree.
    let diff = pending_diff_text(&env, &state.agent_pool, &root).await?;
    if diff.trim().is_empty() {
        return Err(TauriGitError::Backend("nothing staged".into()));
    }

    let message = match env {
        Environment::Local => {
            let claude_path = which::which("claude")
                .or_else(|_| which::which("claude.cmd"))
                .or_else(|_| which::which("claude.exe"))
                .map_err(|e| TauriGitError::Backend(format!("claude not on PATH: {e}")))?;

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
                .stderr(Stdio::piped())
                .hide_console();

            let mut child = cmd
                .spawn()
                .map_err(|e| TauriGitError::Backend(format!("spawn claude: {e}")))?;
            pipe_stdin(&mut child, diff.as_bytes()).await?;
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
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        }
        Environment::Wsl { distro } => {
            // Run claude inside the distro (the user's WSL install + auth) and
            // feed it the git2-computed diff over stdin. Uses the same diff the
            // panel sees — no second `git diff` inside bash to drift from it.
            let escaped_prompt = prompt.replace('\'', "'\\''");
            let script = format!("claude -p '{escaped_prompt}'");
            let mut child = Command::new("wsl.exe")
                .args(["-d", distro.as_str(), "--", "bash", "-lc", &script])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .hide_console()
                .spawn()
                .map_err(|e| TauriGitError::Backend(format!("spawn wsl: {e}")))?;
            pipe_stdin(&mut child, diff.as_bytes()).await?;
            let out = child
                .wait_with_output()
                .await
                .map_err(|e| TauriGitError::Backend(format!("wsl claude wait: {e}")))?;
            if !out.status.success() {
                let stderr = crate::infra::decode_wsl_output_for_command(&out.stderr);
                return Err(TauriGitError::Backend(format!(
                    "wsl claude failed: {}",
                    stderr.trim()
                )));
            }
            crate::infra::decode_wsl_output_for_command(&out.stdout)
                .trim()
                .to_owned()
        }
    };

    Ok(GitGenerateCommitMsgOutput { message })
}

/// Unified diff of every pending change (HEAD → working tree, index included)
/// via git2, so it stays consistent with `git_status`. On a fresh repo with no
/// commits, `diff_revs` can't diff against HEAD — fall back to a file-list
/// summary so Claude can still draft an initial-commit message.
async fn pending_diff_text(
    env: &oxyris_core::Environment,
    agent_pool: &crate::infra::agent_pool::AgentPool,
    root: &str,
) -> Result<String, TauriGitError> {
    let files = match git::diff_revs(
        env,
        agent_pool,
        root,
        "HEAD".into(),
        "WORKTREE".into(),
        true,
    )
    .await
    {
        Ok(files) => files,
        Err(GitError::EmptyRepo) => {
            let report = git::status(env, agent_pool, root).await?;
            let mut out = String::from("New repository (no commits yet). Files to be committed:\n");
            for e in &report.entries {
                out.push_str("  ");
                out.push_str(&e.path);
                out.push('\n');
            }
            return Ok(out);
        }
        Err(e) => return Err(e.into()),
    };

    let mut out = String::new();
    for f in &files {
        match &f.old_path {
            Some(old) => {
                out.push_str(&format!("--- {old}\n+++ {}\n", f.path));
            }
            None => {
                out.push_str(&format!("=== {}\n", f.path));
            }
        }
        out.push_str(&f.unified);
        if !f.unified.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

/// Write `bytes` to the child's stdin and close it.
async fn pipe_stdin(child: &mut tokio::process::Child, bytes: &[u8]) -> Result<(), TauriGitError> {
    use tokio::io::AsyncWriteExt;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(bytes)
            .await
            .map_err(|e| TauriGitError::Backend(format!("write diff: {e}")))?;
        stdin
            .shutdown()
            .await
            .map_err(|e| TauriGitError::Backend(format!("close stdin: {e}")))?;
    }
    Ok(())
}

// ────── stash / tag / cherry-pick / revert ─────────────────────────────────

#[tauri::command]
pub async fn git_stash_list(
    input: GitScopeInput,
    state: State<'_, AppState>,
) -> Result<Vec<StashEntry>, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::stash_list(&env, &state.agent_pool, &root).await?)
}

#[derive(Debug, Deserialize)]
pub struct GitStashSaveInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub message: String,
    #[serde(default)]
    pub include_untracked: bool,
}

#[tauri::command]
pub async fn git_stash_save(
    input: GitStashSaveInput,
    state: State<'_, AppState>,
) -> Result<String, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::stash_save(
        &env,
        &state.agent_pool,
        &root,
        input.message,
        input.include_untracked,
    )
    .await?)
}

#[derive(Debug, Deserialize)]
pub struct GitStashApplyInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub index: u32,
    #[serde(default)]
    pub drop_after: bool,
}

#[tauri::command]
pub async fn git_stash_apply(
    input: GitStashApplyInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::stash_apply(
        &env,
        &state.agent_pool,
        &root,
        input.index,
        input.drop_after,
    )
    .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct GitStashIndexInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub index: u32,
}

#[tauri::command]
pub async fn git_stash_drop(
    input: GitStashIndexInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::stash_drop(&env, &state.agent_pool, &root, input.index).await?;
    Ok(())
}

#[tauri::command]
pub async fn git_tag_list(
    input: GitScopeInput,
    state: State<'_, AppState>,
) -> Result<Vec<TagInfo>, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::tag_list(&env, &state.agent_pool, &root).await?)
}

#[derive(Debug, Deserialize)]
pub struct GitTagCreateInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub name: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub force: bool,
}

#[tauri::command]
pub async fn git_tag_create(
    input: GitTagCreateInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::tag_create(
        &env,
        &state.agent_pool,
        &root,
        input.name,
        input.target,
        input.message,
        input.force,
    )
    .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct GitTagNameInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub name: String,
}

#[tauri::command]
pub async fn git_tag_delete(
    input: GitTagNameInput,
    state: State<'_, AppState>,
) -> Result<(), TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    git::tag_delete(&env, &state.agent_pool, &root, input.name).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct GitCommitOidInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub oid: String,
}

#[derive(Debug, Serialize)]
pub struct GitCommitOidOutput {
    /// `None` when the operation produced conflicts (left in the index for
    /// the user to resolve before re-committing).
    pub oid: Option<String>,
}

#[tauri::command]
pub async fn git_cherry_pick(
    input: GitCommitOidInput,
    state: State<'_, AppState>,
) -> Result<GitCommitOidOutput, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let oid = git::cherry_pick(&env, &state.agent_pool, &root, input.oid).await?;
    Ok(GitCommitOidOutput { oid })
}

#[tauri::command]
pub async fn git_revert(
    input: GitCommitOidInput,
    state: State<'_, AppState>,
) -> Result<GitCommitOidOutput, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let oid = git::revert(&env, &state.agent_pool, &root, input.oid).await?;
    Ok(GitCommitOidOutput { oid })
}

#[derive(Debug, Deserialize)]
pub struct GitDiffRevsInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub from: String,
    pub to: String,
    #[serde(default = "default_true_renames")]
    pub find_renames: bool,
}

fn default_true_renames() -> bool {
    true
}

#[tauri::command]
pub async fn git_diff_revs(
    input: GitDiffRevsInput,
    state: State<'_, AppState>,
) -> Result<Vec<FileDiff>, TauriGitError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(git::diff_revs(
        &env,
        &state.agent_pool,
        &root,
        input.from,
        input.to,
        input.find_renames,
    )
    .await?)
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
