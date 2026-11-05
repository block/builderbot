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

use crate::store::{Session, Store};

/// Whether a finished run leaves its session `disconnected` rather than
/// terminal: a remote run that failed (`error`) because its connection was
/// lost — or detached — with the turn in flight, so the turn may still be
/// running on the workstation.
///
/// Only a turn in flight counts. A loss during setup sent no prompt, so nothing
/// is running remotely; a loss during a post-turn hold settles as a completed
/// turn, never as a failure. Errors the agent answered itself (auth, a refused
/// prompt, a bridge-relayed "process exited") carry no transport loss at all.
/// A Stop always wins, because the user asked for the turn to end; and a local
/// run never qualifies, because its agent went down with the connection.
///
/// The shared proxy's own verdicts count too. Its agent lost for good (bloxlet
/// reports the process gone, or `sq` exited having started it) leaves nothing
/// to come back for, and neither does a session cut off for falling behind,
/// whose turn was cancelled on the agent: both stay failures. Its agent
/// detached (only `sq` went, and the agent may still run) is a lost transport
/// like any other — reached here once reattaching in place has given up. A
/// detach on purpose wins over all of them.
pub(crate) fn run_leaves_session_disconnected(
    is_remote: bool,
    cancel_requested: bool,
    error: Option<&str>,
    settle_reason: Option<SessionSettleReason>,
    transport_loss: Option<TransportLoss>,
) -> bool {
    let Some(error) = error else {
        return false;
    };
    if !is_remote || cancel_requested {
        return false;
    }
    if settle_reason == Some(SessionSettleReason::Detached) {
        return true;
    }
    if acp_client::is_remote_agent_lost_error(error)
        || acp_client::is_remote_session_cut_off_error(error)
    {
        return false;
    }
    transport_loss.is_some_and(|loss| loss.phase == TransportLossPhase::Turn)
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
            TURN
        ));
        // A quit's detach with the turn in flight.
        assert!(run_leaves_session_disconnected(
            true,
            false,
            Some(LOST),
            Some(SessionSettleReason::Detached),
            TURN
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
            TURN
        ));
        assert!(!run_leaves_session_disconnected(
            false,
            false,
            Some(LOST),
            Some(SessionSettleReason::Detached),
            TURN
        ));
    }

    #[test]
    fn a_stop_wins_over_a_lost_turn() {
        assert!(!run_leaves_session_disconnected(
            true,
            true,
            Some(LOST),
            Some(SessionSettleReason::Failed),
            TURN
        ));
    }

    /// Nothing runs remotely after a loss before the prompt, and a loss
    /// during the hold ends the run as a completed turn.
    #[test]
    fn only_a_turn_in_flight_counts() {
        for phase in [TransportLossPhase::Setup, TransportLossPhase::Hold] {
            assert!(
                !run_leaves_session_disconnected(
                    true,
                    false,
                    Some(LOST),
                    Some(SessionSettleReason::Failed),
                    loss(phase)
                ),
                "{phase:?}"
            );
        }
        assert!(!run_leaves_session_disconnected(
            true,
            false,
            None,
            Some(SessionSettleReason::HoldStopped),
            loss(TransportLossPhase::Hold)
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
            None
        ));
        assert!(!run_leaves_session_disconnected(
            true,
            false,
            Some(refused),
            None,
            None
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
                    TURN
                ),
                "{error}"
            );
            assert!(
                run_leaves_session_disconnected(
                    true,
                    false,
                    Some(error),
                    Some(SessionSettleReason::Detached),
                    TURN
                ),
                "{error}"
            );
        }
        // Only `sq` went: once reattaching in place gives up, the turn is
        // still taken to be running on the agent.
        assert!(run_leaves_session_disconnected(
            true,
            false,
            Some("Remote agent detached: the `sq blox acp` proxy exited"),
            None,
            TURN
        ));
    }
}
