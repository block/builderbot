//! The in-process guard every branch setup runs under.
//!
//! Setup is asked for from several places at once — the frontend's
//! `start_workspace`, a status poll, a second window or web client, the
//! startup pass — and each of them means the same thing: "this branch should
//! be set up". Two checkouts in one clone collide on `.git/index.lock`, and
//! a caller that lost such a race used to report a failure for a branch that
//! was being set up fine.
//!
//! So a run is keyed by branch: a second caller for a branch whose setup is
//! in flight gets that run's result rather than starting its own. Runs for
//! different branches of one workstation take turns, since they may share a
//! clone.
//!
//! A run is its own task. A caller that goes away — a dropped web request, a
//! closed window — only stops waiting; the setup still finishes and still
//! writes the branch's status.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::watch;

type Outcome = Option<Result<(), String>>;

#[derive(Default)]
pub(crate) struct SetupGuard {
    /// Branch id → the result of the run in flight for it, once it has one.
    in_flight: Mutex<HashMap<String, watch::Receiver<Outcome>>>,
    /// Workspace name → the lock its runs take turns on.
    workspace_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
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

    /// Run `setup` for `branch_id` on `workspace`, or wait for the run
    /// already in flight for that branch, and return its result.
    ///
    /// `setup` is not called when the caller joins.
    pub(crate) async fn run<F, Fut>(
        self: &Arc<Self>,
        workspace: &str,
        branch_id: &str,
        setup: F,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let mut outcome = {
            let mut in_flight = self.lock_in_flight();
            match in_flight.get(branch_id) {
                Some(outcome) => outcome.clone(),
                None => {
                    let (tx, rx) = watch::channel(None);
                    in_flight.insert(branch_id.to_string(), rx.clone());
                    let turn = self.workspace_lock(workspace);
                    let entry = InFlight {
                        guard: Arc::clone(self),
                        branch_id: branch_id.to_string(),
                    };
                    tauri::async_runtime::spawn(async move {
                        let result = {
                            let _turn = turn.lock().await;
                            setup().await
                        };
                        // Cleared before the result is published: a caller
                        // that sees the result must be able to start a fresh
                        // run, not join a finished one.
                        drop(entry);
                        let _ = tx.send(Some(result));
                    });
                    rx
                }
            }
        };

        loop {
            if let Some(result) = outcome.borrow_and_update().clone() {
                return result;
            }
            // The sender is dropped without a result only if the run
            // panicked; its entry is gone with it.
            if outcome.changed().await.is_err() {
                return Err("Workspace setup ended without a result".to_string());
            }
        }
    }

    fn lock_in_flight(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, watch::Receiver<Outcome>>> {
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
    }
}
