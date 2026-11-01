//! The dead-owner sweep for remote note sessions claims the row before it
//! talks to the workstation, so two Staged instances sharing a store settle
//! each detached session exactly once.

use super::*;
use crate::note_tool::NoteSubmission;
use crate::remote_note_outbox::clear_outbox_script;
use crate::store::{Branch, Note, Project, Session};
use std::cell::RefCell;

/// A running note session on a remote branch; returns (session id, note id).
fn remote_note_session(store: &Arc<Store>) -> (String, String) {
    let project = Project::new("test/remote-notes");
    store.create_project(&project).unwrap();
    let branch = Branch::new_remote(&project.id, "notes", "main", "ws-1");
    store.create_branch(&branch).unwrap();
    let session = Session::new_running("write a note", std::path::Path::new("."));
    store.create_session(&session).unwrap();
    let note = Note::new(&branch.id, "", "").with_session(&session.id);
    store.create_note(&note).unwrap();
    (session.id, note.id)
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// What the collect script prints for an outbox holding a finished note.
fn done_output() -> String {
    let meta = r#"{"title":"Plan","suggestedNextSteps":{"suggestedNextCommitStep":"Ship","suggestedNextNoteStep":null},"turn":"old-run","writtenAt":1,"helperVersion":2}"#;
    format!("done\n{}\n{}\n", b64(meta.as_bytes()), b64(b"Body"))
}

fn is_collect(args: &[&str]) -> bool {
    args.len() == 3 && args[2].contains("echo done; else echo pending")
}

/// Records every workstation command and answers the collect script with
/// `collect_output`.
fn recording_exec<'a>(
    collect_output: &'a str,
    log: &'a RefCell<Vec<Vec<String>>>,
) -> impl Fn(&[&str]) -> Result<String, String> + 'a {
    move |args: &[&str]| {
        log.borrow_mut()
            .push(args.iter().map(|a| a.to_string()).collect());
        if is_collect(args) {
            Ok(collect_output.to_string())
        } else {
            Ok(String::new())
        }
    }
}

/// The production ingest, pointed at the local checkout so no workstation is
/// consulted for media.
fn ingest_locally<'a>(
    store: &'a Arc<Store>,
    session_id: &str,
) -> impl FnMut(NoteSubmission) -> bool + 'a {
    let session_id = session_id.to_string();
    move |submission| {
        run_post_completion_hooks(
            &session_id,
            std::path::Path::new("."),
            None,
            None,
            None,
            store,
            0,
            Some(submission),
        )
        .note_stored
    }
}

/// An ingest whose store write fails: counts the calls, stores nothing.
fn counting_unstored(ingested: &mut usize) -> impl FnMut(NoteSubmission) -> bool + '_ {
    move |_| {
        *ingested += 1;
        false
    }
}

/// The lost-connection error as the Blox proxy reports it: the SDK's
/// transport message nested in two internal errors.
const PROXY_TRANSPORT_CLOSED: &str = r#"ACP protocol failed: Error { code: -32603: Internal error, message: "Internal error", data: Some(String("Prompt failed: Error { code: -32603: Internal error, message: \"Incoming transport closed\", data: None }")) }"#;

#[test]
fn the_sweep_winner_completes_the_session_from_the_outbox_once() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    let output = done_output();
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);

    let outcome = finalize_detached_remote_note_session(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut ingest_locally(&store, &session_id),
    )
    .expect("the first sweep claims the row");
    assert_eq!(
        outcome,
        DetachedRecoveryOutcome {
            status: SessionStatus::Completed,
            reason: CompletionReason::TurnComplete,
            error: None,
        }
    );
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, SessionStatus::Completed);
    assert_eq!(
        session.completion_reason,
        Some(CompletionReason::TurnComplete)
    );
    assert_eq!(session.error_message, None);
    let note = store.get_note(&note_id).unwrap().unwrap();
    assert_eq!(note.title, "Plan");
    assert_eq!(note.content, "Body");
    assert_eq!(note.suggested_next_commit_step.as_deref(), Some("Ship"));
    assert!(note.completed_at.is_some());
    {
        let commands = log.borrow();
        assert_eq!(commands.len(), 2, "collect, then clear: {commands:?}");
        assert!(is_collect(
            &commands[0].iter().map(String::as_str).collect::<Vec<_>>()
        ));
        assert_eq!(commands[1][2], clear_outbox_script(&session_id));
    }

    // A second instance sweeping the same (now settled) row: no claim, no
    // workstation traffic, no second ingest.
    log.borrow_mut().clear();
    let mut ingested = 0;
    let second =
        finalize_detached_remote_note_session(&store, &session_id, "ws-1", &exec, &mut |_| {
            ingested += 1;
            true
        });
    assert_eq!(second, None);
    assert_eq!(ingested, 0);
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
    assert_eq!(
        store.get_session(&session_id).unwrap().unwrap().status,
        SessionStatus::Completed
    );
}

