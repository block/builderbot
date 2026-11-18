//! The in-process guard every branch setup runs under.
//!
//! Setup is asked for from several places at once — the frontend's
//! `start_workspace`, a status poll, a second window or web client, the
//! startup pass, a retry timer, an agent start — and each of them means the
//! same thing: "this branch should be set up". Two checkouts in one clone
//! collide on `.git/index.lock`, and a caller that lost such a race used to
//! report a failure for a branch that was being set up fine.
//!
//! So a run is keyed by branch: a second caller for a branch whose setup is
//! in flight gets that run's result rather than starting its own, and is
//! told that it joined ([`Run::joined`]), since the run it joined was
//! started for someone else's reason. Runs for different branches of one
//! workstation take turns, since they may share a clone.
//!
//! A run is its own task. A caller that goes away — a dropped web request, a
//! closed window — only stops waiting; the setup still finishes and still
//! writes the branch's status. Most callers never wait at all
//! ([`SetupGuard::start`]): the outcome lands on the row.
//!
//! The guard holds three more things that are per process for the same
//! reason the runs are:
//!
//! - the gate of each clone ([`SetupGuard::clone_gate`]), which a setup
//!   holds over its checkout and an agent start holds from reading the
//!   clone until its session is live, so neither happens under the other;
//! - each failed branch's retry count and timer ticket ([`super::retry`]);
//! - which branches have a setup the user asked for still to end, so its
//!   failure is announced as one ([`SetupGuard::user_asked`]).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::{watch, OwnedMutexGuard};

use super::SetupOutcome;

/// How a run ended. `None` in its channel while it is still going.
type RunResult = Result<SetupOutcome, String>;

/// A run a caller started or joined ([`SetupGuard::start`]).
pub(crate) struct Run {
    /// Whether the run was already in flight, started by another caller for
    /// its own reason.
    pub(crate) joined: bool,
    result: watch::Receiver<Option<RunResult>>,
}

impl Run {
    /// Wait for the run's result.
    pub(crate) async fn end(mut self) -> RunResult {
        // The sender is dropped without a result only if the run panicked;
        // its entry is gone with it.
        let lost = || Err("Workspace setup ended without a result".to_string());
        match self.result.wait_for(Option::is_some).await {
            Ok(result) => result.clone().unwrap_or_else(lost),
            Err(_) => lost(),
        }
    }
}

/// One failed branch's place in the retry schedule.
#[derive(Debug, Clone, Copy, Default)]
struct RetryState {
    /// How many retries have been scheduled since the branch last worked.
    attempt: u32,
    /// The timer that counts. An older timer firing finds a newer ticket
    /// and does nothing.
    ticket: u64,
    /// Whether that timer is still to fire.
    armed: bool,
}

/// A clone: the workspace's name and the clone's directory on it.
type CloneKey = (String, Option<String>);

#[derive(Default)]
pub(crate) struct SetupGuard {
    /// Branch id → the result of the run in flight for it.
    in_flight: Mutex<HashMap<String, watch::Receiver<Option<RunResult>>>>,
    /// Workspace name → the lock its runs take turns on.
    workspace_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Clone → its gate.
    clone_gates: Mutex<HashMap<CloneKey, Arc<tokio::sync::Mutex<()>>>>,
    /// Branch id → its retry schedule.
    retries: Mutex<HashMap<String, RetryState>>,
    /// Branches with a setup the user asked for still to end.
    user_asked: Mutex<HashSet<String>>,
}

