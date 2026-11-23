//! The status pollers: what a poll writes, and what it asks setup for
//! instead of writing `Running` itself.

use std::sync::{Arc, Mutex};

use super::super::poll::{self, PollEffects, Reported};
use super::super::SetupTrigger;
use super::{
    head_branch, origin_and_workstation_clone, set_status, set_up, starting_branch, status_of,
    store_with_project, FakeHost, Info, Start, WORKSPACE,
};
use crate::blox::{BloxError, WorkspaceListEntry};
use crate::store::{self, Store, WorkspaceStatus};

/// What a poll did besides writing the row.
#[derive(Default)]
struct Seen {
    written: Mutex<Vec<(String, bool, WorkspaceStatus)>>,
    triggered: Mutex<Vec<String>>,
    retry_armed: Mutex<Vec<String>>,
    failed: Mutex<Vec<(String, String)>>,
}

impl Seen {
    fn effects(&self) -> PollEffects<'_> {
        PollEffects {
            status_written: Box::new(|branch_id, changed, status| {
                self.written
                    .lock()
                    .unwrap()
                    .push((branch_id.to_string(), changed, status.clone()));
            }),
            trigger_setup: Box::new(|branch_id| {
                self.triggered.lock().unwrap().push(branch_id.to_string());
            }),
            arm_retry: Box::new(|branch_id| {
                self.retry_armed.lock().unwrap().push(branch_id.to_string());
            }),
            failed: Box::new(|branch_id, reason| {
                self.failed
                    .lock()
                    .unwrap()
                    .push((branch_id.to_string(), reason.to_string()));
            }),
        }
    }

    fn retry_armed(&self) -> Vec<String> {
        self.retry_armed.lock().unwrap().clone()
    }

    fn failed(&self) -> Vec<(String, String)> {
        self.failed.lock().unwrap().clone()
    }

    fn triggered(&self) -> Vec<String> {
        self.triggered.lock().unwrap().clone()
    }

    fn written(&self) -> Vec<(String, bool, WorkspaceStatus)> {
        self.written.lock().unwrap().clone()
    }
}

fn listed(status: &str) -> Vec<WorkspaceListEntry> {
    vec![WorkspaceListEntry {
        name: WORKSPACE.to_string(),
        workstation_id: Some(42),
        status: Some(status.to_string()),
        extra: serde_json::Value::Null,
    }]
}

fn poll_batch(store: &Store, branch: &store::Branch, blox_status: &str, seen: &Seen) -> String {
    let settled = poll::settle_all(
        store,
        &[branch.id.clone()],
        &listed(blox_status),
        &seen.effects(),
    );
    settled.results[&branch.id].status.clone()
}

fn poll_single(store: &Store, branch: &store::Branch, blox_status: &str, seen: &Seen) -> String {
    poll::settle_one(
        store,
        branch,
        WORKSPACE,
        Ok(Reported {
            status: Some(blox_status),
            workstation_id: Some(42),
        }),
        &seen.effects(),
    )
    .unwrap()
    .status
}

/// The other half of the bug: a poll saw the workstation running and marked
/// the branch `Running` with nothing checked out. Both pollers now leave the
/// row in `Starting` and ask for setup.
#[test]
fn a_poll_triggers_setup_instead_of_promoting_a_starting_branch() {
    for poll in [poll_batch, poll_single] {
        for blox_status in ["running", "ready", "active"] {
            let (store, project) = store_with_project();
            let branch = starting_branch(&store, &project, "feature-new");
            let seen = Seen::default();

            let reported = poll(&store, &branch, blox_status, &seen);

            assert_eq!(reported, "starting");
            assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Starting));
            assert_eq!(seen.triggered(), vec![branch.id.clone()]);
            // Nothing was written, so no session queue was kicked.
            assert_eq!(seen.written(), vec![]);
        }
    }
}

/// A batch poll also keeps fetching bootstrap progress for such a branch.
#[test]
fn a_batch_poll_keeps_a_branch_awaiting_setup_among_the_starting() {
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let seen = Seen::default();

    let settled = poll::settle_all(
        &store,
        &[branch.id.clone()],
        &listed("running"),
        &seen.effects(),
    );

    assert_eq!(
        settled.starting_workspaces.get(WORKSPACE),
        Some(&vec![branch.id.clone()])
    );
}

