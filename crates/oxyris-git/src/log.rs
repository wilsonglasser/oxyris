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

pub fn log(repo_path: &str, limit: usize, rev: Option<&str>) -> Result<Vec<CommitInfo>, GitError> {
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
    for (i, oid) in walk.enumerate() {
        if i >= limit {
            break;
        }
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
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