impl SetupGuard {
    /// The guard the app's setups share.
    pub(crate) fn global() -> &'static Arc<SetupGuard> {
        static GUARD: OnceLock<Arc<SetupGuard>> = OnceLock::new();
        GUARD.get_or_init(|| Arc::new(SetupGuard::default()))
    }

    /// Whether a run for `branch_id` has started and not yet finished.
    pub(crate) fn is_in_flight(&self, branch_id: &str) -> bool {
        self.lock_in_flight().contains_key(branch_id)
    }

    /// Start `setup` for `branch_id` on `workspace` as its own task, or
    /// join the run already in flight for that branch. Returns at once;
    /// [`Run::end`] waits for the result.
    ///
    /// `setup` is not called when the caller joins. It runs once the
    /// workstation's turn comes.
    pub(crate) fn start<F, Fut>(self: &Arc<Self>, workspace: &str, branch_id: &str, setup: F) -> Run
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = RunResult> + Send + 'static,
    {
        let mut in_flight = self.lock_in_flight();
        if let Some(result) = in_flight.get(branch_id) {
            return Run {
                joined: true,
                result: result.clone(),
            };
        }
        let (tx, rx) = watch::channel(None);
        in_flight.insert(branch_id.to_string(), rx.clone());
        let turn = self.workspace_lock(workspace);
        let entry = InFlight {
            guard: Arc::clone(self),
            branch_id: branch_id.to_string(),
        };
        tauri::async_runtime::spawn(async move {
            let result = {
                let _turn = turn.lock_owned().await;
                setup().await
            };
            // Cleared before the result is published: a caller that sees
            // the result must be able to start a fresh run, not join a
            // finished one.
            drop(entry);
            tx.send_replace(Some(result));
        });
        Run {
            joined: false,
            result: rx,
        }
    }

    /// The user asked for `branch_id` to be set up (a new branch, the error
    /// view's Retry, a resume). The next failure announced for it is
    /// announced as theirs, which is what raises a toast
    /// ([`super::failures`]). Forgotten when the run in flight ends.
    pub(crate) fn user_asked(&self, branch_id: &str) {
        self.lock_user_asked().insert(branch_id.to_string());
    }

    /// Whether the user asked for the setup of `branch_id` that is ending,
    /// asked once: a later failure is not theirs.
    pub(crate) fn take_user_asked(&self, branch_id: &str) -> bool {
        self.lock_user_asked().remove(branch_id)
    }

    /// Take the gate of the clone at `clone_dir` on `workspace`.
    ///
    /// Held by a setup from before it looks for live sessions in the clone
    /// until its checkout is done, and by an agent start from before it
    /// reads the clone until its session row is live. So a checkout never
    /// lands between a start's read and its launch, and a start never reads
    /// a clone that is mid-checkout.
    ///
    /// Lock order: a setup takes its workstation's turn first and the gate
    /// second. Nothing asks for a turn while it holds a gate.
    pub(crate) async fn clone_gate(
        &self,
        workspace: &str,
        clone_dir: Option<&str>,
    ) -> OwnedMutexGuard<()> {
        let gate = {
            let mut gates = self.clone_gates.lock().unwrap_or_else(|p| p.into_inner());
            gates.retain(|_, gate| Arc::strong_count(gate) > 1);
            gates
                .entry((workspace.to_string(), clone_dir.map(str::to_string)))
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        gate.lock_owned().await
    }

    /// Count one more retry for `branch_id` and return which attempt it is
    /// (0 for the first) and the ticket its timer must present.
    pub(crate) fn schedule_retry(&self, branch_id: &str) -> (u32, u64) {
        let mut retries = self.lock_retries();
        let state = retries.entry(branch_id.to_string()).or_default();
        let attempt = state.attempt;
        state.attempt = state.attempt.saturating_add(1);
        state.ticket += 1;
        state.armed = true;
        (attempt, state.ticket)
    }

    /// Whether a retry timer is still to fire for `branch_id`.
    pub(crate) fn retry_is_armed(&self, branch_id: &str) -> bool {
        self.lock_retries()
            .get(branch_id)
            .is_some_and(|state| state.armed)
    }

    /// Called by a timer when it fires: whether it is still the one that
    /// counts. Disarms the schedule when it is.
    pub(crate) fn retry_fired(&self, branch_id: &str, ticket: u64) -> bool {
        let mut retries = self.lock_retries();
        match retries.get_mut(branch_id) {
            Some(state) if state.ticket == ticket && state.armed => {
                state.armed = false;
                true
            }
            _ => false,
        }
    }

    /// The branch worked, or is no longer one to retry: forget its schedule.
    /// A timer still out for it finds no ticket and does nothing.
    pub(crate) fn forget_retries(&self, branch_id: &str) {
        self.lock_retries().remove(branch_id);
    }

    fn lock_retries(&self) -> std::sync::MutexGuard<'_, HashMap<String, RetryState>> {
        self.retries.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn lock_user_asked(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.user_asked.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn lock_in_flight(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, watch::Receiver<Option<RunResult>>>> {
        self.in_flight.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn workspace_lock(&self, workspace: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .workspace_locks
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // Drop the locks no run holds or waits on, so the map stays the size
        // of what is being set up.
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        locks
            .entry(workspace.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

/// A run's slot in [`SetupGuard::in_flight`], released when the run ends
/// however it ends — a panic included, or the branch could never be set up
/// again in this process.
struct InFlight {
    guard: Arc<SetupGuard>,
    branch_id: String,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.guard.lock_in_flight().remove(&self.branch_id);
        // A run that ended without announcing a failure used nobody's ask.
        self.guard.take_user_asked(&self.branch_id);
    }
}
