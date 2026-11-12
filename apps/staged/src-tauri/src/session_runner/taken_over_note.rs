//! The note of a remote note session's turn that ended away from the run
//! that prompted it.
//!
//! A remote (Blox) note session publishes its note into the workstation
//! outbox (`remote_staged_mcp::note_outbox`), and the run that prompted the
//! turn collects it when the turn ends. A run whose `sq` proxy dies picks
//! its turn back up inside its own turn loop (`pick_up_detached_turn`), so
//! that collection still happens. Two takeovers end a turn whose run is
//! gone, and with it the run's submission slot and turn id:
//!
//! - the owner process died and the dead-owner sweep reattached to the turn
//!   (`reattach_dead_owner_session`);
//! - the row was `disconnected` — a quit detached the run, or reattaching
//!   gave up — and reconcile found the turn over, or followed it to its end
//!   (`reconcile`).
//!
//! Both used to run the post-completion hooks with no note at all, so a
//! note the agent had published was stored empty and stayed in the outbox
//! until a Resume ingested it. They now read the outbox first, through
//! [`collect_taken_over_note`], and settle from what it holds:
//!
//! - a published note whose Pikchr blocks parse goes to the hooks, like the
//!   note of any completed turn, and the outbox is cleared once it is stored;
//! - a published note with a block that does not parse is not stored: the
//!   repair loop that would have fixed it went with the run. The row is
//!   parked `error`/`detached` with the message a lost connection gets in
//!   that state (`lost_remote_agent_terminal_state`), and the outbox is
//!   cleared so a Resume has the agent write the note again;
//! - no published note: the turn completes as before, with the `---`
//!   fallback or an empty note, and the outbox is cleared;
//! - an outbox that could not be read is never judged empty: the row is
//!   parked `error`/`detached` with the sweep's polling marker, so the
//!   detached-note sweep keeps reading it and completes the session from
//!   the note once the workstation answers.
//!
//! The outbox is read with any turn id: the turn id lived in the run that
//! is gone. Whatever `done` marker is there is a note nobody stored — a run
//! clears the outbox once it has dealt with a note.

use super::{
    lost_remote_agent_terminal_state, note_outbox, remote_staged_mcp, session_has_note_artifact,
    RemoteOutboxUse,
};
use crate::note_tool::NoteSubmission;
use crate::store::{CompletionReason, Store};

/// What the workstation outbox held when a taken-over turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TakenOverNote {
    /// A published note, not yet checked for Pikchr errors.
    Published(NoteSubmission),
    /// The outbox was read and holds no usable published note.
    Unpublished,
    /// The workstation could not be reached: nothing is known.
    Unreadable(String),
}

/// Whether `session_id`'s note is delivered through a workstation outbox:
/// a note session, on a Blox workspace.
pub(crate) fn delivers_note_through_outbox(
    store: &Store,
    session_id: &str,
    workspace_name: Option<&str>,
) -> bool {
    workspace_name.is_some() && session_has_note_artifact(store, session_id)
}

/// Read the outbox of a remote note session whose turn ended on a takeover.
///
/// Content the collector cannot turn into a note (a `done` marker beside a
/// missing file, a body `write_note` would have rejected) counts as nothing
/// published, as it does for the start-of-run recovery: reading it again
/// returns the same bytes.
pub(crate) fn collect_taken_over_note(
    exec: remote_staged_mcp::RemoteExec,
    session_id: &str,
) -> TakenOverNote {
    match note_outbox::collect_classified(exec, session_id, None) {
        Ok(note_outbox::OutboxState::Delivered(submission)) => {
            log::info!(
                "Session {session_id}: collected note \"{}\" from the workstation outbox after \
                 its turn ended on a takeover",
                submission.title
            );
            TakenOverNote::Published(submission)
        }
        // `None` accepts any turn, so the stale arm cannot fire.
        Ok(note_outbox::OutboxState::Pending | note_outbox::OutboxState::StaleTurn { .. }) => {
            log::info!(
                "Session {session_id}: workstation outbox has no completed write_note call \
                 after its turn ended on a takeover"
            );
            TakenOverNote::Unpublished
        }
        Err(note_outbox::CollectError::Content(message)) => {
            log::warn!(
                "Session {session_id}: the workstation outbox holds an unusable note \
                 ({message}); treating the turn as having published nothing"
            );
            TakenOverNote::Unpublished
        }
        Err(note_outbox::CollectError::Transport(message)) => {
            log::warn!(
                "Session {session_id}: could not read the workstation note outbox after its \
                 turn ended on a takeover: {message}"
            );
            TakenOverNote::Unreadable(message)
        }
    }
}

/// How a completed taken-over turn settles, given its outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TakenOverTurnEnd {
    /// The turn completes. `note` goes to the post-completion hooks; `None`
    /// leaves them to the `---` fallback.
    Completed { note: Option<NoteSubmission> },
    /// The row is parked `error`/[`CompletionReason::Detached`] with
    /// `message` and the hooks do not run. `clear_outbox` says the note was
    /// read and rejected, so the outbox is emptied; without it the outbox
    /// was not read and the message carries the sweep's polling marker.
    Parked { message: String, clear_outbox: bool },
}

