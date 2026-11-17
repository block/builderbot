//! Branch setup, its guard, the pollers' share of it and the startup pass,
//! driven without `sq`: a [`FakeHost`] answers for Blox, and a local clone
//! of a bare origin stands in for the workstation's, reached through the
//! same [`branches::GitRunner`] the real checkout runs on.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Semaphore;

use super::guard::SetupGuard;
use super::poll::{self, PollEffects, Reported};
use super::{branches_awaiting_setup, run_guarded, Checkout, ReadyWait, SetupHost, SetupTrigger};
use crate::blox::{BloxError, WorkspaceListEntry};
use crate::branches;
use crate::store::{self, Store, WorkspaceStatus};
use crate::test_utils::{run_git_in, TempDir, TempGitRepo};

const WORKSPACE: &str = "widgets-setup-test";

// ── Fixtures ────────────────────────────────────────────────────────────────

/// A bare origin with `main`, and the clone Blox's bootstrap leaves behind:
/// on `main`, nothing else checked out.
fn origin_and_workstation_clone() -> (TempDir, TempDir) {
    let seed = TempGitRepo::new();
    seed.write_file("file.txt", "base\n");
    seed.commit("base");
    let origin = TempDir::new("origin");
    run_git_in(
        seed.path(),
        &["clone", "--bare", ".", origin.path().to_str().unwrap()],
    );
    let clone = TempDir::new("workstation");
    run_git_in(
        origin.path(),
        &[
            "clone",
            origin.path().to_str().unwrap(),
            clone.path().to_str().unwrap(),
        ],
    );
    run_git_in(clone.path(), &["config", "user.email", "test@example.com"]);
    run_git_in(clone.path(), &["config", "user.name", "Test"]);
    (origin, clone)
}

fn head_branch(clone: &Path) -> String {
    run_git_in(clone, &["rev-parse", "--abbrev-ref", "HEAD"])
        .trim()
        .to_string()
}

fn store_with_project() -> (Arc<Store>, store::Project) {
    let store = Arc::new(Store::in_memory().unwrap());
    let project = store::Project::new("acme/widgets");
    store.create_project(&project).unwrap();
    (store, project)
}

/// A remote branch in `Starting`, the way `create_remote_branch` leaves it.
fn starting_branch(store: &Store, project: &store::Project, name: &str) -> store::Branch {
    let branch = store::Branch::new_remote(&project.id, name, "main", WORKSPACE);
    store.create_branch(&branch).unwrap();
    branch
}

fn status_of(store: &Store, branch: &store::Branch) -> Option<WorkspaceStatus> {
    store
        .get_branch(&branch.id)
        .unwrap()
        .unwrap()
        .workspace_status
}

fn set_status(store: &Store, branch: &store::Branch, status: WorkspaceStatus) -> store::Branch {
    store
        .update_branch_workspace_status(&branch.id, &status)
        .unwrap();
    store.get_branch(&branch.id).unwrap().unwrap()
}

/// What the fake workstation answers a status lookup with.
#[derive(Clone, Copy)]
enum Info {
    Status(&'static str),
    /// `ws info` failed: the workstation isn't listed yet, or the call
    /// dropped.
    Unreachable,
}

#[derive(Clone, Copy)]
enum Start {
    Succeeds,
    /// Exits non-zero or times out, with nothing about auth or onboarding.
    Fails(&'static str),
    NotAuthenticated,
}

/// Blox and the workstation's git, faked.
struct FakeHost {
    clone: PathBuf,
    /// Answers to successive status lookups; the last one repeats.
    info: Mutex<VecDeque<Info>>,
    start: Start,
    checkout_error: Option<&'static str>,
    /// When set, a checkout waits for a permit before touching the clone.
    gate: Option<Arc<Semaphore>>,
    /// Run at each status lookup: what else happens while setup is out.
    meanwhile: Option<Box<dyn Fn() + Send + Sync>>,
    calls: Mutex<Vec<&'static str>>,
    written: Mutex<Vec<(String, bool, WorkspaceStatus)>>,
    in_checkout: AtomicUsize,
    most_in_checkout: AtomicUsize,
}

impl FakeHost {
    fn new(clone: &Path, info: &[Info], start: Start) -> Self {
        Self {
            clone: clone.to_path_buf(),
            info: Mutex::new(info.iter().copied().collect()),
            start,
            checkout_error: None,
            gate: None,
            meanwhile: None,
            calls: Mutex::new(Vec::new()),
            written: Mutex::new(Vec::new()),
            in_checkout: AtomicUsize::new(0),
            most_in_checkout: AtomicUsize::new(0),
        }
    }

