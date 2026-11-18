//! What the checkout does to a clone that is not where it should be: the
//! commits and changes in it either come along or keep the clone where it
//! is. None are left behind, moved to another branch, or reset.

use std::path::Path;
use std::sync::Arc;

use super::super::checkout::{local_branch_exists, BranchCheckout, CheckoutError};
use super::super::SetupTrigger;
use super::{
    head_branch, origin_and_workstation_clone, set_up, starting_branch, status_of,
    store_with_project, FakeHost, Info, Start,
};
use crate::branches;
use crate::store::WorkspaceStatus;
use crate::test_utils::run_git_in;

fn rev(clone: &Path, name: &str) -> String {
    run_git_in(clone, &["rev-parse", name]).trim().to_string()
}

/// Commit a new file in the clone, on whatever it has checked out.
pub(super) fn commit(clone: &Path, file: &str, subject: &str) -> String {
    std::fs::write(clone.join(file), format!("{subject}\n")).unwrap();
    run_git_in(clone, &["add", file]);
    run_git_in(clone, &["commit", "-m", subject]);
    rev(clone, "HEAD")
}

fn uncommitted(clone: &Path) -> Vec<String> {
    run_git_in(clone, &["status", "--porcelain"])
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

fn has_branch(clone: &Path, name: &str) -> bool {
    local_branch_exists(&branches::git_runner(clone, None, None), name).unwrap()
}

fn check_out(clone: &Path, checkout: &BranchCheckout<'_>) -> Result<(), CheckoutError> {
    branches::check_out_workspace_branch(&branches::git_runner(clone, None, None), checkout)
}

/// A refusal, as the text the user reads.
fn refusal(result: Result<(), CheckoutError>) -> String {
    let error = result.unwrap_err();
    assert!(error.is_refusal(), "not a refusal: {error}");
    error.to_string()
}

// ── Commits made on the base branch ─────────────────────────────────────────

/// The state the original bug left behind: the agent ran in a clone still on
/// `main` and committed there. The branch exists nowhere, so it is created
/// where the clone stands and the commits are on it. Local `main` is not
/// reset or moved, and the new branch does not track `origin/main`.
#[test]
fn commits_stranded_on_the_base_branch_come_along_to_a_new_branch() {
    let (_origin, clone) = origin_and_workstation_clone();
    let clone = clone.path();
    let origin_main = rev(clone, "origin/main");
    commit(clone, "one.txt", "agent work one");
    let stranded = commit(clone, "two.txt", "agent work two");

    check_out(clone, &BranchCheckout::new("main", "feature-new")).unwrap();

    assert_eq!(head_branch(clone), "feature-new");
    assert_eq!(rev(clone, "HEAD"), stranded);
    assert_eq!(rev(clone, "main"), stranded);
    assert_eq!(rev(clone, "origin/main"), origin_main);
    let upstream = run_git_in(
        clone,
        &[
            "for-each-ref",
            "--format=%(upstream)",
            "refs/heads/feature-new",
        ],
    );
    assert_eq!(upstream.trim(), "");
}

/// Work in progress on the base branch belongs with the commits beside it.
#[test]
fn uncommitted_changes_beside_stranded_commits_come_along_too() {
    let (_origin, clone) = origin_and_workstation_clone();
    let clone = clone.path();
    let stranded = commit(clone, "one.txt", "agent work");
    std::fs::write(clone.join("file.txt"), "base\nin progress\n").unwrap();
    std::fs::write(clone.join("new.txt"), "untracked\n").unwrap();
    let before = uncommitted(clone);
    assert_eq!(before.len(), 2);

    check_out(clone, &BranchCheckout::new("main", "feature-new")).unwrap();

    assert_eq!(head_branch(clone), "feature-new");
    assert_eq!(rev(clone, "HEAD"), stranded);
    assert_eq!(uncommitted(clone), before);
}

/// Once the commits have a branch of their own they are not stranded, and
/// the next branch of the clone starts from origin's base as any new branch
/// does, whether the clone was left on the first branch or back on `main`.
#[test]
fn a_later_branch_starts_from_origins_base_not_from_the_brought_along_commits() {
    let (_origin, clone) = origin_and_workstation_clone();
    let clone = clone.path();
    let origin_main = rev(clone, "origin/main");
    let stranded = commit(clone, "one.txt", "agent work");
    check_out(clone, &BranchCheckout::new("main", "feature-new")).unwrap();

    check_out(clone, &BranchCheckout::new("main", "feature-second")).unwrap();
    assert_eq!(head_branch(clone), "feature-second");
    assert_eq!(rev(clone, "HEAD"), origin_main);

    run_git_in(clone, &["checkout", "main"]);
    check_out(clone, &BranchCheckout::new("main", "feature-third")).unwrap();
    assert_eq!(head_branch(clone), "feature-third");
    assert_eq!(rev(clone, "HEAD"), origin_main);

    assert_eq!(rev(clone, "feature-new"), stranded);
    assert_eq!(rev(clone, "main"), stranded);
}

/// The clone already has the branch, so the commits on `main` can't be
/// brought along by creating it. Switching would leave them behind: the
/// checkout is refused with both branches and the commits named, and the
/// clone is untouched.
#[test]
fn stranded_commits_are_not_left_behind_for_a_branch_the_clone_has() {
    let (_origin, clone) = origin_and_workstation_clone();
    let clone = clone.path();
    run_git_in(clone, &["branch", "feature-x", "origin/main"]);
    let feature = rev(clone, "feature-x");
    for n in 1..=7 {
        commit(clone, &format!("{n}.txt"), &format!("agent work {n}"));
    }
    let stranded = rev(clone, "HEAD");

    let error = refusal(check_out(clone, &BranchCheckout::new("main", "feature-x")));

    assert!(error.contains("the clone is on 'main'"), "{error}");
    assert!(error.contains("7 commit(s)"), "{error}");
    assert!(
        error.contains("the clone has a branch 'feature-x'"),
        "{error}"
    );
    // Newest first, five of them, then a count.
    assert!(error.contains("agent work 7"), "{error}");
    assert!(error.contains("agent work 3"), "{error}");
    assert!(!error.contains("agent work 2"), "{error}");
    assert!(error.contains("and 2 more"), "{error}");
    assert_eq!(head_branch(clone), "main");
    assert_eq!(rev(clone, "main"), stranded);
    assert_eq!(rev(clone, "feature-x"), feature);
}

/// The same when the branch is only on origin (a pull request's branch).
#[test]
fn stranded_commits_are_not_left_behind_for_a_branch_origin_has() {
    let (_origin, clone) = origin_and_workstation_clone();
    let clone = clone.path();
    run_git_in(clone, &["push", "origin", "HEAD:refs/heads/feature-pr"]);
    let stranded = commit(clone, "one.txt", "agent work");

    let error = refusal(check_out(clone, &BranchCheckout::new("main", "feature-pr")));

    assert!(error.contains("the clone is on 'main'"), "{error}");
    assert!(
        error.contains("origin has a branch 'feature-pr'"),
        "{error}"
    );
    assert!(error.contains("agent work"), "{error}");
    assert_eq!(head_branch(clone), "main");
    assert_eq!(rev(clone, "main"), stranded);
    assert!(!has_branch(clone, "feature-pr"));
}

/// A base branch that is itself a branch Staged works on in this clone (a
/// stacked branch's parent) holds that branch's own commits. They are not
/// brought along, and the new branch starts from origin's copy of the base.
/// Uncommitted changes there are the parent's work in progress and keep the
/// clone where it is.
#[test]
fn commits_on_a_base_branch_that_is_worked_on_stay_on_it() {
    let (_origin, clone) = origin_and_workstation_clone();
    let clone = clone.path();
    let origin_main = rev(clone, "origin/main");
    let parents_own = commit(clone, "one.txt", "the parent's own work");
    let checkout = BranchCheckout {
        base_ref: "main",
        branch_name: "feature-child",
        base_is_worked_on: true,
    };

    std::fs::write(clone.join("file.txt"), "base\nin progress\n").unwrap();
    let error = refusal(check_out(clone, &checkout));
    assert!(error.contains("branch 'main'"), "{error}");
    assert!(
        error.contains("uncommitted changes to 1 file(s)"),
        "{error}"
    );
    assert_eq!(head_branch(clone), "main");
    assert!(!has_branch(clone, "feature-child"));

    run_git_in(clone, &["checkout", "--", "file.txt"]);
    check_out(clone, &checkout).unwrap();
    assert_eq!(head_branch(clone), "feature-child");
    assert_eq!(rev(clone, "HEAD"), origin_main);
    assert_eq!(rev(clone, "main"), parents_own);
}

// ── Other states the clone can't leave ──────────────────────────────────────

/// Uncommitted changes on another branch are that branch's work. Git would
/// carry them onto this one, so the checkout is refused and the clone, its
/// changes included, is as it was.
#[test]
fn a_dirty_tree_on_another_branch_is_not_carried_across() {
    let (_origin, clone) = origin_and_workstation_clone();
    let clone = clone.path();
    run_git_in(clone, &["checkout", "-b", "feature-a"]);
    std::fs::write(clone.join("file.txt"), "base\nin progress\n").unwrap();
    let before = uncommitted(clone);

    let error = refusal(check_out(clone, &BranchCheckout::new("main", "feature-b")));

    assert!(error.contains("branch 'feature-a'"), "{error}");
    assert!(error.contains("'feature-b'"), "{error}");
    assert!(
        error.contains("uncommitted changes to 1 file(s)"),
        "{error}"
    );
    assert_eq!(head_branch(clone), "feature-a");
    assert_eq!(uncommitted(clone), before);
    assert!(!has_branch(clone, "feature-b"));
}

/// A detached HEAD holding commits no ref reaches would lose them. One that
/// sits on a commit some ref has loses nothing and is left freely.
#[test]
fn a_detached_head_is_left_only_when_no_commit_is_lost() {
    let (_origin, clone) = origin_and_workstation_clone();
    let clone = clone.path();
    run_git_in(clone, &["checkout", "--detach"]);
    let orphan = commit(clone, "one.txt", "work on no branch");

    let error = refusal(check_out(
        clone,
        &BranchCheckout::new("main", "feature-new"),
    ));

    assert!(error.contains("a detached HEAD"), "{error}");
    assert!(error.contains("work on no branch"), "{error}");
    assert!(error.contains("'feature-new'"), "{error}");
    assert_eq!(head_branch(clone), "HEAD");
    assert_eq!(rev(clone, "HEAD"), orphan);
    assert!(!has_branch(clone, "feature-new"));

    run_git_in(clone, &["checkout", "--detach", "origin/main"]);
    check_out(clone, &BranchCheckout::new("main", "feature-new")).unwrap();
    assert_eq!(head_branch(clone), "feature-new");
}

// ── Through setup ───────────────────────────────────────────────────────────

/// End to end: a branch whose agent committed on `main` is set up with those
/// commits on it and marked `Running`.
#[tokio::test]
async fn setup_brings_the_commits_an_agent_left_on_the_base_branch() {
    let (_origin, clone) = origin_and_workstation_clone();
    let stranded = commit(clone.path(), "one.txt", "agent work");
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(FakeHost::new(
        clone.path(),
        &[Info::Status("running")],
        Start::Succeeds,
    ));

    set_up(&store, &host, &branch, SetupTrigger::Observed)
        .await
        .unwrap();

    assert_eq!(head_branch(clone.path()), "feature-new");
    assert_eq!(rev(clone.path(), "HEAD"), stranded);
    assert_eq!(rev(clone.path(), "main"), stranded);
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
}

/// A branch that was never `Running` and can't be checked out without
/// leaving commits behind fails where the user can see it, with the reason
/// that names the branches and the commits.
#[tokio::test]
async fn setup_refused_over_stranded_commits_fails_with_the_reason() {
    let (_origin, clone) = origin_and_workstation_clone();
    run_git_in(clone.path(), &["branch", "feature-new", "origin/main"]);
    let stranded = commit(clone.path(), "one.txt", "agent work");
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(FakeHost::new(
        clone.path(),
        &[Info::Status("running")],
        Start::Succeeds,
    ));

    let error = set_up(&store, &host, &branch, SetupTrigger::Requested)
        .await
        .unwrap_err();

    for part in ["'main'", "'feature-new'", "agent work"] {
        assert!(error.contains(part), "{part}: {error}");
    }
    assert_eq!(head_branch(clone.path()), "main");
    assert_eq!(rev(clone.path(), "main"), stranded);
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
    assert_eq!(host.failure_of(&branch), Some(error));
    // Left to the error view's Retry: trying again on a timer changes nothing.
    assert_eq!(host.retries(), vec![]);
}