#[test]
fn a_sweep_without_a_finished_note_leaves_the_row_detached() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    let log = RefCell::new(Vec::new());
    let exec = recording_exec("pending\n\n\n", &log);
    let mut ingested = 0;

    let outcome =
        finalize_detached_remote_note_session(&store, &session_id, "ws-1", &exec, &mut |_| {
            ingested += 1;
            true
        })
        .expect("the sweep claims the row");
    assert_eq!(outcome.status, SessionStatus::Error);
    assert_eq!(outcome.reason, CompletionReason::Detached);
    assert!(
        outcome
            .error
            .as_deref()
            .is_some_and(|e| e.contains("ws-1") && e.ends_with(AWAITING_REMOTE_NOTE_SUFFIX)),
        "the row carries the marker the sweep polls for: {outcome:?}"
    );
    assert_eq!(ingested, 0);
    assert_eq!(log.borrow().len(), 1, "collect only, nothing to clear");
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, SessionStatus::Error);
    assert_eq!(session.completion_reason, Some(CompletionReason::Detached));
    assert_eq!(session.error_message, outcome.error);
    let note = store.get_note(&note_id).unwrap().unwrap();
    assert_eq!(note.content, "");
    assert!(note.completed_at.is_none(), "an empty note is not stored");

    // The row is already claimed: a concurrent sweep does nothing.
    log.borrow_mut().clear();
    assert_eq!(
        finalize_detached_remote_note_session(&store, &session_id, "ws-1", &exec, &mut |_| {
            ingested += 1;
            true
        }),
        None
    );
    assert!(log.borrow().is_empty());
}

#[test]
fn a_sweep_that_cannot_reach_the_workstation_records_why() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, _) = remote_note_session(&store);
    let exec = |_: &[&str]| -> Result<String, String> { Err("workstation suspended".into()) };

    let outcome =
        finalize_detached_remote_note_session(&store, &session_id, "ws-1", &exec, &mut |_| {
            panic!("nothing to ingest")
        })
        .unwrap();
    assert_eq!(outcome.status, SessionStatus::Error);
    assert_eq!(outcome.reason, CompletionReason::Detached);
    assert!(
        outcome.error.as_deref().is_some_and(
            |e| e.contains("workstation suspended") && e.ends_with(AWAITING_REMOTE_NOTE_SUFFIX)
        ),
        "an unreachable workstation is polled again too: {outcome:?}"
    );
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.error_message, outcome.error);
}

/// A row the sweep (or a lost connection) parked as `detached` while the
/// agent may still be writing: the state the re-poll acts on.
fn detach_awaiting_note(store: &Arc<Store>, session_id: &str) {
    assert!(store
        .transition_from_running(
            session_id,
            SessionStatus::Error,
            Some(&awaiting_remote_note_message("ws-1", None)),
            Some(&CompletionReason::Detached),
        )
        .unwrap());
}