/// A setup that failed left the row in `Error`. A client still polling on a
/// stale `starting` is told so, rather than handed a `Running` branch whose
/// checkout never happened. The workstation being up is what a retry waits
/// for, so the poll makes sure the branch has its timer.
#[test]
fn a_poll_does_not_clear_a_failed_setup() {
    for poll in [poll_batch, poll_single] {
        let (store, project) = store_with_project();
        let branch = starting_branch(&store, &project, "feature-new");
        let branch = set_status(&store, &branch, WorkspaceStatus::Error);
        let seen = Seen::default();

        let reported = poll(&store, &branch, "running", &seen);

        assert_eq!(reported, "error");
        assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
        assert_eq!(seen.triggered(), Vec::<String>::new());
        assert_eq!(seen.retry_armed(), vec![branch.id.clone()]);
        assert_eq!(seen.written(), vec![]);
    }
}

/// A workstation that is running behind a row that says `Stopped`,
/// `Suspended` or nothing was resumed outside Staged, or by a poll that had
/// moved a `Starting` row to `Suspended` on the way. Nothing has read its
/// clone, so neither poller says `Running`: the row goes to `Starting` and
/// setup is asked for, the same road a new branch takes.
#[test]
fn a_poll_sends_a_branch_whose_workstation_came_back_through_setup() {
    use WorkspaceStatus::*;
    for poll in [poll_batch, poll_single] {
        for row in [Some(Suspended), Some(Stopped), None] {
            let (store, project) = store_with_project();
            let mut branch =
                store::Branch::new_remote(&project.id, "feature-new", "main", WORKSPACE);
            branch.workspace_status = row.clone();
            store.create_branch(&branch).unwrap();
            let branch = store.get_branch(&branch.id).unwrap().unwrap();
            assert_eq!(branch.workspace_status, row);
            let seen = Seen::default();

            let reported = poll(&store, &branch, "running", &seen);

            assert_eq!(reported, "starting", "{row:?}");
            assert_eq!(status_of(&store, &branch), Some(Starting), "{row:?}");
            assert_eq!(seen.triggered(), vec![branch.id.clone()], "{row:?}");
            // A `Starting` write kicks no session queue.
            assert_eq!(seen.written(), vec![(branch.id.clone(), true, Starting)]);
        }
    }
}

/// The two-poll route to an unverified `Running`: a `Starting` branch whose
/// workstation is seen suspended, then running. The second poll puts it back
/// in `Starting` and asks for setup, which is what ends in `Running`.
#[tokio::test]
async fn a_starting_branch_seen_suspended_then_running_is_still_set_up() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(FakeHost::new(
        clone.path(),
        &[Info::Status("running")],
        Start::Succeeds,
    ));
    let seen = Seen::default();

    assert_eq!(poll_batch(&store, &branch, "suspended", &seen), "suspended");
    let branch = store.get_branch(&branch.id).unwrap().unwrap();
    assert_eq!(poll_batch(&store, &branch, "running", &seen), "starting");
    assert_eq!(head_branch(clone.path()), "main");
    assert_eq!(seen.triggered(), vec![branch.id.clone()]);

    set_up(&store, &host, &branch, SetupTrigger::Observed)
        .await
        .unwrap();

    assert_eq!(head_branch(clone.path()), "feature-new");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
}

/// A poll that moves a row to `Error` itself says why, so the branch has a
/// reason to show and a retry timer like one setup failed.
#[test]
fn a_poll_that_fails_a_branch_says_why() {
    for poll in [poll_batch, poll_single] {
        for (row, blox_status) in [
            (WorkspaceStatus::Running, "degraded"),
            (WorkspaceStatus::Starting, "failed"),
        ] {
            let (store, project) = store_with_project();
            let branch = starting_branch(&store, &project, "feature-new");
            let branch = set_status(&store, &branch, row);
            let seen = Seen::default();

            assert_eq!(poll(&store, &branch, blox_status, &seen), "error");

            let failed = seen.failed();
            assert_eq!(failed.len(), 1, "{failed:?}");
            assert_eq!(failed[0].0, branch.id);
            assert!(failed[0].1.contains(blox_status), "{failed:?}");
            assert!(failed[0].1.contains(WORKSPACE), "{failed:?}");

            // Seen failed again, the reason it already has is kept.
            let branch = store.get_branch(&branch.id).unwrap().unwrap();
            assert_eq!(poll(&store, &branch, blox_status, &seen), "error");
            assert_eq!(seen.failed().len(), 1);
        }
    }

    // A signed-out `sq` ends a single poll in `Error` too.
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let seen = Seen::default();
    let error = poll::settle_one(
        &store,
        &branch,
        WORKSPACE,
        Err(&BloxError::NotAuthenticated),
        &seen.effects(),
    )
    .err()
    .unwrap();
    assert_eq!(error, "Not authenticated with Blox. Run: sq login");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
    assert_eq!(seen.failed(), vec![(branch.id.clone(), error)]);
}

