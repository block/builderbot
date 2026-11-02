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

/// The row as a sweep reads it before polling.
fn observed(store: &Arc<Store>, session_id: &str) -> ObservedDetachedRow {
    ObservedDetachedRow::of(&store.get_session(session_id).unwrap().unwrap())
}

/// A sweep visit planned from a fresh read of the row.
fn plan(store: &Arc<Store>, session_id: &str, past_window: bool) -> DetachedNotePollPlan {
    DetachedNotePollPlan {
        observed: observed(store, session_id),
        past_window,
        prior_unreachable: 0,
    }
}

fn settled(poll: DetachedNotePoll) -> DetachedRecoveryOutcome {
    match poll {
        DetachedNotePoll::Settled(outcome) => outcome,
        other => panic!("expected the poll to settle the row, got {other:?}"),
    }
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

    let as_read = observed(&store, &session_id);
    let outcome = settled(poll_detached_remote_note_session_once(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut ingest_locally(&store, &session_id),
        &as_read,
    ));
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
        poll_detached_remote_note_session_once(
            &store,
            &session_id,
            "ws-1",
            &exec,
            &mut |_| {
                ingested += 1;
                true
            },
            &as_read
        ),
        DetachedNotePoll::Taken
    );
    assert_eq!(ingested, 0);
    assert_eq!(log.borrow().len(), 1, "the read happened, nothing after it");
}

#[test]
fn two_instances_that_read_the_row_before_either_claimed_settle_it_once() {
    // Both sweeps snapshot the same detached row (same marker, same
    // `updated_at`) and both find `done`. The first claim restamps the row;
    // the second instance's claim is guarded on the row *it* read, so it
    // fails even though the row is still `error`/`detached` at that moment.
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);
    let read_by_a = observed(&store, &session_id);
    let read_by_b = read_by_a.clone();
    let output = done_output();
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);

    // Instance A's ingest runs with the row in A's claimed (storing) state;
    // instance B's poll lands right then, before A's `completed` write.
    let store_for_b = Arc::clone(&store);
    let session_for_b = session_id.clone();
    let exec_for_b = recording_exec(&output, &log);
    let mut b_ingested = 0;
    let mut b_result = None;
    let mut a_ingest = ingest_locally(&store, &session_id);
    let mut a_ingest_with_b_racing = |submission: NoteSubmission| {
        let row = store_for_b.get_session(&session_for_b).unwrap().unwrap();
        assert_eq!(row.status, SessionStatus::Error);
        assert_eq!(row.completion_reason, Some(CompletionReason::Detached));
        b_result = Some(poll_detached_remote_note_session_once(
            &store_for_b,
            &session_for_b,
            "ws-1",
            &exec_for_b,
            &mut |_| {
                b_ingested += 1;
                true
            },
            &read_by_b,
        ));
        a_ingest(submission)
    };
    let outcome = settled(poll_detached_remote_note_session_once(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut a_ingest_with_b_racing,
        &read_by_a,
    ));
    assert_eq!(outcome.status, SessionStatus::Completed);
    assert_eq!(b_result, Some(DetachedNotePoll::Taken));
    assert_eq!(b_ingested, 0, "B backed off without ingesting");
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, SessionStatus::Completed);
    assert_eq!(store.get_note(&note_id).unwrap().unwrap().content, "Body");
    let commands = log.borrow();
    assert_eq!(
        commands.len(),
        3,
        "A's collect, B's collect, A's clear: {commands:?}"
    );
    assert_eq!(commands[2][2], clear_outbox_script(&session_id));
}