#[test]
fn the_poll_window_is_bounded_and_keyed_on_the_marker() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, _) = remote_note_session(&store);
    let running = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(
        detached_note_poll_action(&running, crate::store::now_timestamp()),
        DetachedNotePollAction::Skip
    );

    detach_awaiting_note(&store, &session_id);
    let detached = store.get_session(&session_id).unwrap().unwrap();
    let window_ms = DETACHED_NOTE_POLL_WINDOW.as_millis() as i64;
    assert_eq!(
        detached_note_poll_action(&detached, detached.updated_at),
        DetachedNotePollAction::Poll
    );
    assert_eq!(
        detached_note_poll_action(&detached, detached.updated_at + window_ms),
        DetachedNotePollAction::Poll,
        "the window is inclusive"
    );
    assert_eq!(
        detached_note_poll_action(&detached, detached.updated_at + window_ms + 1),
        DetachedNotePollAction::PollThenExpire
    );

    // Without the marker a detached row is never polled: an expired window,
    // a known-bad note, or a row written before polling existed.
    let mut legacy = detached.clone();
    legacy.error_message = Some("Staged lost this session; no completed note was found.".into());
    assert_eq!(
        detached_note_poll_action(&legacy, legacy.updated_at),
        DetachedNotePollAction::Skip
    );
    let mut expired = detached.clone();
    expired.error_message = Some(detached_note_expired_message("ws-1"));
    assert_eq!(
        detached_note_poll_action(&expired, expired.updated_at),
        DetachedNotePollAction::Skip
    );
    let mut crashed = detached.clone();
    crashed.completion_reason = Some(CompletionReason::Crashed);
    assert_eq!(
        detached_note_poll_action(&crashed, crashed.updated_at),
        DetachedNotePollAction::Skip
    );
}

#[test]
fn a_poll_that_finds_the_note_claims_the_row_then_ingests_once() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);
    let output = done_output();
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);

    let outcome = poll_detached_remote_note_session_once(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut ingest_locally(&store, &session_id),
    )
    .expect("the poll that sees `done` completes the row");
    assert_eq!(
        outcome,
        DetachedRecoveryOutcome {
            status: SessionStatus::Completed,
            reason: CompletionReason::TurnComplete,
            error: None,
        }
    );
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, SessionStatus::Completed);
    assert_eq!(session.error_message, None);
    let note = store.get_note(&note_id).unwrap().unwrap();
    assert_eq!(
        (note.title.as_str(), note.content.as_str()),
        ("Plan", "Body")
    );
    assert!(note.completed_at.is_some());
    {
        let commands = log.borrow();
        assert_eq!(commands.len(), 2, "collect, then clear: {commands:?}");
        assert_eq!(commands[1][2], clear_outbox_script(&session_id));
    }

    // A second instance whose poll read the same marker: its claim fails on
    // the already-completed row, so it neither ingests nor clears.
    log.borrow_mut().clear();
    let mut ingested = 0;
    assert_eq!(
        poll_detached_remote_note_session_once(&store, &session_id, "ws-1", &exec, &mut |_| {
            ingested += 1;
            true
        }),
        None
    );
    assert_eq!(ingested, 0);
    assert_eq!(log.borrow().len(), 1, "the read happened, nothing after it");
}

#[test]
fn a_poll_without_a_note_writes_nothing_and_a_resumed_row_is_left_alone() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);
    let before = store.get_session(&session_id).unwrap().unwrap();
    let log = RefCell::new(Vec::new());

    for output in ["pending\n\n\n", "done\n\n\n"] {
        let exec = recording_exec(output, &log);
        assert_eq!(
            poll_detached_remote_note_session_once(&store, &session_id, "ws-1", &exec, &mut |_| {
                panic!("nothing to ingest")
            }),
            None
        );
    }
    let unreachable = |_: &[&str]| -> Result<String, String> { Err("suspended".into()) };
    assert_eq!(
        poll_detached_remote_note_session_once(
            &store,
            &session_id,
            "ws-1",
            &unreachable,
            &mut |_| { panic!("nothing to ingest") }
        ),
        None
    );
    let after = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(
        (after.status, after.error_message, after.updated_at),
        (before.status, before.error_message, before.updated_at),
        "a poll that finds nothing does not restamp the row, so the window keeps counting"
    );
    assert!(log
        .borrow()
        .iter()
        .all(|cmd| is_collect(&cmd.iter().map(String::as_str).collect::<Vec<_>>())));

    // The user resumes while the note is on its way: the new run owns the
    // row, and the poll that then reads `done` must not complete it.
    store
        .update_session_status(&session_id, SessionStatus::Running, None, None)
        .unwrap();
    let output = done_output();
    let exec = recording_exec(&output, &log);
    let mut ingested = 0;
    assert_eq!(
        poll_detached_remote_note_session_once(&store, &session_id, "ws-1", &exec, &mut |_| {
            ingested += 1;
            true
        }),
        None
    );
    assert_eq!(ingested, 0);
    assert_eq!(
        store.get_session(&session_id).unwrap().unwrap().status,
        SessionStatus::Running
    );
    assert_eq!(store.get_note(&note_id).unwrap().unwrap().content, "");
}

