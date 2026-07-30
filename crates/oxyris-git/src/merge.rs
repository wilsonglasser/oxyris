//! Merge and rebase — the two ways a branch's work lands on another.
//!
//! Both are pure git2 so they work identically on Windows and inside the WSL
//! agent. Conflicts are *not* rolled back: the operation is left in progress
//! (`MERGE_HEAD` / `.git/rebase-merge`) with the conflicted entries in the
//! index, exactly like the CLI, so the existing merge editor + commit flow
//! can finish the job. `status::status` reports the in-progress state so the
//! UI can offer continue/abort.

use serde::{Deserialize, Serialize};

use crate::error::GitError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MergeOutcome {
    /// The target is already an ancestor of HEAD — nothing to do.
    UpToDate,
    /// HEAD was moved forward without a merge commit.
    FastForward { oid: String },
    /// A merge commit was created.
    Merged { oid: String },
    /// Left in progress with conflicts staged for resolution.
    Conflicts { paths: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RebaseOutcome {
    UpToDate,
    /// Every commit was replayed; `commits` were rewritten.
    Done {
        commits: u32,
    },
    /// Replay stopped on a conflicted commit. The rebase is still open —
    /// resolve, then call `rebase_continue` (or `rebase_abort`).
    Conflicts {
        paths: Vec<String>,
    },
}

/// Merge `name` (a branch, tag, or commit-ish) into the current HEAD.
///
/// `no_ff` forces a merge commit even when a fast-forward is possible —
/// the `--no-ff` most teams want on release branches.
pub fn merge(repo_path: &str, name: &str, no_ff: bool) -> Result<MergeOutcome, GitError> {
    let repo = open(repo_path)?;
    let (object, reference) = repo.revparse_ext(name)?;
    let annotated = match &reference {
        Some(r) => repo.reference_to_annotated_commit(r)?,
        None => repo.find_annotated_commit(object.id())?,
    };

    let (analysis, _) = repo.merge_analysis(&[&annotated])?;
    if analysis.is_up_to_date() {
        return Ok(MergeOutcome::UpToDate);
    }

    if analysis.is_unborn() {
        // Empty repo: HEAD is a symbolic ref to a branch that has no commit
        // yet. Create the branch at the target and check it out.
        let target = annotated.id();
        let head_ref_name = repo
            .find_reference("HEAD")?
            .symbolic_target()
            .map(str::to_owned)
            .ok_or_else(|| GitError::NonZero("HEAD is not symbolic".into()))?;
        repo.reference(&head_ref_name, target, true, "merge: unborn")?;
        repo.set_head(&head_ref_name)?;
        let obj = repo.find_object(target, None)?;
        repo.checkout_tree(&obj, Some(git2::build::CheckoutBuilder::new().force()))?;
        return Ok(MergeOutcome::FastForward {
            oid: target.to_string(),
        });
    }

    if analysis.is_fast_forward() && !no_ff {
        let target = annotated.id();
        let obj = repo.find_object(target, None)?;
        repo.checkout_tree(&obj, Some(git2::build::CheckoutBuilder::new().safe()))?;
        let mut head = repo.head()?;
        head.set_target(target, &format!("merge {name}: fast-forward"))?;
        return Ok(MergeOutcome::FastForward {
            oid: target.to_string(),
        });
    }

    // Real merge: libgit2 writes the merged index + working tree and records
    // MERGE_HEAD, so a conflicted result is resumable from the UI.
    repo.merge(&[&annotated], None, None)?;

    let mut index = repo.index()?;
    if index.has_conflicts() {
        return Ok(MergeOutcome::Conflicts {
            paths: conflict_paths(&index)?,
        });
    }

    let tree = repo.find_tree(index.write_tree()?)?;
    let signature = signature(&repo)?;
    let head_commit = repo.head()?.peel_to_commit()?;
    let their_commit = repo.find_commit(annotated.id())?;
    let message = format!("Merge {name} into {}", current_name(&repo));
    let oid = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        &message,
        &tree,
        &[&head_commit, &their_commit],
    )?;
    repo.cleanup_state()?;
    Ok(MergeOutcome::Merged {
        oid: oid.to_string(),
    })
}

/// Abandon an in-progress merge / cherry-pick / revert: discard the working
/// tree and index changes it produced and clear the sequencer state.
pub fn merge_abort(repo_path: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let head = repo.head()?.peel(git2::ObjectType::Commit)?;
    repo.reset(&head, git2::ResetType::Hard, None)?;
    repo.cleanup_state()?;
    Ok(())
}

/// Replay the current branch's commits on top of `upstream`.
pub fn rebase(repo_path: &str, upstream: &str) -> Result<RebaseOutcome, GitError> {
    let repo = open(repo_path)?;
    let (object, reference) = repo.revparse_ext(upstream)?;
    let onto = match &reference {
        Some(r) => repo.reference_to_annotated_commit(r)?,
        None => repo.find_annotated_commit(object.id())?,
    };

    // Nothing to replay when upstream is already an ancestor of HEAD.
    let head_oid = repo.head()?.peel_to_commit()?.id();
    let (_ahead, behind) = repo.graph_ahead_behind(head_oid, onto.id())?;
    if behind == 0 {
        return Ok(RebaseOutcome::UpToDate);
    }

    let mut rb = repo.rebase(None, Some(&onto), None, None)?;
    drive(&repo, &mut rb)
}

/// Resume a rebase that stopped on conflicts. Expects the resolution to be
/// staged already (the merge editor's "mark resolved" does that).
pub fn rebase_continue(repo_path: &str) -> Result<RebaseOutcome, GitError> {
    let repo = open(repo_path)?;
    let mut rb = repo.open_rebase(None)?;

    let index = repo.index()?;
    if index.has_conflicts() {
        return Ok(RebaseOutcome::Conflicts {
            paths: conflict_paths(&index)?,
        });
    }
    drop(index);

    // Commit the operation the rebase stopped on, then keep replaying.
    let signature = signature(&repo)?;
    match rb.commit(None, &signature, None) {
        Ok(_) => {}
        // Nothing left after the resolution (the change was already upstream).
        Err(e) if e.code() == git2::ErrorCode::Applied => {}
        Err(e) => return Err(e.into()),
    }
    drive(&repo, &mut rb)
}

pub fn rebase_abort(repo_path: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let mut rb = repo.open_rebase(None)?;
    rb.abort()?;
    Ok(())
}

/// Replay operations until the rebase finishes or hits a conflict.
fn drive(repo: &git2::Repository, rb: &mut git2::Rebase<'_>) -> Result<RebaseOutcome, GitError> {
    let signature = signature(repo)?;
    let mut replayed = 0u32;
    loop {
        // The `Operation` borrows `rb`, so it must be dropped before the
        // `rb.commit()` below — hence the immediately-collapsed match.
        let has_next = match rb.next() {
            Some(op) => {
                op?;
                true
            }
            None => false,
        };
        if !has_next {
            break;
        }
        let index = repo.index()?;
        if index.has_conflicts() {
            return Ok(RebaseOutcome::Conflicts {
                paths: conflict_paths(&index)?,
            });
        }
        drop(index);
        match rb.commit(None, &signature, None) {
            Ok(_) => replayed += 1,
            // Commit became empty (its change is already upstream) — skip it,
            // which is what `git rebase` does by default.
            Err(e) if e.code() == git2::ErrorCode::Applied => {}
            Err(e) => return Err(e.into()),
        }
    }
    rb.finish(Some(&signature))?;
    Ok(RebaseOutcome::Done { commits: replayed })
}

fn conflict_paths(index: &git2::Index) -> Result<Vec<String>, GitError> {
    let mut out = Vec::new();
    for entry in index.conflicts()? {
        let entry = entry?;
        let path = entry
            .our
            .as_ref()
            .or(entry.their.as_ref())
            .or(entry.ancestor.as_ref())
            .map(|e| String::from_utf8_lossy(&e.path).into_owned());
        if let Some(p) = path
            && !out.contains(&p)
        {
            out.push(p);
        }
    }
    Ok(out)
}

fn current_name(repo: &git2::Repository) -> String {
    repo.head()
        .ok()
        .and_then(|h| h.shorthand().map(str::to_owned))
        .unwrap_or_else(|| "HEAD".to_owned())
}

fn signature(repo: &git2::Repository) -> Result<git2::Signature<'static>, GitError> {
    Ok(repo
        .signature()
        .or_else(|_| git2::Signature::now("oxyris", "oxyris@local"))?)
}

fn open(repo_path: &str) -> Result<git2::Repository, GitError> {
    git2::Repository::discover(repo_path).map_err(|_| GitError::NotARepo(repo_path.to_owned()))
}
