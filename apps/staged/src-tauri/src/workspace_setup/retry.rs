//! Retrying a failed branch on its own.
//!
//! `Error` used to be a dead end: nothing polls an `Error` row, the startup
//! pass skipped it and the pollers held it. A dropped exec during a fetch
//! was enough to leave a branch there until the user deleted it.
//!
//! A write of `Error` is now followed by a retry timer, backed off from a
//! minute to half an hour and spread by some jitter ([`RetryPolicy`]), so
//! the branches a restart finds failed are not all retried in one instant.
//! When it fires and the row is still `Error`, it looks at the workstation
//! first and runs setup with [`SetupTrigger::Retry`] only when Blox reports
//! it running. A retry never starts or resumes a workstation — that is the
//! user's call, through the error view's Retry or the project's Resume — so
//! a branch whose workstation is down stays `Error` and the timer is set
//! again.
//!
//! A retry is quiet. Nobody asked for it, so the row stays `Error` and
//! keeps its reason while it runs, and a failure that says what the last
//! one said announces nothing.
//!
//! One failure sets no timer: a checkout refused over what is in the clone
//! ([`CheckoutError::worth_retrying`]). Trying again changes nothing until
//! someone has dealt with it, and then the error view's Retry is theirs to
//! press.
//!
//! Timers are armed by setup itself when it fails, by the pollers when they
//! see an `Error` row behind a running workstation or write `Error`
//! themselves, by a failed resume, and by the startup pass for the rows a
//! previous run left in `Error`. A branch has at most one timer that
//! counts ([`SetupGuard::retry_fired`]).
//!
//! [`CheckoutError::worth_retrying`]: super::checkout::CheckoutError::worth_retrying

use std::sync::Arc;
use std::time::Duration;

use crate::store::{Store, WorkspaceStatus};

use super::guard::SetupGuard;
use super::host::{reports_running, SetupHost};
use super::{run_guarded, SetupRequest, SetupTrigger};

/// How long to wait before each retry.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryPolicy {
    /// The wait before the first retry; it doubles with each further one.
    pub(crate) base: Duration,
    pub(crate) cap: Duration,
    /// How much longer than its backed-off wait a retry may wait, in
    /// percent.
    pub(crate) jitter_percent: u64,
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
        jitter_percent: 20,
    };

    /// The wait before retry number `attempt` (0 for the first): the
    /// backed-off wait, plus the share of its jitter that `random` picks.
    pub(crate) fn delay(&self, attempt: u32, random: u64) -> Duration {
        let doubled = self
            .base
            .checked_mul(2u32.saturating_pow(attempt.min(16)))
            .unwrap_or(self.cap);
        let backed_off = doubled.min(self.cap);
        let spread = backed_off.as_millis() as u64 * self.jitter_percent / 100;
        backed_off + Duration::from_millis(random % (spread + 1))
    }
}

/// Set the retry timer for a branch that has just failed, backing off from
/// its last one.
pub(crate) fn schedule(guard: &SetupGuard, host: &dyn SetupHost, branch_id: &str) {
    let (attempt, ticket) = guard.schedule_retry(branch_id);
    let delay = host.retry_policy().delay(attempt, rand::random());
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

/// A retry timer fired. Runs setup when the timer still counts, the row
/// still says `Error` and its workstation is running. A row that is gone or
/// has moved on has nothing to retry and no reason to keep.
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
    let Some(branch) = row.filter(|row| row.workspace_status == Some(WorkspaceStatus::Error))
    else {
        guard.forget_retries(branch_id);
        host.failure_changed(branch_id, None, false);
        return;
    };
    // A transfer provisions its own branches. Look again once it is over.
    if crate::project_transfer::is_branch_transferring(branch_id) {
        schedule(guard, host.as_ref(), branch_id);
        return;
    }
    // Looked at here, before a run exists: a workstation that is down, or
    // can't be asked about, is looked at again later, and a request never
    // finds itself joined to a retry with nothing to do.
    let reported = match branch.workspace_name.as_deref() {
        Some(workspace) => host.workspace_status(workspace).await.ok().flatten(),
        None => None,
    };
    if !reports_running(reported.as_deref()) {
        schedule(guard, host.as_ref(), branch_id);
        return;
    }
    let request = SetupRequest::new(SetupTrigger::Retry);
    if let Err(e) = run_guarded(guard, store, host, branch_id, request).await {
        // The run recorded the reason and set the next timer.
        log::warn!("[workspace_setup] retry of branch {branch_id} failed: {e}");
    }
}