#[test]
fn an_expired_window_settles_the_row_once_and_stops_the_polling() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, _) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);

    let outcome = expire_detached_remote_note_session(&store, &session_id, "ws-1")
        .expect("the first expiry settles the row");
    assert_eq!(outcome.status, SessionStatus::Error);
    assert_eq!(outcome.reason, CompletionReason::Detached);
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.error_message, outcome.error);
    assert!(
        session
            .error_message
            .as_deref()
            .is_some_and(|m| m.contains("30 minutes")
                && m.contains("Resume")
                && !m.ends_with(AWAITING_REMOTE_NOTE_SUFFIX)),
        "{session:?}"
    );
    assert_eq!(
        detached_note_poll_action(&session, session.updated_at),
        DetachedNotePollAction::Skip,
        "the fresh updated_at does not reopen the window"
    );
    assert!(
        session.completion_reason.unwrap().is_resumable(),
        "the Resume affordance stays"
    );

    // A row that completed (another instance's poll) or was resumed first is
    // not touched by a late expiry.
    store
        .update_session_status(
            &session_id,
            SessionStatus::Completed,
            None,
            Some(&CompletionReason::TurnComplete),
        )
        .unwrap();
    assert_eq!(
        expire_detached_remote_note_session(&store, &session_id, "ws-1"),
        None
    );
    assert_eq!(
        store.get_session(&session_id).unwrap().unwrap().status,
        SessionStatus::Completed
    );
}

#[test]
fn a_lost_connection_completes_only_from_a_valid_outbox_note() {
    let valid = NoteSubmission::from_params(
        crate::note_tool::WriteNoteParams {
            title: "Plan".into(),
            body: "Body\n```pikchr\nbox \"ok\"\n```".into(),
            suggested_next_steps: None,
        },
        NoteSubmissionSource::Outbox,
    )
    .unwrap();
    assert_eq!(
        lost_remote_agent_terminal_state("Incoming transport closed", "ws-1", Some(&valid)),
        ("completed", None, CompletionReason::TurnComplete)
    );

    let broken = NoteSubmission::from_params(
        crate::note_tool::WriteNoteParams {
            title: "Plan".into(),
            body: "Body\n```pikchr\nboxx \"bad\"\n```".into(),
            suggested_next_steps: None,
        },
        NoteSubmissionSource::Outbox,
    )
    .unwrap();
    let (status, error, reason) =
        lost_remote_agent_terminal_state("Incoming transport closed", "ws-1", Some(&broken));
    assert_eq!(
        (status, reason.clone()),
        ("error", CompletionReason::Detached)
    );
    assert!(
        error.as_deref().is_some_and(|e| e.contains("Pikchr")
            && e.contains("Resume")
            && !e.ends_with(AWAITING_REMOTE_NOTE_SUFFIX)),
        "a known-bad note is not stored and not polled for: {error:?}"
    );
    assert!(
        remote_outbox_is_cleared_at_run_end(SETTLED_RUN, &reason, error.as_deref()),
        "...and the outbox holding it is emptied"
    );

    let (status, error, reason) =
        lost_remote_agent_terminal_state("Incoming transport closed", "ws-1", None);
    assert_eq!(
        (status, reason.clone()),
        ("error", CompletionReason::Detached)
    );
    assert!(
        error
            .as_deref()
            .is_some_and(|e| e.contains("incoming transport closed")
                && e.contains("ws-1")
                && e.ends_with(AWAITING_REMOTE_NOTE_SUFFIX)),
        "{error:?}"
    );
    assert!(
        !remote_outbox_is_cleared_at_run_end(SETTLED_RUN, &reason, error.as_deref()),
        "the sweep still needs the outbox"
    );
}

/// A run that attached the helper, whose start-of-run recovery went through.
const SETTLED_RUN: RemoteOutboxUse = RemoteOutboxUse {
    helper_attached: true,
    recovery_settled: true,
    own_note_collected: false,
};

