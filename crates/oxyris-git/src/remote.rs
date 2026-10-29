//! fetch / pull / push via the `git` binary.
//!
//! libgit2 supports remotes but credentials are a pain on Windows — the
//! `git` CLI integrates natively with Windows Credential Manager and the
//! distro's credential helpers, so we shell out and let the user's existing
//! auth Just Work. This is the documented exception in `PLAN.md` §13:
//! shellout when git2 doesn't cover the case cleanly.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

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

/// How long to keep draining stdout/stderr after git itself has exited.
/// Anything still holding the pipes past this is a detached helper, not git.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

fn run(args: &[&str]) -> Result<RemoteOpResult, GitError> {
    run_program("git", args)
}

/// Run `program` and return once *it* exits — not once its pipes close.
///
/// `Command::output()` waits for EOF on stdout/stderr, and git's remote ops
/// routinely leave those fds in the hands of a process that outlives git: an
/// ssh `ControlMaster`/`ControlPersist` daemon forked by the first connection,
/// or `git gc --auto` detaching after a fetch. Both inherit git's stderr, so
/// `output()` hangs forever while FETCH_HEAD has long been updated. Here the
/// pipes are drained on side threads (both at once — a chatty git deadlocks on
/// a full pipe otherwise), the child is waited on, and the readers get a short
/// grace period before whatever they captured is returned. A reader still
/// blocked after that is leaked; it ends when the fd holder finally exits.
///
/// stdin is `/dev/null`: in the WSL agent the inherited stdin is the NDJSON
/// control pipe, which git must never read. Terminal prompts are disabled so
/// a missing credential fails with a message instead of waiting on a tty.
fn run_program(program: &str, args: &[&str]) -> Result<RemoteOpResult, GitError> {
    let mut child = Command::new(program)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .hide_console()
        .spawn()?;
    let stdout_rx = drain(child.stdout.take());
    let stderr_rx = drain(child.stderr.take());
    let status = child.wait()?;
    let stdout = collect(&stdout_rx);
    let stderr = collect(&stderr_rx);
    if !status.success() {
        return Err(GitError::NonZero(if stderr.is_empty() {
            stdout
        } else {
            stderr
        }));
    }
    Ok(RemoteOpResult { stdout, stderr })
}

/// Drain `pipe` to EOF on a thread, delivering the bytes once at the end.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    if let Some(mut pipe) = pipe {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
    }
    rx
}

/// What the reader captured, or empty when something still holds the pipe
/// open past [`DRAIN_GRACE`] after the child exited.
fn collect(rx: &mpsc::Receiver<Vec<u8>>) -> String {
    match rx.recv_timeout(DRAIN_GRACE) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A detached child that inherits stderr must not keep `run_program`
    /// waiting after the program itself has exited (ssh ControlPersist, `git
    /// gc --auto`). `Command::output()` would block here for the full sleep.
    #[cfg(unix)]
    #[test]
    fn returns_when_program_exits_not_when_pipes_close() {
        let started = std::time::Instant::now();
        let out = run_program("sh", &["-c", "(sleep 5 >&2 &); echo ok"]).unwrap();
        assert_eq!(out.stdout.trim(), "ok");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "waited on the detached fd holder: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn non_zero_exit_is_an_error_with_output() {
        #[cfg(unix)]
        let err = run_program("sh", &["-c", "echo nope >&2; exit 3"]).unwrap_err();
        #[cfg(windows)]
        let err = run_program("cmd.exe", &["/C", "echo nope 1>&2 & exit 3"]).unwrap_err();
        assert!(
            matches!(&err, GitError::NonZero(m) if m.contains("nope")),
            "{err:?}"
        );
    }

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
