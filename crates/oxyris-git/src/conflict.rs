//! Three-way conflict inspection + resolution.
//!
//! When git records a conflict, the index keeps three entries for the
//! conflicted path:
//!
//!   - **stage 1** = common ancestor ("base")
//!   - **stage 2** = our side (HEAD)
//!   - **stage 3** = their side (the merge / cherry-pick / rebase target)
//!
//! `get_conflict` resolves those into UTF-8 strings (lossy on non-UTF8 files)
//! so the UI can render a 3-pane merge editor. `resolve` writes the user's
//! merged content to the working tree, removes the conflict entries from
//! the index, and stages the resolved file.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::GitError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictContents {
    pub path: String,
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
    /// Working-tree contents — usually has git's `<<<<<<<`/`=======`/`>>>>>>>`
    /// markers. Useful as a starting point for the result editor.
    pub workdir: Option<String>,
}

pub fn get_conflict(repo_path: &str, path: &str) -> Result<ConflictContents, GitError> {
    let repo = open(repo_path)?;
    let index = repo.index()?;
    let conflicts = index.conflicts()?;

    let mut base = None;
    let mut ours = None;
    let mut theirs = None;

    for entry in conflicts {
        let entry = entry?;
        if entry_path(&entry).as_deref() != Some(path) {
            continue;
        }
        if let Some(e) = entry.ancestor {
            base = Some(read_blob(&repo, e.id)?);
        }
        if let Some(e) = entry.our {
            ours = Some(read_blob(&repo, e.id)?);
        }
        if let Some(e) = entry.their {
            theirs = Some(read_blob(&repo, e.id)?);
        }
        break;
    }

    let workdir = repo.workdir().and_then(|wd| {
        std::fs::read(wd.join(path))
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    });

    Ok(ConflictContents {
        path: path.to_owned(),
        base,
        ours,
        theirs,
        workdir,
    })
}

pub fn resolve(repo_path: &str, path: &str, content: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| GitError::NotARepo("bare repo".into()))?;
    let abs = workdir.join(path);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&abs, content)?;

    let mut index = repo.index()?;
    index.remove_path(Path::new(path))?;
    index.add_path(Path::new(path))?;
    index.write()?;
    Ok(())
}

fn entry_path(entry: &git2::IndexConflict) -> Option<String> {
    entry
        .ancestor
        .as_ref()
        .or(entry.our.as_ref())
        .or(entry.their.as_ref())
        .map(|e| String::from_utf8_lossy(&e.path).into_owned())
}

fn read_blob(repo: &git2::Repository, oid: git2::Oid) -> Result<String, GitError> {
    let blob = repo.find_blob(oid)?;
    Ok(String::from_utf8_lossy(blob.content()).into_owned())
}

fn open(repo_path: &str) -> Result<git2::Repository, GitError> {
    git2::Repository::discover(repo_path).map_err(|_| GitError::NotARepo(repo_path.to_owned()))
}
