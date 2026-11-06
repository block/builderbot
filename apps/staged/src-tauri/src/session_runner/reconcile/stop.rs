//! Stopping a Blox turn no live connection can reach: a `disconnected` row's,
//! and a running session's whose Stop went unanswered.
//!
//! A Stop cancels the turn over a fresh connection, then backfills the
//! transcript from the session's replay on that connection, so the tool call
//! the turn was running ends as the agent ended it. The row settles as soon
//! as the cancel's outcome is known, not after the backfill: a Claude
//! `session/load` takes longer the longer the transcript, and the Stop the
//! user sees should not wait on it. The backfill then writes into a row that
//! has ended, and says so when it is done — reconcile's `idle` event for the
//! session, which an open pane re-fetches the transcript on — and a resume of
//! the session waits for it ([`stop_backfill_finished`]), so the turn it
//! starts can't interleave with the stopped one's tail. The exceptions are
//! the Stops that complete a turn rather than cancel it — a turn already over
//! on the workstation, its session perhaps still working after it
//! ([`HeldWork`]) — whose completion hooks read the transcript: they claim the
//! row first, and backfill before it settles.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_client::{CancelOutcome, LatestPrompt, RecordedTurn, RemoteProcessStatus};

use super::blox::Backfill;
use super::row::{from_ended_turn, recorded_result, shows_held_work, Verdict};
use super::{
    ReconcileBlox, ReconcilePhase, ReconcileReasonKind, ReconcileTarget, Reconciler, RowOutcome,
    SessionReconcileEvent, WorkspaceProbe,
};
use crate::store::{CompletionReason, Session, SessionStatus, Store};

/// How long a cancel over a fresh connection waits for the turn to stop.
const STOP_DEADLINE: Duration = Duration::from_secs(20);
/// How long a resume waits for a Stop's backfill to finish before going
/// ahead: past the backfill's own deadline (the `session/load`'s 30 s), with
/// room for the proxy to stop.
const BACKFILL_WAIT: Duration = Duration::from_secs(40);
const BACKFILL_WAIT_POLL: Duration = Duration::from_millis(100);

/// The message on a row a Stop ended without the workstation confirming it.
pub(super) const MAY_STILL_RUN: &str =
    "Stopped in Staged, but the turn may still be running on the workstation";

/// What a Stop knows about whether the row's session is still working after
/// its turn ([`RowOutcome::Held`]). A Stop on such a row ends the work, as a
/// Stop during the live hold does, and the turn — already over — completes
/// rather than being cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HeldWork {
    /// Nothing to go on: decided from the row's last check, else by asking
    /// Blox.
    Unknown,
    /// The attempt the Stop reached found the session still working.
    Held,
    /// The attempt the Stop reached found it not.
    NotHeld,
}

