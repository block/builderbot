//! The sessions sharing a clone: a checkout never lands between an agent
//! start's read of the clone and its launch, a `disconnected` row does not
//! hold a clone forever, and "the same clone" means the directory on the
//! workstation, whichever project a branch belongs to.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::super::clone::checkout_for;
use super::super::guard::SetupGuard;
use super::super::host::SetupHost;
use super::super::verify::verify_session_checkout;
use super::super::SetupTrigger;
use super::checkout::commit;
use super::verify::{live_session, running_branch, running_host, start_session_on};
use super::{
    head_branch, origin_and_workstation_clone, set_status, set_up_under, starting_branch,
    status_of, store_with_project, FakeHost, WORKSPACE,
};
use crate::store::{self, SessionStatus, Store, WorkspaceStatus};
use crate::test_utils::run_git_in;

/// A session of `branch` whose proxy connection was lost: its turn may
/// still be going on the workstation.
fn disconnected_session(store: &Store, branch: &store::Branch) -> store::Session {
    let session = live_session(store, branch);
    store
        .update_session_status(&session.id, SessionStatus::Disconnected, None, None)
        .unwrap();
    session
}

/// A host whose reconcile service, asked to look at a disconnected session,
/// finds what became of it and writes `settled` on the row.
fn host_settling(clone: &Path, store: &Arc<Store>, settled: SessionStatus) -> Arc<FakeHost> {
    let mut host = running_host(clone);
    let store = Arc::clone(store);
    host.on_check_disconnected = Some(Box::new(move |session_id| {
        store
            .update_session_status(session_id, settled.clone(), None, None)
            .unwrap();
    }));
    Arc::new(host)
}

// ── The start's read and the launch are one step ────────────────────────────

/// A session start read the clone as on its branch and has not launched
/// yet: its row is not live, so nothing would stop a sibling branch's setup
/// from switching the clone under it. The start holds the clone until the
/// row is live. The setup that was waiting then finds a live session there
/// and leaves the clone alone.
#[tokio::test]
async fn a_checkout_does_not_land_between_a_starts_read_and_its_launch() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = running_branch(&store, &project, "feature-a");
    let sibling = starting_branch(&store, &project, "feature-b");
    run_git_in(clone.path(), &["checkout", "-b", "feature-a"]);
    let host = Arc::new(running_host(clone.path()));
    let dyn_host: Arc<dyn SetupHost> = host.clone();
    let guard = Arc::new(SetupGuard::default());

    // The start has read the clone and holds it.
    let hold = verify_session_checkout(&guard, &store, &dyn_host, &branch, "session-a")
        .await
        .unwrap();

    let setup = {
        let (guard, store, host, sibling) = (
            Arc::clone(&guard),
            Arc::clone(&store),
            Arc::clone(&host),
            sibling.clone(),
        );
        tokio::spawn(async move {
            set_up_under(&guard, &store, &host, &sibling, SetupTrigger::Requested).await
        })
    };
    // The setup gets as far as the clone and no further.
    host.wait_for_calls("workspace_status", 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(host.count("check_out"), 0);
    assert_eq!(head_branch(clone.path()), "feature-a");

    // The session goes live, and only then lets go.
    live_session(&store, &branch);
    drop(hold);

    let error = setup.await.unwrap().unwrap_err();
    assert!(error.contains("branch 'feature-a'"), "{error}");
    assert!(error.contains("running in the same clone"), "{error}");
    assert_eq!(host.count("check_out"), 0);
    assert_eq!(head_branch(clone.path()), "feature-a");
}