#[test]
fn a_stale_storing_claim_is_polled_again_and_the_slow_claimant_backs_off() {
    // Instance A claimed the row (storing marker) and died before its
    // `completed` write. The row must not stay parked: once the claim is
    // older than DETACHED_NOTE_STORING_STALE the sweep polls it again, and
    // a late write from A — had it merely been slow — is refused.
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);
    let before_claim = observed(&store, &session_id);
    let claim = DetachedRecoveryOutcome {
        status: SessionStatus::Error,
        reason: CompletionReason::Detached,
        error: Some(detached_note_storing_message("ws-1")),
    };
    let claim_stamp = before_claim
        .settle(&store, &session_id, &claim)
        .expect("A's claim lands");
    let a_claimed = ObservedDetachedRow {
        updated_at: claim_stamp,
        error_message: claim.error.clone(),
    };

    let row = store.get_session(&session_id).unwrap().unwrap();
    assert!(detached_note_recovery_pending(
        &CompletionReason::Detached,
        row.error_message.as_deref()
    ));
    let stale_ms = DETACHED_NOTE_STORING_STALE.as_millis() as i64;
    assert_eq!(
        detached_note_poll_action(&row, row.updated_at + stale_ms),
        DetachedNotePollAction::Skip,
        "a fresh claim is left to its claimant"
    );
    assert_eq!(
        detached_note_poll_action(&row, row.updated_at + stale_ms + 1),
        DetachedNotePollAction::Poll,
        "a stale claim is polled again"
    );
    assert_eq!(
        detached_note_poll_action(
            &row,
            row.updated_at + DETACHED_NOTE_POLL_WINDOW.as_millis() as i64 + 1
        ),
        DetachedNotePollAction::PollThenExpire
    );

    // Instance B's re-poll, observing the stale storing row, completes it.
    let output = done_output();
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);
    let outcome = settled(poll_detached_remote_note_session_once(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut ingest_locally(&store, &session_id),
        &observed(&store, &session_id),
    ));
    assert_eq!(outcome.status, SessionStatus::Completed);
    assert_eq!(store.get_note(&note_id).unwrap().unwrap().content, "Body");

    // A, finally finishing, writes against the row it claimed: refused.
    let a_outcome = DetachedRecoveryOutcome {
        status: SessionStatus::Error,
        reason: CompletionReason::Detached,
        error: Some(detached_note_unstored_message("ws-1", "Plan")),
    };
    assert_eq!(a_claimed.settle(&store, &session_id, &a_outcome), None);
    assert_eq!(
        store.get_session(&session_id).unwrap().unwrap().status,
        SessionStatus::Completed
    );

    // The same guard, on a stale claim B has re-claimed but not yet
    // finished: A's write against its own claim stamp is refused too.
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, _) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);
    let a_stamp = observed(&store, &session_id)
        .settle(&store, &session_id, &claim)
        .unwrap();
    // Force a distinct stamp for B's claim.
    std::thread::sleep(std::time::Duration::from_millis(2));
    let b_stamp = observed(&store, &session_id)
        .settle(&store, &session_id, &claim)
        .expect("B re-claims the stale row");
    assert_ne!(a_stamp, b_stamp);
    let a_claimed = ObservedDetachedRow {
        updated_at: a_stamp,
        error_message: claim.error.clone(),
    };
    assert_eq!(a_claimed.settle(&store, &session_id, &a_outcome), None);
    let row = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(row.updated_at, b_stamp, "B's claim stands");
}

