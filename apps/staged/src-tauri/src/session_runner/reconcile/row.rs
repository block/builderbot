//! One attempt at settling a `disconnected` row.

use acp_client::{
    AttachOutcome, LatestPrompt, RecordedTurn, RemoteProcessStatus, StopReason, TurnResult,
};
use tokio_util::sync::CancellationToken;

use super::blox::Unresolvable;
use super::stop::HeldWork;
use super::{
    AttachControl, ReconcileReason, ReconcileReasonKind, ReconcileTarget, Reconciler, RowOutcome,
    UnreachableKind, WorkspaceProbe,
};
use crate::session_runner::{RegistrationEpoch, SessionStatusEvent};
use crate::store::{CompletionReason, Session, SessionStatus};

pub(super) const WORKSTATION_GONE: &str = "The workstation no longer exists";
/// The turn's process ended mid-turn and Blox recorded no reason.
const PROCESS_DIED_MID_TURN: &str =
    "The agent on the workstation stopped in the middle of the turn";
/// Nothing says whether the turn finished: no turn state in the recording,
/// and no failure recorded for the process.
pub(super) const TURN_UNDETERMINED: &str =
    "Staged couldn't tell whether this turn finished on the workstation";
const NO_WORKSPACE: &str = "Staged couldn't find this session's workstation";
/// The newest prompt never reached the agent: the connection went between
/// Staged recording it and sending it, so the turn that ended was an earlier
/// one.
pub(super) const PROMPT_NOT_RECEIVED: &str =
    "Staged lost its connection before this message reached the workstation";
/// An error result the agent gave no text for.
const TURN_FAILED: &str = "The agent's turn on the workstation ended with an error";
/// The turn is over, and the work it set going is not.
pub(super) const BACKGROUND_WORK_LIVE: &str =
    "The turn has ended, but background work it started is still running on the workstation";

/// How a claimed row ends this attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Verdict {
    /// Back to `disconnected`, reporting the outcome if the write wins: this
    /// attempt stopped following a turn that may still be running (`Left`
    /// for a quit, which detached on purpose; `Backoff` for a failure), or
    /// saw it end with the session still working (`Held`).
    Disconnected(RowOutcome),
    /// `TurnComplete`, or, for a turn whose session was still working, why
    /// the wait for the work was cut short (`HeldUntilCap`, `HoldStopped`),
    /// as the live hold records it.
    Completed {
        reason: CompletionReason,
    },
    Cancelled {
        message: Option<String>,
        reason: CompletionReason,
    },
    /// A Stop reached a turn the attach let go of, which may still be
    /// running: cancel it over a fresh connection, and end the row
    /// `cancelled` for `reason` as soon as the cancel's outcome is known, its
    /// backfill going on after (see [`super::stop`]).
    StopRemoteTurn {
        reason: CompletionReason,
    },
    Failed {
        message: String,
    },
}

impl Verdict {
    /// A completed turn's, recorded with the wait for its session's work cut
    /// short for `reason`; any other verdict as it is.
    pub(super) fn with_held_work_cut_short(self, reason: CompletionReason) -> Self {
        match self {
            Verdict::Completed { .. } => Verdict::Completed { reason },
            verdict => verdict,
        }
    }
}

