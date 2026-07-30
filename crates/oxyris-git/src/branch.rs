//! Branch operations: listing, checkout, create, rename, delete.
//!
//! Pure git2. `checkout` runs a "safe" checkout — refuses if the working
//! tree has uncommitted changes that would conflict. Use the dedicated
//! stash/commit flow first if the user wants to switch with dirty workdir.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::GitError;
use crate::types::AheadBehind;

/// One row in the branch manager. Richer than `BranchInfo` (which only feeds
/// the compare-with-ref picker): carries tracking info, divergence from the
/// upstream, tip metadata for sorting by recency, and which linked worktree
/// already has the branch checked out (git refuses a second checkout).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchDetail {
    /// `main` for local branches, `origin/main` for remote-tracking ones.
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
    /// Upstream shorthand (`origin/main`) for local branches that track one.
    pub upstream: Option<String>,
    /// Divergence from `upstream`. `None` when there is no upstream.
    pub ahead_behind: Option<AheadBehind>,
    pub tip_oid: String,
    pub tip_short: String,
    pub tip_summary: String,
    /// Unix seconds of the tip commit — drives the "Recent" ordering.
    pub tip_time: i64,
    /// Name of the linked worktree holding this branch, when it isn't ours.
    pub checked_out_in: Option<String>,
}

pub fn list_detailed(repo_path: &str) -> Result<Vec<BranchDetail>, GitError> {
    let repo = open(repo_path)?;
    let head = repo.head().ok();
    let head_is_branch = head
        .as_ref()
        .map(git2::Reference::is_branch)
        .unwrap_or(false);
    let current = head.as_ref().and_then(|h| h.shorthand().map(str::to_owned));
    let occupied = worktree_branches(&repo);

    let mut out = Vec::new();
    for entry in repo.branches(None)? {
        let (branch, kind) = entry?;
        let Some(name) = branch.name()?.map(str::to_owned) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let is_remote = matches!(kind, git2::BranchType::Remote);
        // `origin/HEAD` is a symbolic pointer, not a branch anyone checks out.
        if is_remote && name.ends_with("/HEAD") {
            continue;
        }
        let is_current = !is_remote && head_is_branch && current.as_deref() == Some(name.as_str());

        let commit = branch.get().peel_to_commit()?;
        let tip_oid = commit.id().to_string();
        let upstream = if is_remote {
            None
        } else {
            branch
                .upstream()
                .ok()
                .and_then(|u| u.name().ok().flatten().map(str::to_owned))
        };
        let ahead_behind = upstream
            .as_ref()
            .and_then(|up| repo.refname_to_id(&format!("refs/remotes/{up}")).ok())
            .and_then(|up_oid| repo.graph_ahead_behind(commit.id(), up_oid).ok())
            .map(|(ahead, behind)| AheadBehind {
                ahead: ahead as u32,
                behind: behind as u32,
            });

        out.push(BranchDetail {
            is_current,
            upstream,
            ahead_behind,
            tip_short: tip_oid[..tip_oid.len().min(7)].to_owned(),
            tip_oid,
            tip_summary: commit.summary().unwrap_or("").to_owned(),
            tip_time: commit.time().seconds(),
            checked_out_in: if is_current {
                None
            } else {
                occupied.get(&name).cloned()
            },
            name,
            is_remote,
        });
    }

    // Current first, then locals, then by tip recency — the JetBrains ordering.
    out.sort_by(|a, b| {
        b.is_current
            .cmp(&a.is_current)
            .then(a.is_remote.cmp(&b.is_remote))
            .then(b.tip_time.cmp(&a.tip_time))
    });
    Ok(out)
}