#[test]
fn the_detached_message_carries_the_innermost_error_not_the_debug_dump() {
    assert_eq!(
        innermost_error_detail(PROXY_TRANSPORT_CLOSED),
        "incoming transport closed"
    );
    assert_eq!(
        innermost_error_detail(
            r#"Prompt failed: Error { code: -32603: Internal error, message: "Internal error", data: Some(String("agent transport closed")) }"#
        ),
        "agent transport closed",
        "a generic message falls back to the data string"
    );
    assert_eq!(
        innermost_error_detail(
            r#"Prompt failed: Error { code: -32603: Internal error, message: "Internal error", data: None }"#
        ),
        r#"prompt failed: Error { code: -32603: Internal error, message: "Internal error", data: None }"#,
        "nothing better than the whole error: keep it"
    );
    assert_eq!(innermost_error_detail("child exited."), "child exited");
    assert_eq!(
        innermost_error_detail("ACP connection closed before the interrupt resolved"),
        "ACP connection closed before the interrupt resolved",
        "a leading acronym keeps its case"
    );

    let (_, error, _) = lost_remote_agent_terminal_state(PROXY_TRANSPORT_CLOSED, "ws-1", None);
    let error = error.unwrap();
    assert!(
        error.contains("(incoming transport closed)") && !error.contains("Error {"),
        "{error}"
    );
}

#[test]
fn only_a_transport_loss_detaches_a_remote_note_session() {
    use acp_client::SessionSettleReason;
    // The connection failed mid-turn and the error is the pipe going away.
    for message in [
        PROXY_TRANSPORT_CLOSED,
        "Failed to write prompt: Broken pipe (os error 32)",
    ] {
        assert!(
            remote_agent_connection_was_lost(Some(SessionSettleReason::Failed), message),
            "{message}"
        );
    }
    // The agent answered, or never started, or the run failed locally.
    for (settle, message) in [
        (
            Some(SessionSettleReason::Failed),
            "ACP authentication is required to run the prompt. Sign this agent in, then retry.",
        ),
        (
            Some(SessionSettleReason::Failed),
            r#"Prompt failed: Error { code: -32603: Internal error, message: "Internal error", data: Some(String("model refused the request")) }"#,
        ),
        // A bridge reporting its subprocess dying is an answer over a live
        // transport, with nothing left running behind it (see
        // `acp_client::is_transport_loss_error`).
        (Some(SessionSettleReason::Failed), "child exited"),
        (None, "Timed out waiting for ACP protocol startup after 90s"),
        (None, "Failed to create ACP session: transport closed"),
        (
            Some(SessionSettleReason::Quiescent),
            "Failed to persist write_note follow-up prompt: database is locked",
        ),
        (
            Some(SessionSettleReason::ResponseComplete),
            "Pikchr validation failed but no agent session id is available for correction",
        ),
    ] {
        assert!(
            !remote_agent_connection_was_lost(settle, message),
            "{settle:?} / {message}"
        );
    }
}

#[test]
fn an_unsettled_start_of_run_recovery_keeps_the_outbox_unless_the_run_saw_its_own_note() {
    let unsettled = RemoteOutboxUse {
        recovery_settled: false,
        ..SETTLED_RUN
    };
    for (reason, message) in [
        (CompletionReason::TurnComplete, None),
        (CompletionReason::Crashed, Some("auth required")),
        (CompletionReason::Interrupted, None),
    ] {
        assert!(
            !remote_outbox_is_cleared_at_run_end(unsettled, &reason, message),
            "an earlier run's note may still be waiting unseen: {reason:?}"
        );
        assert!(
            remote_outbox_is_cleared_at_run_end(
                RemoteOutboxUse {
                    own_note_collected: true,
                    ..unsettled
                },
                &reason,
                message
            ),
            "this run's note overwrote whatever was there: {reason:?}"
        );
    }
}