    fn gated(mut self) -> (Self, Arc<Semaphore>) {
        let gate = Arc::new(Semaphore::new(0));
        self.gate = Some(Arc::clone(&gate));
        (self, gate)
    }

    fn count(&self, call: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| **c == call)
            .count()
    }

    fn written(&self) -> Vec<(String, bool, WorkspaceStatus)> {
        self.written.lock().unwrap().clone()
    }

    async fn wait_for_checkouts(&self, count: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while self.count("check_out") < count {
            assert!(
                std::time::Instant::now() < deadline,
                "checkout never started"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

#[async_trait]
impl SetupHost for FakeHost {
    async fn check_auth(&self) -> Result<(), BloxError> {
        self.calls.lock().unwrap().push("check_auth");
        Ok(())
    }

    async fn workspace_status(&self, _workspace: &str) -> Result<Option<String>, BloxError> {
        self.calls.lock().unwrap().push("workspace_status");
        if let Some(meanwhile) = &self.meanwhile {
            meanwhile();
        }
        let info = {
            let mut info = self.info.lock().unwrap();
            if info.len() > 1 {
                info.pop_front().unwrap()
            } else {
                *info.front().unwrap()
            }
        };
        match info {
            Info::Status(status) => Ok(Some(status.to_string())),
            Info::Unreachable => Err(BloxError::CommandFailed("workspace not found".into())),
        }
    }

    async fn start_workspace(&self, _workspace: &str, _source: &str) -> Result<(), BloxError> {
        self.calls.lock().unwrap().push("start_workspace");
        match self.start {
            Start::Succeeds => Ok(()),
            Start::Fails(message) => Err(BloxError::CommandFailed(message.to_string())),
            Start::NotAuthenticated => Err(BloxError::NotAuthenticated),
        }
    }

    async fn check_out(&self, checkout: &Checkout) -> Result<(), String> {
        self.calls.lock().unwrap().push("check_out");
        if let Some(gate) = &self.gate {
            gate.acquire().await.unwrap().forget();
        }
        let now = self.in_checkout.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_in_checkout.fetch_max(now, Ordering::SeqCst);
        // Yield with the clone "busy", so a second checkout let in beside
        // this one would be counted.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let result = match self.checkout_error {
            Some(error) => Err(error.to_string()),
            None => branches::check_out_workspace_branch(
                &branches::git_runner(&self.clone, None, None),
                &checkout.base_ref,
                &checkout.branch_name,
            ),
        };
        self.in_checkout.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn status_written(&self, branch_id: &str, changed: bool, status: &WorkspaceStatus) {
        self.written
            .lock()
            .unwrap()
            .push((branch_id.to_string(), changed, status.clone()));
    }

    fn ready_wait(&self) -> ReadyWait {
        ReadyWait {
            timeout: Duration::from_millis(200),
            interval: Duration::from_millis(5),
        }
    }
}

async fn set_up(
    store: &Arc<Store>,
    host: &Arc<FakeHost>,
    branch: &store::Branch,
    trigger: SetupTrigger,
) -> Result<(), String> {
    let guard = Arc::new(SetupGuard::default());
    let host: Arc<dyn SetupHost> = host.clone();
    run_guarded(&guard, store, &host, &branch.id, trigger).await
}

// ── The setup routine ───────────────────────────────────────────────────────

/// The run that left a workstation on `main`: `sq blox ws start` fails after
/// Blox created the workstation, which goes on to boot. Setup used to return
/// success there and leave the checkout undone. It now waits for the
/// workstation and checks the branch out before saying `Running`.
#[tokio::test]
async fn a_failed_ws_start_still_ends_in_a_checked_out_branch() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(FakeHost::new(
        clone.path(),
        &[
            Info::Unreachable,
            Info::Status("creating"),
            Info::Status("starting"),
            Info::Status("running"),
        ],
        Start::Fails("sq blox command timed out after 120s"),
    ));

    set_up(&store, &host, &branch, SetupTrigger::Requested)
        .await
        .unwrap();

    assert_eq!(head_branch(clone.path()), "feature-new");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
    assert_eq!(
        host.written(),
        vec![(branch.id.clone(), true, WorkspaceStatus::Running)]
    );
    assert_eq!(host.count("start_workspace"), 1);
}

/// When the workstation never comes up after a failed start, the branch
/// fails where the user can see it, with the start error, instead of
/// sitting in `Starting`.
#[tokio::test]
async fn a_failed_ws_start_with_no_workstation_fails_visibly() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(FakeHost::new(
        clone.path(),
        &[Info::Unreachable, Info::Status("creating")],
        Start::Fails("quota exceeded"),
    ));

    let error = set_up(&store, &host, &branch, SetupTrigger::Requested)
        .await
        .unwrap_err();

    assert!(error.contains("did not come up"), "{error}");
    assert!(error.contains("quota exceeded"), "{error}");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
    assert_eq!(host.count("check_out"), 0);
    assert_eq!(host.written(), vec![]);
    assert_eq!(head_branch(clone.path()), "main");
}

/// A workstation Blox reports as failed ends the wait at once.
#[tokio::test]
async fn a_workstation_that_fails_to_boot_ends_the_wait() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(FakeHost::new(
        clone.path(),
        &[Info::Unreachable, Info::Status("failed")],
        Start::Fails("connection reset"),
    ));

    let error = set_up(&store, &host, &branch, SetupTrigger::Requested)
        .await
        .unwrap_err();

    assert!(error.contains("failed to start"), "{error}");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
    // The first lookup, then the one that saw `failed`: no waiting it out.
    assert_eq!(host.count("workspace_status"), 2);
}

/// A relaunch can find the workstation a previous run started still booting,
/// and a resume finds it coming back. Either way it is waited for: a second
/// `ws start` on that name is never sent.
#[tokio::test]
async fn a_booting_workstation_is_waited_for_not_started_again() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(FakeHost::new(
        clone.path(),
        &[
            Info::Status("starting"),
            Info::Status("starting"),
            Info::Status("running"),
        ],
        Start::Fails("workspace already exists"),
    ));