/// Map of `branch name -> worktree name` for every linked worktree plus the
/// primary tree. Used to grey out branches git would refuse to check out.
fn worktree_branches(repo: &git2::Repository) -> HashMap<String, String> {
    let mut map = HashMap::new();

    // The primary tree — `commondir` is its `.git`, even when `repo` is a
    // linked worktree.
    if let Ok(main) = git2::Repository::open(repo.commondir())
        && let Some(name) = head_branch(&main)
    {
        map.insert(name, "primary".to_owned());
    }

    if let Ok(names) = repo.worktrees() {
        for wt_name in names.iter().flatten() {
            let Ok(wt) = repo.find_worktree(wt_name) else {
                continue;
            };
            let Ok(wt_repo) = git2::Repository::open(wt.path()) else {
                continue;
            };
            if let Some(branch) = head_branch(&wt_repo) {
                map.insert(branch, wt_name.to_owned());
            }
        }
    }
    map
}

/// Shorthand of the branch HEAD points at, or `None` when detached / unborn.
fn head_branch(repo: &git2::Repository) -> Option<String> {
    let head = repo.head().ok()?;
    if !head.is_branch() {
        return None;
    }
    head.shorthand().map(str::to_owned)
}

pub fn checkout(repo_path: &str, name: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let (object, reference) = repo.revparse_ext(name)?;
    repo.checkout_tree(&object, None)?;
    match reference {
        Some(r) => {
            let ref_name = r
                .name()
                .ok_or_else(|| GitError::NonZero(format!("ref has no utf8 name: {name}")))?;
            repo.set_head(ref_name)?;
        }
        None => {
            // Detached HEAD checkout (commit SHA).
            repo.set_head_detached(object.id())?;
        }
    }
    Ok(())
}

/// Check out a remote-tracking branch (`origin/feature/x`) as a local branch
/// that tracks it. `local` defaults to the ref minus the remote prefix — the
/// same thing `git checkout feature/x` does implicitly.
pub fn checkout_remote(
    repo_path: &str,
    remote_ref: &str,
    local: Option<&str>,
) -> Result<String, GitError> {
    let repo = open(repo_path)?;
    let remote_branch = repo.find_branch(remote_ref, git2::BranchType::Remote)?;
    let commit = remote_branch.get().peel_to_commit()?;

    let local_name = match local {
        Some(n) => n.to_owned(),
        None => remote_ref
            .split_once('/')
            .map(|(_, rest)| rest.to_owned())
            .unwrap_or_else(|| remote_ref.to_owned()),
    };

    // Reuse an existing local branch — checking out `origin/x` twice should
    // land on `x`, not fail with "branch already exists".
    if repo
        .find_branch(&local_name, git2::BranchType::Local)
        .is_err()
    {
        let mut created = repo.branch(&local_name, &commit, false)?;
        created.set_upstream(Some(remote_ref))?;
    }
    checkout(repo_path, &local_name)?;
    Ok(local_name)
}

pub fn create_branch(
    repo_path: &str,
    name: &str,
    from: Option<&str>,
    checkout_after: bool,
) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let target = match from {
        Some(rev) => repo.revparse_single(rev)?.peel_to_commit()?,
        None => repo.head()?.peel_to_commit()?,
    };
    let mut created = repo.branch(name, &target, false)?;
    // Branching off a remote-tracking ref should track it, matching
    // `git switch -c x origin/x`.
    if let Some(rev) = from
        && repo.find_branch(rev, git2::BranchType::Remote).is_ok()
    {
        created.set_upstream(Some(rev))?;
    }
    if checkout_after {
        checkout(repo_path, name)?;
    }
    Ok(())
}

pub fn rename_branch(repo_path: &str, old: &str, new: &str, force: bool) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let mut b = repo.find_branch(old, git2::BranchType::Local)?;
    b.rename(new, force)?;
    Ok(())
}

pub fn delete_branch(repo_path: &str, name: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let mut b = repo.find_branch(name, git2::BranchType::Local)?;
    b.delete()?;
    Ok(())
}

/// Delete a remote-tracking ref locally (`origin/x`). Does *not* touch the
/// remote — deleting there is a push and lives in `remote::push_delete`.
pub fn delete_remote_tracking(repo_path: &str, name: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let mut b = repo.find_branch(name, git2::BranchType::Remote)?;
    b.delete()?;
    Ok(())
}

fn open(repo_path: &str) -> Result<git2::Repository, GitError> {
    git2::Repository::discover(repo_path).map_err(|_| GitError::NotARepo(repo_path.to_owned()))
}
