//! A remote note session's turn that ends on a takeover — its run is gone —
//! settles from the workstation outbox: the dead-owner reattach through
//! `reattached_turn_run_end`, reconcile through `TakenOverNote::turn_end`.
//!
//! Both used to hand the hooks no note, which stored an empty one and left
//! the published note in the outbox until a Resume.

use super::detached_recovery::{b64, is_collect, recording_exec, remote_note_session};
use super::*;
use crate::remote_staged_mcp::note_outbox::clear_outbox_script;
use crate::session_runner::taken_over_note::{
    clear_outbox_after_takeover, collect_taken_over_note, delivers_note_through_outbox,
    reattached_turn_run_end, ReattachedTurnEnding, TakenOverNote, TakenOverRunEnd,
    TakenOverTurnEnd, OWNER_PROCESS_ENDED, SESSION_WAS_DISCONNECTED,
};
use crate::store::{Branch, Project, Session};
use std::cell::RefCell;

/// What the collect script prints for a note `write_note` published in a
/// run that is gone.
fn published(body: &str) -> String {
    let meta = serde_json::json!({ "title": "Plan", "turn": "gone-run", "writtenAt": 1 });
    format!(
        "done\n{}\n{}\ndone\n",
        b64(meta.to_string().as_bytes()),
        b64(body.as_bytes())
    )
}

const NOTHING_PUBLISHED: &str = "pending\n\n\npending\n";
/// The error of a reattached turn whose agent answered the dead owner's
/// prompt as cancelled (`acp-client`'s `owner_prompt_answered`): what an
/// app kill relaunched mid-turn ends with when the reattach's
/// `session/load` interrupts the turn.
const ANSWERED_AS_CANCELLED: &str =
    "Remote agent lost: the agent answered the turn's prompt as cancelled, so it did not finish";
const BROKEN_PIKCHR: &str = "Body\n```pikchr\nboxx \"bad\"\n```";

/// An exec whose every call fails: the workstation does not answer.
fn unreachable(args: &[&str]) -> Result<String, String> {
    let _ = args;
    Err("ssh: connect to host ws-1: Operation timed out".to_string())
}

fn collect_from(output: &str) -> TakenOverNote {
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(output, &log);
    collect_taken_over_note(&exec, "sess-1")
}

/// The run-end outbox use of a takeover that read the outbox.
fn read_outbox(own_note_collected: bool) -> RemoteOutboxUse {
    RemoteOutboxUse {
        staged_mcp_attached: true,
        recovery_settled: true,
        own_note_collected,
        read_after_last_turn: true,
        rejected_note: None,
    }
}

#[test]
fn the_outbox_is_read_with_any_turn_id() {
    let note = collect_from(&published("Body"));
    let TakenOverNote::Published(submission) = &note else {
        panic!("a note published in a run that is gone is still the session's: {note:?}");
    };
    assert_eq!(
        (submission.title.as_str(), submission.body.as_str()),
        ("Plan", "Body")
    );
    assert_eq!(submission.source, NoteSubmissionSource::Outbox);

    assert_eq!(collect_from(NOTHING_PUBLISHED), TakenOverNote::Unpublished);
    assert_eq!(
        collect_from("done\n\n\ndone\n"),
        TakenOverNote::Unpublished,
        "a marker beside missing files reads the same every time"
    );
    assert!(matches!(
        collect_taken_over_note(&unreachable, "sess-1"),
        TakenOverNote::Unreadable(error) if error.contains("Operation timed out")
    ));
}

