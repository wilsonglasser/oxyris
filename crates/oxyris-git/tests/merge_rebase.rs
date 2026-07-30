//! End-to-end merge / rebase behaviour against real temp repositories.
//!
//! These cover the paths the branch manager drives: fast-forward, true merge,
//! conflicted merge (must stay resumable and produce a two-parent commit once
//! resolved), rebase replay, and rebase abort.

use std::fs;
use std::path::Path;

use oxyris_git::merge::{MergeOutcome, RebaseOutcome};
use oxyris_git::types::RepoState;
use oxyris_git::{branch, conflict, merge, status};

/// Repo with one commit containing `base.txt`, on branch `main`.
fn init_repo(dir: &Path) -> git2::Repository {
    let repo = git2::Repository::init(dir).unwrap();
    {
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Tester").unwrap();
        cfg.set_str("user.email", "tester@example.com").unwrap();
    }
    write(dir, "base.txt", "base\n");
    commit_all(&repo, "init");
    // `git2::Repository::init` may land on `master` depending on the host
    // config; normalise so the tests can talk about `main`.
    let head = repo.head().unwrap().shorthand().unwrap().to_owned();
    if head != "main" {
        branch::rename_branch(dir.to_str().unwrap(), &head, "main", true).unwrap();
    }
    repo
}

fn write(dir: &Path, rel: &str, contents: &str) {
    fs::write(dir.join(rel), contents).unwrap();
}

fn commit_all(repo: &git2::Repository, message: &str) -> git2::Oid {
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = repo.signature().unwrap();
    let parents: Vec<git2::Commit> = match repo.head() {
        Ok(h) => vec![h.peel_to_commit().unwrap()],
        Err(_) => Vec::new(),
    };
    let refs: Vec<&git2::Commit> = parents.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &refs)
        .unwrap()
}

fn head_commit(repo: &git2::Repository) -> git2::Commit<'_> {
    repo.head().unwrap().peel_to_commit().unwrap()
}

#[test]
fn merge_fast_forwards_when_head_is_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let repo = init_repo(dir);
    let path = dir.to_str().unwrap();

    branch::create_branch(path, "feature", None, true).unwrap();
    write(dir, "feature.txt", "f\n");
    let feature_tip = commit_all(&repo, "feature work");

    branch::checkout(path, "main").unwrap();
    let outcome = merge::merge(path, "feature", false).unwrap();

    assert_eq!(
        outcome,
        MergeOutcome::FastForward {
            oid: feature_tip.to_string()
        }
    );
    assert_eq!(head_commit(&repo).id(), feature_tip);
    assert!(dir.join("feature.txt").exists());
    assert_eq!(repo.state(), git2::RepositoryState::Clean);
}

#[test]
fn merge_creates_a_two_parent_commit_when_both_sides_moved() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let repo = init_repo(dir);
    let path = dir.to_str().unwrap();

    branch::create_branch(path, "feature", None, true).unwrap();
    write(dir, "feature.txt", "f\n");
    commit_all(&repo, "feature work");

    branch::checkout(path, "main").unwrap();
    write(dir, "main.txt", "m\n");
    commit_all(&repo, "main work");

    let outcome = merge::merge(path, "feature", false).unwrap();

    assert!(
        matches!(outcome, MergeOutcome::Merged { .. }),
        "{outcome:?}"
    );
    assert_eq!(head_commit(&repo).parent_count(), 2);
    // Both sides are present and the sequencer state is cleared.
    assert!(dir.join("feature.txt").exists());
    assert!(dir.join("main.txt").exists());
    assert_eq!(repo.state(), git2::RepositoryState::Clean);
}

#[test]
fn merge_with_no_ff_forces_a_merge_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let repo = init_repo(dir);
    let path = dir.to_str().unwrap();

    branch::create_branch(path, "feature", None, true).unwrap();
    write(dir, "feature.txt", "f\n");
    commit_all(&repo, "feature work");
    branch::checkout(path, "main").unwrap();

    let outcome = merge::merge(path, "feature", true).unwrap();

    assert!(
        matches!(outcome, MergeOutcome::Merged { .. }),
        "{outcome:?}"
    );
    assert_eq!(head_commit(&repo).parent_count(), 2);
}

#[test]
fn conflicted_merge_stays_resumable_and_commits_with_two_parents() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let repo = init_repo(dir);
    let path = dir.to_str().unwrap();

    branch::create_branch(path, "feature", None, true).unwrap();
    write(dir, "base.txt", "theirs\n");
    commit_all(&repo, "feature edit");

    branch::checkout(path, "main").unwrap();
    write(dir, "base.txt", "ours\n");
    commit_all(&repo, "main edit");

    let outcome = merge::merge(path, "feature", false).unwrap();
    let MergeOutcome::Conflicts { paths } = outcome else {
        panic!("expected conflicts, got {outcome:?}");
    };
    assert_eq!(paths, vec!["base.txt".to_owned()]);

    // The panel sees the in-progress merge and one conflicted entry.
    let report = status::status(path).unwrap();
    assert_eq!(report.state, RepoState::Merge);
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.path == "base.txt" && e.bucket == oxyris_git::StatusBucket::Conflicted)
    );

    // Resolving + committing finishes the merge as a real merge commit.
    conflict::resolve(path, "base.txt", "resolved\n").unwrap();
    status::commit(path, "merge resolved", false).unwrap();

    let head = head_commit(&repo);
    assert_eq!(head.parent_count(), 2);
    assert_eq!(repo.state(), git2::RepositoryState::Clean);
    assert_eq!(
        fs::read_to_string(dir.join("base.txt")).unwrap(),
        "resolved\n"
    );
}