impl Reconciler {
    pub(super) fn reconcile_row(&self, session_id: &str) -> RowOutcome {
        let Some(session) = self.disconnected_row(session_id) else {
            return RowOutcome::Done;
        };
        let observed = session.updated_at;
        let mut target = match ReconcileTarget::resolve(&self.store, &session) {
            Ok(target) => target,
            Err(unresolvable) => return self.settle_unresolvable(&session, unresolvable),
        };
        self.note_target(&target);

        match self.blox.workspace(&target.workspace_name) {
            WorkspaceProbe::Running => {}
            WorkspaceProbe::Deleted => {
                log::info!(
                    "[reconcile] Session {session_id}: workstation {} no longer exists",
                    target.workspace_name
                );
                self.settle_unclaimed(
                    &target,
                    observed,
                    SessionStatus::Error,
                    Some(WORKSTATION_GONE),
                    CompletionReason::Crashed,
                );
                return RowOutcome::Done;
            }
            WorkspaceProbe::Suspended => {
                return RowOutcome::Backoff(ReconcileReason::new(
                    ReconcileReasonKind::Suspended,
                    "The workstation is suspended",
                ))
            }
            WorkspaceProbe::NotRunning(state) => {
                return RowOutcome::Backoff(ReconcileReason::new(
                    ReconcileReasonKind::WorkstationNotRunning,
                    format!("The workstation is {state}"),
                ))
            }
            WorkspaceProbe::Unreachable { kind, message } => {
                let kind = match kind {
                    UnreachableKind::Unreachable => ReconcileReasonKind::Unreachable,
                    UnreachableKind::NotAuthenticated => ReconcileReasonKind::NotAuthenticated,
                };
                return RowOutcome::Backoff(ReconcileReason::new(kind, message));
            }
        }

        if let Err(e) = target.resolve_remote_working_dir(self.blox.as_ref()) {
            return RowOutcome::Backoff(ReconcileReason::new(ReconcileReasonKind::Unreachable, e));
        }
        if let Some(reason) = self.take_stop_request(session_id) {
            return self.stop_observed(&target, observed, reason, HeldWork::Unknown);
        }

        let process = match self.blox.process(&target) {
            Ok(process) => process,
            Err(e) => {
                return RowOutcome::Backoff(ReconcileReason::new(
                    ReconcileReasonKind::Unreachable,
                    e,
                ))
            }
        };
        // Pin the process, so this and every later read reaches it rather
        // than a replacement Blox starts in its place (an attach to a gone
        // process starts one).
        if target.command_id.is_none() {
            if let Some(command_id) = &process.command_id {
                if let Err(e) = self
                    .store
                    .set_session_blox_command_id(session_id, Some(command_id))
                {
                    log::warn!("[reconcile] Failed to pin session {session_id}'s process: {e}");
                }
                target.command_id = Some(command_id.clone());
            }
        }
        let mut held = HeldWork::NotHeld;
        let recorded = match process.status {
            // The recording of a gone process is read before the attach,
            // which starts a replacement process with a recording of its own.
            RemoteProcessStatus::Gone { .. } => Some(self.read_recorded_turn(&target)),
            // `IDLE` only says every prompt was answered: the work a turn set
            // going (background tasks, the continuation cycles they wake)
            // runs on with none in flight. Checked before claiming, so a row
            // held for it doesn't flip `running` and back on every check —
            // for the agents whose recording can show it.
            RemoteProcessStatus::Idle
                if acp_client::recording_shows_held_work(target.provider.as_deref()) =>
            {
                let recorded = self.read_recorded_turn(&target);
                if shows_held_work(&recorded) {
                    let tasks = live_task_ids(&recorded);
                    if !self.held_work_ran_out(session_id, tasks, process.idle_since) {
                        return RowOutcome::Held(held_work_reason());
                    }
                    held = HeldWork::Held;
                }
                // A turn the recording shows ended is how the attach would
                // read it once the turn is over: it is.
                matches!(recorded, RecordedTurn::Ended { .. }).then_some(recorded)
            }
            RemoteProcessStatus::Running | RemoteProcessStatus::Idle => None,
        };
        let gone_message = match process.status {
            RemoteProcessStatus::Gone { message } => message,
            _ => None,
        };

        if let Some(reason) = self.take_stop_request(session_id) {
            return self.stop_observed(&target, observed, reason, held);
        }
        if self.host.is_quitting() {
            return RowOutcome::Left;
        }
        self.follow(&target, observed, recorded, gone_message, held)
    }

    /// The row, if it is (still) `disconnected`.
    pub(super) fn disconnected_row(&self, session_id: &str) -> Option<Session> {
        self.store
            .get_session(session_id)
            .ok()
            .flatten()
            .filter(|session| session.status == SessionStatus::Disconnected)
    }

    /// How the turn stands in its pinned process's recording; `Unknown` when
    /// there is no pin or the recording can't be read.
    pub(super) fn read_recorded_turn(&self, target: &ReconcileTarget) -> RecordedTurn {
        let Some(command_id) = target.command_id.as_deref() else {
            return RecordedTurn::Unknown;
        };
        self.blox
            .recorded_turn(target, command_id)
            .unwrap_or_else(|e| {
                log::warn!(
                    "[reconcile] Couldn't read session {}'s process recording: {e}",
                    target.session_id
                );
                RecordedTurn::Unknown
            })
    }

