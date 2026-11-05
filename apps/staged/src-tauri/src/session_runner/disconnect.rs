//! Blox sessions that outlive their connection: the `disconnected` status.
//!
//! A Blox session's agent runs on its workstation; Staged only holds a local
//! `blox acp` proxy to it. When that proxy goes away with a turn in flight —
//! the transport dropped, or Staged quit and detached it — the turn may still
//! be running remotely. Ending the row as failed would be wrong, running its
//! completion hooks would act on half a turn, and freeing the branch would let
//! new work start under it. So the row is written `disconnected` instead: not
//! terminal, owned by no process, and busy to the branch queue. Stop ends it.
//!
//! Local sessions never take this path. Their agent is a child of this
//! process, so losing it means the turn is gone too.

use acp_client::{SessionSettleReason, TransportLoss, TransportLossPhase};

use super::{RegistrationEpoch, SessionRegistry};
use crate::store::{CompletionReason, Session, Store};

/// Whether a finished run leaves its session `disconnected` rather than
/// terminal: a remote run whose connection was lost — or detached — while
/// something may still be running on the workstation.
///
/// That is a turn in flight: the run failed (`error`) with the loss in the
/// turn, or detached. Or work the post-turn hold was waiting for: the turn
/// completed, but the hold lost its connection with background tasks still
/// live (`background_work_left`), and whatever they do next — the
/// continuation they trigger included — would be recorded by no one if the
/// row settled, and the queue drained under it. Reconcile keeps such a row
/// `disconnected` for as long as the process's recording, or an attach's live
/// stream, shows that work going, rechecking it until it ends (bounded by the
/// live hold's ceiling), and only then settles the completed turn. The
/// process reading `IDLE` does not end the wait: that only says the prompt
/// was answered. A loss during setup sent no prompt, so nothing is running
/// remotely; a hold lost with nothing live settles as the completed turn it
/// is. Errors the agent answered itself (auth, a refused prompt, a
/// bridge-relayed "process exited") carry no transport loss at all. A Stop
/// always wins, because the user asked for the turn to end; and a local run
/// never qualifies, because its agent went down with the connection.
///
/// The shared proxy's own verdicts count too. Its agent lost for good (bloxlet
/// reports the process gone) leaves nothing to come back for, and neither
/// does a session cut off for falling behind, whose turn was cancelled on the
/// agent: both stay failures. Its agent detached is a lost transport like any
/// other — reached here once reattaching in place has given up: only `sq`
/// went, and bloxlet still runs the agent process, whether `sq` started it or
/// reused it (an `sq` SIGKILLed or cut off by the network leaves even a
/// process it started running), or bloxlet couldn't be asked, which leaves
/// the row for reconcile's backoff to settle. A detach on purpose wins over
/// all of them.
pub(crate) fn run_leaves_session_disconnected(
    is_remote: bool,
    cancel_requested: bool,
    error: Option<&str>,
    settle_reason: Option<SessionSettleReason>,
    transport_loss: Option<TransportLoss>,
    background_work_left: bool,
) -> bool {
    let lost_in_turn = error.is_some_and(|error| {
        settle_reason == Some(SessionSettleReason::Detached)
            || (!acp_client::is_remote_agent_lost_error(error)
                && !acp_client::is_remote_session_cut_off_error(error)
                && transport_loss.is_some_and(|loss| loss.phase == TransportLossPhase::Turn))
    });
    let lost_holding_live_work = background_work_left
        && transport_loss.is_some_and(|loss| loss.phase == TransportLossPhase::Hold);
    is_remote && !cancel_requested && (lost_in_turn || lost_holding_live_work)
}

/// Whether a run whose connection ended with `error` picks its turn back up
/// on the agent rather than settling: its `sq` proxy went while bloxlet still
/// runs the agent ([`acp_client::is_remote_agent_detached_error`]). Not after
/// a Stop, which ends the turn, nor after a quit's detach, which leaves it
/// running: its SIGKILL of the proxy can read as the proxy going, when
/// bloxlet is asked about it first (`settle_reason` says which it was). A
/// quit in progress is checked by the caller, once it can be told the quit
/// apart from the reattach's own.
pub(crate) fn run_picks_up_detached_turn(
    error: &str,
    stopped: bool,
    settle_reason: Option<SessionSettleReason>,
) -> bool {
    acp_client::is_remote_agent_detached_error(error)
        && !stopped
        && settle_reason != Some(SessionSettleReason::Detached)
}

