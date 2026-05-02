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
        let wt = repo.find_worktree(name)?;
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

pub fn remove_worktree(repo_path: &str, name: &str) -> Result<(), GitError> {
    let repo = open_repo(Path::new(repo_path))?;
    let wt = repo.find_worktree(name)?;
    let path = wt.path().to_path_buf();
    let mut opts = git2::WorktreePruneOptions::new();
    opts.working_tree(true).valid(true);
    wt.prune(Some(&mut opts))?;
    if path.exists() {
        std::fs::remove_dir_all(&path).ok();
    }
    Ok(())
}

fn open_repo(path: &Path) -> Result<git2::Repository, GitError> {
    git2::Repository::discover(path).map_err(|_| GitError::NotARepo(path.display().to_string()))
}