    /// A row that can't be reconciled: nothing to ask Blox about, so nothing
    /// says how its turn ended.
    fn settle_unresolvable(&self, session: &Session, unresolvable: Unresolvable) -> RowOutcome {
        log::warn!(
            "[reconcile] Session {} can't be reconciled ({unresolvable:?})",
            session.id
        );
        let target = ReconcileTarget::unresolved(&self.store, session);
        let message = match unresolvable {
            Unresolvable::NoWorkspace => NO_WORKSPACE,
            Unresolvable::NoAgentSession => TURN_UNDETERMINED,
        };
        self.settle_unclaimed(
            &target,
            session.updated_at,
            SessionStatus::Cancelled,
            Some(message),
            CompletionReason::Interrupted,
        );
        RowOutcome::Done
    }

    /// End a row that was never claimed, if it is still the row that was
    /// read, and move its branch on. Returns whether the write won.
    pub(super) fn settle_unclaimed(
        &self,
        target: &ReconcileTarget,
        observed: i64,
        status: SessionStatus,
        message: Option<&str>,
        reason: CompletionReason,
    ) -> bool {
        let won = self
            .store
            .transition_from_disconnected_observed_at(
                &target.session_id,
                observed,
                status.clone(),
                message,
                Some(&reason),
            )
            .unwrap_or_else(|e| {
                log::warn!(
                    "[reconcile] Failed to settle session {}: {e}",
                    target.session_id
                );
                false
            });
        if !won {
            return false;
        }
        self.host.completion_side_effects(
            &target.session_id,
            target.branch_id.as_deref(),
            status.as_str(),
        );
        self.emit_status(target, &status, message.map(str::to_string), Some(&reason));
        if status == SessionStatus::Cancelled {
            self.host.skip_dependents_of_cancelled(&target.session_id);
        }
        self.host
            .drain_and_kick(&target.session_id, target.branch_id.as_deref(), false);
        true
    }

    pub(super) fn emit_status(
        &self,
        target: &ReconcileTarget,
        status: &SessionStatus,
        error_message: Option<String>,
        reason: Option<&CompletionReason>,
    ) {
        self.host
            .emit_status(self.status_event(target, status, error_message, reason));
    }

    fn status_event(
        &self,
        target: &ReconcileTarget,
        status: &SessionStatus,
        error_message: Option<String>,
        reason: Option<&CompletionReason>,
    ) -> SessionStatusEvent {
        SessionStatusEvent {
            session_id: target.session_id.clone(),
            status: status.as_str().to_string(),
            error_message,
            completion_reason: reason.map(|r| r.as_str().to_string()),
            rate_limit_resets_at: None,
            rate_limit_kind: None,
            branch_id: target.branch_id.clone(),
            project_id: target.project_id.clone(),
            // Every emit carries it: the frontend labels running work with
            // it, and a terminal emit after a `disconnected` one (which
            // cleared the frontend's record of the session) is the only
            // place its completion handling can learn it from.
            session_type: target.session_type.clone(),
            dependency_skipped: false,
        }
    }