/// Deregister a run that left its session `disconnected`, returning the Stop
/// that reached its entry after the run decided — between reading its token
/// and the deregister. That Stop wrote nothing, trusting the run to record
/// it, and a `disconnected` row records none: the caller hands it to
/// reconcile. A quit's cancel is no Stop; the turn is left running.
pub(super) fn deregister_disconnected_run(
    registry: &SessionRegistry,
    session_id: &str,
    epoch: RegistrationEpoch,
) -> Option<CompletionReason> {
    registry
        .deregister_reporting_cancellation_of(session_id, epoch)
        .filter(|reason| *reason != CompletionReason::AppQuit)
}

/// Whether a remote run that completed with a Stop during its post-turn hold
/// must cancel the held work again over a fresh connection: the hold's
/// `session/cancel` went out of turn, so nothing answers it, and nothing
/// showed the work stopping (`background_work_left`) — over a hung transport
/// it may never have arrived. `stopped` excludes a quit's cancel, which
/// leaves remote work running on purpose.
pub(crate) fn held_work_needs_fresh_cancel(
    is_remote: bool,
    stopped: bool,
    background_work_left: bool,
) -> bool {
    is_remote && stopped && background_work_left
}

/// Whether a remote run that ended `cancelled` must cancel its turn again over
/// a fresh connection: the turn may still be running on the workstation,
/// because the Stop's `session/cancel` went unanswered (`cancel_unacknowledged`)
/// or never reached it — the transport went under with the turn in flight,
/// before or while the Stop was sent. A Stop wins over that loss in
/// [`run_leaves_session_disconnected`], so the row is not left `disconnected`
/// for reconcile to settle; the cancel has to happen here.
pub(crate) fn stopped_turn_needs_fresh_cancel(
    is_remote: bool,
    cancelled: bool,
    cancel_unacknowledged: bool,
    transport_loss: Option<TransportLoss>,
) -> bool {
    is_remote
        && cancelled
        && (cancel_unacknowledged
            || transport_loss.is_some_and(|loss| loss.phase == TransportLossPhase::Turn))
}

/// Whether a `running` row that no live process owns any more should be left
/// `disconnected` rather than ended: it runs on a Blox workstation and has an
/// agent session there whose turn may still be going.
///
/// The recovery paths only see the row, not how its connection ended, so this
/// errs towards `disconnected` — a row with an agent session may have been
/// mid-turn. A row without one never got as far as a prompt.
pub(crate) fn may_still_run_on_workstation(store: &Store, session: &Session) -> bool {
    crate::session_commands::resumable_agent_session_id(session.agent_id.as_deref()).is_some()
        && runs_on_blox(store, session)
}