/// The other order: a start does not read a clone that is mid-checkout. It
/// reads after, sees where the clone was left, and repairs it.
#[tokio::test]
async fn a_start_does_not_read_a_clone_that_is_mid_checkout() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = running_branch(&store, &project, "feature-a");
    let sibling = starting_branch(&store, &project, "feature-b");
    run_git_in(clone.path(), &["checkout", "-b", "feature-a"]);
    let (host, gate) = running_host(clone.path()).gated();
    let host = Arc::new(host);
    let guard = Arc::new(SetupGuard::default());

    let setup = {
        let (guard, store, host, sibling) = (
            Arc::clone(&guard),
            Arc::clone(&store),
            Arc::clone(&host),
            sibling.clone(),
        );
        tokio::spawn(async move {
            set_up_under(&guard, &store, &host, &sibling, SetupTrigger::Requested).await
        })
    };
    host.wait_for_checkouts(1).await;

    let start = {
        let (guard, store, branch) = (Arc::clone(&guard), Arc::clone(&store), branch.clone());
        let host: Arc<dyn SetupHost> = host.clone();
        tokio::spawn(async move {
            verify_session_checkout(&guard, &store, &host, &branch, "session-a")
                .await
                .map(drop)
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(host.count("clone_head"), 0, "read under a checkout");

    gate.add_permits(8);
    assert_eq!(setup.await.unwrap(), Ok(Some(WorkspaceStatus::Running)));
    start.await.unwrap().unwrap();
    assert_eq!(head_branch(clone.path()), "feature-a");
}

// ── Disconnected sessions ───────────────────────────────────────────────────

/// A `disconnected` row used to hold its clone until the reconcile service
/// got round to it, up to half an hour later. Setup now asks the service to
/// look at once; a turn that turns out to be over stops counting, and the
/// repair goes ahead.
#[tokio::test]
async fn a_disconnected_session_that_is_over_stops_holding_the_clone() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let sibling = running_branch(&store, &project, "feature-a");
    let branch = running_branch(&store, &project, "feature-b");
    run_git_in(clone.path(), &["checkout", "-b", "feature-a"]);
    let lost = disconnected_session(&store, &sibling);
    let host = host_settling(clone.path(), &store, SessionStatus::Completed);

    start_session_on(&store, &host, &branch, "session-b")
        .await
        .unwrap();

    assert_eq!(host.checked_sessions(), vec![lost.id]);
    assert_eq!(head_branch(clone.path()), "feature-b");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
}

/// One whose turn is still going is claimed `running` again and holds the
/// clone for what it is.
#[tokio::test]
async fn a_disconnected_session_that_is_still_working_holds_the_clone() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let sibling = running_branch(&store, &project, "feature-a");
    let branch = running_branch(&store, &project, "feature-b");
    run_git_in(clone.path(), &["checkout", "-b", "feature-a"]);
    disconnected_session(&store, &sibling);
    let host = host_settling(clone.path(), &store, SessionStatus::Running);

    let error = start_session_on(&store, &host, &branch, "session-b")
        .await
        .unwrap_err();

    assert!(error.contains("is running in the same clone"), "{error}");
    assert_eq!(host.count("check_out"), 0);
    assert_eq!(head_branch(clone.path()), "feature-a");
}

/// One the service could not settle within the wait still counts as live,
/// and the refusal says what is being waited for. The branch that asked is
/// left `Running`.
#[tokio::test]
async fn a_disconnected_session_nobody_settled_holds_the_clone_and_says_so() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let sibling = running_branch(&store, &project, "feature-a");
    let branch = running_branch(&store, &project, "feature-b");
    run_git_in(clone.path(), &["checkout", "-b", "feature-a"]);
    let lost = disconnected_session(&store, &sibling);
    let host = Arc::new(running_host(clone.path()));

    let error = start_session_on(&store, &host, &branch, "session-b")
        .await
        .unwrap_err();

    assert!(error.contains("branch 'feature-a'"), "{error}");
    assert!(error.contains("lost its connection"), "{error}");
    assert!(error.contains("Staged is checking on it"), "{error}");
    assert_eq!(host.checked_sessions(), vec![lost.id]);
    assert_eq!(host.count("check_out"), 0);
    assert_eq!(head_branch(clone.path()), "feature-a");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
}

// ── What "the same clone" covers ────────────────────────────────────────────

/// Two projects on one repository and one workstation share the clone. A
/// live session of the other project's branch holds it like any other.
#[tokio::test]
async fn a_live_session_of_another_project_holds_the_clone() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let other_project = store::Project::named("widgets again").with_primary_repo("acme/widgets");
    store.create_project(&other_project).unwrap();
    let sibling = running_branch(&store, &other_project, "feature-a");
    let branch = running_branch(&store, &project, "feature-b");
    run_git_in(clone.path(), &["checkout", "-b", "feature-a"]);
    live_session(&store, &sibling);
    let host = Arc::new(running_host(clone.path()));

    let error = start_session_on(&store, &host, &branch, "session-b")
        .await
        .unwrap_err();

    assert!(error.contains("branch 'feature-a'"), "{error}");
    assert!(error.contains("running in the same clone"), "{error}");
    assert_eq!(host.count("check_out"), 0);
    assert_eq!(head_branch(clone.path()), "feature-a");
}