#[test]
fn a_published_note_completes_the_turn_and_goes_to_the_hooks() {
    let note = collect_from(&published("Body"));
    let TakenOverNote::Published(submission) = note.clone() else {
        panic!("{note:?}");
    };
    assert_eq!(
        note.clone().turn_end("ws-1", SESSION_WAS_DISCONNECTED),
        TakenOverTurnEnd::Completed {
            note: Some(submission.clone())
        }
    );
    assert_eq!(
        note.run_end(OWNER_PROCESS_ENDED),
        TakenOverRunEnd {
            note_submission: Some(submission),
            outbox_use: read_outbox(true),
            lost_remote_note_agent: None,
        }
    );

    assert_eq!(
        TakenOverNote::Unpublished.turn_end("ws-1", SESSION_WAS_DISCONNECTED),
        TakenOverTurnEnd::Completed { note: None },
        "nothing published leaves the hooks to the `---` fallback, as before"
    );
    assert_eq!(
        TakenOverNote::Unpublished.run_end(OWNER_PROCESS_ENDED),
        TakenOverRunEnd {
            note_submission: None,
            outbox_use: read_outbox(false),
            lost_remote_note_agent: None,
        }
    );
}

#[test]
fn a_published_note_with_a_broken_diagram_parks_the_row_and_is_not_stored() {
    let note = collect_from(&published(BROKEN_PIKCHR));

    // Reconcile: the row is parked without the sweep's marker, and the
    // rejected note is cleared so a Resume has the agent write it again.
    let TakenOverTurnEnd::Parked {
        message,
        clear_outbox,
    } = note.clone().turn_end("ws-1", SESSION_WAS_DISCONNECTED)
    else {
        panic!("a note whose diagram does not parse must not complete the row");
    };
    assert!(clear_outbox);
    assert!(
        message.contains("Pikchr")
            && message.contains("Resume")
            && !message.ends_with(AWAITING_REMOTE_NOTE_SUFFIX),
        "{message}"
    );

    // The dead-owner reattach: the same ending through `finish_session_run`.
    let run_end = note.run_end(OWNER_PROCESS_ENDED);
    assert_eq!(
        run_end.lost_remote_note_agent.as_deref(),
        Some(OWNER_PROCESS_ENDED)
    );
    let (status, error, reason) = lost_remote_agent_terminal_state(
        OWNER_PROCESS_ENDED,
        "ws-1",
        run_end.note_submission.as_ref(),
    );
    assert_eq!(
        (status, reason.clone()),
        ("error", CompletionReason::Detached)
    );
    assert!(
        error.as_deref().is_some_and(|e| e.contains("Pikchr")),
        "{error:?}"
    );
    assert!(!detached_note_poll_is_pending(&reason, error.as_deref()));
    assert_eq!(
        remote_outbox_clear_at_run_end(
            run_end.outbox_use,
            &reason,
            error.as_deref(),
            RunEndForOutbox::default()
        ),
        RunEndOutboxClear::Clear
    );
}

#[test]
fn an_outbox_that_cannot_be_read_parks_the_row_for_the_sweep_and_is_never_cleared() {
    let note = collect_taken_over_note(&unreachable, "sess-1");

    let TakenOverTurnEnd::Parked {
        message,
        clear_outbox,
    } = note.clone().turn_end("ws-1", SESSION_WAS_DISCONNECTED)
    else {
        panic!("an unread outbox must not be judged empty");
    };
    assert!(!clear_outbox);
    assert!(
        message.ends_with(AWAITING_REMOTE_NOTE_SUFFIX)
            && message.contains("Operation timed out")
            && message.contains("ws-1"),
        "{message}"
    );
    assert!(detached_note_poll_is_pending(
        &CompletionReason::Detached,
        Some(&message)
    ));

    let run_end = note.run_end(OWNER_PROCESS_ENDED);
    assert_eq!(run_end.note_submission, None);
    assert!(!run_end.outbox_use.read_after_last_turn);
    let detail = run_end
        .lost_remote_note_agent
        .expect("the run settles from the outbox, not as completed");
    let (status, error, reason) = lost_remote_agent_terminal_state(&detail, "ws-1", None);
    assert_eq!(
        (status, reason.clone()),
        ("error", CompletionReason::Detached)
    );
    assert_eq!(
        remote_outbox_clear_at_run_end(
            run_end.outbox_use,
            &reason,
            error.as_deref(),
            RunEndForOutbox::default()
        ),
        RunEndOutboxClear::Keep
    );
}

