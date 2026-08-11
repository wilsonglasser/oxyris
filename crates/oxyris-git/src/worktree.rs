use std::path::Path;

use crate::error::GitError;
use crate::types::{BranchInfo, WorktreeRef};

pub fn list_branches(repo_path: &str) -> Result<Vec<BranchInfo>, GitError> {
    let repo = open_repo(Path::new(repo_path))?;
    let current = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(str::to_owned));
    let mut out = Vec::new();
    for b in repo.branches(None)? {
        let (branch, t) = b?;
        let name = branch.name()?.unwrap_or("").to_owned();
        if name.is_empty() {
            continue;
        }
        let is_remote = matches!(t, git2::BranchType::Remote);
        let is_current = Some(&name) == current.as_ref();
        out.push(BranchInfo {
            name,
            is_current,
            is_remote,
        });
    }
    Ok(out)
}

pub fn list_worktrees(repo_path: &str) -> Result<Vec<WorktreeRef>, GitError> {
    let repo = open_repo(Path::new(repo_path))?;
    let mut out = Vec::new();

    // Primary tree.
    let primary_path = repo
        .workdir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let primary_branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(str::to_owned));
    out.push(WorktreeRef {
        name: "primary".into(),
        path: primary_path,
        branch: primary_branch,
        is_primary: true,
    });

    for name in repo.worktrees()?.iter().flatten() {
        // Skip entries libgit2 can't look up. A half-deleted admin dir (user
        // ran `rm -rf` on `.git/worktrees/<name>` or on the checkout) makes
        // `find_worktree` fail; propagating would blank the whole list and
        // leave the UI unable to remove anything.
        let Ok(wt) = repo.find_worktree(name) else {
            continue;
        };
        let path = wt.path().to_string_lossy().into_owned();
        let branch = git2::Repository::open(wt.path())
            .ok()
            .and_then(|wt_repo| wt_repo.head().ok()?.shorthand().map(str::to_owned));
        out.push(WorktreeRef {
            name: name.to_owned(),
            path,
            branch,
            is_primary: false,
        });
    }
    Ok(out)
}

pub fn create_worktree(
    repo_path: &str,
    name: &str,
    branch: &str,
    target_dir: &str,
) -> Result<WorktreeRef, GitError> {
    let repo = open_repo(Path::new(repo_path))?;
    let mut opts = git2::WorktreeAddOptions::new();

    let reference = match repo.find_reference(&format!("refs/heads/{branch}")) {
        Ok(r) => r,
        Err(_) => {
            let head = repo.head().map_err(|e| match e.code() {
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound => GitError::EmptyRepo,
                _ => GitError::Git2(e),
            })?;
            let head_commit = head.peel_to_commit()?;
            let new_branch = repo.branch(branch, &head_commit, false)?;
            new_branch.into_reference()
        }
    };
    opts.reference(Some(&reference));

    let target = Path::new(target_dir);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let wt = repo.worktree(name, target, Some(&opts))?;
    Ok(WorktreeRef {
        name: name.to_owned(),
        path: wt.path().to_string_lossy().into_owned(),
        branch: Some(branch.to_owned()),
        is_primary: false,
    })
}

/// Removes a worktree, tolerating one whose directory is already gone.
///
/// Removal has to be idempotent: users delete worktree directories outside the
/// app, and libgit2 can fail on both `find_worktree` and `prune` for a stale
/// entry (it stats files under the admin dir). Failing there would leave a row
/// the UI can never get rid of, so any git2 failure falls back to deleting the
/// admin dir (`<commondir>/worktrees/<name>`) by hand — which is all `git
/// worktree prune` does anyway.
pub fn remove_worktree(repo_path: &str, name: &str) -> Result<(), GitError> {
    let repo = open_repo(Path::new(repo_path))?;
    // `commondir`, not `path`: the admin dirs live in the main repo's `.git`
    // even when `repo_path` resolved to a linked worktree.
    let admin_dir = repo.commondir().join("worktrees").join(name);
    // Resolve the checkout path from git metadata before anything is deleted;
    // `find_worktree` may be unusable, so fall back to the admin dir's `gitdir`
    // file, which holds `<checkout>/.git`.
    let found = repo.find_worktree(name).ok();
    let path = found
        .as_ref()
        .map(|wt| wt.path().to_path_buf())
        .or_else(|| gitdir_target(&admin_dir));

    let pruned = match found {
        Some(wt) => {
            let mut opts = git2::WorktreePruneOptions::new();
            opts.working_tree(true).valid(true);
            wt.prune(Some(&mut opts)).is_ok()
        }
        None => false,
    };
    if !pruned && admin_dir.exists() {
        std::fs::remove_dir_all(&admin_dir)?;
    }

    if let Some(path) = path
        && path.exists()
    {
        std::fs::remove_dir_all(&path).ok();
    }
    Ok(())
}

/// Reads `<admin_dir>/gitdir` (`<checkout>/.git`) and returns the checkout path.
fn gitdir_target(admin_dir: &Path) -> Option<std::path::PathBuf> {
    let raw = std::fs::read_to_string(admin_dir.join("gitdir")).ok()?;
    Path::new(raw.trim()).parent().map(Path::to_path_buf)
}

fn open_repo(path: &Path) -> Result<git2::Repository, GitError> {
    git2::Repository::discover(path).map_err(|_| GitError::NotARepo(path.display().to_string()))
}
