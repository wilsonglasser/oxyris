//! Branch operations: checkout, create, delete.
//!
//! Pure git2. `checkout` runs a "safe" checkout — refuses if the working
//! tree has uncommitted changes that would conflict. Use the dedicated
//! stash/commit flow first if the user wants to switch with dirty workdir.

use crate::error::GitError;

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
    repo.branch(name, &target, false)?;
    if checkout_after {
        checkout(repo_path, name)?;
    }
    Ok(())
}

pub fn delete_branch(repo_path: &str, name: &str) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let mut b = repo.find_branch(name, git2::BranchType::Local)?;
    b.delete()?;
    Ok(())
}

fn open(repo_path: &str) -> Result<git2::Repository, GitError> {
    git2::Repository::discover(repo_path).map_err(|_| GitError::NotARepo(repo_path.to_owned()))
}
