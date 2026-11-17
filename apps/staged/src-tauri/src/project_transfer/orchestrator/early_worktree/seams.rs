//! What tests reach into an early worktree with: a failure to inject, a
//! gate that proves the add and the snapshot overlap, and a git too old to
//! fire hooks on request.
//!
//! Each is asked for on the thread that runs the transfer and picked up
//! when a leg plans its worktree. The legs of a one-repository transfer,
//! or of one under [`super::super::SerialLegs`], run on that thread.

use std::cell::RefCell;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

thread_local! {
    static SEAMS: RefCell<Seams> = RefCell::new(Seams::default());
}

/// What the early worktrees planned from a thread are to do differently.
#[derive(Clone, Default)]
pub(super) struct Seams {
    pub(super) fail_add: bool,
    pub(super) no_hook_run: bool,
    pub(super) gate: Option<Arc<EarlyAddGate>>,
}

impl Seams {
    pub(super) fn of_this_thread() -> Self {
        SEAMS.with(|seams| seams.borrow().clone())
    }

    fn change(change: impl FnOnce(&mut Seams)) {
        SEAMS.with(|seams| change(&mut seams.borrow_mut()));
    }
}

/// While it lives, the early worktree adds of legs run from this thread
/// fail part way, leaving a directory git knows nothing of — for a test of
/// the fallback.
pub(crate) struct FailingEarlyAdds;

impl FailingEarlyAdds {
    pub(crate) fn for_this_thread() -> Self {
        Seams::change(|seams| seams.fail_add = true);
        Self
    }
}

impl Drop for FailingEarlyAdds {
    fn drop(&mut self) {
        Seams::change(|seams| seams.fail_add = false);
    }
}

/// While it lives, legs run from this thread find a git that cannot fire a
/// hook on request (one before 2.36), so they plan no early worktree.
pub(crate) struct GitWithoutHookRun;

impl GitWithoutHookRun {
    pub(crate) fn for_this_thread() -> Self {
        Seams::change(|seams| seams.no_hook_run = true);
        Self
    }
}

impl Drop for GitWithoutHookRun {
    fn drop(&mut self) {
        Seams::change(|seams| seams.no_hook_run = false);
    }
}

/// While it lives, the early worktree adds of legs run from this thread
/// wait at its [`EarlyAddGate`].
pub(crate) struct GatedEarlyAdds(Arc<EarlyAddGate>);

impl GatedEarlyAdds {
    pub(crate) fn for_this_thread() -> Self {
        let gate = Arc::new(EarlyAddGate::default());
        Seams::change(|seams| seams.gate = Some(Arc::clone(&gate)));
        Self(gate)
    }

    pub(crate) fn gate(&self) -> Arc<EarlyAddGate> {
        Arc::clone(&self.0)
    }
}

impl Drop for GatedEarlyAdds {
    fn drop(&mut self) {
        Seams::change(|seams| seams.gate = None);
    }
}

/// Holds an early add until the leg's snapshot has started, and that
/// snapshot until the add has finished, so a test can tell the two ran at
/// once: an add made before the capture starts never sees the snapshot
/// start, and one made after it never finishes inside it. Either wait gives
/// up after [`EarlyAddGate::PATIENCE`] and says so.
#[derive(Default)]
pub(crate) struct EarlyAddGate {
    state: Mutex<GateState>,
    changed: Condvar,
}

#[derive(Default)]
struct GateState {
    snapshot_started: bool,
    /// Whether the snapshot had started when the add went ahead, once it
    /// has.
    add_began_in_the_snapshot: Option<bool>,
    add_finished: bool,
}

impl EarlyAddGate {
    const PATIENCE: Duration = Duration::from_secs(20);

    /// For the test's script hook, at the start of the snapshot: let the
    /// add go, and hold the snapshot until the add has finished. Returns
    /// whether it did.
    pub(crate) fn snapshot_started(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        state.snapshot_started = true;
        self.changed.notify_all();
        let (state, _) = self
            .changed
            .wait_timeout_while(state, Self::PATIENCE, |state| !state.add_finished)
            .unwrap();
        state.add_finished
    }

    /// Whether the snapshot had started when the add went ahead; `None` if
    /// no add came to the gate.
    pub(crate) fn add_began_in_the_snapshot(&self) -> Option<bool> {
        self.state.lock().unwrap().add_began_in_the_snapshot
    }

    /// The add, before it does anything: wait for the snapshot to start.
    pub(super) fn hold_the_add(&self) {
        let state = self.state.lock().unwrap();
        let (mut state, _) = self
            .changed
            .wait_timeout_while(state, Self::PATIENCE, |state| !state.snapshot_started)
            .unwrap();
        state.add_began_in_the_snapshot = Some(state.snapshot_started);
    }

    /// The add, with the worktree made and its capture started.
    pub(super) fn add_finished(&self) {
        self.state.lock().unwrap().add_finished = true;
        self.changed.notify_all();
    }
}
