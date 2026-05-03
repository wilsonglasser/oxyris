//! Tag management — list, create (lightweight + annotated), delete.

use serde::{Deserialize, Serialize};

use crate::error::GitError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagInfo {
    pub name: String,
    /// Resolved commit OID (annotated tags are peeled).
    pub oid: String,
    /// Annotation message when present.
    pub message: Option<String>,
    pub annotated: bool,
}

pub fn list(repo_path: &str) -> Result<Vec<TagInfo>, GitError> {
    let repo = open(repo_path)?;
    let mut out = Vec::new();
    repo.tag_foreach(|oid, name_bytes| {
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        // `name` is `refs/tags/<short>` — strip the prefix.
        let short = name.strip_prefix("refs/tags/").unwrap_or(&name).to_owned();
        let (oid_resolved, message, annotated) = match repo.find_tag(oid) {
            Ok(tag) => {
                let target = tag.target_id();
                (target.to_string(), tag.message().map(str::to_owned), true)
            }
            Err(_) => (oid.to_string(), None, false),
        };
        out.push(TagInfo {
            name: short,
            oid: oid_resolved,
            message,
            annotated,
        });
        true
    })?;
    Ok(out)
}

pub fn create(
    repo_path: &str,
    name: &str,
    target: Option<&str>,
    message: Option<&str>,
    force: bool,
) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let target_obj = match target {
        Some(rev) => repo.revparse_single(rev)?,
        None => repo.head()?.peel(git2::ObjectType::Any)?,
    };
    match message {
        Some(msg) if !msg.is_empty() => {
            let signature = repo
                .signature()
                .or_else(|_| git2::Signature::now("oxyris", "oxyris@local"))?;
            repo.tag(name, &target_obj, &signature, msg, force)?;
        }
        _ => {
            repo.tag_lightweight(name, &target_obj, force)?;
        }
    }
    Ok(())
}

pub fn delete(repo_path: &str, name: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    repo.tag_delete(name)?;
    Ok(())
}

fn open(repo_path: &str) -> Result<git2::Repository, GitError> {
    git2::Repository::discover(repo_path).map_err(|_| GitError::NotARepo(repo_path.to_owned()))
}
