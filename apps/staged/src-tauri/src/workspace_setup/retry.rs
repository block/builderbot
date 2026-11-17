//! Retrying a failed branch on its own.
//!
//! `Error` used to be a dead end: nothing polls an `Error` row, the startup
//! pass skipped it and the pollers held it. A dropped exec during a fetch
//! was enough to leave a branch there until the user deleted it.
//!
//! Every write of `Error` is now followed by a retry timer, backed off from
//! a minute to half an hour ([`RetryPolicy`]). When it fires and the row is
//! still `Error`, setup runs with [`SetupTrigger::Retry`]: it looks at the
//! workstation first and goes ahead only when Blox reports it running. A
//! retry never starts or resumes a workstation — that is the user's call,
//! through the error view's Retry or the project's Resume — so a branch
//! whose workstation is down stays `Error` and the timer is set again.
//!
//! Timers are armed by setup itself when it fails, by the pollers when they
//! see an `Error` row behind a running workstation or write `Error`
//! themselves, by a failed resume, and by the startup pass for the rows a
//! previous run left in `Error`. A branch has at most one timer that
//! counts ([`SetupGuard::retry_fired`]).

use std::sync::Arc;
use std::time::Duration;

use crate::store::{Store, WorkspaceStatus};

use super::guard::{Patience, SetupGuard};
use super::host::SetupHost;
use super::{run_guarded, SetupRequest, SetupTrigger};

/// How long to wait before each retry.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryPolicy {
    /// The wait before the first retry; it doubles with each further one.
    pub(crate) base: Duration,
    pub(crate) cap: Duration,
}

impl RetryPolicy {
    /// A minute covers a dropped exec or a workstation that had not quite
    /// reconnected. Half an hour keeps a branch that can't be set up (a
    /// signed-out `sq`, a clone that needs someone's attention) from costing
    /// more than one `ws info` in that time, with no end: the branch
    /// recovers whenever its cause goes away.
    pub(crate) const DEFAULT: RetryPolicy = RetryPolicy {
        base: Duration::from_secs(60),
        cap: Duration::from_secs(30 * 60),
    };

    /// The wait before retry number `attempt` (0 for the first).
    pub(crate) fn delay(&self, attempt: u32) -> Duration {
        let doubled = self
            .base
            .checked_mul(2u32.saturating_pow(attempt.min(16)))
            .unwrap_or(self.cap);
        doubled.min(self.cap)
    }
}

/// Set the retry timer for a branch that has just failed, backing off from
/// its last one.
pub(crate) fn schedule(guard: &SetupGuard, host: &dyn SetupHost, branch_id: &str) {
    let (attempt, ticket) = guard.schedule_retry(branch_id);
    let delay = host.retry_policy().delay(attempt);
    log::info!(
        "[workspace_setup] branch={branch_id} retry {} in {}s",
        attempt + 1,
        delay.as_secs()
    );
    host.retry_later(branch_id, delay, ticket);
}

/// Set the retry timer for a branch seen in `Error`, unless one is already
/// out for it. For what finds such a row without having failed it: a poll,
/// the startup pass.
pub(crate) fn arm(guard: &SetupGuard, host: &dyn SetupHost, branch_id: &str) {
    if guard.retry_is_armed(branch_id) || guard.is_in_flight(branch_id) {
        return;
    }
    schedule(guard, host, branch_id);
}

/// A retry timer fired. Runs setup when the timer still counts and the row
/// still says `Error`; anything else has moved the branch on.
pub(crate) async fn fire(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &Arc<dyn SetupHost>,
    branch_id: &str,
    ticket: u64,
) {
    if !guard.retry_fired(branch_id, ticket) {
        return;
    }
    let row = store.get_branch(branch_id).ok().flatten();
    if !matches!(&row, Some(branch) if branch.workspace_status == Some(WorkspaceStatus::Error)) {
        guard.forget_retries(branch_id);
        return;
    }
    // A transfer provisions its own branches. Look again once it is over.
    if crate::project_transfer::is_branch_transferring(branch_id) {
        schedule(guard, host.as_ref(), branch_id);
        return;
    }
    let request = SetupRequest::new(SetupTrigger::Retry);
    if let Err(e) = run_guarded(guard, store, host, branch_id, request, Patience::ToTheEnd).await {
        // The run recorded the reason and set the next timer.
        log::warn!("[workspace_setup] retry of branch {branch_id} failed: {e}");
    }
}
