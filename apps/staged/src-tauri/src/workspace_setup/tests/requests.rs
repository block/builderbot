//! What asking for setup does: the `start_workspace` request returns at
//! once and leaves the run to the backend, a caller that waits gets an
//! answer to what it asked, and only a failure of a setup the user asked for
//! is announced as theirs.

use std::sync::Arc;
use std::time::Duration;

use super::super::guard::SetupGuard;
use super::super::host::{ReadyWait, SetupHost};
use super::super::{request_setup, SetupTrigger};
use super::verify::{running_branch, running_host};
use super::{
    eventually, head_branch, origin_and_workstation_clone, set_status, set_up, set_up_under,
    starting_branch, status_of, store_with_project, FakeHost, Info, Start,
};
use crate::store::{self, Store, WorkspaceStatus};

/// A request as the `start_workspace` command makes it.
fn request(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &Arc<FakeHost>,
    branch: &store::Branch,
) -> Result<(), String> {
    let host: Arc<dyn SetupHost> = host.clone();
    request_setup(guard, store, &host, &branch.id)
}

type Waited = tokio::task::JoinHandle<Result<Option<WorkspaceStatus>, String>>;

/// A caller that waits on its setup, on a task of its own.
fn spawn_set_up(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &Arc<FakeHost>,
    branch: &store::Branch,
    trigger: SetupTrigger,
) -> Waited {
    let (guard, store, host, branch) = (
        Arc::clone(guard),
        Arc::clone(store),
        Arc::clone(host),
        branch.clone(),
    );
    tokio::spawn(async move { set_up_under(&guard, &store, &host, &branch, trigger).await })
}

/// A run in a gated checkout, holding its workstation's turn.
async fn hold_the_workstation(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &Arc<FakeHost>,
    branch: &store::Branch,
) -> Waited {
    let holder = spawn_set_up(guard, store, host, branch, SetupTrigger::Requested);
    host.wait_for_checkouts(1).await;
    holder
}

/// A resolved request used to mean nothing about the branch: a `Requested`
/// call that joined a poll's run got that run's `Ok`, and when the run had
/// found nothing to do (a poll had moved the row on while it queued) the
/// branch was left as it was with the caller told all was well. The caller
/// now looks again and runs its own setup.
#[tokio::test]
async fn a_request_that_joins_a_run_with_nothing_to_do_still_gets_its_setup() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let first = starting_branch(&store, &project, "feature-a");
    let branch = starting_branch(&store, &project, "feature-b");
    let (host, gate) =
        FakeHost::new(clone.path(), &[Info::Status("running")], Start::Succeeds).gated();
    let host = Arc::new(host);
    let guard = Arc::new(SetupGuard::default());

    // Another branch's run has the workstation, so the poll's run for this
    // branch queues behind it.
    let holder = hold_the_workstation(&guard, &store, &host, &first).await;
    let observed = spawn_set_up(&guard, &store, &host, &branch, SetupTrigger::Observed);
    eventually("the observed run is in flight", || {
        guard.is_in_flight(&branch.id)
    })
    .await;
    // A poll sees the workstation suspended: the observed run, when its
    // turn comes, has nothing to do.
    set_status(&store, &branch, WorkspaceStatus::Suspended);

    // The request joins that run on its first poll, before the gate opens.
    let (requested, ()) = tokio::join!(
        set_up_under(&guard, &store, &host, &branch, SetupTrigger::Requested),
        async {
            tokio::task::yield_now().await;
            gate.add_permits(8);
        }
    );

    assert_eq!(requested, Ok(Some(WorkspaceStatus::Running)));
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
    assert_eq!(head_branch(clone.path()), "feature-b");
    // The observed caller's own run did nothing, which is its answer.
    assert!(observed.await.unwrap().is_ok());
    assert_eq!(holder.await.unwrap(), Ok(Some(WorkspaceStatus::Running)));
    assert_eq!(host.count("check_out"), 2);
}

/// A workstation that is still booting can take minutes. The request does
/// not wait for it, or for anything: it returns with the run under way, and
/// the run ends in a checked-out `Running` branch with nobody waiting.
#[tokio::test]
async fn a_request_returns_at_once_and_the_backend_finishes_the_setup() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let mut host = FakeHost::new(
        clone.path(),
        &[Info::Unreachable],
        Start::Fails("timed out"),
    );
    // Far longer than the test: the request must not be what waits it out.
    host.ready_wait = ReadyWait {
        timeout: Duration::from_secs(600),
        interval: Duration::from_millis(5),
    };
    let host = Arc::new(host);
    let guard = Arc::new(SetupGuard::default());

    request(&guard, &store, &host, &branch).unwrap();

    assert!(guard.is_in_flight(&branch.id));
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Starting));
    assert_eq!(host.count("check_out"), 0);

    // Asking again while it runs starts nothing more.
    host.wait_for_calls("start_workspace", 1).await;
    request(&guard, &store, &host, &branch).unwrap();

    // The workstation arrives; nobody is waiting on the run.
    *host.info.lock().unwrap() = [Info::Status("running")].into_iter().collect();
    eventually("the run finishes on its own", || {
        status_of(&store, &branch) == Some(WorkspaceStatus::Running)
    })
    .await;
    assert_eq!(head_branch(clone.path()), "feature-new");
    assert_eq!(host.count("start_workspace"), 1);
    assert_eq!(host.count("check_out"), 1);
}