/// Every transition that does not end in `Running` is still the pollers',
/// and so is confirming a row setup already marked `Running`.
#[test]
fn a_poll_still_owns_the_other_transitions() {
    use WorkspaceStatus::*;
    for poll in [poll_batch, poll_single] {
        for (row, blox_status, expected) in [
            (Running, "suspended", Suspended),
            (Running, "stopped", Stopped),
            (Running, "degraded", Error),
            (Running, "running", Running),
            (Starting, "suspended", Suspended),
            (Starting, "failed", Error),
            (Starting, "provisioning", Starting),
            // Blox briefly reports a new workstation as stopped.
            (Starting, "stopped", Starting),
            (Suspended, "stopped", Stopped),
            (Stopped, "suspended", Suspended),
        ] {
            let (store, project) = store_with_project();
            let branch = starting_branch(&store, &project, "feature-new");
            let branch = set_status(&store, &branch, row.clone());
            let seen = Seen::default();

            let reported = poll(&store, &branch, blox_status, &seen);

            assert_eq!(reported, expected.as_str(), "{row:?} + {blox_status}");
            assert_eq!(status_of(&store, &branch), Some(expected.clone()));
            assert_eq!(seen.triggered(), Vec::<String>::new());
            let changed = row != expected;
            assert_eq!(
                seen.written(),
                vec![(branch.id.clone(), changed, expected)],
                "{row:?} + {blox_status}"
            );
        }
    }
}

/// A repo joining a workstation a peer already runs on is still held at
/// `Starting` by the pollers, and left to the setup that is cloning it.
#[test]
fn a_secondary_clone_is_still_held_at_starting() {
    let (store, project) = store_with_project();
    let peer = starting_branch(&store, &project, "feature-one");
    set_status(&store, &peer, WorkspaceStatus::Running);
    let repo = store::ProjectRepo::new(&project.id, "acme/gadgets", "feature-two", None);
    store.create_project_repo(&repo).unwrap();
    let secondary = store::Branch::new_remote(&project.id, "feature-two", "main", WORKSPACE)
        .with_project_repo(&repo.id);
    store.create_branch(&secondary).unwrap();
    let seen = Seen::default();

    assert_eq!(poll::is_secondary_clone_setup(&store, &secondary), Ok(true));
    assert_eq!(poll_batch(&store, &secondary, "running", &seen), "starting");

    assert_eq!(
        status_of(&store, &secondary),
        Some(WorkspaceStatus::Starting)
    );
    assert_eq!(seen.triggered(), Vec::<String>::new());
    assert_eq!(seen.written(), vec![]);
}

/// The poll's trigger, followed through: the branch a poll refused to
/// promote ends up checked out and `Running` by setup's hand.
#[tokio::test]
async fn a_polled_starting_branch_is_set_up_and_then_reported_running() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(FakeHost::new(
        clone.path(),
        &[Info::Status("running")],
        Start::Succeeds,
    ));
    let seen = Seen::default();

    assert_eq!(poll_batch(&store, &branch, "running", &seen), "starting");
    for branch_id in seen.triggered() {
        assert_eq!(branch_id, branch.id);
        set_up(&store, &host, &branch, SetupTrigger::Observed)
            .await
            .unwrap();
    }

    assert_eq!(host.count("start_workspace"), 0);
    assert_eq!(head_branch(clone.path()), "feature-new");
    let branch = store.get_branch(&branch.id).unwrap().unwrap();
    assert_eq!(poll_batch(&store, &branch, "running", &seen), "running");
    assert_eq!(seen.triggered().len(), 1);
}
