//! `Error` has a way out: every failure records why and sets a retry, the
//! retry sets the branch up once its workstation is running, and a refused
//! repair does not turn a branch that was `Running` into a failed one.

use std::sync::Arc;
use std::time::Duration;

use super::super::guard::SetupGuard;
use super::super::host::SetupHost;
use super::super::retry::{self, RetryPolicy};
use super::super::SetupTrigger;
use super::verify::{running_branch, running_host, start_session_on};
use super::{
    head_branch, origin_and_workstation_clone, set_status, set_up_under, starting_branch,
    status_of, store_with_project, FakeHost, Info, Start,
};
use crate::store::{self, Store, WorkspaceStatus};
use crate::test_utils::run_git_in;

const MINUTE: Duration = Duration::from_secs(60);

/// A host whose checkout fails the way a dropped exec does.
fn failing_host(clone: &std::path::Path) -> Arc<FakeHost> {
    let mut host = running_host(clone);
    host.checkout_error = Some("could not fetch base branch 'main': connection reset");
    Arc::new(host)
}

fn host_reporting(clone: &std::path::Path, info: Info) -> Arc<FakeHost> {
    Arc::new(FakeHost::new(clone, &[info], Start::Succeeds))
}

/// Fail `branch`'s setup once and return the ticket of the timer that set.
async fn fail_once(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &Arc<FakeHost>,
    branch: &store::Branch,
) -> u64 {
    set_up_under(guard, store, host, branch, SetupTrigger::Requested)
        .await
        .unwrap_err();
    host.retries().last().unwrap().2
}

async fn fire(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &Arc<FakeHost>,
    branch: &store::Branch,
    ticket: u64,
) {
    let host: Arc<dyn SetupHost> = host.clone();
    retry::fire(guard, store, &host, &branch.id, ticket).await;
}

fn calls(host: &FakeHost) -> Vec<&'static str> {
    host.calls.lock().unwrap().clone()
}

#[test]
fn the_wait_doubles_from_a_minute_to_half_an_hour() {
    let policy = RetryPolicy::DEFAULT;
    let waits: Vec<u64> = (0..7).map(|n| policy.delay(n).as_secs()).collect();
    assert_eq!(waits, vec![60, 120, 240, 480, 960, 1800, 1800]);
    assert_eq!(policy.delay(u32::MAX), Duration::from_secs(1800));
}

/// A failed setup leaves the row in `Error` with its reason announced and a
/// retry set, and each further failure waits twice as long.
#[tokio::test]
async fn a_failed_setup_says_why_and_sets_a_retry() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = failing_host(clone.path());
    let guard = Arc::new(SetupGuard::default());

    let error = set_up_under(&guard, &store, &host, &branch, SetupTrigger::Requested)
        .await
        .unwrap_err();

    assert!(error.contains("connection reset"), "{error}");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
    assert_eq!(
        host.failures(),
        vec![
            // Setting out clears what the last attempt left.
            (branch.id.clone(), None),
            (branch.id.clone(), Some(error)),
        ]
    );
    assert_eq!(host.retries(), vec![(branch.id.clone(), MINUTE, 1)]);
    assert!(guard.retry_is_armed(&branch.id));

    fail_once(&guard, &store, &host, &branch).await;
    assert_eq!(host.retries()[1], (branch.id.clone(), 2 * MINUTE, 2));
}

/// The timer fires with the workstation running: the branch is set up
/// without anyone asking, its reason is cleared and its schedule forgotten.
/// The workstation is never started for it.
#[tokio::test]
async fn a_retry_sets_the_branch_up_when_its_workstation_is_running() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let guard = Arc::new(SetupGuard::default());
    let ticket = fail_once(&guard, &store, &failing_host(clone.path()), &branch).await;
    let host = Arc::new(running_host(clone.path()));

    fire(&guard, &store, &host, &branch, ticket).await;

    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
    assert_eq!(head_branch(clone.path()), "feature-new");
    assert_eq!(host.count("start_workspace"), 0);
    assert_eq!(host.failures(), vec![(branch.id.clone(), None)]);
    assert_eq!(
        host.written(),
        vec![(branch.id.clone(), true, WorkspaceStatus::Running)]
    );
    assert_eq!(host.retries(), vec![]);
    assert!(!guard.retry_is_armed(&branch.id));
}