    /// Claim the row `running`, attach, and settle from what the attach saw.
    /// `held` is what the attempt found about the session's work after its
    /// turn, for a Stop that lands before the claim.
    fn follow(
        &self,
        target: &ReconcileTarget,
        observed: i64,
        recorded: Option<RecordedTurn>,
        gone_message: Option<String>,
        held: HeldWork,
    ) -> RowOutcome {
        let session_id = target.session_id.as_str();
        // Registered before the claim, like every owner of a `running` row:
        // a `running` row this process owns with no entry is what the
        // stranded-session sweep ends.
        let (token, epoch) = self.registry.register(session_id);
        // The last checkpoint: a Stop from here on cancels the entry instead
        // (see `Reconciler::begin_stop`).
        if let Some(reason) = self.take_stop_request(session_id) {
            self.registry.deregister(session_id, epoch);
            return self.stop_observed(target, observed, reason, held);
        }
        let claimed = self
            .store
            .claim_disconnected_session(session_id, observed)
            .unwrap_or_else(|e| {
                log::warn!("[reconcile] Failed to claim session {session_id}: {e}");
                false
            });
        if !claimed {
            // Someone else moved the row since it was read. A Stop that
            // reached this entry meanwhile wrote nothing, trusting this
            // attempt to record it: pass it on. A quit's cancel is no Stop:
            // the turn is left running.
            // Read with the removal, so a Stop either lands before it and is
            // passed on here or finds no entry and goes its own way.
            let stop = self
                .registry
                .deregister_reporting_cancellation_of(session_id, epoch)
                .filter(|reason| *reason != CompletionReason::AppQuit);
            if let Some(reason) = stop {
                if self.disconnected_row(session_id).is_some() {
                    return self.stop_disconnected(session_id, reason, HeldWork::Unknown);
                }
                self.host.restop(session_id);
            }
            return RowOutcome::Done;
        }
        log::info!("[reconcile] Session {session_id}: attaching to its workstation turn");

        let registry = std::sync::Arc::clone(&self.registry);
        let detach_session_id = session_id.to_string();
        // `running` goes out once the attach is caught up and following, not
        // at the claim: an attach that keeps failing to connect would
        // otherwise flip the row running and back to disconnected on every
        // attempt.
        let host = std::sync::Arc::clone(&self.host);
        let running = self.status_event(target, &SessionStatus::Running, None, None);
        let announced = std::sync::atomic::AtomicBool::new(false);
        let control = AttachControl {
            token: token.clone(),
            on_detach_handle: Box::new(move |handle| {
                registry.set_detach_handle(&detach_session_id, handle);
            }),
            on_connected: Box::new(move || {
                if !announced.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    host.emit_status(running.clone());
                }
            }),
        };
        let attached = self.blox.attach(target, &self.store, &control);
        let verdict = self.verdict(target, epoch, &token, attached, recorded, gone_message);
        self.settle_claimed(target, epoch, &token, verdict)
    }

    fn verdict(
        &self,
        target: &ReconcileTarget,
        epoch: RegistrationEpoch,
        token: &CancellationToken,
        attached: Result<AttachOutcome, String>,
        recorded: Option<RecordedTurn>,
        gone_message: Option<String>,
    ) -> Verdict {
        let session_id = target.session_id.as_str();
        let cancellation = self
            .registry
            .cancellation_completion_reason_of(session_id, epoch);
        let quit = self.registry.detach_reason_of(session_id, epoch).is_some()
            || cancellation == Some(CompletionReason::AppQuit);
        let stop = cancellation.filter(|reason| *reason != CompletionReason::AppQuit);

        match attached {
            Ok(AttachOutcome::Ended {
                result,
                latest_prompt,
                background_work_live,
            }) => {
                // Read once the turn is over, so it holds the turn's end:
                // whether the session is still working (tasks started before
                // the attach are only in the recording), and how the turn
                // ended when the attach didn't see it. A turn that ended
                // before the attach sent its `result` frame, or its prompt's
                // answer, to the connection that is gone, and the replay
                // re-sends neither; without them an errored or aborted turn
                // would read as completed.
                let recorded = recorded.unwrap_or_else(|| self.read_recorded_turn(target));
                let cut_short = if background_work_live || shows_held_work(&recorded) {
                    if stop.is_some() {
                        // As a Stop during the live hold: the turn stands,
                        // and its work is cancelled.
                        if let Some(message) = self.cancel_remote_turn(target) {
                            log::warn!("[reconcile] Session {session_id}: {message}");
                        }
                        Some(CompletionReason::HoldStopped)
                    } else if self.held_work_ran_out(session_id, live_task_ids(&recorded), None) {
                        log::warn!(
                            "[reconcile] Session {session_id}: settling with its background \
                             work still live, past the hold's cap"
                        );
                        Some(CompletionReason::HeldUntilCap)
                    } else {
                        return Verdict::Disconnected(RowOutcome::Held(held_work_reason()));
                    }
                } else {
                    None
                };
                let verdict = from_ended_turn(
                    result.or_else(|| recorded_result(recorded)),
                    latest_prompt,
                    stop,
                );
                match cut_short {
                    Some(reason) => verdict.with_held_work_cut_short(reason),
                    None => verdict,
                }
            }
            Ok(AttachOutcome::ProcessGone {
                message,
                latest_prompt,
            }) => {
                let recorded = recorded.unwrap_or_else(|| self.read_recorded_turn(target));
                from_gone_process(recorded, latest_prompt, message.or(gone_message), stop)
            }
            // The attach let go of a turn that may still be running.
            Ok(AttachOutcome::Live) | Ok(AttachOutcome::TransportLost) | Err(_)
                if stop.is_some() =>
            {
                Verdict::StopRemoteTurn {
                    reason: stop.unwrap_or(CompletionReason::Interrupted),
                }
            }
            Ok(AttachOutcome::Live) => Verdict::Disconnected(RowOutcome::Left),
            Ok(AttachOutcome::TransportLost) | Err(_) if quit || token.is_cancelled() => {
                Verdict::Disconnected(RowOutcome::Left)
            }
            Ok(AttachOutcome::TransportLost) => {
                Verdict::Disconnected(RowOutcome::Backoff(ReconcileReason::new(
                    ReconcileReasonKind::AttachFailed,
                    "The connection to the workstation was lost",
                )))
            }
            Err(e) => {
                log::warn!("[reconcile] Session {session_id}: attach failed: {e}");
                Verdict::Disconnected(RowOutcome::Backoff(ReconcileReason::new(
                    ReconcileReasonKind::AttachFailed,
                    e,
                )))
            }
        }
    }

    /// Write the verdict on a row this attempt claimed, then do what follows
    /// a terminal state.
    pub(super) fn settle_claimed(
        &self,
        target: &ReconcileTarget,
        epoch: RegistrationEpoch,
        token: &CancellationToken,
        verdict: Verdict,
    ) -> RowOutcome {
        let session_id = target.session_id.as_str();
        let cancelled_before_hooks = token.is_cancelled();
        let (status, message, reason) = match verdict {
            Verdict::Disconnected(outcome) => {
                self.registry.mark_finished(session_id, epoch);
                let written = self
                    .store
                    .transition_from_running(session_id, SessionStatus::Disconnected, None, None)
                    .unwrap_or(false);
                self.registry.deregister(session_id, epoch);
                if !written {
                    return RowOutcome::Done;
                }
                self.emit_status(target, &SessionStatus::Disconnected, None, None);
                return outcome;
            }
            Verdict::Completed { reason } => {
                self.run_completion_hooks(target);
                if !cancelled_before_hooks && token.is_cancelled() {
                    log::info!(
                        "[reconcile] Session {session_id} cancelled during its post-completion hooks"
                    );
                    let reason = self
                        .registry
                        .cancellation_completion_reason_of(session_id, epoch)
                        .unwrap_or(CompletionReason::Interrupted);
                    (SessionStatus::Cancelled, None, reason)
                } else {
                    (SessionStatus::Completed, None, reason)
                }
            }
            Verdict::Cancelled { message, reason } => (SessionStatus::Cancelled, message, reason),
            Verdict::StopRemoteTurn { reason } => {
                let mut outcome = RowOutcome::Done;
                self.cancel_then_settle(target, |message| {
                    outcome = self.settle_claimed(
                        target,
                        epoch,
                        token,
                        Verdict::Cancelled { message, reason },
                    );
                });
                return outcome;
            }
            Verdict::Failed { message } => (
                SessionStatus::Error,
                Some(message),
                CompletionReason::Crashed,
            ),
        };

        self.registry.mark_finished(session_id, epoch);
        let transitioned = self
            .store
            .transition_from_running(
                session_id,
                status.clone(),
                message.as_deref(),
                Some(&reason),
            )
            .unwrap_or(false);
        self.registry.deregister(session_id, epoch);
        log::info!(
            "[reconcile] Session {session_id} settled {}{}",
            status.as_str(),
            if transitioned {
                ""
            } else {
                " (row already moved on)"
            }
        );
        if transitioned {
            self.host.completion_side_effects(
                session_id,
                target.branch_id.as_deref(),
                status.as_str(),
            );
        }
        self.emit_status(target, &status, message, Some(&reason));
        if transitioned {
            // A Stop anywhere up to the deregister says the queued follow-up
            // should not run, as for any runner.
            let drain_message = status == SessionStatus::Completed && !token.is_cancelled();
            self.host
                .drain_and_kick(session_id, target.branch_id.as_deref(), drain_message);
        }
        RowOutcome::Done
    }

    /// The commit, note and review hooks, against the turn's persisted base
    /// and from the prompt that started it.
    fn run_completion_hooks(&self, target: &ReconcileTarget) {
        let turn_start_message_id = self
            .store
            .latest_user_message_id(&target.session_id)
            .ok()
            .flatten()
            .unwrap_or(0);
        crate::session_runner::run_post_completion_hooks(
            &target.session_id,
            &target.working_dir,
            target.pre_head_sha.as_deref(),
            self.blox.checkout_workspace(target),
            target
                .remote_working_dir
                .as_deref()
                .and_then(|dir| dir.to_str()),
            &self.store,
            turn_start_message_id,
        );
    }
}

