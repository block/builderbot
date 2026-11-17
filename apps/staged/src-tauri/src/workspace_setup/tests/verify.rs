//! The clone is read to be on the branch before `Running` is written and
//! before an agent starts, and a read or probe that didn't answer is never
//! taken for an answer.

use std::path::Path;
use std::sync::{Arc, Mutex};

use super::super::guard::SetupGuard;
use super::super::verify::{parse_clone_head, verify_session_checkout, CloneHead};
use super::super::{run_guarded, SetupHost, SetupTrigger};
use super::{
    head_branch, origin_and_workstation_clone, probe_clone_head, set_status, set_up,
    starting_branch, status_of, store_with_project, FakeHost, Head, Info, Start,
};
use crate::branches;
use crate::store::{self, Store, WorkspaceStatus};
use crate::test_utils::{run_git_in, TempDir};

/// A branch whose row says `Running`, the way setup or a poller left it.
fn running_branch(store: &Store, project: &store::Project, name: &str) -> store::Branch {
    let branch = starting_branch(store, project, name);
    set_status(store, &branch, WorkspaceStatus::Running)
}

fn running_host(clone: &Path) -> FakeHost {
    FakeHost::new(clone, &[Info::Status("running")], Start::Succeeds)
}

/// A session row in flight on `branch`.
fn live_session(store: &Store, branch: &store::Branch) -> store::Session {
    let session = store::Session::new_running("work", Path::new("/tmp")).with_branch(&branch.id);
    store.create_session(&session).unwrap();
    session
}

/// The session-start check, as `ensure_session_checkout` runs it.
async fn start_session_on(
    store: &Arc<Store>,
    host: &Arc<FakeHost>,
    branch: &store::Branch,
    session_id: &str,
) -> Result<(), String> {
    let guard = Arc::new(SetupGuard::default());
    let host: Arc<dyn SetupHost> = host.clone();
    verify_session_checkout(&guard, store, &host, branch, session_id).await
}

fn calls(host: &FakeHost) -> Vec<&'static str> {
    host.calls.lock().unwrap().clone()
}

// ── The probe ───────────────────────────────────────────────────────────────

/// The script answers on stdout for every state of the clone, so no state
/// is told by an exit status an exec failure could also produce.
#[test]
fn the_head_probe_tells_a_branch_from_a_detached_head_from_no_repository() {
    let (_origin, clone) = origin_and_workstation_clone();
    assert_eq!(
        probe_clone_head(clone.path()),
        Ok(CloneHead::Branch("main".to_string()))
    );

    run_git_in(clone.path(), &["checkout", "-b", "team/feature-x"]);
    assert_eq!(
        probe_clone_head(clone.path()),
        Ok(CloneHead::Branch("team/feature-x".to_string()))
    );

    run_git_in(clone.path(), &["checkout", "--detach"]);
    assert_eq!(probe_clone_head(clone.path()), Ok(CloneHead::Detached));

    let empty = TempDir::new("no-clone");
    assert_eq!(probe_clone_head(empty.path()), Ok(CloneHead::NoRepo));
    assert_eq!(
        probe_clone_head(&empty.path().join("never-created")),
        Ok(CloneHead::NoRepo)
    );
}

/// Only the script's own lines are answers. An empty or cut-off reply is an
/// error, so it can be neither "on the branch" nor "on another one".
#[test]
fn an_unrecognised_probe_reply_is_an_error() {
    assert_eq!(
        parse_clone_head("branch refs/heads/feature-x\n"),
        Ok(CloneHead::Branch("feature-x".to_string()))
    );
    assert_eq!(parse_clone_head("detached\n"), Ok(CloneHead::Detached));
    assert_eq!(parse_clone_head("norepo\n"), Ok(CloneHead::NoRepo));
    // HEAD on a ref that is not a local branch is not "on a branch".
    assert_eq!(
        parse_clone_head("branch refs/remotes/origin/main\n"),
        Ok(CloneHead::Detached)
    );
    for reply in [
        "",
        "\n",
        "branch",
        "feature-x",
        "fatal: not a git repository",
    ] {
        assert!(parse_clone_head(reply).is_err(), "{reply:?}");
    }
}