/// A retry is not what starts or resumes a workstation. One that is down,
/// or can't be asked about, leaves the branch in `Error` with its reason,
/// touches nothing and looks again later.
#[tokio::test]
async fn a_retry_never_starts_a_workstation() {
    for info in [
        Info::Status("stopped"),
        Info::Status("suspended"),
        Info::Unreachable,
    ] {
        let (_origin, clone) = origin_and_workstation_clone();
        let (store, project) = store_with_project();
        let branch = starting_branch(&store, &project, "feature-new");
        let guard = Arc::new(SetupGuard::default());
        let ticket = fail_once(&guard, &store, &failing_host(clone.path()), &branch).await;
        let host = host_reporting(clone.path(), info);

        fire(&guard, &store, &host, &branch, ticket).await;

        assert_eq!(calls(&host), vec!["workspace_status"]);
        assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
        assert_eq!(host.failures(), vec![]);
        assert_eq!(head_branch(clone.path()), "main");
        assert_eq!(host.retries(), vec![(branch.id.clone(), 2 * MINUTE, 2)]);
        assert!(guard.retry_is_armed(&branch.id));
    }
}

/// A branch has one timer that counts. An older one, superseded by a later
/// failure, fires into nothing.
#[tokio::test]
async fn only_the_latest_timer_counts() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let guard = Arc::new(SetupGuard::default());
    let failing = failing_host(clone.path());
    let first = fail_once(&guard, &store, &failing, &branch).await;
    let second = fail_once(&guard, &store, &failing, &branch).await;
    let host = Arc::new(running_host(clone.path()));

    fire(&guard, &store, &host, &branch, first).await;
    assert_eq!(calls(&host), Vec::<&str>::new());
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));

    fire(&guard, &store, &host, &branch, second).await;
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));

    // And one that has fired does not fire twice.
    let before = calls(&host);
    fire(&guard, &store, &host, &branch, second).await;
    assert_eq!(calls(&host), before);
}

/// A timer for a branch that is no longer `Error` (a poll saw it suspended,
/// the user's Retry got there first) does nothing and its schedule is
/// dropped, so the next failure starts from a minute again.
#[tokio::test]
async fn a_timer_for_a_branch_that_moved_on_does_nothing() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let guard = Arc::new(SetupGuard::default());
    let failing = failing_host(clone.path());
    let ticket = fail_once(&guard, &store, &failing, &branch).await;
    set_status(&store, &branch, WorkspaceStatus::Suspended);
    let host = Arc::new(running_host(clone.path()));

    fire(&guard, &store, &host, &branch, ticket).await;

    assert_eq!(calls(&host), Vec::<&str>::new());
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Suspended));
    assert!(!guard.retry_is_armed(&branch.id));

    fail_once(&guard, &store, &failing, &branch).await;
    assert_eq!(failing.retries().last().unwrap().1, MINUTE);
}

/// A setup that works, however it was asked for, ends the schedule: the
/// timer still out for the branch finds nothing to do.
#[tokio::test]
async fn a_setup_that_works_forgets_the_schedule() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let guard = Arc::new(SetupGuard::default());
    let ticket = fail_once(&guard, &store, &failing_host(clone.path()), &branch).await;
    let host = Arc::new(running_host(clone.path()));

    // The error view's Retry.
    let status = set_up_under(&guard, &store, &host, &branch, SetupTrigger::Requested).await;

    assert_eq!(status, Ok(Some(WorkspaceStatus::Running)));
    assert_eq!(host.failures(), vec![(branch.id.clone(), None)]);
    assert!(!guard.retry_is_armed(&branch.id));
    let before = calls(&host);
    fire(&guard, &store, &host, &branch, ticket).await;
    assert_eq!(calls(&host), before);
}