/// The app-kill variant of the lost note: a dead owner's turn the sweep
/// reattached to. It stored an empty note and left the published one for a
/// Resume; the reattach now collects it for the hooks.
#[test]
fn a_dead_owners_reattached_turn_reads_the_outbox_unless_it_was_stopped() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    let output = published("Body");
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);

    let run_end = reattached_turn_run_end(
        &store,
        &session_id,
        Some("ws-1"),
        ReattachedTurnEnding::Completed,
        &exec,
    );
    assert_eq!(run_end.outbox_use, read_outbox(true));
    assert_eq!(run_end.lost_remote_note_agent, None);
    assert_eq!(log.borrow().len(), 1, "one read: {:?}", log.borrow());
    assert!(is_collect(
        &log.borrow()[0]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    ));
    // What `finish_session_run` does with it: the hooks store the note.
    let hooks = run_post_completion_hooks(
        &session_id,
        std::path::Path::new("."),
        None,
        None,
        None,
        &store,
        0,
        run_end.note_submission,
    );
    assert!(hooks.note_stored);
    let note = store.get_note(&note_id).unwrap().unwrap();
    assert_eq!(
        (note.title.as_str(), note.content.as_str()),
        ("Plan", "Body")
    );

    // A reattached turn that was stopped is not read; the end-of-run clear
    // then never removes a published note unread.
    log.borrow_mut().clear();
    let unread = reattached_turn_run_end(
        &store,
        &session_id,
        Some("ws-1"),
        ReattachedTurnEnding::Stopped,
        &exec,
    );
    assert!(log.borrow().is_empty());
    assert_eq!(unread.note_submission, None);
    assert_eq!(unread.lost_remote_note_agent, None);
    assert_eq!(
        remote_outbox_clear_at_run_end(
            unread.outbox_use,
            &CompletionReason::Crashed,
            Some("agent exited"),
            RunEndForOutbox::default()
        ),
        RunEndOutboxClear::ClearUnlessPublished
    );
}

/// How a reattached turn's result reads for its note: a Stop outranks
/// everything, a completed turn and a failed one are both over, and a turn
/// the agent cancelled on its own produced nothing to read.
#[test]
fn a_reattached_turns_ending_is_read_from_its_result_and_any_stop() {
    let completed: Result<AgentRunOutcome, String> = Ok(AgentRunOutcome::Completed);
    let cancelled: Result<AgentRunOutcome, String> = Ok(AgentRunOutcome::Cancelled);
    let failed: Result<AgentRunOutcome, String> = Err(ANSWERED_AS_CANCELLED.to_string());

    assert_eq!(
        ReattachedTurnEnding::of(&completed, false),
        ReattachedTurnEnding::Completed
    );
    assert_eq!(
        ReattachedTurnEnding::of(&failed, false),
        ReattachedTurnEnding::Failed(ANSWERED_AS_CANCELLED)
    );
    assert_eq!(
        ReattachedTurnEnding::of(&cancelled, false),
        ReattachedTurnEnding::Stopped
    );
    for result in [&completed, &cancelled, &failed] {
        assert_eq!(
            ReattachedTurnEnding::of(result, true),
            ReattachedTurnEnding::Stopped,
            "{result:?} with a Stop requested"
        );
    }
}