#[test]
fn a_poll_without_a_note_writes_nothing_and_a_resumed_row_is_left_alone() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);
    let before = store.get_session(&session_id).unwrap().unwrap();
    let before_read = ObservedDetachedRow::of(&before);
    let log = RefCell::new(Vec::new());

    // Nothing yet, and a `done` marker next to nothing usable: both are an
    // outbox without a note, not an unreachable workstation.
    for output in ["pending\n\n\n", "done\n\n\n"] {
        let exec = recording_exec(output, &log);
        assert_eq!(
            poll_detached_remote_note_session_once(
                &store,
                &session_id,
                "ws-1",
                &exec,
                &mut |_| { panic!("nothing to ingest") },
                &before_read
            ),
            DetachedNotePoll::NoNoteYet,
            "{output:?}"
        );
    }
    let unreachable = |_: &[&str]| -> Result<String, String> { Err("suspended".into()) };
    assert!(matches!(
        poll_detached_remote_note_session_once(
            &store,
            &session_id,
            "ws-1",
            &unreachable,
            &mut |_| { panic!("nothing to ingest") },
            &before_read
        ),
        DetachedNotePoll::Unreachable(ref e) if e.contains("suspended")
    ));
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
        poll_detached_remote_note_session_once(
            &store,
            &session_id,
            "ws-1",
            &exec,
            &mut |_| {
                ingested += 1;
                true
            },
            &before_read
        ),
        DetachedNotePoll::Taken
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
    let as_read = observed(&store, &session_id);

    let outcome = expire_detached_remote_note_session(
        &store,
        &session_id,
        &as_read,
        detached_note_expired_message("ws-1"),
    )
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
        expire_detached_remote_note_session(
            &store,
            &session_id,
            &as_read,
            detached_note_expired_message("ws-1")
        ),
        None
    );
    assert_eq!(
        store.get_session(&session_id).unwrap().unwrap().status,
        SessionStatus::Completed
    );

    // The unreachable expiry reads as what it is, and ends the polling too.
    let unreachable = detached_note_unreachable_message("ws-1");
    assert!(
        unreachable.contains("could not reach")
            && unreachable.contains("Resume")
            && !unreachable.ends_with(AWAITING_REMOTE_NOTE_SUFFIX)
            && !unreachable.ends_with(STORING_REMOTE_NOTE_SUFFIX),
        "{unreachable}"
    );
    assert!(!note_recovery_is_pending(
        Some("detached"),
        Some(&unreachable)
    ));
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
        "Connection reset by peer (os error 54)",
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
        // A bridge relaying its own agent subprocess dying, over a live
        // transport: the agent is gone, not the connection to it.
        (
            Some(SessionSettleReason::Failed),
            r#"ACP protocol failed: Error { code: -32603: Internal error, message: "Internal error", data: Some(String("Claude Code process exited with code 1")) }"#,
        ),
        (Some(SessionSettleReason::Failed), "child exited"),
        (
            Some(SessionSettleReason::Failed),
            "subprocess exited with status 137",
        ),
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
    let outcome = settled(poll_detached_remote_note_session_once(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut counting_unstored(&mut ingested),
        &observed(&store, &session_id),
    ));
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
    let outcome = settled(poll_detached_remote_note_session_once(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut ingest,
        &observed(&store, &session_id),
    ));
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
    let visit = poll_detached_remote_note_session(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut ingest_locally(&store, &session_id),
        &plan(&store, &session_id, true),
    );
    let outcome = visit.outcome.expect("the last poll completes the row");
    assert_eq!(outcome.status, SessionStatus::Completed);
    assert!(!visit.unreachable);
    assert_eq!(
        store.get_session(&session_id).unwrap().unwrap().status,
        SessionStatus::Completed
    );
    assert_eq!(store.get_note(&note_id).unwrap().unwrap().content, "Body");

    // Nothing there: that last poll is what expires the row.
    let pending = |_: &[&str]| -> Result<String, String> { Ok("pending\n\n\n".into()) };
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, _) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);
    let visit = poll_detached_remote_note_session(
        &store,
        &session_id,
        "ws-1",
        &pending,
        &mut |_| panic!("nothing to ingest"),
        &plan(&store, &session_id, true),
    );
    let outcome = visit.outcome.expect("the expiry is recorded");
    assert_eq!(outcome.reason, CompletionReason::Detached);
    assert!(outcome
        .error
        .as_deref()
        .is_some_and(|m| m.contains("30 minutes")));
    assert!(!visit.unreachable);

    // Inside the window the same empty poll writes nothing.
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, _) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);
    let before = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(
        poll_detached_remote_note_session(
            &store,
            &session_id,
            "ws-1",
            &pending,
            &mut |_| panic!("nothing to ingest"),
            &plan(&store, &session_id, false),
        ),
        DetachedNotePollVisit {
            outcome: None,
            unreachable: false
        }
    );
    let after = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(
        (after.error_message, after.updated_at),
        (before.error_message, before.updated_at)
    );
}

