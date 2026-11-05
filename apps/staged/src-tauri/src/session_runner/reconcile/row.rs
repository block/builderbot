//! One attempt at settling a `disconnected` row.

use acp_client::{
    AttachOutcome, LatestPrompt, RecordedTurn, RemoteProcessStatus, StopReason, TurnResult,
};
use tokio_util::sync::CancellationToken;

use super::blox::Unresolvable;
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

/// How a claimed row ends this attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// Back to `disconnected`: this attempt stopped following a turn that
    /// may still be running. `reason` is why, when it is a failure to back
    /// off from; `None` for a quit, which detached on purpose.
    Disconnected(Option<ReconcileReason>),
    Completed,
    Cancelled {
        message: Option<String>,
        reason: CompletionReason,
    },
    Failed {
        message: String,
    },
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

        if let Some(subpath) = target.workspace_subpath.clone() {
            match self
                .blox
                .remote_working_dir(&target.workspace_name, &subpath)
            {
                Ok(dir) => target.remote_working_dir = Some(dir),
                Err(e) => {
                    return RowOutcome::Backoff(ReconcileReason::new(
                        ReconcileReasonKind::Unreachable,
                        e,
                    ))
                }
            }
        }
        if let Some(reason) = self.take_stop_request(session_id) {
            return self.stop_observed(&target, observed, reason);
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
        // The recording of a gone process is read before the attach, which
        // starts a replacement process with a recording of its own.
        let recorded = match process.status {
            RemoteProcessStatus::Gone { .. } => Some(self.read_recorded_turn(&target)),
            RemoteProcessStatus::Running | RemoteProcessStatus::Idle => None,
        };
        let gone_message = match process.status {
            RemoteProcessStatus::Gone { message } => message,
            _ => None,
        };

        if let Some(reason) = self.take_stop_request(session_id) {
            return self.stop_observed(&target, observed, reason);
        }
        if self.host.is_quitting() {
            return RowOutcome::Left;
        }
        self.follow(&target, observed, recorded, gone_message)
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
    fn read_recorded_turn(&self, target: &ReconcileTarget) -> RecordedTurn {
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
    fn follow(
        &self,
        target: &ReconcileTarget,
        observed: i64,
        recorded: Option<RecordedTurn>,
        gone_message: Option<String>,
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
            return self.stop_observed(target, observed, reason);
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
                    return self.stop_disconnected(session_id, reason);
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
            }) => {
                // A turn that ended before the attach was made sent its
                // `result` frame to the connection that is gone, and the
                // replay re-sends none: the process's recording still holds
                // it. Without it an errored or aborted turn would read as
                // completed.
                let result = result.or_else(|| {
                    match recorded.unwrap_or_else(|| self.read_recorded_turn(target)) {
                        RecordedTurn::Ended { result } => result,
                        RecordedTurn::InFlight | RecordedTurn::Unknown => None,
                    }
                });
                from_ended_turn(result, latest_prompt, stop)
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
                let reason = stop.unwrap_or(CompletionReason::Interrupted);
                let message = self.cancel_remote_turn(target);
                Verdict::Cancelled { message, reason }
            }
            Ok(AttachOutcome::Live) => Verdict::Disconnected(None),
            Ok(AttachOutcome::TransportLost) | Err(_) if quit || token.is_cancelled() => {
                Verdict::Disconnected(None)
            }
            Ok(AttachOutcome::TransportLost) => Verdict::Disconnected(Some(ReconcileReason::new(
                ReconcileReasonKind::AttachFailed,
                "The connection to the workstation was lost",
            ))),
            Err(e) => {
                log::warn!("[reconcile] Session {session_id}: attach failed: {e}");
                Verdict::Disconnected(Some(ReconcileReason::new(
                    ReconcileReasonKind::AttachFailed,
                    e,
                )))
            }
        }
    }

    /// Write the verdict on a row this attempt claimed, then do what follows
    /// a terminal state.
    fn settle_claimed(
        &self,
        target: &ReconcileTarget,
        epoch: RegistrationEpoch,
        token: &CancellationToken,
        verdict: Verdict,
    ) -> RowOutcome {
        let session_id = target.session_id.as_str();
        let cancelled_before_hooks = token.is_cancelled();
        let (status, message, reason) = match verdict {
            Verdict::Disconnected(failure) => {
                self.registry.mark_finished(session_id, epoch);
                let written = self
                    .store
                    .transition_from_running(session_id, SessionStatus::Disconnected, None, None)
                    .unwrap_or(false);
                self.registry.deregister(session_id, epoch);
                if written {
                    self.emit_status(target, &SessionStatus::Disconnected, None, None);
                }
                return match failure {
                    Some(reason) if written => RowOutcome::Backoff(reason),
                    _ if written => RowOutcome::Left,
                    _ => RowOutcome::Done,
                };
            }
            Verdict::Completed => {
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
                    (
                        SessionStatus::Completed,
                        None,
                        CompletionReason::TurnComplete,
                    )
                }
            }
            Verdict::Cancelled { message, reason } => (SessionStatus::Cancelled, message, reason),
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

/// The verdict for a turn an attach saw end, from the session's last
/// `result` frame — read as the live path reads the prompt's answer — and
/// whether the agent received the newest prompt at all. `stop` is a Stop
/// that reached the attempt meanwhile.
fn from_ended_turn(
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
        Some(TurnResult::Stopped(_)) | None => Verdict::Completed,
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
        RecordedTurn::Ended { result } => from_ended_turn(result, latest_prompt, None),
        RecordedTurn::InFlight => Verdict::Failed {
            message: process_message.unwrap_or_else(|| PROCESS_DIED_MID_TURN.to_string()),
        },
        RecordedTurn::Unknown => match process_message {
            Some(message) => Verdict::Failed { message },
            None => Verdict::Cancelled {
                message: Some(TURN_UNDETERMINED.to_string()),
                reason: CompletionReason::Interrupted,
            },
        },
    }
}