impl TakenOverNote {
    /// How the turn settles on workstation `ws_name`. `takeover` says, in a
    /// few lowercase words, what took the turn from its run; it is the
    /// parenthetical detail of a parked row's message.
    pub(crate) fn turn_end(self, ws_name: &str, takeover: &str) -> TakenOverTurnEnd {
        match self {
            TakenOverNote::Published(submission) => {
                match lost_remote_agent_terminal_state(takeover, ws_name, Some(&submission)) {
                    (_, Some(message), CompletionReason::Detached) => TakenOverTurnEnd::Parked {
                        message,
                        clear_outbox: true,
                    },
                    _ => TakenOverTurnEnd::Completed {
                        note: Some(submission),
                    },
                }
            }
            TakenOverNote::Unpublished => TakenOverTurnEnd::Completed { note: None },
            TakenOverNote::Unreadable(error) => {
                let detail = unreadable_outbox_detail(takeover, &error);
                match lost_remote_agent_terminal_state(&detail, ws_name, None) {
                    (_, Some(message), _) => TakenOverTurnEnd::Parked {
                        message,
                        clear_outbox: false,
                    },
                    // Not reachable: without a note the state always has a
                    // message. Complete with no note rather than panic.
                    _ => TakenOverTurnEnd::Completed { note: None },
                }
            }
        }
    }

    /// The same settlement as the arguments [`super::finish_session_run`]
    /// takes, for a takeover that ends through it: the note for the hooks,
    /// what the takeover did with the outbox, and — for the two parked
    /// endings — the lost-agent detail that makes the run settle from the
    /// outbox instead of completing.
    pub(super) fn run_end(self, takeover: &str) -> TakenOverRunEnd {
        // The takeover owns the outbox from here, and read it with any turn
        // id: nothing an earlier run left can be sitting there unseen.
        let outbox_use = |collected: bool, read: bool| RemoteOutboxUse {
            staged_mcp_attached: true,
            recovery_settled: true,
            own_note_collected: collected,
            read_after_last_turn: read,
        };
        match self {
            TakenOverNote::Published(submission) => {
                let broken = crate::pikchr_validation::validate_pikchr_blocks(&submission.body)
                    .into_iter()
                    .next()
                    .is_some();
                TakenOverRunEnd {
                    note_submission: Some(submission),
                    outbox_use: outbox_use(true, true),
                    lost_remote_note_agent: broken.then(|| takeover.to_string()),
                }
            }
            TakenOverNote::Unpublished => TakenOverRunEnd {
                note_submission: None,
                outbox_use: outbox_use(false, true),
                lost_remote_note_agent: None,
            },
            TakenOverNote::Unreadable(error) => TakenOverRunEnd {
                note_submission: None,
                outbox_use: outbox_use(false, false),
                lost_remote_note_agent: Some(unreadable_outbox_detail(takeover, &error)),
            },
        }
    }
}

/// What a takeover that ends through [`super::finish_session_run`] hands it
/// (see [`TakenOverNote::run_end`]). The default is a session with no
/// outbox: no note, nothing to clear.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct TakenOverRunEnd {
    pub(super) note_submission: Option<NoteSubmission>,
    pub(super) outbox_use: RemoteOutboxUse,
    pub(super) lost_remote_note_agent: Option<String>,
}

/// How a dead owner's turn, reattached to by the sweep, ends as far as its
/// note goes. `completed` says the reattached turn completed and no Stop
/// asked for it to end; only then is the outbox read, through `exec`.
///
/// Any other ending of a session with an outbox still owns it, unread: the
/// end-of-run clear then discards the note after a Stop, as for a run of its
/// own, and otherwise removes only an unpublished outbox
/// (`RunEndOutboxClear::ClearUnlessPublished`).
pub(super) fn reattached_turn_run_end(
    store: &Store,
    session_id: &str,
    workspace_name: Option<&str>,
    completed: bool,
    exec: remote_staged_mcp::RemoteExec,
) -> TakenOverRunEnd {
    if !delivers_note_through_outbox(store, session_id, workspace_name) {
        return TakenOverRunEnd::default();
    }
    if !completed {
        return TakenOverRunEnd {
            note_submission: None,
            outbox_use: RemoteOutboxUse {
                staged_mcp_attached: true,
                recovery_settled: true,
                own_note_collected: false,
                read_after_last_turn: false,
            },
            lost_remote_note_agent: None,
        };
    }
    collect_taken_over_note(exec, session_id).run_end(OWNER_PROCESS_ENDED)
}

fn unreadable_outbox_detail(takeover: &str, error: &str) -> String {
    format!("{takeover}, and the workstation note outbox could not be read when it ended: {error}")
}

/// What took the turn from its run, for a dead owner's session the sweep
/// reattached to.
pub(super) const OWNER_PROCESS_ENDED: &str = "the Staged process that started the turn ended";

/// What took the turn from its run, for a `disconnected` row reconcile
/// settled.
pub(crate) const SESSION_WAS_DISCONNECTED: &str =
    "the session was disconnected while the turn was running";

/// Empty the outbox of a taken-over turn that settled, when the takeover has
/// dealt with what was there: `note_handed_to_hooks` says a published note
/// went to the hooks, and `note_stored` what they reported. A note the hooks
/// could not store stays, with its draft, for the next run's start-of-run
/// recovery.
pub(crate) fn clear_outbox_after_takeover(
    exec: remote_staged_mcp::RemoteExec,
    session_id: &str,
    note_handed_to_hooks: bool,
    note_stored: bool,
) {
    if note_handed_to_hooks && !note_stored {
        log::warn!(
            "Session {session_id}: the note collected from the workstation outbox could not be \
             stored; leaving it there for a resume"
        );
        return;
    }
    if let Err(e) = note_outbox::clear_outbox(exec, session_id) {
        log::warn!(
            "Session {session_id}: failed to clear the workstation note outbox after its turn \
             ended on a takeover: {e}"
        );
    }
}