#[test]
fn an_unreadable_outbox_past_the_window_is_retried_before_the_row_expires_as_unreachable() {
    // A failed read says nothing about the note. Past the window it is
    // retried DETACHED_NOTE_POLL_MAX_UNREACHABLE sweeps in a row, then the
    // row is expired with copy that says the workstation was unreachable —
    // never with the "no note appeared" copy.
    let unreachable = |_: &[&str]| -> Result<String, String> { Err("suspended".into()) };
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, _) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);
    let before = store.get_session(&session_id).unwrap().unwrap();

    for prior in 0..DETACHED_NOTE_POLL_MAX_UNREACHABLE - 1 {
        for past_window in [false, true] {
            let visit = poll_detached_remote_note_session(
                &store,
                &session_id,
                "ws-1",
                &unreachable,
                &mut |_| panic!("nothing to ingest"),
                &DetachedNotePollPlan {
                    observed: ObservedDetachedRow::of(&before),
                    past_window,
                    prior_unreachable: prior,
                },
            );
            assert_eq!(
                visit,
                DetachedNotePollVisit {
                    outcome: None,
                    unreachable: true
                },
                "prior={prior} past_window={past_window}"
            );
        }
    }
    let after = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(
        (after.error_message.clone(), after.updated_at),
        (before.error_message.clone(), before.updated_at),
        "the row keeps its marker and stamp through the retries"
    );

    // Inside the window even the last allowed failure does not expire it.
    assert_eq!(
        poll_detached_remote_note_session(
            &store,
            &session_id,
            "ws-1",
            &unreachable,
            &mut |_| panic!("nothing to ingest"),
            &DetachedNotePollPlan {
                observed: ObservedDetachedRow::of(&before),
                past_window: false,
                prior_unreachable: DETACHED_NOTE_POLL_MAX_UNREACHABLE - 1,
            },
        ),
        DetachedNotePollVisit {
            outcome: None,
            unreachable: true
        }
    );

    // The Nth consecutive failure past the window gives up, honestly.
    let visit = poll_detached_remote_note_session(
        &store,
        &session_id,
        "ws-1",
        &unreachable,
        &mut |_| panic!("nothing to ingest"),
        &DetachedNotePollPlan {
            observed: ObservedDetachedRow::of(&before),
            past_window: true,
            prior_unreachable: DETACHED_NOTE_POLL_MAX_UNREACHABLE - 1,
        },
    );
    assert!(visit.unreachable);
    let outcome = visit.outcome.expect("the unreachable expiry is recorded");
    assert_eq!(outcome.reason, CompletionReason::Detached);
    assert_eq!(
        outcome.error.as_deref(),
        Some(detached_note_unreachable_message("ws-1").as_str())
    );
    assert!(!outcome
        .error
        .as_deref()
        .unwrap()
        .contains("no completed note appeared"));
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.error_message, outcome.error);
    assert_eq!(
        detached_note_poll_action(&session, session.updated_at),
        DetachedNotePollAction::Skip,
        "never polled again"
    );
    assert!(session.completion_reason.unwrap().is_resumable());
}