// ── The end of setup ────────────────────────────────────────────────────────

/// A checkout that reports success and leaves the clone on the base branch
/// is a failed setup: `Running` is never written over it.
#[tokio::test]
async fn setup_does_not_say_running_over_a_clone_on_another_branch() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(
        running_host(clone.path())
            .with_heads(&[Head::Reads(CloneHead::Branch("main".to_string()))]),
    );

    let error = set_up(&store, &host, &branch, SetupTrigger::Requested)
        .await
        .unwrap_err();

    assert!(error.contains("branch 'main'"), "{error}");
    assert!(error.contains("'feature-new'"), "{error}");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
    assert_eq!(host.written(), vec![]);
}

/// A detached HEAD after setup is not the branch either.
#[tokio::test]
async fn setup_does_not_say_running_over_a_detached_head() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(running_host(clone.path()).with_heads(&[Head::Reads(CloneHead::Detached)]));

    let error = set_up(&store, &host, &branch, SetupTrigger::Observed)
        .await
        .unwrap_err();

    assert!(error.contains("a detached HEAD"), "{error}");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
}

/// The read that confirms the checkout failing is not a confirmation: after
/// its one retry setup fails, though the checkout itself worked.
#[tokio::test]
async fn setup_fails_when_the_checked_out_branch_cannot_be_read() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(running_host(clone.path()).with_heads(&[
        Head::Unanswered("workstation disconnected"),
        Head::Unanswered("sq blox command timed out after 300s"),
    ]));

    let error = set_up(&store, &host, &branch, SetupTrigger::Requested)
        .await
        .unwrap_err();

    assert!(error.contains("Could not verify"), "{error}");
    assert!(error.contains("timed out"), "{error}");
    assert_eq!(host.count("clone_head"), 2);
    assert_eq!(head_branch(clone.path()), "feature-new");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
    assert_eq!(host.written(), vec![]);
}

/// One read that didn't answer is read again, and a second that does
/// settles it.
#[tokio::test]
async fn one_unanswered_read_at_the_end_of_setup_is_read_again() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(
        running_host(clone.path()).with_heads(&[Head::Unanswered("workstation disconnected")]),
    );

    set_up(&store, &host, &branch, SetupTrigger::Requested)
        .await
        .unwrap();

    assert_eq!(host.count("clone_head"), 2);
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
}

// ── Session start ───────────────────────────────────────────────────────────

/// The usual start costs one read and nothing else: no fetch, no checkout,
/// no write.
#[tokio::test]
async fn a_session_on_a_checked_out_branch_starts_after_one_read() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = running_branch(&store, &project, "feature-new");
    run_git_in(clone.path(), &["checkout", "-b", "feature-new"]);
    let host = Arc::new(running_host(clone.path()));

    start_session_on(&store, &host, &branch, "session-1")
        .await
        .unwrap();

    assert_eq!(calls(&host), vec!["clone_head"]);
    assert_eq!(host.written(), vec![]);
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
}

/// The gap setup alone can't close: a branch that was never set up, resumed
/// outside Staged and promoted to `Running` by a poller, still on `main`.
/// The session start checks the branch out through the shared setup and
/// only then lets the agent start. The session being started is itself
/// `running` on the branch by then and does not stand in the way.
#[tokio::test]
async fn a_session_start_repairs_a_clone_left_on_the_base_branch() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = running_branch(&store, &project, "feature-new");
    let session = live_session(&store, &branch);
    let host = Arc::new(running_host(clone.path()));

    start_session_on(&store, &host, &branch, &session.id)
        .await
        .unwrap();

    assert_eq!(head_branch(clone.path()), "feature-new");
    assert_eq!(host.count("check_out"), 1);
    assert_eq!(host.count("start_workspace"), 0);
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
    assert_eq!(
        host.written(),
        vec![(branch.id.clone(), true, WorkspaceStatus::Running)]
    );
}