/// The verdict for a newest prompt the agent never received: whatever the
/// session shows ended is an earlier turn, and this one never ran. Ended
/// resumable, with the message saying why, so it can be sent again.
fn prompt_not_received(stop: Option<CompletionReason>) -> Verdict {
    Verdict::Cancelled {
        message: Some(PROMPT_NOT_RECEIVED.to_string()),
        reason: stop.unwrap_or(CompletionReason::Interrupted),
    }
}

/// Whether a recording read from a process that reads `IDLE` (every prompt
/// answered) shows the session still working: background tasks live after
/// its turn, or a cycle running with no prompt in flight, one a settled task
/// woke.
pub(super) fn shows_held_work(recorded: &RecordedTurn) -> bool {
    matches!(
        recorded,
        RecordedTurn::Holding { .. } | RecordedTurn::InFlight
    )
}

/// The background tasks a recording shows live, for timing the hold on
/// them; none for a cycle running between them.
fn live_task_ids(recorded: &RecordedTurn) -> Vec<String> {
    match recorded {
        RecordedTurn::Holding { task_ids, .. } => task_ids.clone(),
        _ => Vec::new(),
    }
}

fn held_work_reason() -> ReconcileReason {
    ReconcileReason::new(ReconcileReasonKind::BackgroundWork, BACKGROUND_WORK_LIVE)
}