/// Scenario 2 of the live run ("App kill, relaunch 5 s later"): the agent
/// published its note, the app was killed, and the relaunch's reattach had
/// the agent answer the dead owner's prompt as cancelled. The row ended
/// `error`/`remote_agent_lost` with the note left in the outbox until a
/// Resume. The failed ending now reads the outbox, and a valid note
/// completes the session with nobody pressing anything.
#[test]
fn a_reattached_turn_answered_as_cancelled_still_stores_its_published_note() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    let output = published("Published before the app was killed.");
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);

    let run_end = reattached_turn_run_end(
        &store,
        &session_id,
        Some("ws-1"),
        ReattachedTurnEnding::Failed(ANSWERED_AS_CANCELLED),
        &exec,
    );
    assert_eq!(log.borrow().len(), 1, "one read: {:?}", log.borrow());
    assert!(is_collect(
        &log.borrow()[0]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    ));
    assert_eq!(run_end.outbox_use, read_outbox(true));
    let detail = run_end
        .lost_remote_note_agent
        .clone()
        .expect("a failed turn with a published note settles from the outbox");
    assert!(
        detail.starts_with(OWNER_PROCESS_ENDED) && detail.ends_with(ANSWERED_AS_CANCELLED),
        "{detail}"
    );

    // The terminal state `finish_session_run` writes for that detail and
    // note: the same rule as a run that loses its agent after `write_note`.
    let submission = run_end.note_submission.expect("the published note");
    assert_eq!(submission.source, NoteSubmissionSource::Outbox);
    let (status, error, reason) =
        lost_remote_agent_terminal_state(&detail, "ws-1", Some(&submission));
    assert_eq!(
        (status, error, reason.clone()),
        ("completed", None, CompletionReason::TurnComplete)
    );
    assert_eq!(
        turn_artifact_hooks(
            terminal_state_completed_successfully(status, &reason),
            &reason
        ),
        TurnArtifactHooks::All
    );
    let hooks = run_post_completion_hooks(
        &session_id,
        std::path::Path::new("."),
        None,
        None,
        None,
        &store,
        0,
        Some(submission),
    );
    assert!(hooks.note_stored);
    let note = store.get_note(&note_id).unwrap().unwrap();
    assert_eq!(
        (note.title.as_str(), note.content.as_str()),
        ("Plan", "Published before the app was killed.")
    );
    assert!(note.completed_at.is_some());
    // Stored, so the read outbox is cleared.
    assert_eq!(
        remote_outbox_clear_at_run_end(
            run_end.outbox_use,
            &reason,
            None,
            RunEndForOutbox::default()
        ),
        RunEndOutboxClear::Clear
    );
}

/// The same ending with a diagram that does not parse goes through the
/// validation every takeover ending uses: the note is not stored, the row
/// is parked `error`/`detached` with no polling marker, and the note is
/// cleared so a Resume has the agent write it again.
#[test]
fn a_reattached_turn_answered_as_cancelled_rejects_a_note_with_a_broken_diagram() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    let output = published(BROKEN_PIKCHR);
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);

    let run_end = reattached_turn_run_end(
        &store,
        &session_id,
        Some("ws-1"),
        ReattachedTurnEnding::Failed(ANSWERED_AS_CANCELLED),
        &exec,
    );
    let detail = run_end.lost_remote_note_agent.clone().unwrap();
    let submission = run_end.note_submission.clone().unwrap();
    let (status, error, reason) =
        lost_remote_agent_terminal_state(&detail, "ws-1", Some(&submission));
    let message = error.expect("a rejected note says why");
    assert_eq!(
        (status, reason.clone()),
        ("error", CompletionReason::Detached)
    );
    assert!(
        message.contains("does not parse")
            && message.contains("answered the turn's prompt as cancelled")
            && !message.ends_with(AWAITING_REMOTE_NOTE_SUFFIX),
        "{message}"
    );
    assert!(!detached_note_poll_is_pending(&reason, Some(&message)));
    assert_eq!(
        turn_artifact_hooks(
            terminal_state_completed_successfully(status, &reason),
            &reason
        ),
        TurnArtifactHooks::Skip,
        "the broken note never reaches the hooks"
    );
    assert_eq!(store.get_note(&note_id).unwrap().unwrap().content, "");
    assert_eq!(
        remote_outbox_clear_at_run_end(
            run_end.outbox_use,
            &reason,
            Some(&message),
            RunEndForOutbox::default()
        ),
        RunEndOutboxClear::Clear
    );
}

