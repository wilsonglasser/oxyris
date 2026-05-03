//! Stash management — list / save / apply / pop / drop.
//!
//! libgit2's stash API needs a mutable repo handle (the `&mut self` flavor
//! on `Repository`), so we route everything through `git2::Repository::open`
//! after `discover` resolves the worktree root.

use serde::{Deserialize, Serialize};

use crate::error::GitError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StashEntry {
    /// Stash position (0 = most recent).
    pub index: u32,
    /// `stash@{N}` short id.
    pub short_id: String,
    /// Underlying commit OID.
    pub oid: String,
    /// First line of the stash message.
    pub message: String,
    /// Unix seconds when the stash was created.
    pub time: i64,
}

pub fn list(repo_path: &str) -> Result<Vec<StashEntry>, GitError> {
    let mut repo = open(repo_path)?;
    // Collect (index, message, oid) tuples first; `stash_foreach` borrows
    // `repo` mutably so we can't peek at commit times in the closure.
    let mut raw: Vec<(u32, String, git2::Oid)> = Vec::new();
    repo.stash_foreach(|index, msg, oid| {
        raw.push((index as u32, msg.to_owned(), *oid));
        true
    })?;

    let repo_ro = git2::Repository::discover(repo_path)
        .map_err(|_| GitError::NotARepo(repo_path.to_owned()))?;
    let mut out = Vec::with_capacity(raw.len());
    for (index, message, oid) in raw {
        let time = repo_ro
            .find_commit(oid)
            .map(|c| c.time().seconds())
            .unwrap_or(0);
        out.push(StashEntry {
            index,
            short_id: format!("stash@{{{index}}}"),
            oid: oid.to_string(),
            message,
            time,
        });
    }
    Ok(out)
}

pub fn save(repo_path: &str, message: &str, include_untracked: bool) -> Result<String, GitError> {
    let mut repo = open(repo_path)?;
    let signature = repo
        .signature()
        .or_else(|_| git2::Signature::now("oxyris", "oxyris@local"))?;
    let mut flags = git2::StashFlags::DEFAULT;
    if include_untracked {
        flags |= git2::StashFlags::INCLUDE_UNTRACKED;
    }
    let oid = repo.stash_save2(&signature, Some(message), Some(flags))?;
    Ok(oid.to_string())
}

pub fn apply(repo_path: &str, index: u32, drop_after: bool) -> Result<(), GitError> {
    let mut repo = open(repo_path)?;
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.safe();
    let mut opts = git2::StashApplyOptions::default();
    opts.checkout_options(checkout);
    repo.stash_apply(index as usize, Some(&mut opts))?;
    if drop_after {
        repo.stash_drop(index as usize)?;
    }
    Ok(())
}

pub fn drop(repo_path: &str, index: u32) -> Result<(), GitError> {
    let mut repo = open(repo_path)?;
    repo.stash_drop(index as usize)?;
    Ok(())
}

fn open(repo_path: &str) -> Result<git2::Repository, GitError> {
    git2::Repository::discover(repo_path).map_err(|_| GitError::NotARepo(repo_path.to_owned()))
}