/// The error view's Retry. The row says `Starting` and its reason is gone
/// before the request returns, so the card that asked turns at once, also
/// when the run is queued behind another branch's. Here the run it joins is
/// a retry timer's, waiting for its turn: that run is the user's now, and
/// its failure is announced as theirs.
#[tokio::test]
async fn a_retry_request_turns_the_card_at_once_and_fails_as_the_users() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let first = starting_branch(&store, &project, "feature-a");
    let branch = starting_branch(&store, &project, "feature-b");
    let branch = set_status(&store, &branch, WorkspaceStatus::Error);
    let mut host = FakeHost::new(clone.path(), &[Info::Status("running")], Start::Succeeds);
    host.checkout_error = Some("could not fetch base branch 'main': connection reset");
    let (host, gate) = host.gated();
    let host = Arc::new(host);
    let guard = Arc::new(SetupGuard::default());
    let holder = hold_the_workstation(&guard, &store, &host, &first).await;
    let retry = spawn_set_up(&guard, &store, &host, &branch, SetupTrigger::Retry);
    eventually("the retry is in flight", || guard.is_in_flight(&branch.id)).await;

    request(&guard, &store, &host, &branch).unwrap();

    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Starting));
    assert!(host.failures().contains(&(branch.id.clone(), None)));
    assert_eq!(host.count("check_out"), 1);

    gate.add_permits(8);
    holder.await.unwrap().unwrap_err();
    retry.await.unwrap().unwrap_err();
    assert_eq!(host.count("check_out"), 2);
    eventually("the queued run fails", || {
        status_of(&store, &branch) == Some(WorkspaceStatus::Error)
    })
    .await;
    eventually("the failure is announced", || {
        host.failure_of(&branch).is_some()
    })
    .await;
    assert_eq!(host.user_asked_failures(), vec![branch.id.clone()]);
}

/// A new branch's request finds its row `Starting` with nothing running:
/// its failure is the user's too. A request that joins a run something else
/// started for a `Starting` row (the startup pass, a poll) is not what set
/// it going, and that run fails into the card only.
#[tokio::test]
async fn only_a_request_that_set_the_run_going_makes_its_failure_the_users() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let new = starting_branch(&store, &project, "feature-new");
    let left = starting_branch(&store, &project, "feature-left-starting");
    let mut host = FakeHost::new(clone.path(), &[Info::Status("running")], Start::Succeeds);
    host.checkout_error = Some("could not fetch base branch 'main': connection reset");
    let (host, gate) = host.gated();
    let host = Arc::new(host);
    let guard = Arc::new(SetupGuard::default());

    // The startup pass's run for the branch a previous launch left behind.
    let observed = spawn_set_up(&guard, &store, &host, &left, SetupTrigger::Observed);
    host.wait_for_checkouts(1).await;
    // A window loads and asks for both.
    request(&guard, &store, &host, &left).unwrap();
    request(&guard, &store, &host, &new).unwrap();
    gate.add_permits(8);

    observed.await.unwrap().unwrap_err();
    eventually("both runs fail", || {
        host.failure_of(&left).is_some() && host.failure_of(&new).is_some()
    })
    .await;
    assert_eq!(host.user_asked_failures(), vec![new.id.clone()]);
}

/// A request for a branch that is already `Running` is told so and starts
/// nothing. Any other trigger with nothing to do is told what the row says.
#[tokio::test]
async fn a_caller_with_nothing_to_do_is_told_what_the_row_says() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let running = running_branch(&store, &project, "feature-running");
    let failed = starting_branch(&store, &project, "feature-failed");
    let failed = set_status(&store, &failed, WorkspaceStatus::Error);
    let host = Arc::new(running_host(clone.path()));

    assert_eq!(
        set_up(&store, &host, &running, SetupTrigger::Requested).await,
        Ok(Some(WorkspaceStatus::Running))
    );
    assert_eq!(
        set_up(&store, &host, &failed, SetupTrigger::Observed).await,
        Ok(Some(WorkspaceStatus::Error))
    );
    assert_eq!(
        set_up(&store, &host, &running, SetupTrigger::Retry).await,
        Ok(Some(WorkspaceStatus::Running))
    );
    assert_eq!(host.calls.lock().unwrap().len(), 0);
    assert_eq!(head_branch(clone.path()), "main");
}
