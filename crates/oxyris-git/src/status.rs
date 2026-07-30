//! Working-tree status, per-file diffs, staging and commit ops.
//!
//! All pure git2 — no shellouts. Repos are opened via `Repository::discover`
//! so callers can pass either the repo root or any subdirectory inside the
//! worktree.

use std::path::Path;

use crate::error::GitError;
use crate::types::{
    AheadBehind, CommitResult, DiffMode, FileDiff, FileStatus, RepoState, StatusBucket,
    StatusEntry, StatusReport,
};

pub fn status(repo_path: &str) -> Result<StatusReport, GitError> {
    let repo = open(repo_path)?;
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);

    let statuses = repo.statuses(Some(&mut opts))?;
    let mut entries: Vec<StatusEntry> = Vec::new();

    for s in statuses.iter() {
        let flags = s.status();
        let path = s.path().unwrap_or("").to_owned();
        if path.is_empty() {
            continue;
        }

        if flags.is_conflicted() {
            entries.push(StatusEntry {
                path: path.clone(),
                old_path: None,
                bucket: StatusBucket::Conflicted,
                status: FileStatus::Modified,
            });
            // Conflicted files don't get a separate staged/unstaged entry —
            // they need resolution first.
            continue;
        }

        // Index vs HEAD (the "staged" side).
        let staged = bucket_for_index(flags);
        if let Some(status) = staged {
            let old_path = s
                .head_to_index()
                .and_then(|d| d.old_file().path())
                .map(|p| p.to_string_lossy().into_owned())
                .filter(|p| p.as_str() != path.as_str());
            entries.push(StatusEntry {
                path: path.clone(),
                old_path,
                bucket: StatusBucket::Staged,
                status,
            });
        }

        // Workdir vs index (the "unstaged" side) + untracked.
        if flags.is_wt_new() {
            entries.push(StatusEntry {
                path: path.clone(),
                old_path: None,
                bucket: StatusBucket::Untracked,
                status: FileStatus::Added,
            });
        } else if let Some(status) = bucket_for_workdir(flags) {
            let old_path = s
                .index_to_workdir()
                .and_then(|d| d.old_file().path())
                .map(|p| p.to_string_lossy().into_owned())
                .filter(|p| p.as_str() != path.as_str());
            entries.push(StatusEntry {
                path: path.clone(),
                old_path,
                bucket: StatusBucket::Unstaged,
                status,
            });
        }
    }

    let branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(String::from));
    let ahead_behind = ahead_behind(&repo).ok().flatten();

    Ok(StatusReport {
        entries,
        branch,
        ahead_behind,
        state: RepoState::from(repo.state()),
    })
}

fn bucket_for_index(s: git2::Status) -> Option<FileStatus> {
    if s.is_index_new() {
        Some(FileStatus::Added)
    } else if s.is_index_modified() {
        Some(FileStatus::Modified)
    } else if s.is_index_deleted() {
        Some(FileStatus::Deleted)
    } else if s.is_index_renamed() {
        Some(FileStatus::Renamed)
    } else if s.is_index_typechange() {
        Some(FileStatus::Typechange)
    } else {
        None
    }
}

fn bucket_for_workdir(s: git2::Status) -> Option<FileStatus> {
    if s.is_wt_modified() {
        Some(FileStatus::Modified)
    } else if s.is_wt_deleted() {
        Some(FileStatus::Deleted)
    } else if s.is_wt_renamed() {
        Some(FileStatus::Renamed)
    } else if s.is_wt_typechange() {
        Some(FileStatus::Typechange)
    } else {
        None
    }
}

/// OIDs recorded in `.git/MERGE_HEAD` — the "theirs" side(s) of an in-progress
/// merge. Empty when no merge is pending. Read from the file rather than
/// `mergehead_foreach`, which needs `&mut Repository` and would collide with
/// the index/tree borrows the caller is already holding.
fn merge_heads(repo: &git2::Repository) -> Result<Vec<git2::Oid>, GitError> {
    if repo.state() != git2::RepositoryState::Merge {
        return Ok(Vec::new());
    }
    let raw = match std::fs::read_to_string(repo.path().join("MERGE_HEAD")) {
        Ok(s) => s,
        Err(_) => return Ok(Vec::new()),
    };
    Ok(raw
        .lines()
        .filter_map(|line| git2::Oid::from_str(line.trim()).ok())
        .collect())
}

fn ahead_behind(repo: &git2::Repository) -> Result<Option<AheadBehind>, GitError> {
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return Ok(None),
    };
    let local = match head.target() {
        Some(o) => o,
        None => return Ok(None),
    };
    let shorthand = match head.shorthand() {
        Some(s) => s.to_owned(),
        None => return Ok(None),
    };
    let branch = match repo.find_branch(&shorthand, git2::BranchType::Local) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    let upstream = match branch.upstream() {
        Ok(u) => u,
        Err(_) => return Ok(None),
    };
    let Some(remote) = upstream.into_reference().target() else {
        return Ok(None);
    };
    let (ahead, behind) = repo.graph_ahead_behind(local, remote)?;
    Ok(Some(AheadBehind {
        ahead: ahead as u32,
        behind: behind as u32,
    }))
}