/// With nothing published, or a workstation that does not answer, the
/// failed turn fails as it did before: the row keeps `remote_agent_lost`
/// and its own error. An outbox read as empty is cleared; one that could
/// not be read keeps whatever was published for a Resume.
#[test]
fn a_reattached_turn_answered_as_cancelled_with_no_note_fails_as_before() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, _) = remote_note_session(&store);
    let log = RefCell::new(Vec::new());

    let nothing = reattached_turn_run_end(
        &store,
        &session_id,
        Some("ws-1"),
        ReattachedTurnEnding::Failed(ANSWERED_AS_CANCELLED),
        &recording_exec(NOTHING_PUBLISHED, &log),
    );
    assert_eq!(nothing.note_submission, None);
    assert_eq!(
        nothing.lost_remote_note_agent, None,
        "the run fails with its own error"
    );
    assert_eq!(nothing.outbox_use, read_outbox(false));
    assert_eq!(
        failed_run_completion_reason(ANSWERED_AS_CANCELLED),
        CompletionReason::RemoteAgentLost
    );
    assert_eq!(
        remote_outbox_clear_at_run_end(
            nothing.outbox_use,
            &CompletionReason::RemoteAgentLost,
            Some(ANSWERED_AS_CANCELLED),
            RunEndForOutbox::default()
        ),
        RunEndOutboxClear::Clear
    );

    let unread = reattached_turn_run_end(
        &store,
        &session_id,
        Some("ws-1"),
        ReattachedTurnEnding::Failed(ANSWERED_AS_CANCELLED),
        &unreachable,
    );
    assert_eq!(unread.note_submission, None);
    assert_eq!(unread.lost_remote_note_agent, None);
    assert_eq!(
        remote_outbox_clear_at_run_end(
            unread.outbox_use,
            &CompletionReason::RemoteAgentLost,
            Some(ANSWERED_AS_CANCELLED),
            RunEndForOutbox::default()
        ),
        RunEndOutboxClear::ClearUnlessPublished,
        "a published note nobody could read is never removed"
    );
}

#[test]
fn a_session_without_an_outbox_is_left_alone() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (note_session, _) = remote_note_session(&store);
    assert!(delivers_note_through_outbox(
        &store,
        &note_session,
        Some("ws-1")
    ));
    assert!(
        !delivers_note_through_outbox(&store, &note_session, None),
        "a local note session has no workstation outbox"
    );

    // A commit session on the same workstation.
    let project = Project::new("test/remote-commits");
    store.create_project(&project).unwrap();
    let branch = Branch::new_remote(&project.id, "commits", "main", "ws-1");
    store.create_branch(&branch).unwrap();
    let session = Session::new_running("commit", std::path::Path::new(".")).with_branch(&branch.id);
    store.create_session(&session).unwrap();
    assert!(!delivers_note_through_outbox(
        &store,
        &session.id,
        Some("ws-1")
    ));

    let log = RefCell::new(Vec::new());
    let exec = recording_exec(NOTHING_PUBLISHED, &log);
    assert_eq!(
        reattached_turn_run_end(
            &store,
            &session.id,
            Some("ws-1"),
            ReattachedTurnEnding::Completed,
            &exec
        ),
        TakenOverRunEnd::default()
    );
    assert_eq!(
        reattached_turn_run_end(
            &store,
            &session.id,
            Some("ws-1"),
            ReattachedTurnEnding::Failed(ANSWERED_AS_CANCELLED),
            &exec
        ),
        TakenOverRunEnd::default(),
        "a failed turn of a session with no outbox is not read either"
    );
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
    assert_eq!(
        remote_outbox_clear_at_run_end(
            TakenOverRunEnd::default().outbox_use,
            &CompletionReason::TurnComplete,
            None,
            RunEndForOutbox::default()
        ),
        RunEndOutboxClear::Keep
    );
}

#[test]
fn the_takeover_clears_the_outbox_unless_its_note_could_not_be_stored() {
    let log = RefCell::new(Vec::new());
    let exec = recording_exec("", &log);

    // A note the hooks could not store is the only copy.
    clear_outbox_after_takeover(&exec, "sess-1", true, false);
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());

    // Stored, or read and found unpublished or rejected.
    clear_outbox_after_takeover(&exec, "sess-1", true, true);
    clear_outbox_after_takeover(&exec, "sess-1", false, false);
    let scripts: Vec<String> = log.borrow().iter().map(|args| args[2].clone()).collect();
    assert_eq!(
        scripts,
        vec![clear_outbox_script("sess-1"), clear_outbox_script("sess-1")]
    );
}