/// What finds a branch in `Error` without having failed it (a poll, the
/// startup pass) sets its timer once, however often it looks.
#[tokio::test]
async fn a_branch_seen_failed_gets_one_timer() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let branch = set_status(&store, &branch, WorkspaceStatus::Error);
    let guard = Arc::new(SetupGuard::default());
    let host = host_reporting(clone.path(), Info::Status("stopped"));

    for _ in 0..3 {
        retry::arm(&guard, host.as_ref(), &branch.id);
    }
    assert_eq!(host.retries(), vec![(branch.id.clone(), MINUTE, 1)]);

    // Fired and set again by the retry itself: still one.
    fire(&guard, &store, &host, &branch, 1).await;
    retry::arm(&guard, host.as_ref(), &branch.id);
    assert_eq!(host.retries().len(), 2);
}

/// A repair that is refused changed nothing in the clone, so a branch that
/// was `Running` is `Running` again: only the session that asked fails,
/// with the reason. Here the clone is on another branch with uncommitted
/// changes, which a checkout would carry across.
#[tokio::test]
async fn a_refused_repair_leaves_a_running_branch_running() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = running_branch(&store, &project, "feature-b");
    run_git_in(clone.path(), &["checkout", "-b", "feature-a"]);
    std::fs::write(clone.path().join("file.txt"), "base\nin progress\n").unwrap();
    let host = Arc::new(running_host(clone.path()));

    let error = start_session_on(&store, &host, &branch, "session-b")
        .await
        .unwrap_err();

    for part in [
        "had branch 'feature-a' checked out",
        "this session's branch 'feature-b'",
        "uncommitted changes to 1 file(s)",
    ] {
        assert!(error.contains(part), "{part}: {error}");
    }
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
    assert_eq!(host.failure_of(&branch), None);
    assert_eq!(host.retries(), vec![]);
    // The row went through `Starting`; the write back kicks the queue that
    // held meanwhile.
    assert_eq!(
        host.written(),
        vec![(branch.id.clone(), true, WorkspaceStatus::Running)]
    );
    assert_eq!(head_branch(clone.path()), "feature-a");
    let dirty = run_git_in(clone.path(), &["status", "--porcelain"]);
    assert!(dirty.contains("file.txt"), "{dirty}");
}

/// The same for commits a switch would leave behind on the base branch.
#[tokio::test]
async fn a_repair_refused_over_stranded_commits_leaves_the_branch_running() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = running_branch(&store, &project, "feature-b");
    run_git_in(clone.path(), &["branch", "feature-b", "origin/main"]);
    let stranded = super::checkout::commit(clone.path(), "one.txt", "agent work");
    let host = Arc::new(running_host(clone.path()));

    let error = start_session_on(&store, &host, &branch, "session-b")
        .await
        .unwrap_err();

    for part in ["'main'", "'feature-b'", "agent work"] {
        assert!(error.contains(part), "{part}: {error}");
    }
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
    assert_eq!(host.failure_of(&branch), None);
    assert_eq!(host.retries(), vec![]);
    assert_eq!(head_branch(clone.path()), "main");
    let main = run_git_in(clone.path(), &["rev-parse", "main"]);
    assert_eq!(main.trim(), stranded);
}

/// A repair that finds the workstation down is refused too: what the row of
/// a stopped workstation should say is the pollers' to settle.
#[tokio::test]
async fn a_repair_never_starts_a_workstation_for_a_running_row() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = running_branch(&store, &project, "feature-b");
    // The clone reads as on another branch; Blox says the workstation is
    // stopped by the time the repair looks.
    let host = host_reporting(clone.path(), Info::Status("stopped"));

    let error = start_session_on(&store, &host, &branch, "session-b")
        .await
        .unwrap_err();

    assert!(error.contains("is not running"), "{error}");
    assert_eq!(host.count("start_workspace"), 0);
    assert_eq!(host.count("check_out"), 0);
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
    assert_eq!(host.retries(), vec![]);
}