/// A session on another workstation is in another clone and holds nothing
/// here, even within the one project.
#[tokio::test]
async fn a_live_session_on_another_workstation_does_not_hold_the_clone() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let elsewhere =
        store::Branch::new_remote(&project.id, "feature-a", "main", "another-workstation");
    store.create_branch(&elsewhere).unwrap();
    let elsewhere = set_status(&store, &elsewhere, WorkspaceStatus::Running);
    live_session(&store, &elsewhere);
    let branch = running_branch(&store, &project, "feature-b");
    let host = Arc::new(running_host(clone.path()));

    start_session_on(&store, &host, &branch, "session-b")
        .await
        .unwrap();

    assert_eq!(head_branch(clone.path()), "feature-b");
}

/// The session a repair was asked for is not what keeps its own clone from
/// being repaired (a resume's row is `running` before its checkout check).
#[tokio::test]
async fn the_session_being_started_does_not_hold_its_own_clone() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = running_branch(&store, &project, "feature-b");
    let own = live_session(&store, &branch);
    let host = Arc::new(running_host(clone.path()));

    start_session_on(&store, &host, &branch, &own.id)
        .await
        .unwrap();

    assert_eq!(head_branch(clone.path()), "feature-b");
}

// ── A base branch that is worked on ─────────────────────────────────────────

/// Whether the base branch is one Staged works on in this clone is read
/// from the rows: a remote branch of that name on the same workstation and
/// clone, in any project.
#[test]
fn the_base_branch_is_worked_on_when_a_branch_of_that_name_shares_the_clone() {
    let (store, project) = store_with_project();
    let child = store::Branch::new_remote(&project.id, "feature-child", "feature-a", WORKSPACE);
    store.create_branch(&child).unwrap();
    let worked_on = |store: &Arc<Store>| checkout_for(store, &child).unwrap().base_is_worked_on;
    assert!(!worked_on(&store));

    // Not a local branch of that name, nor one on another workstation.
    store
        .create_branch(&store::Branch::new(&project.id, "feature-a", "main"))
        .unwrap();
    let elsewhere =
        store::Branch::new_remote(&project.id, "feature-a", "main", "another-workstation");
    store.create_branch(&elsewhere).unwrap();
    assert!(!worked_on(&store));

    let other_project = store::Project::named("widgets again").with_primary_repo("acme/widgets");
    store.create_project(&other_project).unwrap();
    starting_branch(&store, &other_project, "feature-a");
    assert!(worked_on(&store));

    // A branch named like its own base is not what works on that base.
    let (store, project) = store_with_project();
    let on_itself = store::Branch::new_remote(&project.id, "main", "main", WORKSPACE);
    store.create_branch(&on_itself).unwrap();
    assert!(!checkout_for(&store, &on_itself).unwrap().base_is_worked_on);
}

/// A stacked branch set up in a clone standing on its parent, which has
/// commits of its own that are not pushed: they stay the parent's, and the
/// child starts from origin's copy of the parent.
#[tokio::test]
async fn a_stacked_branch_does_not_take_its_parents_unpushed_commits() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    running_branch(&store, &project, "feature-a");
    run_git_in(clone.path(), &["checkout", "-b", "feature-a"]);
    commit(clone.path(), "pushed.txt", "pushed parent work");
    run_git_in(clone.path(), &["push", "origin", "feature-a"]);
    let pushed = run_git_in(clone.path(), &["rev-parse", "origin/feature-a"]);
    let unpushed = commit(clone.path(), "unpushed.txt", "unpushed parent work");
    let child = store::Branch::new_remote(&project.id, "feature-child", "feature-a", WORKSPACE);
    store.create_branch(&child).unwrap();
    let host = Arc::new(running_host(clone.path()));
    let guard = Arc::new(SetupGuard::default());

    set_up_under(&guard, &store, &host, &child, SetupTrigger::Requested)
        .await
        .unwrap();

    assert_eq!(head_branch(clone.path()), "feature-child");
    assert_eq!(run_git_in(clone.path(), &["rev-parse", "HEAD"]), pushed);
    let parent = run_git_in(clone.path(), &["rev-parse", "feature-a"]);
    assert_eq!(parent.trim(), unpushed);
}
