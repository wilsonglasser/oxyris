//! fetch / pull / push via the `git` binary.
//!
//! libgit2 supports remotes but credentials are a pain on Windows — the
//! `git` CLI integrates natively with Windows Credential Manager and the
//! distro's credential helpers, so we shell out and let the user's existing
//! auth Just Work. This is the documented exception in `PLAN.md` §13:
//! shellout when git2 doesn't cover the case cleanly.

use std::process::Command;

use oxyris_procutil::HideConsole;
use serde::{Deserialize, Serialize};

use crate::error::GitError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteOpResult {
    pub stdout: String,
    pub stderr: String,
}

/// Reject a user-supplied positional that git would read as an option. Paired
/// with the `--` end-of-options separator in every command below, this blocks
/// option injection via a url/remote/branch beginning with `-` (e.g.
/// `--upload-pack=...`, `--exec=...`, the classic `git` argument-injection RCE).
fn deny_option_like(arg: &str) -> Result<(), GitError> {
    if arg.starts_with('-') {
        return Err(GitError::RejectedArg(arg.to_owned()));
    }
    Ok(())
}

/// `git clone -- <url> <target_dir>`. Shelled out (not git2) for the same
/// credential-helper reason as fetch/pull/push. `target_dir` is the directory
/// the working tree lands in — git creates it (and missing parents) and
/// refuses if it already exists and is non-empty.
pub fn clone(url: &str, target_dir: &str) -> Result<RemoteOpResult, GitError> {
    deny_option_like(url)?;
    run(&["clone", "--", url, target_dir])
}

pub fn fetch(repo_path: &str, remote: Option<&str>) -> Result<RemoteOpResult, GitError> {
    let mut args = vec!["-C", repo_path, "fetch", "--prune"];
    // Options before `--`, the positional remote after it.
    if let Some(r) = remote {
        deny_option_like(r)?;
        args.push("--");
        args.push(r);
    } else {
        args.push("--all");
    }
    run(&args)
}

pub fn pull(
    repo_path: &str,
    remote: Option<&str>,
    branch: Option<&str>,
    rebase: bool,
) -> Result<RemoteOpResult, GitError> {
    let mut args = vec!["-C", repo_path, "pull"];
    if rebase {
        args.push("--rebase");
    }
    if let Some(r) = remote {
        deny_option_like(r)?;
        args.push("--");
        args.push(r);
        if let Some(b) = branch {
            deny_option_like(b)?;
            args.push(b);
        }
    }
    run(&args)
}

pub fn push(
    repo_path: &str,
    remote: Option<&str>,
    branch: Option<&str>,
    force: bool,
    set_upstream: bool,
) -> Result<RemoteOpResult, GitError> {
    let mut args = vec!["-C", repo_path, "push"];
    if force {
        args.push("--force-with-lease");
    }
    if set_upstream {
        args.push("--set-upstream");
    }
    if let Some(r) = remote {
        deny_option_like(r)?;
        args.push("--");
        args.push(r);
        if let Some(b) = branch {
            deny_option_like(b)?;
            args.push(b);
        }
    }
    run(&args)
}

/// `git push <remote> --delete <branch>` — removes the branch on the remote.
/// Shelled out for the same credential-helper reason as the other remote ops.
pub fn push_delete(
    repo_path: &str,
    remote: &str,
    branch: &str,
) -> Result<RemoteOpResult, GitError> {
    deny_option_like(remote)?;
    deny_option_like(branch)?;
    run(&["-C", repo_path, "push", "--delete", "--", remote, branch])
}

fn run(args: &[&str]) -> Result<RemoteOpResult, GitError> {
    let out = Command::new("git").args(args).hide_console().output()?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        return Err(GitError::NonZero(if stderr.is_empty() {
            stdout
        } else {
            stderr
        }));
    }
    Ok(RemoteOpResult { stdout, stderr })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_option_like_positional() {
        // Classic argument-injection payloads must be refused before they reach
        // the git CLI.
        for bad in ["--upload-pack=touch pwned", "--exec=evil", "-o"] {
            assert!(matches!(
                deny_option_like(bad),
                Err(GitError::RejectedArg(_))
            ));
        }
    }

    #[test]
    fn allows_normal_refs() {
        for ok in ["origin", "main", "feature/x", "https://example.com/r.git"] {
            assert!(deny_option_like(ok).is_ok());
        }
    }
}