/// Whether `session` runs on a Blox workstation: its branch has one.
pub(crate) fn runs_on_blox(store: &Store, session: &Session) -> bool {
    let branch_id = session
        .branch_id
        .clone()
        .or_else(|| store.get_branch_id_for_session(&session.id).ok().flatten());
    branch_id
        .and_then(|id| store.get_branch(&id).ok().flatten())
        .is_some_and(|branch| branch.workspace_name.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TURN: Option<TransportLoss> = Some(TransportLoss {
        phase: TransportLossPhase::Turn,
    });

    /// What a run lost with its transport fails with.
    const LOST: &str = "ACP transport lost with the turn in flight (the agent's stdout closed)";

    fn loss(phase: TransportLossPhase) -> Option<TransportLoss> {
        Some(TransportLoss { phase })
    }

    #[test]
    fn a_remote_turn_lost_in_flight_leaves_the_session_disconnected() {
        assert!(run_leaves_session_disconnected(
            true,
            false,
            Some(LOST),
            Some(SessionSettleReason::Failed),
            TURN,
            false
        ));
        // A quit's detach with the turn in flight.
        assert!(run_leaves_session_disconnected(
            true,
            false,
            Some(LOST),
            Some(SessionSettleReason::Detached),
            TURN,
            false
        ));
    }

    /// The same loss on a local run is the agent dying with it: the run
    /// fails as it always has.
    #[test]
    fn a_local_run_is_never_left_disconnected() {
        assert!(!run_leaves_session_disconnected(
            false,
            false,
            Some(LOST),
            Some(SessionSettleReason::Failed),
            TURN,
            false
        ));
        assert!(!run_leaves_session_disconnected(
            false,
            false,
            Some(LOST),
            Some(SessionSettleReason::Detached),
            TURN,
            false
        ));
    }

    /// A run picks a turn its `sq` proxy dropped back up on the agent — but
    /// not one a quit detached, whose SIGKILL of the proxy reads as the proxy
    /// going when bloxlet is asked about it first; nor after a Stop; nor
    /// for a loss that isn't `sq` alone.
    #[test]
    fn only_a_proxy_that_went_by_itself_is_reattached_to() {
        let detached = "Remote agent detached: the `sq blox acp` proxy exited, but bloxlet \
                        still runs the agent process (command 27294439)";
        assert!(run_picks_up_detached_turn(
            detached,
            false,
            Some(SessionSettleReason::Failed)
        ));
        assert!(run_picks_up_detached_turn(detached, false, None));

        let quit = format!("Detached with the turn still in flight ({detached})");
        assert!(!run_picks_up_detached_turn(
            &quit,
            false,
            Some(SessionSettleReason::Detached)
        ));
        assert!(!run_picks_up_detached_turn(
            detached,
            true,
            Some(SessionSettleReason::Failed)
        ));
        assert!(!run_picks_up_detached_turn(
            LOST,
            false,
            Some(SessionSettleReason::Failed)
        ));
    }

    #[test]
    fn a_stop_wins_over_a_lost_turn() {
        assert!(!run_leaves_session_disconnected(
            true,
            true,
            Some(LOST),
            Some(SessionSettleReason::Failed),
            TURN,
            false
        ));
    }

    /// Nothing runs remotely after a loss before the prompt, and a loss
    /// during the hold with nothing live ends the run as a completed turn.
    #[test]
    fn only_a_turn_in_flight_counts() {
        for phase in [TransportLossPhase::Setup, TransportLossPhase::Hold] {
            assert!(
                !run_leaves_session_disconnected(
                    true,
                    false,
                    Some(LOST),
                    Some(SessionSettleReason::Failed),
                    loss(phase),
                    false
                ),
                "{phase:?}"
            );
        }
        assert!(!run_leaves_session_disconnected(
            true,
            false,
            None,
            Some(SessionSettleReason::HoldStopped),
            loss(TransportLossPhase::Hold),
            false
        ));
    }

    /// A hold lost with background work still live on the workstation: the
    /// completed turn is left for reconcile, as a lost turn is, rather than
    /// settled — and its queue drained — under work no one records.
    #[test]
    fn a_hold_lost_with_work_live_leaves_the_session_disconnected() {
        let hold = loss(TransportLossPhase::Hold);
        assert!(run_leaves_session_disconnected(
            true,
            false,
            None,
            Some(SessionSettleReason::HoldStopped),
            hold,
            true
        ));
        // Not when the user stopped it, and never for a local run.
        assert!(!run_leaves_session_disconnected(
            true,
            true,
            None,
            Some(SessionSettleReason::HoldStopped),
            hold,
            true
        ));
        assert!(!run_leaves_session_disconnected(
            false,
            false,
            None,
            Some(SessionSettleReason::HoldStopped),
            hold,
            true
        ));
        // Work outliving a Stop's cancel, with the transport intact, is the
        // Stop's to finish (see `held_work_needs_fresh_cancel`).
        assert!(!run_leaves_session_disconnected(
            true,
            false,
            None,
            Some(SessionSettleReason::HoldStopped),
            None,
            true
        ));
    }

    /// Auth failures, refused prompts and relayed process exits are answers
    /// from the agent, not a lost transport.
    #[test]
    fn a_failure_the_agent_reported_is_not_a_disconnect() {
        let refused = "Authentication required";
        assert!(!run_leaves_session_disconnected(
            true,
            false,
            Some(refused),
            Some(SessionSettleReason::Failed),
            None,
            false
        ));
        assert!(!run_leaves_session_disconnected(
            true,
            false,
            Some(refused),
            None,
            None,
            false
        ));
    }

    /// The shared proxy's verdicts: an agent lost for good, or a session cut
    /// off (its turn cancelled on the agent), leaves nothing running; a
    /// detach on purpose still leaves the turn there.
    #[test]
    fn a_lost_agent_or_a_cut_off_session_is_not_a_disconnect() {
        for error in [
            "Remote agent lost: bloxlet no longer lists the agent process",
            "Remote session cut off: it fell 64 MiB behind",
        ] {
            assert!(
                !run_leaves_session_disconnected(
                    true,
                    false,
                    Some(error),
                    Some(SessionSettleReason::Failed),
                    TURN,
                    false
                ),
                "{error}"
            );
            assert!(
                run_leaves_session_disconnected(
                    true,
                    false,
                    Some(error),
                    Some(SessionSettleReason::Detached),
                    TURN,
                    false
                ),
                "{error}"
            );
        }
        // Only `sq` went: once reattaching in place gives up, the turn is
        // still taken to be running on the agent — whether `sq` started the
        // process or reused it, and when bloxlet couldn't say.
        for error in [
            "Remote agent detached: the `sq blox acp` proxy exited",
            "Remote agent detached: the `sq blox acp` proxy exited, but bloxlet still runs \
             the agent process (command cmd-1)",
            "Remote agent detached: the `sq blox acp` proxy exited, and whether the agent \
             process (running `goose acp`) still runs is unknown: `sq blox ws commands` \
             timed out",
        ] {
            assert!(
                run_leaves_session_disconnected(
                    true,
                    false,
                    Some(error),
                    Some(SessionSettleReason::Failed),
                    TURN,
                    false
                ),
                "{error}"
            );
        }
        // bloxlet confirmed the process gone, whichever `sq` reached.
        assert!(!run_leaves_session_disconnected(
            true,
            false,
            Some("Remote agent lost: bloxlet lists no running agent process running `goose acp`"),
            Some(SessionSettleReason::Failed),
            TURN,
            false
        ));
        // Lost while setting up: no prompt went out, so nothing runs.
        assert!(!run_leaves_session_disconnected(
            true,
            false,
            Some("Remote agent detached: the `sq blox acp` proxy exited"),
            Some(SessionSettleReason::Failed),
            loss(TransportLossPhase::Setup),
            false
        ));
    }

    #[test]
    fn an_unanswered_stop_cancels_again_over_a_fresh_connection() {
        assert!(stopped_turn_needs_fresh_cancel(true, true, true, None));
    }

    /// The Stop won over a transport that went under mid-turn, so its
    /// `session/cancel` may never have reached the workstation.
    #[test]
    fn a_stop_over_a_lost_turn_cancels_again_over_a_fresh_connection() {
        assert!(stopped_turn_needs_fresh_cancel(true, true, false, TURN));
    }

    #[test]
    fn an_answered_stop_or_a_loss_outside_the_turn_needs_no_fresh_cancel() {
        assert!(!stopped_turn_needs_fresh_cancel(true, true, false, None));
        for phase in [TransportLossPhase::Setup, TransportLossPhase::Hold] {
            assert!(
                !stopped_turn_needs_fresh_cancel(true, true, false, loss(phase)),
                "{phase:?}"
            );
        }
    }

    #[test]
    fn only_a_cancelled_remote_run_cancels_again() {
        // Completed or failed: nothing was stopped.
        assert!(!stopped_turn_needs_fresh_cancel(true, false, true, TURN));
        // Local: the agent went down with the connection.
        assert!(!stopped_turn_needs_fresh_cancel(false, true, true, TURN));
    }

    #[test]
    fn held_work_outliving_a_stop_cancels_again_over_a_fresh_connection() {
        assert!(held_work_needs_fresh_cancel(true, true, true));
        // The cancel showed landing, there was no Stop (or only a quit's), or
        // the agent was local: nothing to do.
        assert!(!held_work_needs_fresh_cancel(true, true, false));
        assert!(!held_work_needs_fresh_cancel(true, false, true));
        assert!(!held_work_needs_fresh_cancel(false, true, true));
    }

    /// A Stop that lands between the run reading its token and deregistering
    /// is reported, so the run can hand it to reconcile; once the entry is
    /// gone a Stop no longer finds it and records itself.
    #[test]
    fn a_stop_landing_as_the_run_disconnects_is_handed_on() {
        let registry = SessionRegistry::new();
        let (_token, epoch) = registry.register("s1");
        registry.mark_finished("s1", epoch);
        assert!(
            registry.cancel("s1"),
            "a finished entry still takes the Stop"
        );

        assert_eq!(
            deregister_disconnected_run(&registry, "s1", epoch),
            Some(CompletionReason::Interrupted)
        );
        assert!(!registry.is_live("s1"));
        assert!(!registry.cancel("s1"));
    }

    #[test]
    fn no_stop_or_a_quit_is_not_handed_on() {
        let registry = SessionRegistry::new();
        let (_token, epoch) = registry.register("s1");
        assert_eq!(deregister_disconnected_run(&registry, "s1", epoch), None);
        assert!(!registry.is_live("s1"));

        let (_token, epoch) = registry.register("s2");
        registry.cancel_with_completion_reason("s2", CompletionReason::AppQuit);
        assert_eq!(deregister_disconnected_run(&registry, "s2", epoch), None);
    }

    /// A successor's entry under the same id is neither removed nor read.
    #[test]
    fn a_successor_entry_is_left_alone() {
        let registry = SessionRegistry::new();
        let (_token, stale) = registry.register("s1");
        registry.mark_finished("s1", stale);
        registry.deregister("s1", stale);
        let (_token, _current) = registry.register("s1");
        registry.cancel("s1");

        assert_eq!(deregister_disconnected_run(&registry, "s1", stale), None);
        assert!(registry.is_live("s1"));
    }
}
