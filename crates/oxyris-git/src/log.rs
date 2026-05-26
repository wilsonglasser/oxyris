//! Git log via `git2::Revwalk`.

use serde::{Deserialize, Serialize};

use crate::error::GitError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub oid: String,
    pub short_oid: String,
    pub summary: String,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    /// Unix seconds.
    pub author_time: i64,
    pub parents: Vec<String>,
}

pub fn log(
    repo_path: &str,
    limit: usize,
    rev: Option<&str>,
    path: Option<&str>,
) -> Result<Vec<CommitInfo>, GitError> {
    let repo = git2::Repository::discover(repo_path)
        .map_err(|_| GitError::NotARepo(repo_path.to_owned()))?;
    let mut walk = repo.revwalk()?;
    match rev {
        Some(r) => {
            let oid = repo.revparse_single(r)?.peel_to_commit()?.id();
            walk.push(oid)?;
        }
        None => {
            // HEAD; if no HEAD (empty repo) return empty.
            if walk.push_head().is_err() {
                return Ok(Vec::new());
            }
        }
    }
    walk.set_sorting(git2::Sort::TIME)?;

    let mut out = Vec::new();
    for oid in walk {
        if out.len() >= limit {
            break;
        }
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
        // File history: keep only commits whose diff against the first parent
        // touched `path`. The root commit is compared against the empty tree.
        if let Some(p) = path
            && !commit_touches_path(&repo, &commit, p)?
        {
            continue;
        }
        let message = commit.message().unwrap_or("").to_owned();
        let summary = commit.summary().unwrap_or("").to_owned();
        let author = commit.author();
        out.push(CommitInfo {
            oid: oid.to_string(),
            short_oid: oid.to_string()[..oid.to_string().len().min(7)].to_owned(),
            summary,
            message,
            author_name: author.name().unwrap_or("").to_owned(),
            author_email: author.email().unwrap_or("").to_owned(),
            author_time: author.when().seconds(),
            parents: commit.parent_ids().map(|p| p.to_string()).collect(),
        });
    }
    Ok(out)
}

/// True when `commit`'s diff against its first parent (or the empty tree for
/// a root commit) contains at least one delta under `path`.
fn commit_touches_path(
    repo: &git2::Repository,
    commit: &git2::Commit,
    path: &str,
) -> Result<bool, GitError> {
    let new_tree = commit.tree()?;
    let old_tree = match commit.parent(0) {
        Ok(parent) => Some(parent.tree()?),
        Err(_) => None,
    };
    let mut opts = git2::DiffOptions::new();
    opts.pathspec(path);
    let diff = repo.diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), Some(&mut opts))?;
    Ok(diff.deltas().len() > 0)
}
