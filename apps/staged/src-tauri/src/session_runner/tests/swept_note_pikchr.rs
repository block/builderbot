//! The detached sweep checks a found note's Pikchr before it stores it, and
//! a Resume repairs what the sweep turned down.
//!
//! Every other reader of the workstation outbox validated a note before the
//! hooks got it (the live run, the takeover, reconcile, the start-of-run
//! recovery). The sweep did not: a detached or hold-stopped row whose agent
//! published a note with a diagram that does not parse was completed with
//! that note within a minute. Both sweep paths now check first and park the
//! row with a message that names the block, and they leave the outbox and
//! the draft on the workstation.
//!
//! That only helps if the Resume the message asks for can repair the note.
//! Its start-of-run recovery used to delete such a note with its draft and
//! tell the agent nothing. It now removes the publication only, keeps the
//! draft, and hands the parse error to the run's first prompt.

use super::detached_recovery::{
    b64, detach_awaiting_note, hold_stop_without_note, hold_stopped_rows, ingest_locally, observed,
    plan, recording_exec, remote_note_session, settled,
};
use super::*;
use crate::note_tool::NoteSubmission;
use crate::remote_staged_mcp::note_outbox::{
    collect_turn_into_slot, new_collected_note_memory, recover_uncollected_outbox, DraftSeed,
    OutboxRecovery,
};
use crate::remote_staged_mcp::test_support::local_exec;
use std::cell::RefCell;

/// A note body whose Pikchr block, starting at line 3, does not parse.
const BROKEN: &str = "Intro\n\n```pikchr\nboxx \"bad\"\n```";
/// The same note with the block repaired.
const FIXED: &str = "Intro\n\n```pikchr\nbox \"good\"\n```";

/// What the Pikchr check says about [`BROKEN`].
fn broken_block() -> crate::pikchr_validation::PikchrValidationError {
    let error = crate::pikchr_validation::validate_pikchr_blocks(BROKEN)
        .into_iter()
        .next()
        .expect("the block does not parse");
    assert_eq!(error.line_number, 3);
    error
}

/// What the collect script prints for an outbox holding [`BROKEN`] as a
/// finished note titled "Plan".
fn broken_output() -> String {
    let meta = r#"{"title":"Plan","turn":"old-run","writtenAt":1}"#;
    format!(
        "done\n{}\n{}\ndone\n",
        b64(meta.as_bytes()),
        b64(BROKEN.as_bytes())
    )
}

/// The hooks must not see a note the sweep turned down.
fn never_ingested(submission: NoteSubmission) -> bool {
    panic!(
        "a note with a broken diagram reached the hooks: {}",
        submission.title
    )
}

/// What every message for a rejected note has to tell the user, and the
/// markers it must not carry: it is a settled row, not one the sweep is
/// still working on, and the chat pane shows it as written.
fn assert_says_rejected(message: &str) {
    let error = broken_block();
    for said in [
        "\"Plan\"",
        "the Pikchr block at line 3 does not parse",
        error.message.as_str(),
        "the note was not stored",
        "Resume to have the agent fix the diagram in its draft.",
    ] {
        assert!(message.contains(said), "{said}: {message}");
    }
    for marker in [
        AWAITING_REMOTE_NOTE_SUFFIX,
        STORING_REMOTE_NOTE_SUFFIX,
        UNREAD_OUTBOX_DETAIL_MARKER,
        UNACKNOWLEDGED_STOP_DETAIL_MARKER,
    ] {
        assert!(!message.contains(marker), "{marker}: {message}");
    }
}

/// The startup sweep of a dead owner's row: claim, read, check. Before the
/// check this completed the row and stored the note.
#[test]
fn the_startup_sweep_does_not_store_a_note_with_a_broken_diagram() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    let output = broken_output();
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);

    let outcome = finalize_detached_remote_note_session(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut never_ingested,
    )
    .expect("the sweep claims the row");
    assert_eq!(
        outcome,
        DetachedRecoveryOutcome {
            status: SessionStatus::Error,
            reason: CompletionReason::Detached,
            error: Some(detached_note_rejected_message(
                "ws-1",
                "Plan",
                &broken_block()
            )),
        }
    );
    assert_says_rejected(outcome.error.as_deref().unwrap());
    assert_eq!(
        log.borrow().len(),
        1,
        "the read only, the outbox and the draft are kept: {:?}",
        log.borrow()
    );

    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, SessionStatus::Error);
    assert_eq!(session.completion_reason, Some(CompletionReason::Detached));
    assert_eq!(session.error_message, outcome.error);
    assert_eq!(
        detached_note_poll_action(&session, session.updated_at),
        DetachedNotePollAction::Skip,
        "a rejected note is not polled for again; Resume repairs it"
    );
    assert!(!detached_note_recovery_pending(
        &CompletionReason::Detached,
        session.error_message.as_deref()
    ));
    let note = store.get_note(&note_id).unwrap().unwrap();
    assert_eq!(note.content, "");
    assert!(note.completed_at.is_none());
}