#[test]
fn a_stop_the_agent_never_acknowledged_or_a_silent_connection_count_as_a_lost_agent() {
    use acp_client::SessionSettleReason;
    assert!(
        UNACKNOWLEDGED_STOP_DETAIL_MARKER.contains("did not acknowledge the stop"),
        "the frontend sniffs this phrase"
    );
    let transport_error = Err(AgentRunError::Other(PROXY_TRANSPORT_CLOSED.to_string()));
    assert_eq!(
        lost_remote_agent_detail(
            &transport_error,
            Some(SessionSettleReason::Failed),
            false,
            None,
            STOPPED
        )
        .as_deref(),
        Some("incoming transport closed")
    );

    // The user pressed Stop, the proxy never answered: the Stop is not one
    // the agent honoured, so the row must not end `cancelled`.
    let cancelled = Ok(AgentRunOutcome::Cancelled);
    let detail = lost_remote_agent_detail(
        &cancelled,
        Some(SessionSettleReason::Cancelled),
        true,
        None,
        STOPPED,
    )
    .expect("an unacknowledged stop is a lost agent");
    assert!(
        detail.contains(UNACKNOWLEDGED_STOP_DETAIL_MARKER),
        "{detail}"
    );
    let (status, error, reason) = lost_remote_agent_terminal_state(&detail, "ws-1", None);
    assert_eq!(
        (status, reason.clone()),
        ("error", CompletionReason::Detached)
    );
    let error = error.unwrap();
    assert!(
        error.contains(UNACKNOWLEDGED_STOP_DETAIL_MARKER)
            && error.contains("may or may not appear")
            && error.ends_with(AWAITING_REMOTE_NOTE_SUFFIX),
        "the copy does not promise a note: {error}"
    );
    assert!(
        !remote_outbox_is_cleared_at_run_end(SETTLED_RUN, &reason, Some(&error)),
        "the outbox is kept for the sweep"
    );

    // A Stop the agent acknowledged is a plain Stop.
    assert_eq!(
        lost_remote_agent_detail(
            &cancelled,
            Some(SessionSettleReason::Cancelled),
            false,
            None,
            STOPPED
        ),
        None
    );

    // The inactivity watchdog found the finished note behind a silent
    // connection and cancelled the turn itself: lost, with the note.
    let detail = lost_remote_agent_detail(
        &cancelled,
        Some(SessionSettleReason::Cancelled),
        false,
        Some(Duration::from_secs(10 * 60)),
        STOPPED,
    )
    .expect("a silent connection with a finished note is a lost agent");
    assert!(detail.contains("10 minutes"), "{detail}");
    let valid = NoteSubmission::from_params(
        crate::note_tool::WriteNoteParams {
            title: "Plan".into(),
            body: "Body".into(),
            suggested_next_steps: None,
        },
        NoteSubmissionSource::Outbox,
    )
    .unwrap();
    assert_eq!(
        lost_remote_agent_terminal_state(&detail, "ws-1", Some(&valid)),
        ("completed", None, CompletionReason::TurnComplete)
    );

    // Everything else is what it was.
    for (result, settle) in [
        (
            Ok(AgentRunOutcome::Completed),
            Some(SessionSettleReason::Immediate),
        ),
        (
            Err(AgentRunError::Other(
                "ACP authentication is required to run the prompt.".into(),
            )),
            Some(SessionSettleReason::Failed),
        ),
        (
            Err(AgentRunError::PikchrRepairExhausted(
                "Pikchr validation failed".into(),
            )),
            Some(SessionSettleReason::Quiescent),
        ),
        (
            Err(AgentRunError::Other(PROXY_TRANSPORT_CLOSED.to_string())),
            None,
        ),
    ] {
        assert_eq!(
            lost_remote_agent_detail(&result, settle, false, None, STOPPED),
            None,
            "{result:?} / {settle:?}"
        );
    }
}

/// The context of a user Stop: the session's cancel token fired. The agent's
/// silence is irrelevant there — the proxy answered nothing for the deadline.
const STOPPED: UnacknowledgedCancelContext = UnacknowledgedCancelContext {
    stop_requested: true,
    agent_idle: Some(Duration::from_secs(10)),
};

