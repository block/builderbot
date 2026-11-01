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
fn ingest_locally<'a>(store: &'a Arc<Store>, session_id: &str) -> impl FnMut(NoteSubmission) + 'a {
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
    }
}

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
            ingested += 1
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
            ingested += 1
        })
        .expect("the sweep claims the row");
    assert_eq!(outcome.status, SessionStatus::Error);
    assert_eq!(outcome.reason, CompletionReason::Detached);
    assert!(
        outcome
            .error
            .as_deref()
            .is_some_and(|e| e.contains("no completed note") && e.contains("ws-1")),
        "{outcome:?}"
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
            ingested += 1
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
        outcome
            .error
            .as_deref()
            .is_some_and(|e| e.contains("workstation suspended")),
        "{outcome:?}"
    );
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.error_message, outcome.error);
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