#[test]
fn a_note_that_cannot_be_stored_stays_on_the_workstation_in_both_sweep_paths() {
    // The startup sweep's path: claim, collect, ingest fails.
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    let output = done_output();
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);
    let mut ingested = 0;
    let outcome = finalize_detached_remote_note_session(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut counting_unstored(&mut ingested),
    )
    .expect("the sweep claims the row");
    assert_eq!(ingested, 1);
    assert_eq!(outcome.status, SessionStatus::Error);
    assert_eq!(outcome.reason, CompletionReason::Detached);
    assert!(
        outcome
            .error
            .as_deref()
            .is_some_and(|e| e.contains("\"Plan\"")
                && e.contains("could not be stored")
                && e.contains("Resume")
                && !e.ends_with(AWAITING_REMOTE_NOTE_SUFFIX)),
        "{outcome:?}"
    );
    assert_eq!(
        log.borrow().len(),
        1,
        "collect only, no clear: {:?}",
        log.borrow()
    );
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, SessionStatus::Error);
    assert_eq!(session.error_message, outcome.error);
    assert_eq!(store.get_note(&note_id).unwrap().unwrap().content, "");

    // The polling path: claim with the storing marker, ingest fails.
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);
    let mut ingested = 0;
    let outcome = poll_detached_remote_note_session_once(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut counting_unstored(&mut ingested),
    )
    .expect("the poll that sees `done` settles the row");
    assert_eq!(ingested, 1);
    assert_eq!(outcome.status, SessionStatus::Error);
    assert_eq!(outcome.reason, CompletionReason::Detached);
    assert!(
        outcome
            .error
            .as_deref()
            .is_some_and(|e| e.contains("could not be stored") && e.contains("Resume")),
        "{outcome:?}"
    );
    assert_eq!(
        log.borrow().len(),
        1,
        "collect only, no clear: {:?}",
        log.borrow()
    );
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, SessionStatus::Error);
    assert_eq!(session.error_message, outcome.error);
    assert_eq!(
        detached_note_poll_action(&session, session.updated_at),
        DetachedNotePollAction::Skip,
        "an unstorable note is not polled for again; Resume collects it"
    );
    assert_eq!(store.get_note(&note_id).unwrap().unwrap().content, "");
}

#[test]
fn a_poll_that_finds_the_note_claims_with_the_storing_marker_before_completing() {
    // Mid-ingest another instance's poll reads the same `done`: the storing
    // marker has no polling suffix, so it neither polls nor claims the row.
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, _) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);
    let output = done_output();
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);
    let store_for_ingest = Arc::clone(&store);
    let session_for_ingest = session_id.clone();
    let mut ingest = |_: NoteSubmission| {
        let row = store_for_ingest
            .get_session(&session_for_ingest)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, SessionStatus::Error);
        assert_eq!(row.completion_reason, Some(CompletionReason::Detached));
        assert_eq!(
            row.error_message.as_deref(),
            Some(detached_note_storing_message("ws-1").as_str())
        );
        assert_eq!(
            detached_note_poll_action(&row, row.updated_at),
            DetachedNotePollAction::Skip
        );
        true
    };
    let outcome =
        poll_detached_remote_note_session_once(&store, &session_id, "ws-1", &exec, &mut ingest)
            .unwrap();
    assert_eq!(outcome.status, SessionStatus::Completed);
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, SessionStatus::Completed);
    assert_eq!(session.error_message, None);
    assert_eq!(log.borrow().len(), 2, "collect, then clear");
}

#[test]
fn a_row_past_the_window_gets_one_last_poll_before_it_expires() {
    // The laptop was closed overnight: the note is waiting, the window is
    // long gone. The first tick completes the row instead of expiring it.
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);
    let output = done_output();
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);
    let outcome = poll_detached_remote_note_session(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut ingest_locally(&store, &session_id),
        true,
    )
    .expect("the last poll completes the row");
    assert_eq!(outcome.status, SessionStatus::Completed);
    assert_eq!(
        store.get_session(&session_id).unwrap().unwrap().status,
        SessionStatus::Completed
    );
    assert_eq!(store.get_note(&note_id).unwrap().unwrap().content, "Body");

    // Nothing there, or no answer: that last poll is what expires the row.
    for exec in [
        &(|_: &[&str]| -> Result<String, String> { Ok("pending\n\n\n".into()) })
            as remote_note_outbox::RemoteExec,
        &(|_: &[&str]| -> Result<String, String> { Err("suspended".into()) }),
    ] {
        let store = Arc::new(Store::in_memory().unwrap());
        let (session_id, _) = remote_note_session(&store);
        detach_awaiting_note(&store, &session_id);
        let outcome = poll_detached_remote_note_session(
            &store,
            &session_id,
            "ws-1",
            exec,
            &mut |_| panic!("nothing to ingest"),
            true,
        )
        .expect("the expiry is recorded");
        assert_eq!(outcome.reason, CompletionReason::Detached);
        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|m| m.contains("30 minutes")));
        // Inside the window the same empty poll writes nothing.
        let store = Arc::new(Store::in_memory().unwrap());
        let (session_id, _) = remote_note_session(&store);
        detach_awaiting_note(&store, &session_id);
        assert_eq!(
            poll_detached_remote_note_session(
                &store,
                &session_id,
                "ws-1",
                exec,
                &mut |_| panic!("nothing to ingest"),
                false,
            ),
            None
        );
    }
}