    set_up(&store, &host, &branch, SetupTrigger::Observed)
        .await
        .unwrap();

    assert_eq!(host.count("start_workspace"), 0);
    assert_eq!(head_branch(clone.path()), "feature-new");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
}

/// ...and one that never arrives is a visible failure too.
#[tokio::test]
async fn a_booting_workstation_that_never_arrives_fails_visibly() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(FakeHost::new(
        clone.path(),
        &[Info::Status("provisioning")],
        Start::Succeeds,
    ));

    let error = set_up(&store, &host, &branch, SetupTrigger::Observed)
        .await
        .unwrap_err();

    assert!(error.contains("still booting"), "{error}");
    assert_eq!(host.count("start_workspace"), 0);
    assert_eq!(host.count("check_out"), 0);
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
}

/// A poll that saw the workstation suspended while setup waited has said
/// something truer than `Error`, and the branch keeps its Resume button. The
/// caller still gets the failure.
#[tokio::test]
async fn a_failed_setup_does_not_overwrite_what_a_poll_settled() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let mut host = FakeHost::new(
        clone.path(),
        &[Info::Status("suspended")],
        Start::Fails("workspace is suspended"),
    );
    host.meanwhile = Some(Box::new({
        let (store, id) = (Arc::clone(&store), branch.id.clone());
        move || {
            store
                .update_branch_workspace_status(&id, &WorkspaceStatus::Suspended)
                .unwrap();
        }
    }));
    let host = Arc::new(host);

    let error = set_up(&store, &host, &branch, SetupTrigger::Observed)
        .await
        .unwrap_err();

    assert!(error.contains("did not come up"), "{error}");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Suspended));
}

