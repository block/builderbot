//! Stopping a Blox turn no live connection can reach: a `disconnected` row's,
//! and a running session's whose Stop went unanswered.

use std::time::Duration;

use acp_client::CancelOutcome;

use super::{
    ReconcileBlox, ReconcilePhase, ReconcileTarget, Reconciler, RowOutcome, WorkspaceProbe,
};
use crate::store::{CompletionReason, Session, SessionStatus, Store};

/// How long a cancel over a fresh connection waits for the turn to stop.
const STOP_DEADLINE: Duration = Duration::from_secs(20);

/// The message on a row a Stop ended without the workstation confirming it.
pub(super) const MAY_STILL_RUN: &str =
    "Stopped in Staged, but the turn may still be running on the workstation";

impl Reconciler {
    /// A Stop on a `disconnected` row no attempt has claimed: cancel its
    /// remote turn, then end the row `cancelled` for `reason`.
    pub(super) fn stop_disconnected(
        &self,
        session_id: &str,
        reason: CompletionReason,
    ) -> RowOutcome {
        let Some(session) = self.disconnected_row(session_id) else {
            return RowOutcome::Done;
        };
        let observed = session.updated_at;
        match ReconcileTarget::resolve(&self.store, &session) {
            Ok(target) => {
                self.note_target(&target);
                self.stop_observed(&target, observed, reason)
            }
            // Nothing to cancel through; end the row all the same.
            Err(_) => {
                let target = ReconcileTarget::unresolved(&self.store, &session);
                self.settle_unclaimed(
                    &target,
                    observed,
                    SessionStatus::Cancelled,
                    Some(MAY_STILL_RUN),
                    reason,
                );
                RowOutcome::Done
            }
        }
    }

    /// Cancel `target`'s remote turn, then end the row as read at
    /// `observed`, `cancelled` for `reason`. When the workstation can't be
    /// reached the row is ended anyway — the user asked — with a message
    /// saying the turn may still be running.
    pub(super) fn stop_observed(
        &self,
        target: &ReconcileTarget,
        observed: i64,
        reason: CompletionReason,
    ) -> RowOutcome {
        self.set_phase(&target.session_id, ReconcilePhase::Stopping);
        let message = match self.blox.workspace(&target.workspace_name) {
            WorkspaceProbe::Running => self.cancel_remote_turn(target),
            // Nothing runs on a workstation that is gone or suspended.
            WorkspaceProbe::Deleted | WorkspaceProbe::Suspended => None,
            WorkspaceProbe::NotRunning(_) | WorkspaceProbe::Unreachable { .. } => {
                Some(MAY_STILL_RUN.to_string())
            }
        };
        self.settle_unclaimed(
            target,
            observed,
            SessionStatus::Cancelled,
            message.as_deref(),
            reason,
        );
        RowOutcome::Done
    }

    /// Cancel the turn over a fresh connection. `Some(message)` when it
    /// couldn't be confirmed stopped.
    pub(super) fn cancel_remote_turn(&self, target: &ReconcileTarget) -> Option<String> {
        cancel_over_fresh_connection(self.blox.as_ref(), target)
    }
}

fn cancel_over_fresh_connection(
    blox: &dyn ReconcileBlox,
    target: &ReconcileTarget,
) -> Option<String> {
    match blox.cancel(target, STOP_DEADLINE) {
        Ok(CancelOutcome::Stopped) | Ok(CancelOutcome::ProcessGone { .. }) => None,
        Ok(outcome @ (CancelOutcome::Unconfirmed | CancelOutcome::TransportLost)) => {
            log::warn!(
                "[reconcile] Session {}: cancel not confirmed ({outcome:?})",
                target.session_id
            );
            Some(MAY_STILL_RUN.to_string())
        }
        Err(e) => {
            log::warn!(
                "[reconcile] Session {}: couldn't cancel over a fresh connection: {e}",
                target.session_id
            );
            Some(MAY_STILL_RUN.to_string())
        }
    }
}

/// Cancel a `disconnected` row's remote turn, best effort and in the
/// background, for a caller about to end or delete the row itself: a branch
/// teardown (whether or not it is expected to delete the workstation), or a
/// session delete. Once the row is gone nothing — reconcile included — could
/// stop the turn, and it would run on in a checkout other work may be using.
///
/// The target is resolved now, while the row and its branch still exist; the
/// cancel, which can take [`STOP_DEADLINE`], runs on a thread of its own.
/// `None` when there is nothing to cancel: the row isn't `disconnected`, or
/// can't be resolved to a turn.
pub(crate) fn cancel_abandoned_turn(
    store: &Store,
    session: &Session,
) -> Option<std::thread::JoinHandle<()>> {
    cancel_abandoned_turn_with(store, std::sync::Arc::new(super::LiveBlox), session)
}

pub(super) fn cancel_abandoned_turn_with(
    store: &Store,
    blox: std::sync::Arc<dyn ReconcileBlox>,
    session: &Session,
) -> Option<std::thread::JoinHandle<()>> {
    if session.status != SessionStatus::Disconnected {
        return None;
    }
    let target = match ReconcileTarget::resolve(store, session) {
        Ok(target) => target,
        Err(why) => {
            log::info!(
                "[reconcile] Session {}: nothing to cancel before it goes ({why:?})",
                session.id
            );
            return None;
        }
    };
    log::info!(
        "[reconcile] Session {}: cancelling its remote turn before the row goes",
        session.id
    );
    Some(std::thread::spawn(move || {
        if let Some(message) = cancel_over_fresh_connection(blox.as_ref(), &target) {
            log::warn!("[reconcile] Session {}: {message}", target.session_id);
        }
    }))
}

/// A Stop on a running Blox session whose `session/cancel` went unanswered
/// (see `acp_client::SessionSettled::cancel_unacknowledged`): the transport
/// may be hung with the turn still going on the workstation, so cancel it
/// again over a fresh connection. Returns the message for the `cancelled`
/// row when that couldn't be confirmed either.
pub(crate) fn stop_unacknowledged_turn(
    store: &Store,
    blox: &dyn ReconcileBlox,
    session_id: &str,
) -> Option<String> {
    let target = store
        .get_session(session_id)
        .ok()
        .flatten()
        .and_then(|session| ReconcileTarget::resolve(store, &session).ok());
    match target {
        Some(target) => cancel_over_fresh_connection(blox, &target),
        None => Some(MAY_STILL_RUN.to_string()),
    }
}