// ────── single-file diff ───────────────────────────────────────────────────

pub fn diff_file(repo_path: &str, path: &str, mode: DiffMode) -> Result<FileDiff, GitError> {
    let repo = open(repo_path)?;
    let mut opts = git2::DiffOptions::new();
    opts.pathspec(path)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_binary(false)
        .context_lines(3);

    let diff = match mode {
        DiffMode::WorkingVsHead => {
            let head_tree = head_tree(&repo)?;
            repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))?
        }
        DiffMode::StagedVsHead => {
            let head_tree = head_tree(&repo)?;
            repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))?
        }
        DiffMode::WorkingVsStaged => repo.diff_index_to_workdir(None, Some(&mut opts))?,
    };

    let mut out = FileDiff {
        path: path.to_owned(),
        old_path: None,
        status: FileStatus::Unchanged,
        old_content: None,
        new_content: None,
        unified: String::new(),
    };

    for delta in diff.deltas() {
        let new_path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if new_path != path {
            // Pathspec usually filters this out, but be defensive.
            continue;
        }
        out.status = FileStatus::from(delta.status());
        let old_path = delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned());
        out.old_path = old_path.filter(|p| p.as_str() != new_path.as_str());

        out.old_content = if delta.old_file().id().is_zero() {
            None
        } else {
            read_blob(&repo, delta.old_file().id()).ok()
        };

        // For the new side: if it's an in-workdir file (modes that include
        // workdir), read from disk so we surface unsaved-but-on-disk content
        // rather than a stale blob id.
        out.new_content = match mode {
            DiffMode::WorkingVsHead | DiffMode::WorkingVsStaged => {
                read_workdir(&repo, &new_path).ok()
            }
            DiffMode::StagedVsHead => {
                if delta.new_file().id().is_zero() {
                    None
                } else {
                    read_blob(&repo, delta.new_file().id()).ok()
                }
            }
        };
    }

    let mut buf = String::new();
    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        let p = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if p != path {
            return true;
        }
        let origin = line.origin();
        if matches!(origin, '+' | '-' | ' ') {
            buf.push(origin);
        }
        buf.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;
    out.unified = buf;

    Ok(out)
}

// ────── stage / unstage ────────────────────────────────────────────────────

pub fn stage(repo_path: &str, paths: &[String]) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let mut index = repo.index()?;
    // `add_all` handles new + modified + deleted in one shot. For deletes
    // we need `update_all` so the entry is removed from the index instead
    // of re-added — but `add_all` with file specs does the right thing on
    // current git2, including marking deletes.
    let pathspecs: Vec<&str> = paths.iter().map(String::as_str).collect();
    if pathspecs.is_empty() {
        return Ok(());
    }
    index.add_all(pathspecs.iter(), git2::IndexAddOption::DEFAULT, None)?;
    // Some deletes are missed by `add_all` when the file is gone from
    // workdir — `update_all` catches those.
    index.update_all(pathspecs.iter(), None)?;
    index.write()?;
    Ok(())
}

pub fn unstage(repo_path: &str, paths: &[String]) -> Result<(), GitError> {
    let repo = open(repo_path)?;
    let head = match repo.head() {
        Ok(h) => Some(h.peel_to_commit()?),
        Err(_) => None,
    };
    let pathspecs: Vec<&std::path::Path> = paths.iter().map(Path::new).collect();
    if pathspecs.is_empty() {
        return Ok(());
    }
    if let Some(commit) = head {
        repo.reset_default(Some(commit.as_object()), pathspecs.iter())?;
    } else {
        // No HEAD yet (initial commit) — drop the entry from the index.
        let mut index = repo.index()?;
        for p in &pathspecs {
            let _ = index.remove_path(p);
        }
        index.write()?;
    }
    Ok(())
}

// ────── commit ─────────────────────────────────────────────────────────────

pub fn commit(repo_path: &str, message: &str, amend: bool) -> Result<CommitResult, GitError> {
    let repo = open(repo_path)?;
    let signature = repo
        .signature()
        .or_else(|_| git2::Signature::now("oxyris", "oxyris@local"))?;

    let mut index = repo.index()?;
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;

    let head = repo.head().ok();
    let mut parents: Vec<git2::Commit> = match (&head, amend) {
        (Some(h), true) => {
            // Amend: replace HEAD's commit, keep its parents.
            let head_commit = h.peel_to_commit()?;
            head_commit.parents().collect()
        }
        (Some(h), false) => vec![h.peel_to_commit()?],
        (None, _) => Vec::new(),
    };
    // A commit that finishes a conflicted merge must keep the second parent,
    // otherwise the merge is recorded as a plain commit and MERGE_HEAD is left
    // dangling. `merge_heads` is empty for every non-merge commit.
    if !amend {
        for oid in merge_heads(&repo)? {
            parents.push(repo.find_commit(oid)?);
        }
    }
    let parent_refs: Vec<&git2::Commit> = parents.iter().collect();

    let oid = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parent_refs,
    )?;
    // Clears MERGE_HEAD / CHERRY_PICK_HEAD and friends once the commit lands.
    if repo.state() != git2::RepositoryState::Clean {
        repo.cleanup_state()?;
    }

    let branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(String::from));

    Ok(CommitResult {
        oid: oid.to_string(),
        message: message.to_owned(),
        branch,
    })
}