/// How a recording says the session's last turn ended, for a turn an attach
/// saw end: the Claude bridge's `result` frame, or, for an agent with no turn
/// state, the prompt's own answer.
pub(super) fn recorded_result(recorded: RecordedTurn) -> Option<TurnResult> {
    match recorded {
        RecordedTurn::Ended { result } | RecordedTurn::Holding { result, .. } => result,
        RecordedTurn::Answered { result } => Some(result),
        RecordedTurn::InFlight | RecordedTurn::Unknown => None,
    }
}

/// The verdict for a turn an attach saw end, from how it ended — the
/// session's last `result` frame, or the prompt's answer in the recording,
/// read as the live path reads the prompt's answer — and whether the agent
/// received the newest prompt at all. `stop` is a Stop that reached the
/// attempt meanwhile.
///
/// Known limitation: a `result` of `None` settles `completed`. The process
/// read `IDLE`, so the prompt was answered, but nothing says how: the
/// recording couldn't be read, or the process is shared with other sessions
/// (an agent this build doesn't know runs on Blox's shared default), whose
/// answers can't be told from this session's. Every agent Staged runs on a
/// process of its own (Claude, Codex, Goose, Pi, Amp) is read from its
/// recording.
pub(super) fn from_ended_turn(
    result: Option<TurnResult>,
    latest_prompt: LatestPrompt,
    stop: Option<CompletionReason>,
) -> Verdict {
    if latest_prompt == LatestPrompt::Missing {
        return prompt_not_received(stop);
    }
    match result {
        Some(TurnResult::Stopped(StopReason::Cancelled)) => Verdict::Cancelled {
            message: None,
            reason: stop.unwrap_or(CompletionReason::Interrupted),
        },
        Some(TurnResult::Failed { message }) => Verdict::Failed {
            message: message.unwrap_or_else(|| TURN_FAILED.to_string()),
        },
        Some(TurnResult::Stopped(_)) | None => Verdict::Completed {
            reason: CompletionReason::TurnComplete,
        },
    }
}

/// The verdict for a turn whose process is gone, from its recording, what
/// Blox recorded about the process, and whether the replay showed the agent
/// receiving the newest prompt.
fn from_gone_process(
    recorded: RecordedTurn,
    latest_prompt: LatestPrompt,
    process_message: Option<String>,
    stop: Option<CompletionReason>,
) -> Verdict {
    if latest_prompt == LatestPrompt::Missing {
        return prompt_not_received(stop);
    }
    match recorded {
        // Work the turn left running died with the process; the turn stands.
        RecordedTurn::Ended { result } | RecordedTurn::Holding { result, .. } => {
            from_ended_turn(result, latest_prompt, None)
        }
        RecordedTurn::InFlight => Verdict::Failed {
            message: process_message.unwrap_or_else(|| PROCESS_DIED_MID_TURN.to_string()),
        },
        // A prompt answer doesn't say a later prompt wasn't in flight when
        // the process went.
        RecordedTurn::Answered { .. } | RecordedTurn::Unknown => match process_message {
            Some(message) => Verdict::Failed { message },
            None => Verdict::Cancelled {
                message: Some(TURN_UNDETERMINED.to_string()),
                reason: CompletionReason::Interrupted,
            },
        },
    }
}