/// An interrupt-and-reprompt whose cancel went unanswered is a lost agent
/// only when the connection relayed nothing for the whole ack wait. A remote
/// agent that kept streaming while a long tool call wound down was merely
/// slow to answer, and parking it `detached` for half an hour would tell the
/// user Staged lost a session that was just stopping.
#[test]
fn an_unacknowledged_interrupt_is_lost_only_when_the_agent_was_silent_throughout() {
    use acp_client::{SessionSettleReason, INTERRUPT_ACK_TIMEOUT};
    let cancelled = Ok(AgentRunOutcome::Cancelled);
    let unacknowledged = Some(SessionSettleReason::Cancelled);

    // Still streaming 5 s ago: a slow agent, so the plain cancelled arm.
    let streaming = UnacknowledgedCancelContext {
        stop_requested: false,
        agent_idle: Some(Duration::from_secs(5)),
    };
    assert_eq!(
        lost_remote_agent_detail(&cancelled, unacknowledged, true, None, streaming),
        None
    );

    // Nothing for the whole ack wait: the connection is presumed lost.
    let silent = UnacknowledgedCancelContext {
        stop_requested: false,
        agent_idle: Some(INTERRUPT_ACK_TIMEOUT),
    };
    let detail = lost_remote_agent_detail(&cancelled, unacknowledged, true, None, silent)
        .expect("a silent unacknowledged interrupt is a lost agent");
    assert!(detail.contains("interrupt"), "{detail}");
    assert!(
        !detail.contains(UNACKNOWLEDGED_STOP_DETAIL_MARKER),
        "not the Stop copy: {detail}"
    );

    // A user Stop is lost whatever the agent was doing a moment before.
    let stop_while_streaming = UnacknowledgedCancelContext {
        stop_requested: true,
        agent_idle: Some(Duration::from_secs(1)),
    };
    assert!(
        lost_remote_agent_detail(&cancelled, unacknowledged, true, None, stop_while_streaming)
            .is_some_and(|detail| detail.contains(UNACKNOWLEDGED_STOP_DETAIL_MARKER))
    );
}

#[test]
fn a_crashed_row_carries_the_innermost_sentence_as_written() {
    assert_eq!(
        innermost_error_sentence(PROXY_TRANSPORT_CLOSED).as_deref(),
        Some("Incoming transport closed")
    );
    assert_eq!(
        innermost_error_sentence(
            r#"ACP protocol failed: Error { code: -32603: Internal error, message: "Internal error", data: Some(String("Claude Code process exited with code 1.")) }"#
        )
        .as_deref(),
        Some("Claude Code process exited with code 1")
    );
    assert_eq!(
        innermost_error_sentence(
            r#"Prompt failed: Error { code: -32603: Internal error, message: "Internal error", data: None }"#
        ),
        None,
        "nothing better than the whole error"
    );
    assert_eq!(innermost_error_sentence("child exited"), None);
}

#[test]
fn the_timeline_flag_covers_polling_and_storing_rows_only() {
    let awaiting = awaiting_remote_note_message("ws-1", None);
    let storing = detached_note_storing_message("ws-1");
    assert!(note_recovery_is_pending(Some("detached"), Some(&awaiting)));
    assert!(note_recovery_is_pending(Some("detached"), Some(&storing)));
    for (reason, message) in [
        (
            Some("detached"),
            Some(detached_note_expired_message("ws-1")),
        ),
        (
            Some("detached"),
            Some(detached_note_unreachable_message("ws-1")),
        ),
        (
            Some("detached"),
            Some(detached_note_unstored_message("ws-1", "Plan")),
        ),
        (Some("crashed"), Some(awaiting.clone())),
        (None, Some(awaiting.clone())),
        (Some("detached"), None),
    ] {
        assert!(
            !note_recovery_is_pending(reason, message.as_deref()),
            "{reason:?} / {message:?}"
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
    assert!(
        helpers.contains(&format!("'{STORING_REMOTE_NOTE_SUFFIX}'")),
        "REMOTE_NOTE_STORING_SUFFIX in sessionModalHelpers.ts must equal STORING_REMOTE_NOTE_SUFFIX"
    );
    assert!(
        helpers.contains(&format!("'{UNACKNOWLEDGED_STOP_DETAIL_MARKER}'")),
        "REMOTE_NOTE_UNACKNOWLEDGED_STOP_MARKER in sessionModalHelpers.ts must equal \
         UNACKNOWLEDGED_STOP_DETAIL_MARKER"
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
