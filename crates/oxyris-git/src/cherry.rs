//! Cherry-pick + revert single commits.
//!
//! libgit2 has `cherrypick` and `revert` primitives that produce an in-memory
//! merge result. To match the user's mental model (the change is committed
//! immediately, leaving conflicts in the index when they happen), we wrap
//! the libgit2 ops with a follow-up commit — same shape as
//! `status::commit` but without amend support.

use crate::error::GitError;

pub fn cherry_pick(repo_path: &str, oid: &str) -> Result<Option<String>, GitError> {
    let repo = open(repo_path)?;
    let target = git2::Oid::from_str(oid)?;
    let commit = repo.find_commit(target)?;
    repo.cherrypick(&commit, None)?;
    finalize_commit(
        &repo,
        &format!("cherry-pick: {}", commit.summary().unwrap_or("")),
    )
}

pub fn revert(repo_path: &str, oid: &str) -> Result<Option<String>, GitError> {
    let repo = open(repo_path)?;
    let target = git2::Oid::from_str(oid)?;
    let commit = repo.find_commit(target)?;
    repo.revert(&commit, None)?;
    finalize_commit(
        &repo,
        &format!("revert: {}", commit.summary().unwrap_or("")),
    )
}

/// Stage all index changes from the cherry-pick / revert and finalize as a
/// new commit on HEAD. Returns `None` when the operation produced
/// conflicts (the index is left in a conflicted state for manual
/// resolution; the caller surfaces that as a status refresh).
fn finalize_commit(repo: &git2::Repository, message: &str) -> Result<Option<String>, GitError> {
    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Ok(None);
    }
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;
    let signature = repo
        .signature()
        .or_else(|_| git2::Signature::now("oxyris", "oxyris@local"))?;
    let parent = repo.head()?.peel_to_commit()?;
    let oid = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &[&parent],
    )?;
    // Clear the cherry-pick / revert state once the commit lands.
    repo.cleanup_state()?;
    Ok(Some(oid.to_string()))
}

fn open(repo_path: &str) -> Result<git2::Repository, GitError> {
    git2::Repository::discover(repo_path).map_err(|_| GitError::NotARepo(repo_path.to_owned()))
}