/// A request for a branch whose last setup failed is a retry. The row goes
/// back to `Starting` for its length, where the pollers hold it, and ends
/// `Running` once the checkout is done.
#[tokio::test]
async fn a_request_retries_a_failed_branch_from_starting() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let branch = set_status(&store, &branch, WorkspaceStatus::Error);
    let during = Arc::new(Mutex::new(Vec::new()));
    let mut host = FakeHost::new(clone.path(), &[Info::Status("running")], Start::Succeeds);
    host.meanwhile = Some(Box::new({
        let (store, id, during) = (Arc::clone(&store), branch.id.clone(), Arc::clone(&during));
        move || {
            let row = store.get_branch(&id).unwrap().unwrap();
            during.lock().unwrap().push(row.workspace_status);
        }
    }));
    let host = Arc::new(host);

    set_up(&store, &host, &branch, SetupTrigger::Requested)
        .await
        .unwrap();

    assert_eq!(
        *during.lock().unwrap(),
        vec![Some(WorkspaceStatus::Starting)]
    );
    assert_eq!(head_branch(clone.path()), "feature-new");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
}

/// Auth failures stay definitive: no wait, and the message says what to do.
#[tokio::test]
async fn an_unauthenticated_ws_start_is_not_waited_out() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(FakeHost::new(
        clone.path(),
        &[Info::Unreachable],
        Start::NotAuthenticated,
    ));

    let error = set_up(&store, &host, &branch, SetupTrigger::Requested)
        .await
        .unwrap_err();

    assert_eq!(error, "Not authenticated with Blox. Run: sq login");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
    assert_eq!(host.count("workspace_status"), 1);
}

/// A checkout that fails is the command's error and the branch's `Error`.
/// It used to be a log line under a success.
#[tokio::test]
async fn a_failed_checkout_is_returned_and_never_marked_running() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let mut host = FakeHost::new(clone.path(), &[Info::Unreachable], Start::Succeeds);
    host.checkout_error = Some("could not create branch 'feature-new': index.lock exists");
    let host = Arc::new(host);

    let error = set_up(&store, &host, &branch, SetupTrigger::Requested)
        .await
        .unwrap_err();

    assert!(error.contains("index.lock"), "{error}");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
    assert_eq!(host.written(), vec![]);
}

/// A fresh workstation and one that is already up end in the same checkout;
/// only the second skips `ws start`.
#[tokio::test]
async fn fresh_and_running_workstations_share_one_checkout() {
    for (info, starts) in [(Info::Unreachable, 1), (Info::Status("running"), 0)] {
        let (_origin, clone) = origin_and_workstation_clone();
        let (store, project) = store_with_project();
        let branch = starting_branch(&store, &project, "feature-new");
        let host = Arc::new(FakeHost::new(clone.path(), &[info], Start::Succeeds));

        set_up(&store, &host, &branch, SetupTrigger::Requested)
            .await
            .unwrap();

        assert_eq!(host.count("start_workspace"), starts);
        assert_eq!(host.count("check_out"), 1);
        assert_eq!(head_branch(clone.path()), "feature-new");
        assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
    }
}

/// Resume puts the branch back in `Starting` on a workstation that already
/// has it, unpushed commits and all. Setup ends with it checked out and
/// `Running`, and the commits still there.
#[tokio::test]
async fn a_resumed_branch_ends_checked_out_with_its_commits() {
    let (_origin, clone) = origin_and_workstation_clone();
    run_git_in(clone.path(), &["checkout", "-b", "feature-new"]);
    std::fs::write(clone.path().join("file.txt"), "base\nunpushed\n").unwrap();
    run_git_in(clone.path(), &["commit", "-am", "unpushed"]);
    let unpushed = run_git_in(clone.path(), &["rev-parse", "HEAD"]);
    // Another branch of the repo had the shared clone last.
    run_git_in(clone.path(), &["checkout", "main"]);

    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let branch = set_status(&store, &branch, WorkspaceStatus::Suspended);
    store
        .update_workspace_status_by_workspace_name(WORKSPACE, &WorkspaceStatus::Starting)
        .unwrap();
    let host = Arc::new(FakeHost::new(
        clone.path(),
        &[Info::Status("running")],
        Start::Succeeds,
    ));

    set_up(&store, &host, &branch, SetupTrigger::Observed)
        .await
        .unwrap();

    assert_eq!(head_branch(clone.path()), "feature-new");
    assert_eq!(run_git_in(clone.path(), &["rev-parse", "HEAD"]), unpushed);
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
}