impl Reconciler {
    /// A Stop on a `disconnected` row no attempt has claimed: cancel its
    /// remote turn, then end the row `cancelled` for `reason`.
    pub(super) fn stop_disconnected(
        &self,
        session_id: &str,
        reason: CompletionReason,
        held: HeldWork,
    ) -> RowOutcome {
        let Some(session) = self.disconnected_row(session_id) else {
            return RowOutcome::Done;
        };
        let observed = session.updated_at;
        match ReconcileTarget::resolve(&self.store, &session) {
            Ok(target) => {
                self.note_target(&target);
                self.stop_observed(&target, observed, reason, held)
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

    /// Cancel `target`'s remote turn and end the row as read at `observed`,
    /// `cancelled` for `reason`, once the cancel's outcome is known; the
    /// backfill of the turn's end follows (see the module docs). When the
    /// workstation can't be reached the row is ended anyway — the user asked
    /// — with a message saying the turn may still be running. A row whose
    /// turn is already over on the workstation completes instead (see
    /// [`Self::turn_already_over`]).
    pub(super) fn stop_observed(
        &self,
        target: &ReconcileTarget,
        observed: i64,
        reason: CompletionReason,
        held: HeldWork,
    ) -> RowOutcome {
        self.set_phase(&target.session_id, ReconcilePhase::Stopping);
        let message = match self.blox.workspace(&target.workspace_name) {
            WorkspaceProbe::Running => {
                let mut target = target.clone();
                // The backfill's `session/load` runs in the checkout, and the
                // hooks of a turn the Stop completes read its commit there.
                let backfill = self.backfill_store(&mut target);
                if let Some(recorded) = self.turn_already_over(&mut target, held) {
                    return self.stop_over_turn(&target, observed, reason, recorded, backfill);
                }
                cancel_then_backfill(self.blox.as_ref(), &target, backfill, |message| {
                    self.settle_unclaimed(
                        &target,
                        observed,
                        SessionStatus::Cancelled,
                        message.as_deref(),
                        reason,
                    );
                });
                return RowOutcome::Done;
            }
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

    /// The turn as its recording has it, when the Stop reaches one already
    /// over on the workstation: one whose session is still working after it,
    /// or one that simply ended, its process `IDLE` and nothing left running
    /// — for an agent with no turn state, its last prompt answered (see
    /// [`RecordedTurn::Answered`]). `None` for an ordinary Stop, on a turn
    /// still running. Pins `target` to the process it looked at, so the
    /// cancel's attach reaches that one without looking it up again.
    fn turn_already_over(
        &self,
        target: &mut ReconcileTarget,
        held: HeldWork,
    ) -> Option<RecordedTurn> {
        let known_held = match held {
            HeldWork::NotHeld => return None,
            HeldWork::Held => true,
            HeldWork::Unknown => self.last_check_found_held_work(&target.session_id),
        };
        if !known_held || target.command_id.is_none() {
            let process = match self.blox.process(target) {
                Ok(process) => process,
                Err(e) => {
                    log::warn!(
                        "[reconcile] Session {}: couldn't find its process for the Stop: {e}",
                        target.session_id
                    );
                    if !known_held {
                        return None;
                    }
                    return Some(RecordedTurn::Unknown);
                }
            };
            let idle = process.status == RemoteProcessStatus::Idle;
            if target.command_id.is_none() {
                target.command_id = process.command_id;
            }
            // A process with a prompt in flight is running a turn, not one
            // that is over.
            if !known_held && !idle {
                return None;
            }
        }
        let recorded = self.read_recorded_turn(target);
        // The process reads `IDLE`, every prompt answered: the last answer is
        // the turn's.
        let over = known_held || shows_held_work(&recorded) || ended(&recorded);
        over.then_some(recorded)
    }

    /// Whether the row's last check found its session still working after
    /// its turn.
    fn last_check_found_held_work(&self, session_id: &str) -> bool {
        self.rows
            .lock()
            .unwrap()
            .get(session_id)
            .and_then(|row| row.reason.as_ref())
            .is_some_and(|reason| reason.kind == ReconcileReasonKind::BackgroundWork)
    }

    /// A Stop on a turn already over on the workstation (see
    /// [`Self::turn_already_over`]): as during the live hold, the work the
    /// turn left running, if any, is cancelled and the turn completes from its
    /// recorded result — `hold_stopped` when the Stop cut the wait for its
    /// work short — with its hooks, and without draining the queued
    /// follow-up.
    ///
    /// The row is claimed before the cancel, as an attempt claims it before
    /// its attach: nothing the backfill writes meanwhile (a session title)
    /// can then move the row from under the Stop. The backfill is done
    /// before the row settles — nothing attached to this turn, and the hooks
    /// read the transcript — and its replay says whether the newest prompt
    /// ever reached the workstation: when not, the recording is an earlier
    /// turn's, and the row ends resumable instead.
    fn stop_over_turn(
        &self,
        target: &ReconcileTarget,
        observed: i64,
        reason: CompletionReason,
        recorded: RecordedTurn,
        backfill: Option<&Arc<Store>>,
    ) -> RowOutcome {
        let session_id = target.session_id.as_str();
        // Registered before the claim, like every owner of a `running` row.
        let (token, epoch) = self.registry.register(session_id);
        let claimed = self
            .store
            .claim_disconnected_session(session_id, observed)
            .unwrap_or_else(|e| {
                log::warn!("[reconcile] Failed to claim session {session_id}: {e}");
                false
            });
        if !claimed {
            self.registry.deregister(session_id, epoch);
            // The row moved since it was read: the Stop goes to it as it is
            // now, as an attempt that loses its claim passes one on.
            if self.disconnected_row(session_id).is_some() {
                return self.stop_disconnected(session_id, reason, HeldWork::Unknown);
            }
            self.host.restop(session_id);
            return RowOutcome::Done;
        }
        // The Stop's own reason on the entry: a Stop, so nothing queued runs.
        self.registry
            .cancel_with_completion_reason(session_id, reason.clone());
        log::info!("[reconcile] Session {session_id}: stopping its turn, over on the workstation");
        let mut message = None;
        let latest_prompt =
            cancel_then_backfill(self.blox.as_ref(), target, backfill, |m| message = m);
        if let Some(message) = message {
            log::warn!("[reconcile] Session {session_id}: {message}");
        }
        let verdict = over_turn_stop_verdict(recorded, latest_prompt, reason);
        self.settle_claimed(target, epoch, &token, verdict)
    }

    /// The store to backfill `target`'s transcript through, once its
    /// checkout on the workstation is known: the session is loaded in it.
    /// `None`, cancelling without a backfill, when that can't be resolved.
    fn backfill_store(&self, target: &mut ReconcileTarget) -> Option<&Arc<Store>> {
        match target.resolve_remote_working_dir(self.blox.as_ref()) {
            Ok(()) => Some(&self.store),
            Err(e) => {
                log::warn!(
                    "[reconcile] Session {}: couldn't resolve its checkout on the workstation, \
                     so the Stop won't backfill its transcript: {e}",
                    target.session_id
                );
                None
            }
        }
    }

    /// Cancel the turn over a fresh connection, then backfill the
    /// transcript from the session's replay, so the tool call the turn was
    /// running ends as the agent ended it instead of running for good.
    /// `settle` gets the row's message — `Some` when the cancel couldn't be
    /// confirmed stopped — as soon as the cancel's outcome is known, before
    /// the backfill; this returns once the backfill is over too.
    pub(super) fn cancel_then_settle(
        &self,
        target: &ReconcileTarget,
        settle: impl FnOnce(Option<String>),
    ) {
        let mut target = target.clone();
        let backfill = self.backfill_store(&mut target);
        cancel_then_backfill(self.blox.as_ref(), &target, backfill, settle);
    }

    /// [`Self::cancel_then_settle`], for a caller that settles only once the
    /// backfill is done too: a turn that completes, whose hooks read the
    /// transcript.
    pub(super) fn cancel_remote_turn(&self, target: &ReconcileTarget) -> Option<String> {
        let mut message = None;
        self.cancel_then_settle(target, |m| message = m);
        message
    }
}

/// The verdict for a Stop on a turn already over on the workstation: the
/// turn's own, from its recording, with a held turn's wait for its work cut
/// short — unless the recording shows the work over by now. `latest_prompt`
/// is what the cancel's backfill replayed: a newest prompt the agent never
/// received makes the recording an earlier turn's, and this one never ran.
fn over_turn_stop_verdict(
    recorded: RecordedTurn,
    latest_prompt: LatestPrompt,
    reason: CompletionReason,
) -> Verdict {
    let work_over = ended(&recorded);
    let verdict = from_ended_turn(recorded_result(recorded), latest_prompt, Some(reason));
    if work_over {
        verdict
    } else {
        verdict.with_held_work_cut_short(CompletionReason::HoldStopped)
    }
}

/// Whether `recorded` shows the turn over with nothing it set going left
/// running: the Claude bridge's `idle` with no live task, or, for an agent
/// with no turn state, an answer to the prompt — which describes the turn
/// only of a process that reads `IDLE`.
fn ended(recorded: &RecordedTurn) -> bool {
    matches!(
        recorded,
        RecordedTurn::Ended { .. } | RecordedTurn::Answered { .. }
    )
}

/// The message for a row a cancel ended: `Some` when the turn couldn't be
/// confirmed stopped.
fn cancel_message(
    target: &ReconcileTarget,
    cancelled: Result<CancelOutcome, String>,
) -> Option<String> {
    match cancelled {
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

/// Cancel `target`'s turn over a fresh connection, then, with `backfill`,
/// catch its transcript up (see [`ReconcileBlox::cancel`]). `settle` gets
/// the message for the row as soon as the cancel's outcome is known, before
/// the backfill; this returns once the backfill is over too, with whether
/// its replay showed the agent receiving the newest prompt.
fn cancel_then_backfill(
    blox: &dyn ReconcileBlox,
    target: &ReconcileTarget,
    backfill: Option<&Arc<Store>>,
    settle: impl FnOnce(Option<String>),
) -> LatestPrompt {
    let Some(store) = backfill else {
        settle(cancel_message(
            target,
            blox.cancel(target, None, STOP_DEADLINE)
                .map(|(outcome, _)| outcome),
        ));
        return LatestPrompt::Unknown;
    };
    // `settle` may end the row with the backfill still going.
    let _backfilling = BackfillInFlight::start(&target.session_id);
    let (heard_tx, heard_rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let cancel = scope.spawn(move || {
            let on_cancelled = move |outcome: &CancelOutcome| {
                let _ = heard_tx.send(outcome.clone());
            };
            blox.cancel(
                target,
                Some(Backfill {
                    store,
                    on_cancelled: Box::new(on_cancelled),
                }),
                STOP_DEADLINE,
            )
        });
        let joined = |cancel: std::thread::ScopedJoinHandle<'_, _>| {
            cancel
                .join()
                .unwrap_or_else(|_| Err("the cancel panicked".to_string()))
        };
        // Closed without a message when the cancel ended with no outcome to
        // hear (an `Err`).
        let cancelled = match heard_rx.recv() {
            Ok(outcome) => {
                settle(cancel_message(target, Ok(outcome)));
                joined(cancel)
            }
            Err(_) => {
                let cancelled = joined(cancel);
                settle(cancel_message(
                    target,
                    cancelled.clone().map(|(outcome, _)| outcome),
                ));
                cancelled
            }
        };
        cancelled.map_or(LatestPrompt::Unknown, |(_, latest_prompt)| latest_prompt)
    })
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
        // Nothing to backfill: the row is about to go.
        let cancelled = blox
            .cancel(&target, None, STOP_DEADLINE)
            .map(|(outcome, _)| outcome);
        if let Some(message) = cancel_message(&target, cancelled) {
            log::warn!("[reconcile] Session {}: {message}", target.session_id);
        }
    }))
}

/// A Stop on a running Blox session whose `session/cancel` went unanswered
/// (see `acp_client::SessionSettled::cancel_unacknowledged`): the transport
/// may be hung with the turn still going on the workstation, so cancel it
/// again over a fresh connection, and backfill what the hung transport never
/// delivered. Returns the message for the `cancelled` row when that couldn't
/// be confirmed either.
///
/// Returns as soon as the cancel's outcome is known, so the runner can end
/// the row; the backfill goes on on a thread of its own, and `announce` gets
/// the session's `idle` reconcile event once it is done (see the module
/// docs).
pub(crate) fn stop_unacknowledged_turn(
    store: &Arc<Store>,
    blox: Arc<dyn ReconcileBlox>,
    session_id: &str,
    announce: impl FnOnce(&SessionReconcileEvent) + Send + 'static,
) -> Option<String> {
    let Some((target, backfills)) = running_turn_target(store, blox.as_ref(), session_id) else {
        return Some(MAY_STILL_RUN.to_string());
    };
    if !backfills {
        return cancel_message(
            &target,
            blox.cancel(&target, None, STOP_DEADLINE)
                .map(|(outcome, _)| outcome),
        );
    }
    let (heard_tx, heard_rx) = std::sync::mpsc::channel();
    let store = Arc::clone(store);
    let backfilling = BackfillInFlight::start(session_id);
    let cancel = {
        let target = target.clone();
        std::thread::spawn(move || {
            let on_cancelled = move |outcome: &CancelOutcome| {
                let _ = heard_tx.send(outcome.clone());
            };
            let cancelled = blox.cancel(
                &target,
                Some(Backfill {
                    store: &store,
                    on_cancelled: Box::new(on_cancelled),
                }),
                STOP_DEADLINE,
            );
            drop(backfilling);
            announce(&backfilled_event(&target));
            cancelled.map(|(outcome, _)| outcome)
        })
    };
    // Closed without a message when the cancel ended with no outcome to hear.
    match heard_rx.recv() {
        Ok(outcome) => cancel_message(&target, Ok(outcome)),
        Err(_) => cancel_message(
            &target,
            cancel
                .join()
                .unwrap_or_else(|_| Err("the cancel panicked".to_string())),
        ),
    }
}

/// A Stop during a running Blox session's post-turn hold whose cancel
/// nothing showed landing: the turn stands completed, but the work it left
/// running may go on, so cancel it over a fresh connection, and backfill the
/// transcript. Returns once both are done — the turn's completion hooks,
/// next, read the transcript — with a message when the cancel couldn't be
/// confirmed.
pub(crate) fn stop_outliving_work(
    store: &Arc<Store>,
    blox: &dyn ReconcileBlox,
    session_id: &str,
) -> Option<String> {
    let Some((target, backfills)) = running_turn_target(store, blox, session_id) else {
        return Some(MAY_STILL_RUN.to_string());
    };
    let mut message = None;
    cancel_then_backfill(blox, &target, backfills.then_some(store), |m| {
        message = m;
    });
    message
}

/// The turn a running session's fresh cancel reaches, and whether the cancel
/// can backfill its transcript: whether its checkout on the workstation
/// resolved. `None` when there is nothing to cancel through.
fn running_turn_target(
    store: &Store,
    blox: &dyn ReconcileBlox,
    session_id: &str,
) -> Option<(ReconcileTarget, bool)> {
    let session = store.get_session(session_id).ok().flatten()?;
    let mut target = ReconcileTarget::resolve(store, &session).ok()?;
    let backfills = match target.resolve_remote_working_dir(blox) {
        Ok(()) => true,
        Err(e) => {
            log::warn!(
                "[reconcile] Session {session_id}: couldn't resolve its checkout on the \
                 workstation, so the Stop won't backfill its transcript: {e}"
            );
            false
        }
    };
    Some((target, backfills))
}

/// The `idle` reconcile event a Stop's backfill ends with, for a pane
/// showing the session to re-read its transcript.
fn backfilled_event(target: &ReconcileTarget) -> SessionReconcileEvent {
    SessionReconcileEvent {
        session_id: target.session_id.clone(),
        branch_id: target.branch_id.clone(),
        project_id: target.project_id.clone(),
        phase: ReconcilePhase::Idle,
        reason: None,
        attempts: 0,
        next_attempt_at: None,
    }
}

// =============================================================================
// Backfills that outlive their Stop
// =============================================================================

/// Sessions a Stop's backfill is still writing into, after the row ended.
static BACKFILLING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// A session's entry in [`BACKFILLING`], for as long as this lives.
pub(super) struct BackfillInFlight(String);

impl BackfillInFlight {
    pub(super) fn start(session_id: &str) -> Self {
        BACKFILLING.lock().unwrap().insert(session_id.to_string());
        Self(session_id.to_string())
    }
}

impl Drop for BackfillInFlight {
    fn drop(&mut self) {
        BACKFILLING.lock().unwrap().remove(&self.0);
    }
}

/// Whether a Stop's backfill is still writing the session's transcript.
pub(crate) fn stop_backfill_in_flight(session_id: &str) -> bool {
    BACKFILLING.lock().unwrap().contains(session_id)
}

/// Wait, up to [`BACKFILL_WAIT`], for a Stop's backfill of the session to
/// finish: a turn started meanwhile would have the stopped turn's tail
/// written after its prompt.
pub(crate) async fn stop_backfill_finished(session_id: &str) {
    if !stop_backfill_in_flight(session_id) {
        return;
    }
    log::info!("[reconcile] Session {session_id}: waiting for its Stop's backfill");
    let waited = tokio::time::timeout(BACKFILL_WAIT, async {
        while stop_backfill_in_flight(session_id) {
            tokio::time::sleep(BACKFILL_WAIT_POLL).await;
        }
    })
    .await;
    if waited.is_err() {
        log::warn!(
            "[reconcile] Session {session_id}: going ahead with its Stop's backfill still running"
        );
    }
}
