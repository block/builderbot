//! A `disconnected` remote note session whose turn ended with no run
//! watching it — Staged quit mid-turn, or reattaching gave up — stores the
//! note the agent published into the workstation outbox (see
//! `session_runner::taken_over_note`).
//!
//! Reconcile used to run the hooks with no note: the row completed with an
//! empty note while the published one sat in the outbox until a Resume.

use super::*;
use crate::remote_staged_mcp::note_outbox::{clear_outbox_script, collect_script};
use crate::session_runner::AWAITING_REMOTE_NOTE_SUFFIX;

/// A note session on the Blox branch, left `disconnected` mid-turn, and the
/// id of the note it is writing.
fn disconnected_note_session(fx: &Fixture) -> (Session, String) {
    let mut session =
        Session::new_running("write the note", fx.repo.path()).with_branch(&fx.branch.id);
    session.agent_id = Some("acp-1".to_string());
    fx.store.create_session(&session).unwrap();
    let note = store::Note::new(&fx.branch.id, "Draft", "").with_session(&session.id);
    fx.store.create_note(&note).unwrap();
    fx.store
        .add_session_message(&session.id, MessageRole::User, "write the note")
        .unwrap();
    fx.store
        .transition_from_running(&session.id, SessionStatus::Disconnected, None, None)
        .unwrap();
    (fx.row(&session.id), note.id)
}

/// The attach finds the turn over, with `reply` as the agent's last message.
fn turn_ended_with(fx: &Fixture, reply: &'static str) {
    fx.blox.on_attach(move |target, store, _| {
        store
            .add_session_message(&target.session_id, MessageRole::Assistant, reply)
            .unwrap();
        Ok(ended_turn())
    });
}

#[test]
fn a_published_note_is_stored_and_only_then_cleared() {
    let fx = Fixture::new();
    let (session, note_id) = disconnected_note_session(&fx);
    fx.blox
        .publish_note("Offline findings", "Published while Staged was away.");
    turn_ended_with(&fx, "The note is published.");

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Completed,
        None,
        CompletionReason::TurnComplete,
    );
    let note = fx.store.get_note(&note_id).unwrap().unwrap();
    assert_eq!(note.title, "Offline findings");
    assert_eq!(note.content, "Published while Staged was away.");
    assert!(note.completed_at.is_some());
    assert_eq!(
        fx.blox.execs(),
        vec![
            collect_script(&session.id),
            clear_outbox_script(&session.id)
        ],
        "one read, then the clear of a note that is stored"
    );
}

#[test]
fn a_published_note_with_a_broken_diagram_parks_the_row_instead_of_storing_it() {
    let fx = Fixture::new();
    let (session, note_id) = disconnected_note_session(&fx);
    fx.blox
        .publish_note("Offline findings", "Body\n```pikchr\nboxx \"bad\"\n```");
    turn_ended_with(&fx, "The note is published.");

    fx.reconcile(&session);

    let row = fx.row(&session.id);
    assert_eq!(row.status, SessionStatus::Error);
    assert_eq!(row.completion_reason, Some(CompletionReason::Detached));
    let message = row.error_message.expect("a parked row says why");
    assert!(
        message.contains("Pikchr")
            && message.contains("Resume")
            && !message.ends_with(AWAITING_REMOTE_NOTE_SUFFIX),
        "{message}"
    );
    let note = fx.store.get_note(&note_id).unwrap().unwrap();
    assert_eq!(
        note.content, "",
        "a note with a broken diagram is not stored"
    );
    assert!(note.completed_at.is_none());
    assert_eq!(
        fx.blox.execs(),
        vec![
            collect_script(&session.id),
            clear_outbox_script(&session.id)
        ],
        "the rejected note is cleared so a Resume writes it again"
    );
}

#[test]
fn an_outbox_that_cannot_be_read_parks_the_row_for_the_sweep_and_clears_nothing() {
    let fx = Fixture::new();
    let (session, note_id) = disconnected_note_session(&fx);
    fx.blox
        .outbox_unreachable("ssh: connect to host ws-1: Operation timed out");
    turn_ended_with(&fx, "The note is published.");

    fx.reconcile(&session);

    let row = fx.row(&session.id);
    assert_eq!(row.status, SessionStatus::Error);
    assert_eq!(row.completion_reason, Some(CompletionReason::Detached));
    let message = row.error_message.expect("a parked row says why");
    assert!(
        message.ends_with(AWAITING_REMOTE_NOTE_SUFFIX) && message.contains("Operation timed out"),
        "the detached-note sweep keeps reading the outbox: {message}"
    );
    let note = fx.store.get_note(&note_id).unwrap().unwrap();
    assert_eq!(note.content, "");
    assert!(
        note.completed_at.is_none(),
        "an unread outbox is not an empty note"
    );
    assert_eq!(
        fx.blox.execs(),
        vec![collect_script(&session.id)],
        "nothing is cleared unread"
    );
}

#[test]
fn a_turn_that_published_nothing_completes_as_before_and_leaves_no_draft() {
    let fx = Fixture::new();
    let (session, note_id) = disconnected_note_session(&fx);
    turn_ended_with(&fx, "I could not find anything to write.");

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Completed,
        None,
        CompletionReason::TurnComplete,
    );
    let note = fx.store.get_note(&note_id).unwrap().unwrap();
    assert_eq!(note.content, "");
    assert!(note.completed_at.is_some());
    assert_eq!(
        fx.blox.execs(),
        vec![
            collect_script(&session.id),
            clear_outbox_script(&session.id)
        ]
    );
}

#[test]
fn a_session_with_no_note_never_touches_the_outbox() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox
        .set_process(RemoteProcessStatus::Idle, Some("cmd-1"));
    turn_ended_with(&fx, "Committed the change.");

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Completed,
        None,
        CompletionReason::TurnComplete,
    );
    assert_eq!(fx.blox.execs(), Vec::<String>::new());
}