/// A `Running` branch may have a session in its clone: no trigger touches
/// it. An observed trigger also leaves alone anything that isn't `Starting`.
#[tokio::test]
async fn a_settled_branch_is_left_alone() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(FakeHost::new(
        clone.path(),
        &[Info::Status("running")],
        Start::Succeeds,
    ));

    let running = set_status(&store, &branch, WorkspaceStatus::Running);
    for trigger in [SetupTrigger::Requested, SetupTrigger::Observed] {
        set_up(&store, &host, &running, trigger).await.unwrap();
    }
    let failed = set_status(&store, &branch, WorkspaceStatus::Error);
    set_up(&store, &host, &failed, SetupTrigger::Observed)
        .await
        .unwrap();

    assert_eq!(*host.calls.lock().unwrap(), Vec::<&str>::new());
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
    assert_eq!(head_branch(clone.path()), "main");
}

/// A fetch that never reached origin is a failure. Read as "the branch is
/// not on origin", it would create the branch from the base and leave a PR's
/// commits behind.
#[test]
fn a_failed_origin_probe_is_not_read_as_a_new_branch() {
    let (_origin, clone) = origin_and_workstation_clone();
    let real = branches::git_runner(clone.path(), None, None);
    let git: branches::GitRunner<'_> = Box::new(move |args: &[&str]| {
        if args.first() == Some(&"ls-remote") {
            return Err("sq blox command timed out after 300s".to_string());
        }
        real(args)
    });

    let error = branches::check_out_workspace_branch(&git, "main", "feature-new").unwrap_err();

    assert!(error.contains("is on origin"), "{error}");
    assert_eq!(head_branch(clone.path()), "main");
}

// ── The guard ───────────────────────────────────────────────────────────────

/// The frontend's call, a poll's trigger, a second window and the startup
/// pass all asking for one branch get one setup, and all of them its result.
#[tokio::test]
async fn concurrent_callers_join_one_setup() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let (host, gate) =
        FakeHost::new(clone.path(), &[Info::Status("running")], Start::Succeeds).gated();
    let host = Arc::new(host);
    let dyn_host: Arc<dyn SetupHost> = host.clone();
    let guard = Arc::new(SetupGuard::default());

    let callers: Vec<_> = [
        SetupTrigger::Requested,
        SetupTrigger::Observed,
        SetupTrigger::Requested,
        SetupTrigger::Observed,
    ]
    .into_iter()
    .map(|trigger| {
        let (guard, store, host, id) = (
            Arc::clone(&guard),
            Arc::clone(&store),
            Arc::clone(&dyn_host),
            branch.id.clone(),
        );
        tokio::spawn(async move { run_guarded(&guard, &store, &host, &id, trigger).await })
    })
    .collect();

    host.wait_for_checkouts(1).await;
    assert!(guard.is_in_flight(&branch.id));
    // Room for every caller to run its own checkout, were it going to.
    gate.add_permits(4);
    for caller in callers {
        assert_eq!(caller.await.unwrap(), Ok(()));
    }

    assert_eq!(host.count("check_out"), 1);
    assert_eq!(host.written().len(), 1);
    assert!(!guard.is_in_flight(&branch.id));
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
    assert_eq!(head_branch(clone.path()), "feature-new");
}

/// A caller that joins a failing setup gets the failure it joined, not one
/// of its own from colliding with it.
#[tokio::test]
async fn joined_callers_share_a_failure() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let mut host = FakeHost::new(clone.path(), &[Info::Status("running")], Start::Succeeds);
    host.checkout_error = Some("could not fetch base branch 'main': network");
    let (host, gate) = host.gated();
    let host = Arc::new(host);
    let dyn_host: Arc<dyn SetupHost> = host.clone();
    let guard = Arc::new(SetupGuard::default());

    let callers: Vec<_> = (0..2)
        .map(|_| {
            let (guard, store, host, id) = (
                Arc::clone(&guard),
                Arc::clone(&store),
                Arc::clone(&dyn_host),
                branch.id.clone(),
            );
            tokio::spawn(async move {
                run_guarded(&guard, &store, &host, &id, SetupTrigger::Requested).await
            })
        })
        .collect();
    host.wait_for_checkouts(1).await;
    gate.add_permits(2);

    for caller in callers {
        let error = caller.await.unwrap().unwrap_err();
        assert!(error.contains("could not fetch base branch"), "{error}");
    }
    assert_eq!(host.count("check_out"), 1);
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Error));
}