// ────── diff between two revisions (or rev → workdir) ─────────────────────

/// Diff every file changed between `from` and `to`. Either side can be a
/// commit SHA, branch name, tag, or `"WORKTREE"` to mean the working tree
/// (the same shape `git diff <rev>` uses). Pass `find_renames` to run
/// libgit2's similarity matcher so the result surfaces renames as renames
/// instead of delete + add.
pub fn diff_revs(
    repo_path: &str,
    from: &str,
    to: &str,
    find_renames: bool,
) -> Result<Vec<FileDiff>, GitError> {
    let repo = open(repo_path)?;
    let mut opts = git2::DiffOptions::new();
    opts.show_binary(false).context_lines(3);

    let diff = match (from, to) {
        ("WORKTREE", _) | (_, "WORKTREE") => {
            // Workdir comparisons go through `diff_tree_to_workdir_with_index`
            // so unstaged + staged + untracked all show up consistently.
            let rev = if from == "WORKTREE" { to } else { from };
            let tree = repo.revparse_single(rev)?.peel_to_tree()?;
            opts.include_untracked(true).recurse_untracked_dirs(true);
            repo.diff_tree_to_workdir_with_index(Some(&tree), Some(&mut opts))?
        }
        _ => {
            let from_tree = repo.revparse_single(from)?.peel_to_tree()?;
            let to_tree = repo.revparse_single(to)?.peel_to_tree()?;
            repo.diff_tree_to_tree(Some(&from_tree), Some(&to_tree), Some(&mut opts))?
        }
    };

    let mut diff = diff;
    if find_renames {
        let mut find_opts = git2::DiffFindOptions::new();
        find_opts.renames(true).copies(true);
        diff.find_similar(Some(&mut find_opts))?;
    }

    let mut files: Vec<FileDiff> = Vec::new();
    for delta in diff.deltas() {
        let new_path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let old_path = delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned());
        let renamed_old = old_path.filter(|p| p.as_str() != new_path.as_str());

        let old_content = if delta.old_file().id().is_zero() {
            None
        } else {
            read_blob(&repo, delta.old_file().id()).ok()
        };
        let new_content = if delta.new_file().id().is_zero() {
            None
        } else {
            read_blob(&repo, delta.new_file().id()).ok()
        };

        files.push(FileDiff {
            path: new_path,
            old_path: renamed_old,
            status: FileStatus::from(delta.status()),
            old_content,
            new_content,
            unified: String::new(),
        });
    }

    let mut buffers: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        let path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let buf = buffers.entry(path).or_default();
        let origin = line.origin();
        if matches!(origin, '+' | '-' | ' ') {
            buf.push(origin);
        }
        buf.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;
    for f in &mut files {
        if let Some(text) = buffers.remove(&f.path) {
            f.unified = text;
        }
    }

    Ok(files)
}

// ────── apply patch (hunk-level stage / unstage) ───────────────────────────

/// Apply a unified-diff patch to the index. Used for hunk-level staging.
/// Shells out to the `git` binary because libgit2's apply API is surprisingly
/// awkward and doesn't handle `--cached` cleanly.
pub fn apply_patch(
    repo_path: &str,
    patch: &str,
    reverse: bool,
    cached: bool,
) -> Result<(), GitError> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use oxyris_procutil::HideConsole;

    let mut args: Vec<&str> = vec!["-C", repo_path, "apply"];
    if cached {
        args.push("--cached");
    }
    if reverse {
        args.push("--reverse");
    }
    args.push("--unidiff-zero");
    args.push("--whitespace=nowarn");
    args.push("-");

    let mut child = Command::new("git")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .hide_console()
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(patch.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(GitError::NonZero(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    Ok(())
}

// ────── helpers ────────────────────────────────────────────────────────────

fn open(repo_path: &str) -> Result<git2::Repository, GitError> {
    git2::Repository::discover(repo_path).map_err(|_| GitError::NotARepo(repo_path.to_owned()))
}

fn head_tree(repo: &git2::Repository) -> Result<Option<git2::Tree<'_>>, GitError> {
    match repo.head() {
        Ok(h) => Ok(Some(h.peel_to_tree()?)),
        Err(_) => Ok(None), // empty repo
    }
}

fn read_blob(repo: &git2::Repository, oid: git2::Oid) -> Result<String, GitError> {
    let blob = repo.find_blob(oid)?;
    Ok(String::from_utf8_lossy(blob.content()).into_owned())
}

fn read_workdir(repo: &git2::Repository, rel: &str) -> Result<String, GitError> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| GitError::NotARepo("bare repo".into()))?;
    let bytes = std::fs::read(workdir.join(rel))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