/// The 60 s poll of a detached row: the check comes before the storing
/// claim, so the row goes straight from the polling marker to the rejected
/// message and is never shown as "storing the note".
#[test]
fn the_poll_does_not_claim_or_store_a_note_with_a_broken_diagram() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);
    let output = broken_output();
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);

    let outcome = settled(poll_detached_remote_note_session_once(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut never_ingested,
        &observed(&store, &session_id),
    ));
    assert_eq!(
        outcome,
        RemoteNoteParkedState::Detached.note_rejected("ws-1", "Plan", &broken_block())
    );
    assert_eq!(
        (&outcome.status, &outcome.reason),
        (&SessionStatus::Error, &CompletionReason::Detached)
    );
    assert_says_rejected(outcome.error.as_deref().unwrap());
    assert_eq!(log.borrow().len(), 1, "the read only: {:?}", log.borrow());
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, SessionStatus::Error);
    assert_eq!(session.error_message, outcome.error);
    assert_eq!(
        detached_note_poll_action(&session, session.updated_at),
        DetachedNotePollAction::Skip
    );
    assert_eq!(store.get_note(&note_id).unwrap().unwrap().content, "");

    // A storing claim whose claimant died: the re-poll checks the note too,
    // and the row does not stay parked on the claim.
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    assert!(store
        .transition_from_running(
            &session_id,
            SessionStatus::Error,
            Some(&detached_note_storing_message("ws-1")),
            Some(&CompletionReason::Detached),
        )
        .unwrap());
    let outcome = settled(poll_detached_remote_note_session_once(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut never_ingested,
        &observed(&store, &session_id),
    ));
    assert_says_rejected(outcome.error.as_deref().unwrap());
    assert_eq!(
        store
            .get_session(&session_id)
            .unwrap()
            .unwrap()
            .error_message,
        outcome.error
    );
    assert_eq!(store.get_note(&note_id).unwrap().unwrap().content, "");
}

/// The rejected message goes through the same guarded write as every other
/// sweep write: a row the user resumed between the sweep's read of it and
/// the poll's answer belongs to its new run.
#[test]
fn a_resumed_row_is_not_overwritten_with_the_rejected_message() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, _) = remote_note_session(&store);
    detach_awaiting_note(&store, &session_id);
    let read_before_the_resume = observed(&store, &session_id);
    store
        .update_session_status(&session_id, SessionStatus::Running, None, None)
        .unwrap();
    let output = broken_output();
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);

    assert_eq!(
        poll_detached_remote_note_session_once(
            &store,
            &session_id,
            "ws-1",
            &exec,
            &mut never_ingested,
            &read_before_the_resume,
        ),
        DetachedNotePoll::Taken
    );
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, SessionStatus::Running);
    assert_eq!(session.error_message, None);
    assert_eq!(log.borrow().len(), 1, "the read only: {:?}", log.borrow());
}

/// A hold-stopped row is completed already. A late note with a broken
/// diagram leaves it `completed`/`hold_stopped` with the note still empty;
/// the message settles the row, so the poll query stops returning it, and
/// the outbox is kept. That holds past the window too, where a row with no
/// note has its outbox cleared.
#[test]
fn a_late_note_with_a_broken_diagram_settles_the_hold_stopped_row_and_keeps_the_outbox() {
    for past_window in [false, true] {
        let store = Arc::new(Store::in_memory().unwrap());
        let (session_id, note_id) = remote_note_session(&store);
        hold_stop_without_note(&store, &session_id);
        assert_eq!(hold_stopped_rows(&store), vec![session_id.clone()]);
        let output = broken_output();
        let log = RefCell::new(Vec::new());
        let exec = recording_exec(&output, &log);

        let visit = poll_detached_remote_note_session(
            &store,
            &session_id,
            "ws-1",
            &exec,
            &mut never_ingested,
            &plan(&store, &session_id, past_window),
        );
        assert!(!visit.unreachable);
        let outcome = visit.outcome.expect("the visit settles the row");
        assert_eq!(
            outcome,
            DetachedRecoveryOutcome {
                status: SessionStatus::Completed,
                reason: CompletionReason::HoldStopped,
                error: Some(hold_stopped_note_rejected_message(
                    "ws-1",
                    "Plan",
                    &broken_block()
                )),
            },
            "past the window: {past_window}"
        );
        assert_says_rejected(outcome.error.as_deref().unwrap());
        assert_eq!(
            log.borrow().len(),
            1,
            "the read only, no clear (past the window: {past_window}): {:?}",
            log.borrow()
        );

        let session = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.status, SessionStatus::Completed);
        assert_eq!(
            session.completion_reason,
            Some(CompletionReason::HoldStopped)
        );
        assert_eq!(session.error_message, outcome.error);
        assert_eq!(
            detached_note_poll_action(&session, session.updated_at),
            DetachedNotePollAction::Skip
        );
        assert_eq!(
            hold_stopped_rows(&store),
            Vec::<String>::new(),
            "a settled row is no longer returned for polling"
        );
        assert_eq!(store.get_note(&note_id).unwrap().unwrap().content, "");

        // A resume clears the message with its status write.
        store
            .update_session_status(&session_id, SessionStatus::Running, None, None)
            .unwrap();
        assert_eq!(
            store
                .get_session(&session_id)
                .unwrap()
                .unwrap()
                .error_message,
            None
        );
    }
}