/// Two branches of one workstation can share a clone, so their setups take
/// turns rather than meeting on `.git/index.lock`.
#[tokio::test]
async fn setups_in_one_workstation_take_turns() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let first = starting_branch(&store, &project, "feature-one");
    let second = starting_branch(&store, &project, "feature-two");
    let host = Arc::new(FakeHost::new(
        clone.path(),
        &[Info::Status("running")],
        Start::Succeeds,
    ));
    let dyn_host: Arc<dyn SetupHost> = host.clone();
    let guard = Arc::new(SetupGuard::default());

    let (a, b) = tokio::join!(
        run_guarded(
            &guard,
            &store,
            &dyn_host,
            &first.id,
            SetupTrigger::Requested
        ),
        run_guarded(
            &guard,
            &store,
            &dyn_host,
            &second.id,
            SetupTrigger::Requested
        ),
    );

    assert_eq!((a, b), (Ok(()), Ok(())));
    assert_eq!(host.count("check_out"), 2);
    assert_eq!(host.most_in_checkout.load(Ordering::SeqCst), 1);
    assert_eq!(status_of(&store, &first), Some(WorkspaceStatus::Running));
    assert_eq!(status_of(&store, &second), Some(WorkspaceStatus::Running));
}

/// A setup that panics gives its callers an error and frees the branch for
/// the next attempt.
#[tokio::test]
async fn a_panicked_setup_releases_the_branch() {
    let guard = Arc::new(SetupGuard::default());

    let outcome = guard
        .run(WORKSPACE, "branch", || async { panic!("setup blew up") })
        .await;
    assert!(outcome.is_err());
    assert!(!guard.is_in_flight("branch"));

    let retry = guard.run(WORKSPACE, "branch", || async { Ok(()) }).await;
    assert_eq!(retry, Ok(()));
}

// ── The pollers ─────────────────────────────────────────────────────────────

/// What a poll did besides writing the row.
#[derive(Default)]
struct Seen {
    written: Mutex<Vec<(String, bool, WorkspaceStatus)>>,
    triggered: Mutex<Vec<String>>,
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
        }
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
/// checkout never happened.
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
        assert_eq!(seen.written(), vec![]);
    }
}

/// Every other transition is still the pollers'.
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
            // Resumed outside Staged: it was set up before it was suspended.
            (Suspended, "running", Running),
            (Stopped, "running", Running),
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

// ── The startup pass ────────────────────────────────────────────────────────

/// The startup pass takes the remote branches a previous run left in
/// `Starting` and nothing else.
#[test]
fn the_startup_pass_picks_up_only_branches_left_starting() {
    let (store, project) = store_with_project();
    let left_starting = starting_branch(&store, &project, "feature-starting");
    for (name, status) in [
        ("feature-running", WorkspaceStatus::Running),
        ("feature-error", WorkspaceStatus::Error),
        ("feature-suspended", WorkspaceStatus::Suspended),
    ] {
        let branch = starting_branch(&store, &project, name);
        set_status(&store, &branch, status);
    }
    store
        .create_branch(&store::Branch::new(&project.id, "feature-local", "main"))
        .unwrap();

    let pending: Vec<String> = branches_awaiting_setup(&store)
        .unwrap()
        .into_iter()
        .map(|branch| branch.id)
        .collect();

    assert_eq!(pending, vec![left_starting.id]);
}

/// A run that quit between a workstation coming up and its checkout left
/// the row in `Starting` with nothing asking for setup. The next launch
/// finishes the job with no window involved.
#[tokio::test]
async fn the_startup_pass_finishes_a_setup_the_last_run_dropped() {
    let (_origin, clone) = origin_and_workstation_clone();
    let (store, project) = store_with_project();
    let branch = starting_branch(&store, &project, "feature-new");
    let host = Arc::new(FakeHost::new(
        clone.path(),
        &[Info::Status("running")],
        Start::Succeeds,
    ));

    for pending in branches_awaiting_setup(&store).unwrap() {
        set_up(&store, &host, &pending, SetupTrigger::Observed)
            .await
            .unwrap();
    }

    assert_eq!(head_branch(clone.path()), "feature-new");
    assert_eq!(status_of(&store, &branch), Some(WorkspaceStatus::Running));
    assert!(branches_awaiting_setup(&store).unwrap().is_empty());
}
