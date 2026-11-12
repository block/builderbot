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
//! A dead owner's reattached turn that *failed* is read too
//! ([`ReattachedTurnEnding::Failed`]): the agent answered the dead owner's
//! prompt as cancelled, or was lost for good, so the turn is over and
//! nothing further will be published. A published note is the deliverable
//! whatever became of the rest of the turn, as it is for a run that loses
//! its agent after `write_note`: one whose Pikchr blocks parse completes the
//! session and is stored, one with a broken block parks the row
//! `error`/`detached` and is cleared. With nothing published the turn fails
//! as it did before the read existed.
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
        match self {
            TakenOverNote::Published(submission) => {
                let broken = crate::pikchr_validation::validate_pikchr_blocks(&submission.body)
                    .into_iter()
                    .next()
                    .is_some();
                TakenOverRunEnd {
                    note_submission: Some(submission),
                    outbox_use: taken_over_outbox_use(true, true),
                    lost_remote_note_agent: broken.then(|| takeover.to_string()),
                }
            }
            TakenOverNote::Unpublished => TakenOverRunEnd {
                note_submission: None,
                outbox_use: taken_over_outbox_use(false, true),
                lost_remote_note_agent: None,
            },
            TakenOverNote::Unreadable(error) => TakenOverRunEnd {
                note_submission: None,
                outbox_use: taken_over_outbox_use(false, false),
                lost_remote_note_agent: Some(unreadable_outbox_detail(takeover, &error)),
            },
        }
    }

    /// [`Self::run_end`] for a taken-over turn that failed with `error`
    /// instead of completing: the agent answered its prompt as cancelled, or
    /// was lost. The turn is over either way, so what the outbox holds now
    /// is all it will hold.
    ///
    /// - A published note settles the run from the outbox
    ///   (`lost_remote_agent_terminal_state`): with every Pikchr block
    ///   parsing the session completes and the hooks store it; with a broken
    ///   one the row is parked `error`/`detached` and the note cleared.
    /// - Nothing published: the run fails with its own error, as before.
    ///   The outbox was read, so its leftovers (the draft) are cleared.
    /// - An outbox that could not be read: the run fails with its own error
    ///   too. It is not parked for the sweep as a completed turn is: the
    ///   failure names why the agent was lost, which a polling message would
    ///   replace, and the sweep stores what it finds without the Pikchr
    ///   check. The outbox was not read, so the end-of-run clear removes
    ///   only an unpublished one and a published note waits for a Resume.
    pub(super) fn run_end_of_failed_turn(self, takeover: &str, error: &str) -> TakenOverRunEnd {
        match self {
            TakenOverNote::Published(submission) => TakenOverRunEnd {
                note_submission: Some(submission),
                outbox_use: taken_over_outbox_use(true, true),
                lost_remote_note_agent: Some(format!(
                    "{takeover}, and its turn then ended without finishing: {error}"
                )),
            },
            TakenOverNote::Unpublished => TakenOverRunEnd {
                note_submission: None,
                outbox_use: taken_over_outbox_use(false, true),
                lost_remote_note_agent: None,
            },
            TakenOverNote::Unreadable(_) => TakenOverRunEnd {
                note_submission: None,
                outbox_use: taken_over_outbox_use(false, false),
                lost_remote_note_agent: None,
            },
        }
    }
}

/// What a takeover did with the outbox, for the end-of-run clear. The
/// takeover owns the outbox from the moment it takes the turn, and reads it
/// with any turn id: nothing an earlier run left can be sitting there
/// unseen. `collected` says a published note was found, `read` that the
/// outbox was read at all.
fn taken_over_outbox_use(collected: bool, read: bool) -> RemoteOutboxUse {
    RemoteOutboxUse {
        staged_mcp_attached: true,
        recovery_settled: true,
        own_note_collected: collected,
        read_after_last_turn: read,
        // A takeover never asks for a repair, so it never leaves a note it
        // rejected behind an unread ending: a rejected note is cleared with
        // the read that found it.
        rejected_note: None,
    }
}

/// How a dead owner's reattached turn ended, as far as its note goes (see
/// [`reattached_turn_run_end`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReattachedTurnEnding<'a> {
    /// The turn completed and no Stop asked for it to end.
    Completed,
    /// The turn failed with this error and no Stop asked for it to end: the
    /// agent answered the dead owner's prompt as cancelled, or was lost.
    /// (A reattach that merely lost its proxy leaves the row `disconnected`
    /// and never gets here.)
    Failed(&'a str),
    /// A Stop ended it, or the agent cancelled it: the turn's output is
    /// not wanted, or there is none.
    Stopped,
}

impl<'a> ReattachedTurnEnding<'a> {
    /// The ending of a reattached turn with this `result`, given whether a
    /// Stop was requested.
    pub(super) fn of<E: AsRef<str>>(
        result: &'a Result<acp_client::AgentRunOutcome, E>,
        stop_requested: bool,
    ) -> Self {
        match result {
            _ if stop_requested => Self::Stopped,
            Ok(acp_client::AgentRunOutcome::Completed) => Self::Completed,
            Ok(_) => Self::Stopped,
            Err(error) => Self::Failed(error.as_ref()),
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
/// note goes. The outbox is read, through `exec`, when the turn is over and
/// was not stopped: it completed, or it failed
/// ([`TakenOverNote::run_end`], [`TakenOverNote::run_end_of_failed_turn`]).
///
/// A stopped turn of a session with an outbox still owns it, unread: the
/// end-of-run clear then discards the note after a Stop, as for a run of its
/// own, and otherwise removes only an unpublished outbox
/// (`RunEndOutboxClear::ClearUnlessPublished`).
///
/// The failed ending used to be left unread as well. A relaunch while the
/// dead owner's turn is still running has the reattach's `session/load`
/// interrupt it on some bridges, the agent answers the owner's prompt as
/// cancelled, and the turn fails as `remote_agent_lost`: a note the agent
/// had already published stayed in the outbox until the user pressed Resume.
///
/// `ending` was decided before the read, and the read is a round trip to
/// the workstation with the row still `running`. `stopped_since` is asked
/// after it whether the user has pressed Stop meanwhile (not a quit, see
/// `user_stop_requested`). If so the turn ends as a stopped one and what
/// the read returned is dropped: a note or an unreadable outbox would
/// otherwise settle the row ahead of the cancel, storing the note or
/// parking the row for the sweep to store it later.
pub(super) fn reattached_turn_run_end(
    store: &Store,
    session_id: &str,
    workspace_name: Option<&str>,
    ending: ReattachedTurnEnding<'_>,
    exec: remote_staged_mcp::RemoteExec,
    stopped_since: &dyn Fn() -> bool,
) -> TakenOverRunEnd {
    if !delivers_note_through_outbox(store, session_id, workspace_name) {
        return TakenOverRunEnd::default();
    }
    let stopped = || TakenOverRunEnd {
        note_submission: None,
        outbox_use: taken_over_outbox_use(false, false),
        lost_remote_note_agent: None,
    };
    let read = match ending {
        ReattachedTurnEnding::Completed => {
            collect_taken_over_note(exec, session_id).run_end(OWNER_PROCESS_ENDED)
        }
        ReattachedTurnEnding::Failed(error) => collect_taken_over_note(exec, session_id)
            .run_end_of_failed_turn(OWNER_PROCESS_ENDED, error),
        ReattachedTurnEnding::Stopped => return stopped(),
    };
    if stopped_since() {
        log::info!(
            "Session {session_id}: stopped while the workstation note outbox was read after \
             its turn ended on a takeover; ending as stopped"
        );
        return stopped();
    }
    read
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