/// Write `body` into the session's outbox under `home` as `write_note` does
/// for the run whose turn id is `turn`, with the draft it was published
/// from.
fn publish(home: &std::path::Path, session_id: &str, turn: &str, body: &str) {
    let outbox = home.join(".staged/sessions").join(session_id);
    std::fs::create_dir_all(&outbox).unwrap();
    for (file, content) in [
        ("draft.md", format!("{body}\n")),
        ("note.md", body.to_string()),
        (
            "meta.json",
            format!(r#"{{"title":"Plan","turn":"{turn}","writtenAt":1}}"#),
        ),
        ("done", String::new()),
    ] {
        std::fs::write(outbox.join(file), content).unwrap();
    }
}

/// Both halves together, against a real shell: the sweep turns the note
/// down and leaves everything on the workstation, the Resume's recovery
/// keeps the draft and opens the run with the repair request, and the note
/// the agent then publishes from the repaired draft is the one stored.
/// Before part B the recovery deleted the note and the draft and the agent,
/// told nothing, wrote the note again from memory.
#[test]
fn a_note_the_sweep_turned_down_is_repaired_from_its_draft_by_the_resume() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    let home = tempfile::tempdir().unwrap();
    let exec = local_exec(home.path());
    let outbox = home.path().join(".staged/sessions").join(&session_id);

    // The lost run's agent published a note with a broken diagram, and the
    // sweep found it.
    publish(home.path(), &session_id, "run-1", BROKEN);
    detach_awaiting_note(&store, &session_id);
    let outcome = settled(poll_detached_remote_note_session_once(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut never_ingested,
        &observed(&store, &session_id),
    ));
    assert_says_rejected(outcome.error.as_deref().unwrap());
    for file in ["done", "meta.json", "note.md", "draft.md"] {
        assert!(outbox.join(file).exists(), "the sweep leaves {file}");
    }

    // The Resume: the recovery finds the note, does not store it, removes
    // its publication and keeps the draft.
    store
        .update_session_status(&session_id, SessionStatus::Running, None, None)
        .unwrap();
    let recovery = recover_uncollected_outbox(&exec, &session_id, &mut never_ingested, &mut || {
        panic!("a kept draft is not seeded over")
    });
    let rejected = note_rejected_by_recovery(&recovery).expect("the recovery rejects the note");
    assert_eq!(rejected.title, "Plan");
    assert_eq!(rejected.error, broken_block());
    assert!(
        matches!(
            &recovery,
            Ok(recovery)
                if recovery.draft == DraftSeed::Kept
                    && matches!(recovery.outbox, OutboxRecovery::Rejected(_))
        ),
        "{recovery:?}"
    );
    assert!(remote_outbox_recovery_settled_for_run(
        &session_id,
        recovery
    ));
    for file in ["done", "meta.json", "note.md"] {
        assert!(!outbox.join(file).exists(), "the recovery removes {file}");
    }
    assert_eq!(
        std::fs::read_to_string(outbox.join("draft.md")).unwrap(),
        format!("{BROKEN}\n")
    );
    assert_eq!(store.get_note(&note_id).unwrap().unwrap().content, "");

    // The run opens with the repair request, which points at the block's
    // line in the draft that is still there.
    let draft_path = outbox.join("draft.md").to_string_lossy().into_owned();
    let prompt = first_turn_prompt(
        "Continue.".to_string(),
        true,
        true,
        Some((&rejected, &draft_path)),
    );
    for said in [
        "Continue.",
        draft_path.as_str(),
        "starting at line 3 of the note body in that file",
        rejected.error.message.as_str(),
    ] {
        assert!(prompt.contains(said), "{said}: {prompt}");
    }

    // The agent edits the draft and publishes again. This run's collector
    // takes it, the check passes and the hooks store it.
    publish(home.path(), &session_id, "run-2", FIXED);
    let slot = crate::note_tool::new_submission_slot();
    let memory = new_collected_note_memory();
    assert!(matches!(
        collect_turn_into_slot(&exec, &session_id, "run-2", &slot, &memory),
        Ok(OutboxState::Delivered(_))
    ));
    assert_eq!(validate_submitted_note_pikchr(&slot), None);
    let submission = crate::note_tool::current_submission(&slot).unwrap();
    assert!(ingest_locally(&store, &session_id)(submission));
    let note = store.get_note(&note_id).unwrap().unwrap();
    assert_eq!(note.title, "Plan");
    assert_eq!(note.content, FIXED);
}
