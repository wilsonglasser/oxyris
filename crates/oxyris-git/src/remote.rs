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

pub fn fetch(repo_path: &str, remote: Option<&str>) -> Result<RemoteOpResult, GitError> {
    let mut args = vec!["-C", repo_path, "fetch"];
    if let Some(r) = remote {
        args.push(r);
    } else {
        args.push("--all");
    }
    args.push("--prune");
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
        args.push(r);
        if let Some(b) = branch {
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
        args.push(r);
        if let Some(b) = branch {
            args.push(b);
        }
    }
    run(&args)
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