#[test]
fn merge_abort_restores_head_and_clears_state() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let repo = init_repo(dir);
    let path = dir.to_str().unwrap();

    branch::create_branch(path, "feature", None, true).unwrap();
    write(dir, "base.txt", "theirs\n");
    commit_all(&repo, "feature edit");
    branch::checkout(path, "main").unwrap();
    write(dir, "base.txt", "ours\n");
    let main_tip = commit_all(&repo, "main edit");

    assert!(matches!(
        merge::merge(path, "feature", false).unwrap(),
        MergeOutcome::Conflicts { .. }
    ));
    merge::merge_abort(path).unwrap();

    assert_eq!(head_commit(&repo).id(), main_tip);
    assert_eq!(repo.state(), git2::RepositoryState::Clean);
    // Checked out through git's filters, so the host's `core.autocrlf` decides
    // the line endings — compare on normalised content.
    assert_eq!(read_lf(dir, "base.txt"), "ours\n");
}

/// File contents with CRLF collapsed to LF.
fn read_lf(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel))
        .unwrap()
        .replace("\r\n", "\n")
}

#[test]
fn merge_of_an_ancestor_is_up_to_date() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let repo = init_repo(dir);
    let path = dir.to_str().unwrap();

    branch::create_branch(path, "old", None, false).unwrap();
    write(dir, "main.txt", "m\n");
    commit_all(&repo, "main work");

    assert_eq!(
        merge::merge(path, "old", false).unwrap(),
        MergeOutcome::UpToDate
    );
}

#[test]
fn rebase_replays_commits_onto_upstream() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let repo = init_repo(dir);
    let path = dir.to_str().unwrap();

    branch::create_branch(path, "feature", None, true).unwrap();
    write(dir, "feature.txt", "f\n");
    commit_all(&repo, "feature work");

    branch::checkout(path, "main").unwrap();
    write(dir, "main.txt", "m\n");
    let main_tip = commit_all(&repo, "main work");

    branch::checkout(path, "feature").unwrap();
    let outcome = merge::rebase(path, "main").unwrap();

    assert_eq!(outcome, RebaseOutcome::Done { commits: 1 });
    // The replayed commit now sits directly on main's tip.
    let head = head_commit(&repo);
    assert_eq!(head.parent(0).unwrap().id(), main_tip);
    assert_eq!(head.summary().unwrap(), "feature work");
    assert!(dir.join("main.txt").exists());
    assert_eq!(repo.state(), git2::RepositoryState::Clean);
}

#[test]
fn rebase_without_new_upstream_commits_is_up_to_date() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let repo = init_repo(dir);
    let path = dir.to_str().unwrap();

    branch::create_branch(path, "feature", None, true).unwrap();
    write(dir, "feature.txt", "f\n");
    commit_all(&repo, "feature work");

    assert_eq!(
        merge::rebase(path, "main").unwrap(),
        RebaseOutcome::UpToDate
    );
}

#[test]
fn conflicted_rebase_can_be_aborted() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let repo = init_repo(dir);
    let path = dir.to_str().unwrap();

    branch::create_branch(path, "feature", None, true).unwrap();
    write(dir, "base.txt", "theirs\n");
    let feature_tip = commit_all(&repo, "feature edit");

    branch::checkout(path, "main").unwrap();
    write(dir, "base.txt", "ours\n");
    commit_all(&repo, "main edit");

    branch::checkout(path, "feature").unwrap();
    let outcome = merge::rebase(path, "main").unwrap();
    assert!(
        matches!(outcome, RebaseOutcome::Conflicts { .. }),
        "{outcome:?}"
    );
    assert_eq!(status::status(path).unwrap().state, RepoState::Rebase);

    merge::rebase_abort(path).unwrap();
    assert_eq!(head_commit(&repo).id(), feature_tip);
    assert_eq!(repo.state(), git2::RepositoryState::Clean);
}

#[test]
fn conflicted_rebase_continues_after_resolution() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let repo = init_repo(dir);
    let path = dir.to_str().unwrap();

    branch::create_branch(path, "feature", None, true).unwrap();
    write(dir, "base.txt", "theirs\n");
    commit_all(&repo, "feature edit");

    branch::checkout(path, "main").unwrap();
    write(dir, "base.txt", "ours\n");
    let main_tip = commit_all(&repo, "main edit");

    branch::checkout(path, "feature").unwrap();
    assert!(matches!(
        merge::rebase(path, "main").unwrap(),
        RebaseOutcome::Conflicts { .. }
    ));

    conflict::resolve(path, "base.txt", "both\n").unwrap();
    let outcome = merge::rebase_continue(path).unwrap();

    assert_eq!(outcome, RebaseOutcome::Done { commits: 0 });
    assert_eq!(repo.state(), git2::RepositoryState::Clean);
    let head = head_commit(&repo);
    assert_eq!(head.parent(0).unwrap().id(), main_tip);
    assert_eq!(fs::read_to_string(dir.join("base.txt")).unwrap(), "both\n");
}

#[test]
fn list_detailed_reports_current_branch_and_ordering() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let repo = init_repo(dir);
    let path = dir.to_str().unwrap();

    branch::create_branch(path, "feature/a", None, true).unwrap();
    write(dir, "a.txt", "a\n");
    commit_all(&repo, "a");

    let rows = branch::list_detailed(path).unwrap();
    assert_eq!(rows.len(), 2);
    // Current branch sorts first.
    assert_eq!(rows[0].name, "feature/a");
    assert!(rows[0].is_current);
    assert!(rows[0].upstream.is_none());
    assert_eq!(rows[0].tip_summary, "a");
    assert!(rows.iter().any(|r| r.name == "main" && !r.is_current));
}
