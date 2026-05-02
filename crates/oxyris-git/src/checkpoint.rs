//! Turn-by-turn git checkpointing — pure git2.
//!
//! Before Claude sees a user message we snapshot the working tree
//! (committed + uncommitted + untracked) by asking git for a stash object
//! and tagging it with a hidden ref `refs/oxyris/cp/<session>/<turn>-pre`.
//! When the turn finishes we do the same for `-post`. The pair lets the UI
//! render a diff of exactly what that turn changed without depending on
//! anything the user staged or committed.
//!
//! `git stash create` is invoked via `git` on PATH (not shellout-to-remote)
//! because git2 doesn't expose a "stash tree without mutating workdir"
//! primitive. Callers are expected to have git installed — the desktop
//! binary does, and the WSL agent can rely on the distro having git (we use
//! git2 for everything else so this is the only shellout).

use std::path::Path;
use std::process::Command;

use crate::error::GitError;
use crate::types::{CheckpointPhase, FileDiff, FileStatus, TurnDiff};

pub fn ref_name(session_id: &str, turn_id: &str, phase: CheckpointPhase) -> String {
    format!("refs/oxyris/cp/{session_id}/{turn_id}-{}", phase.suffix())
}

pub fn capture(
    repo_path: &str,
    session_id: &str,
    turn_id: &str,
    phase: CheckpointPhase,
) -> Result<Option<String>, GitError> {
    let repo = match git2::Repository::discover(repo_path) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    let full_ref = ref_name(session_id, turn_id, phase);

    let out = Command::new("git")
        .args(["-C", repo_path, "stash", "create", "oxyris checkpoint"])
        .output()?;
    if !out.status.success() {
        return Err(GitError::NonZero(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    let stash_sha = String::from_utf8(out.stdout)?.trim().to_owned();

    let oid = if stash_sha.is_empty() {
        // Empty workdir — point the ref at HEAD so pre/post still diff.
        match repo.head() {
            Ok(h) => h
                .target()
                .ok_or_else(|| GitError::NonZero("HEAD has no commit target".into()))?,
            Err(_) => return Ok(None),
        }
    } else {
        git2::Oid::from_str(&stash_sha)?
    };

    repo.reference(&full_ref, oid, true, "oxyris checkpoint")?;
    Ok(Some(full_ref))
}

pub fn revert_to_pre(repo_path: &str, session_id: &str, turn_id: &str) -> Result<(), GitError> {
    let full_ref = ref_name(session_id, turn_id, CheckpointPhase::Pre);
    let repo = git2::Repository::discover(repo_path)?;
    let oid = repo
        .find_reference(&full_ref)
        .map_err(|_| GitError::RefMissing(full_ref.clone()))?
        .target()
        .ok_or_else(|| GitError::RefMissing(full_ref.clone()))?;

    let out = Command::new("git")
        .args([
            "-C",
            repo_path,
            "read-tree",
            "-u",
            "--reset",
            &oid.to_string(),
        ])
        .output()?;
    if !out.status.success() {
        return Err(GitError::NonZero(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    Ok(())
}

pub fn diff(repo_path: &str, session_id: &str, turn_id: &str) -> Result<TurnDiff, GitError> {
    let repo = git2::Repository::discover(repo_path)?;
    // If either ref is missing, treat as empty diff — the turn didn't
    // capture a checkpoint (no changes), UI hides the block.
    let pre = match resolve_ref(&repo, session_id, turn_id, CheckpointPhase::Pre) {
        Ok(oid) => oid,
        Err(GitError::RefMissing(_)) => return Ok(TurnDiff { files: Vec::new() }),
        Err(e) => return Err(e),
    };
    let post = match resolve_ref(&repo, session_id, turn_id, CheckpointPhase::Post) {
        Ok(oid) => oid,
        Err(GitError::RefMissing(_)) => return Ok(TurnDiff { files: Vec::new() }),
        Err(e) => return Err(e),
    };

    let pre_tree = repo.find_commit(pre)?.tree()?;
    let post_tree = repo.find_commit(post)?.tree()?;

    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(false);
    opts.show_binary(false);
    opts.context_lines(3);
    let diff = repo.diff_tree_to_tree(Some(&pre_tree), Some(&post_tree), Some(&mut opts))?;

    let mut files: Vec<FileDiff> = Vec::new();
    for delta in diff.deltas() {
        let new_path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let old_path = delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned());

        let old_content = if delta.old_file().id().is_zero() {
            None
        } else {
            read_blob_text(&repo, delta.old_file().id()).ok()
        };
        let new_content = if delta.new_file().id().is_zero() {
            None
        } else {
            read_blob_text(&repo, delta.new_file().id()).ok()
        };

        files.push(FileDiff {
            path: new_path.clone(),
            old_path: if old_path.as_deref() == Some(new_path.as_str()) {
                None
            } else {
                old_path
            },
            status: FileStatus::from(delta.status()),
            old_content,
            new_content,
            unified: String::new(),
        });
    }

    // Second pass fills `unified` per file.
    let mut buffers: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        let path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let buf = buffers.entry(path).or_default();
        let origin = line.origin();
        if matches!(origin, '+' | '-' | ' ') {
            buf.push(origin);
        }
        buf.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;
    for f in &mut files {
        if let Some(text) = buffers.remove(&f.path) {
            f.unified = text;
        }
    }

    Ok(TurnDiff { files })
}

fn resolve_ref(
    repo: &git2::Repository,
    session_id: &str,
    turn_id: &str,
    phase: CheckpointPhase,
) -> Result<git2::Oid, GitError> {
    let name = ref_name(session_id, turn_id, phase);
    let r = repo
        .find_reference(&name)
        .map_err(|_| GitError::RefMissing(name.clone()))?;
    r.target().ok_or(GitError::RefMissing(name))
}

fn read_blob_text(repo: &git2::Repository, oid: git2::Oid) -> Result<String, GitError> {
    let blob = repo.find_blob(oid)?;
    Ok(String::from_utf8_lossy(blob.content()).into_owned())
}

/// Garbage-collect checkpoints older than `older_than_days` by scanning refs
/// under `refs/oxyris/cp/` and deleting anything whose commit's author time
/// is older than the threshold.
pub fn gc(repo_path: &Path, older_than_days: i64) -> Result<usize, GitError> {
    let repo = git2::Repository::discover(repo_path)?;
    let cutoff = chrono::Utc::now().timestamp() - older_than_days * 86_400;
    let mut removed = 0;
    let refs = repo.references_glob("refs/oxyris/cp/**/*")?;
    for r in refs {
        let Ok(mut r) = r else { continue };
        let Some(oid) = r.target() else { continue };
        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        if commit.time().seconds() < cutoff {
            let _ = r.delete();
            removed += 1;
        }
    }
    Ok(removed)
}