/// A clone with nothing in it reads as `norepo`, a definite answer, and is
/// repaired like any other clone that is not on the branch.
#[tokio::test]
async fn a_session_start_repairs_a_missing_clone() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = running_branch(&store, &project, "feature-new");
    let host = Arc::new(running_host(clone.path()).with_heads(&[
        // The session's read, then the repair's look before it starts.
        Head::Reads(CloneHead::NoRepo),
        Head::Reads(CloneHead::NoRepo),
    ]));

    start_session_on(&store, &host, &branch, "session-1")
        .await
        .unwrap();

    assert_eq!(host.count("check_out"), 1);
    assert_eq!(head_branch(clone.path()), "feature-new");
}

/// When the repair fails the agent is not started: the session fails with
/// both branch names and the cause, and the branch stops saying `Running`,
/// so whatever is queued behind it waits instead of failing in turn.
#[tokio::test]
async fn a_failed_repair_fails_the_session_and_the_branch() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = running_branch(&store, &project, "feature-new");
    let mut host = running_host(clone.path());
    host.checkout_error = Some("could not fetch base branch 'main': network unreachable");
    let host = Arc::new(host);

    let error = start_session_on(&store, &host, &branch, "session-1")
        .await
        .unwrap_err();

    assert!(error.contains("branch 'main'"), "{error}");
    assert!(error.contains("'feature-new'"), "{error}");
    assert!(error.contains("network unreachable"), "{error}");
    assert_eq!(head_branch(clone.path()), "main");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
    assert_eq!(host.written(), vec![]);
}

/// A read that doesn't answer is not "wrong branch" (no checkout runs) and
/// not "right branch" (the start fails), whatever the clone is on. It is
/// read twice, then the start fails and the branch is left as it was.
#[tokio::test]
async fn an_unanswered_read_is_neither_a_wrong_branch_nor_a_right_one() {
    for clone_is_on in ["main", "feature-new"] {
        let (_origin, clone) = origin_and_workstation_clone();
        let (store, project) = store_with_project();
        let branch = running_branch(&store, &project, "feature-new");
        if clone_is_on != "main" {
            run_git_in(clone.path(), &["checkout", "-b", clone_is_on]);
        }
        let host = Arc::new(running_host(clone.path()).with_heads(&[
            Head::Unanswered("workstation disconnected"),
            Head::Unanswered("workstation disconnected"),
        ]));

        let error = start_session_on(&store, &host, &branch, "session-1")
            .await
            .unwrap_err();

        assert!(error.contains("Could not verify"), "{error}");
        assert!(error.contains("workstation disconnected"), "{error}");
        assert_eq!(calls(&host), vec!["clone_head", "clone_head"]);
        assert_eq!(head_branch(clone.path()), clone_is_on);
        assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
    }
}

/// One blip costs a second read, not the session.
#[tokio::test]
async fn a_session_start_reads_once_more_after_an_unanswered_read() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = running_branch(&store, &project, "feature-new");
    run_git_in(clone.path(), &["checkout", "-b", "feature-new"]);
    let host = Arc::new(
        running_host(clone.path()).with_heads(&[Head::Unanswered("workstation disconnected")]),
    );

    start_session_on(&store, &host, &branch, "session-1")
        .await
        .unwrap();

    assert_eq!(calls(&host), vec!["clone_head", "clone_head"]);
}

