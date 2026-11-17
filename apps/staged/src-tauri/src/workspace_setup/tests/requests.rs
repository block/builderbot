//! What a caller of setup gets back: an answer to what it asked, whatever
//! run it happened to land on, and without being held for a wait that is
//! not its own.

use std::sync::Arc;
use std::time::Duration;

use super::super::guard::{Patience, SetupGuard};
use super::super::host::{ReadyWait, SetupHost};
use super::super::{run_guarded, SetupRequest, SetupTrigger};
use super::verify::{running_branch, running_host};
use super::{
    eventually, head_branch, origin_and_workstation_clone, set_status, set_up, set_up_under,
    starting_branch, status_of, store_with_project, FakeHost, Info, Start,
};
use crate::store::{self, Store, WorkspaceStatus};

/// A request as the `start_workspace` command makes it: it stays while the
/// run is working and leaves when the run is only waiting.
async fn request(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &Arc<FakeHost>,
    branch: &store::Branch,
) -> Result<Option<WorkspaceStatus>, String> {
    let host: Arc<dyn SetupHost> = host.clone();
    let request = SetupRequest::new(SetupTrigger::Requested);
    run_guarded(
        guard,
        store,
        &host,
        &branch.id,
        request,
        Patience::WhileWorking,
    )
    .await
}

/// A run in a gated checkout, holding its workstation's turn.
async fn hold_the_workstation(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &Arc<FakeHost>,
    branch: &store::Branch,
) -> tokio::task::JoinHandle<Result<Option<WorkspaceStatus>, String>> {
    let holder = {
        let (guard, store, host, branch) = (
            Arc::clone(guard),
            Arc::clone(store),
            Arc::clone(host),
            branch.clone(),
        );
        tokio::spawn(async move {
            set_up_under(&guard, &store, &host, &branch, SetupTrigger::Requested).await
        })
    };
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
    let observed = {
        let (guard, store, host, branch) = (
            Arc::clone(&guard),
            Arc::clone(&store),
            Arc::clone(&host),
            branch.clone(),
        );
        tokio::spawn(async move {
            set_up_under(&guard, &store, &host, &branch, SetupTrigger::Observed).await
        })
    };
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

/// A workstation that is still booting can take minutes. The request is
/// answered `Starting` as soon as the run starts waiting, so it does not
/// hold one of the frontend's setup slots; the run goes on and ends in a
/// checked-out `Running` branch.
#[tokio::test]
async fn a_request_does_not_wait_out_a_booting_workstation() {
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

    let answered = request(&guard, &store, &host, &branch).await;

    assert_eq!(answered, Ok(Some(WorkspaceStatus::Starting)));
    assert!(guard.is_in_flight(&branch.id));
    assert_eq!(host.count("start_workspace"), 1);
    assert_eq!(host.count("check_out"), 0);

    // The workstation arrives; nobody is waiting on the run.
    *host.info.lock().unwrap() = [Info::Status("running")].into_iter().collect();
    eventually("the run finishes on its own", || {
        status_of(&store, &branch) == Some(WorkspaceStatus::Running)
    })
    .await;
    assert_eq!(head_branch(clone.path()), "feature-new");
    assert_eq!(host.count("start_workspace"), 1);
}

/// The same for a run that is behind another branch's on the workstation,
/// which may itself be waiting for it: the request is answered `Starting`
/// at once, whatever the row said (a failed branch's Retry), and the run
/// takes its turn.
#[tokio::test]
async fn a_request_queued_behind_another_branch_is_answered_at_once() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let first = starting_branch(&store, &project, "feature-a");
    let branch = starting_branch(&store, &project, "feature-b");
    let branch = set_status(&store, &branch, WorkspaceStatus::Error);
    let (host, gate) =
        FakeHost::new(clone.path(), &[Info::Status("running")], Start::Succeeds).gated();
    let host = Arc::new(host);
    let guard = Arc::new(SetupGuard::default());
    let holder = hold_the_workstation(&guard, &store, &host, &first).await;

    let answered = request(&guard, &store, &host, &branch).await;

    assert_eq!(answered, Ok(Some(WorkspaceStatus::Starting)));
    assert!(guard.is_in_flight(&branch.id));
    assert_eq!(host.count("check_out"), 1);
    // The row agrees with the answer, so a poll of it does too.
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Starting));

    gate.add_permits(8);
    assert_eq!(holder.await.unwrap(), Ok(Some(WorkspaceStatus::Running)));
    eventually("the queued run takes its turn", || {
        status_of(&store, &branch) == Some(WorkspaceStatus::Running)
    })
    .await;
    assert_eq!(head_branch(clone.path()), "feature-b");
}

/// A request that can be answered while the run works is: `Running`, with
/// the branch checked out.
#[tokio::test]
async fn a_request_for_a_running_workstation_is_answered_running() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(running_host(clone.path()));
    let guard = Arc::new(SetupGuard::default());

    let answered = request(&guard, &store, &host, &branch).await;

    assert_eq!(answered, Ok(Some(WorkspaceStatus::Running)));
    assert_eq!(head_branch(clone.path()), "feature-new");
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