#[test]
fn the_outbox_is_cleared_whenever_the_run_is_done_with_it() {
    for (reason, message) in [
        (CompletionReason::TurnComplete, None),
        (CompletionReason::HeldUntilCap, None),
        (CompletionReason::Interrupted, None),
        (
            CompletionReason::Crashed,
            Some("Pikchr validation failed after 2 attempts"),
        ),
        (
            CompletionReason::Detached,
            Some("a note whose Pikchr block does not parse"),
        ),
    ] {
        assert!(
            remote_outbox_is_cleared_at_run_end(SETTLED_RUN, &reason, message),
            "{reason:?} / {message:?}"
        );
    }
    let awaiting = awaiting_remote_note_message("ws-1", Some("incoming transport closed"));
    assert!(!remote_outbox_is_cleared_at_run_end(
        SETTLED_RUN,
        &CompletionReason::Detached,
        Some(&awaiting)
    ));
    assert!(
        !remote_outbox_is_cleared_at_run_end(
            RemoteOutboxUse {
                helper_attached: false,
                ..SETTLED_RUN
            },
            &CompletionReason::TurnComplete,
            None
        ),
        "a run without the helper never touched the outbox"
    );
}

#[test]
fn the_frontend_mirrors_the_polling_marker() {
    let helpers = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../src/lib/features/sessions/sessionModalHelpers.ts"
    ))
    .expect("sessionModalHelpers.ts is part of the app");
    assert!(
        helpers.contains(&format!("'{AWAITING_REMOTE_NOTE_SUFFIX}'")),
        "REMOTE_NOTE_POLL_SUFFIX in sessionModalHelpers.ts must equal AWAITING_REMOTE_NOTE_SUFFIX"
    );
}

#[test]
fn a_resume_during_collection_keeps_the_row() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, _) = remote_note_session(&store);
    let output = done_output();
    // The user presses Resume while the claimant is reading the outbox: the
    // row is `running` again by the time the result comes back.
    let store_for_exec = Arc::clone(&store);
    let session_for_exec = session_id.clone();
    let exec = move |args: &[&str]| -> Result<String, String> {
        if is_collect(args) {
            store_for_exec
                .update_session_status(&session_for_exec, SessionStatus::Running, None, None)
                .unwrap();
            Ok(output.clone())
        } else {
            Ok(String::new())
        }
    };

    let outcome = finalize_detached_remote_note_session(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut ingest_locally(&store, &session_id),
    );
    assert_eq!(outcome, None, "the new run owns the row");
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, SessionStatus::Running);
    assert_eq!(session.completion_reason, None);
}

#[test]
fn only_a_detached_row_can_be_finished_by_the_recovery_write() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, _) = remote_note_session(&store);
    assert!(
        !store
            .finish_detached_recovery(
                &session_id,
                SessionStatus::Completed,
                None,
                Some(&CompletionReason::TurnComplete)
            )
            .unwrap(),
        "a running row is not a claim"
    );
    store
        .transition_from_running(
            &session_id,
            SessionStatus::Error,
            Some("lost"),
            Some(&CompletionReason::AppQuit),
        )
        .unwrap();
    assert!(
        !store
            .finish_detached_recovery(
                &session_id,
                SessionStatus::Completed,
                None,
                Some(&CompletionReason::TurnComplete)
            )
            .unwrap(),
        "an error row with another reason is not a claim either"
    );
    store
        .update_session_status(
            &session_id,
            SessionStatus::Error,
            Some("lost"),
            Some(&CompletionReason::Detached),
        )
        .unwrap();
    assert!(store
        .finish_detached_recovery(
            &session_id,
            SessionStatus::Completed,
            None,
            Some(&CompletionReason::TurnComplete)
        )
        .unwrap());
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, SessionStatus::Completed);
    assert_eq!(session.error_message, None);
}