/// Two branches of one repository share a clone. While a session of one is
/// live in it the clone is not switched under that agent: the other
/// branch's session fails, naming both. Once the clone is free the same
/// start repairs it.
#[tokio::test]
async fn a_clone_another_live_session_works_in_is_not_switched() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let sibling = running_branch(&store, &project, "feature-a");
    let branch = running_branch(&store, &project, "feature-b");
    run_git_in(clone.path(), &["checkout", "-b", "feature-a"]);
    let sibling_session = live_session(&store, &sibling);
    let host = Arc::new(running_host(clone.path()));

    let error = start_session_on(&store, &host, &branch, "session-b")
        .await
        .unwrap_err();

    assert!(error.contains("branch 'feature-a'"), "{error}");
    assert!(error.contains("'feature-b'"), "{error}");
    assert!(error.contains("running in the same clone"), "{error}");
    assert_eq!(host.count("check_out"), 0);
    assert_eq!(head_branch(clone.path()), "feature-a");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));

    store
        .update_session_status(
            &sibling_session.id,
            store::SessionStatus::Completed,
            None,
            None,
        )
        .unwrap();
    start_session_on(&store, &host, &branch, "session-b")
        .await
        .unwrap();

    assert_eq!(head_branch(clone.path()), "feature-b");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
}

/// A repair whose turn comes after the clone was put right (another
/// session's repair ran first) leaves the `Running` branch untouched.
#[tokio::test]
async fn a_repair_that_finds_the_clone_on_the_branch_touches_nothing() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = running_branch(&store, &project, "feature-new");
    run_git_in(clone.path(), &["checkout", "-b", "feature-new"]);
    let host = Arc::new(running_host(clone.path()));

    set_up(&store, &host, &branch, SetupTrigger::Repair)
        .await
        .unwrap();

    assert_eq!(calls(&host), vec!["clone_head"]);
    assert_eq!(host.written(), vec![]);
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
}

/// A repair does not start a workstation. For a branch that stopped after
/// the session read its clone, the repair has nothing to do and the start
/// fails on the read that follows it, naming both branches.
#[tokio::test]
async fn a_session_start_on_a_stopped_branch_fails_without_a_repair() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let branch = set_status(&store, &branch, WorkspaceStatus::Stopped);
    let host = Arc::new(running_host(clone.path()));

    let error = start_session_on(&store, &host, &branch, "session-1")
        .await
        .unwrap_err();

    assert!(error.contains("branch 'main'"), "{error}");
    assert!(error.contains("'feature-new'"), "{error}");
    assert_eq!(calls(&host), vec!["clone_head", "clone_head"]);
    assert_eq!(head_branch(clone.path()), "main");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Stopped));
}

/// Two sessions that both read the clone off the branch get one repair.
#[tokio::test]
async fn concurrent_session_starts_share_one_repair() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = running_branch(&store, &project, "feature-new");
    let (host, gate) = running_host(clone.path()).gated();
    let host = Arc::new(host);
    let dyn_host: Arc<dyn SetupHost> = host.clone();
    let guard = Arc::new(SetupGuard::default());

    let starts: Vec<_> = ["session-1", "session-2"]
        .into_iter()
        .map(|session_id| {
            let (guard, store, host, branch) = (
                Arc::clone(&guard),
                Arc::clone(&store),
                Arc::clone(&dyn_host),
                branch.clone(),
            );
            tokio::spawn(async move {
                verify_session_checkout(&guard, &store, &host, &branch, session_id).await
            })
        })
        .collect();

    host.wait_for_checkouts(1).await;
    gate.add_permits(2);
    for start in starts {
        assert_eq!(start.await.unwrap(), Ok(()));
    }

    assert_eq!(host.count("check_out"), 1);
    assert_eq!(head_branch(clone.path()), "feature-new");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
}

/// A repair that is the only trigger in flight is a real setup, so a
/// request that joins it gets a branch that was set up.
#[tokio::test]
async fn a_repair_runs_the_shared_setup_under_the_guard() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = running_branch(&store, &project, "feature-new");
    let host = Arc::new(running_host(clone.path()));
    let dyn_host: Arc<dyn SetupHost> = host.clone();
    let guard = Arc::new(SetupGuard::default());

    run_guarded(&guard, &store, &dyn_host, &branch.id, SetupTrigger::Repair)
        .await
        .unwrap();

    assert_eq!(host.count("check_auth"), 1);
    assert_eq!(host.count("check_out"), 1);
    assert_eq!(head_branch(clone.path()), "feature-new");
    assert!(!guard.is_in_flight(&branch.id));
}

// ── The checkout routine's local-branch probe ───────────────────────────────

/// A clone on `main` that also has `feature-x`, one commit ahead of origin.
fn clone_with_an_unpushed_feature_branch() -> (TempDir, TempDir, String) {
    let (origin, clone) = origin_and_workstation_clone();
    run_git_in(clone.path(), &["checkout", "-b", "feature-x"]);
    std::fs::write(clone.path().join("file.txt"), "base\nunpushed\n").unwrap();
    run_git_in(clone.path(), &["commit", "-am", "unpushed"]);
    let unpushed = run_git_in(clone.path(), &["rev-parse", "HEAD"]);
    run_git_in(clone.path(), &["checkout", "main"]);
    (origin, clone, unpushed)
}

/// The probe that asks whether the clone has the branch used to answer by
/// exit status, so an exec that never ran read as "missing" and the branch
/// was recreated from the base with `checkout -B`, off its unpushed commit.
/// A failed probe is now an error and nothing is checked out.
#[test]
fn a_failed_local_branch_probe_is_not_read_as_a_missing_branch() {
    let (_origin, clone, unpushed) = clone_with_an_unpushed_feature_branch();
    let real = branches::git_runner(clone.path(), None, None);
    let ran = Mutex::new(Vec::<String>::new());
    let git: branches::GitRunner<'_> = Box::new(|args: &[&str]| {
        ran.lock().unwrap().push(args.join(" "));
        if matches!(args.first(), Some(&"for-each-ref") | Some(&"rev-parse")) {
            return Err("sq blox command failed: workstation disconnected".to_string());
        }
        real(args)
    });

    let error = branches::check_out_workspace_branch(&git, "main", "feature-x").unwrap_err();

    assert!(error.contains("already has branch 'feature-x'"), "{error}");
    assert!(error.contains("workstation disconnected"), "{error}");
    let ran = ran.lock().unwrap();
    assert!(
        !ran.iter().any(|command| command.starts_with("checkout")),
        "{ran:?}"
    );
    assert_eq!(head_branch(clone.path()), "main");
    assert_eq!(
        run_git_in(clone.path(), &["rev-parse", "feature-x"]),
        unpushed
    );
}

/// The probe that ran: an existing branch is checked out as it is, unpushed
/// commit and all, and a missing one is created.
#[test]
fn the_local_branch_probe_answers_through_its_output() {
    let (_origin, clone, unpushed) = clone_with_an_unpushed_feature_branch();
    let git = branches::git_runner(clone.path(), None, None);

    assert_eq!(branches::local_branch_exists(&git, "feature-x"), Ok(true));
    assert_eq!(branches::local_branch_exists(&git, "feature-y"), Ok(false));
    // A prefix of a branch name is not that branch.
    run_git_in(clone.path(), &["branch", "team/feature-z"]);
    assert_eq!(branches::local_branch_exists(&git, "team"), Ok(false));
    assert_eq!(
        branches::local_branch_exists(&git, "team/feature-z"),
        Ok(true)
    );

    branches::check_out_workspace_branch(&git, "main", "feature-x").unwrap();
    assert_eq!(head_branch(clone.path()), "feature-x");
    assert_eq!(run_git_in(clone.path(), &["rev-parse", "HEAD"]), unpushed);

    branches::check_out_workspace_branch(&git, "main", "feature-y").unwrap();
    assert_eq!(head_branch(clone.path()), "feature-y");
    assert_eq!(
        run_git_in(clone.path(), &["rev-parse", "HEAD"]),
        run_git_in(clone.path(), &["rev-parse", "origin/main"])
    );
}
