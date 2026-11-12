//! How a remote note session's run ends when it did not read the
//! workstation outbox after its last turn, in the two states the review of
//! ea2ebbb3 found wrong:
//!
//! - the run had already read a note and rejected it (a broken Pikchr
//!   block), and the repair turn ended with no read. The guarded clear kept
//!   the rejected note's `done` marker, and the next Resume's start-of-run
//!   recovery stored the note with no Pikchr check;
//! - the run completed, but the read after its last turn failed. It stored
//!   an empty note and left the row `completed`, with a published note
//!   possibly sitting on the workstation and nothing looking for it.
//!
//! And the two the review of d395ca77 found in those fixes: a repair turn
//! that republished a note that is still broken, which the guarded clear
//! keeps as unread and the next run's recovery then stored unchecked, and a
//! Stop pressed while the second read of the completed run was out, which
//! the settlement from that read overrode.

use super::detached_recovery::{b64, recording_exec, remote_note_session, SETTLED_RUN};
use super::*;
use crate::remote_staged_mcp::note_outbox::{
    clear_outbox_script, clear_unpublished_outbox_script, collect, collect_turn_into_slot,
    new_collected_note_memory, recover_uncollected_outbox, OutboxRecovery,
};
use crate::remote_staged_mcp::test_support::local_exec;
use std::cell::RefCell;

const BROKEN_PIKCHR: &str = "Body\n```pikchr\nboxx \"bad\"\n```";
const FIXED_PIKCHR: &str = "Body\n```pikchr\nbox \"good\"\n```";

/// Publish a note into `sess-1`'s outbox under `home`, as `write_note` does
/// for the run whose turn id is `run-1`.
fn publish(home: &std::path::Path, written_at: u64, body: &str) {
    let outbox = home.join(".staged/sessions/sess-1");
    std::fs::create_dir_all(&outbox).unwrap();
    for (file, content) in [
        ("draft.md", body.to_string()),
        ("note.md", body.to_string()),
        (
            "meta.json",
            format!(r#"{{"title":"Plan","turn":"run-1","writtenAt":{written_at}}}"#),
        ),
        ("done", String::new()),
    ] {
        std::fs::write(outbox.join(file), content).unwrap();
    }
}

/// The outbox use of a run whose last turn ended with no read, given what
/// its collector remembers.
fn unread_after(memory: &note_outbox::CollectedNoteMemory) -> RemoteOutboxUse {
    RemoteOutboxUse {
        own_note_collected: true,
        read_after_last_turn: false,
        rejected_note: rejected_remote_note(memory),
        ..SETTLED_RUN
    }
}

/// What the next run's start-of-run recovery would hand the hooks.
fn recovered_by_the_next_run(home: &std::path::Path) -> (OutboxRecovery, Vec<String>) {
    let exec = local_exec(home);
    let mut ingested = Vec::new();
    let recovery = recover_uncollected_outbox(
        &exec,
        "sess-1",
        &mut |submission| {
            ingested.push(submission.body);
            true
        },
        &mut || None,
    )
    .unwrap();
    (recovery.outbox, ingested)
}

/// Review warning 1, against a real shell. Turn 1 publishes a note with a
/// broken diagram and the run reads it; the repair turn ends in an agent
/// error (a usage limit, expired auth) without republishing and without a
/// read. The end-of-run clear must remove that note: it is known bad, and
/// when this was written the next Resume's recovery stored whatever was
/// published with no Pikchr check.
#[test]
fn a_note_the_run_read_and_rejected_is_not_left_for_the_next_run_to_store() {
    let home = tempfile::tempdir().unwrap();
    let exec = local_exec(home.path());
    let outbox = home.path().join(".staged/sessions/sess-1");
    let slot = crate::note_tool::new_submission_slot();
    let memory = new_collected_note_memory();

    // Turn 1: published, read by the loop, and rejected.
    publish(home.path(), 1, BROKEN_PIKCHR);
    assert!(matches!(
        collect_turn_into_slot(&exec, "sess-1", "run-1", &slot, &memory),
        Ok(OutboxState::Delivered(_))
    ));
    assert!(
        validate_submitted_note_pikchr(&slot).is_some(),
        "the loop answers this note with a repair prompt"
    );

    // The repair turn fails with no read. The run still knows which note it
    // turned down, and its clear names it.
    let outbox_use = unread_after(&memory);
    let rejected = outbox_use
        .rejected_note
        .expect("the run remembers the note it rejected");
    let clear = remote_outbox_clear_at_run_end(
        outbox_use,
        &CompletionReason::Crashed,
        Some("usage limit reached"),
        RunEndForOutbox::default(),
    );
    assert_eq!(clear, RunEndOutboxClear::ClearUnlessRepublished(rejected));
    clear_remote_outbox_at_run_end(&exec, "sess-1", clear);
    assert!(!outbox.exists(), "the rejected note is gone: {outbox:?}");

    // The Resume: nothing waits to be stored unchecked.
    let (recovery, ingested) = recovered_by_the_next_run(home.path());
    assert_eq!(recovery, OutboxRecovery::Empty);
    assert_eq!(ingested, Vec::<String>::new());

    // The same ending for every way a repair turn ends unread: an agent that
    // cancelled its own turn, and a quit's cancel.
    for (reason, message) in [
        (CompletionReason::Interrupted, None),
        (CompletionReason::AppQuit, None),
        (
            CompletionReason::RemoteAgentLost,
            Some("agent process gone"),
        ),
    ] {
        assert_eq!(
            remote_outbox_clear_at_run_end(
                outbox_use,
                &reason,
                message,
                RunEndForOutbox::default()
            ),
            RunEndOutboxClear::ClearUnlessRepublished(rejected),
            "{reason:?}"
        );
    }
}

/// The other side of the same rule: only the note the run rejected goes. A
/// note the repair turn published before it failed is one nobody has read,
/// and a note the run read and found valid was never rejected.
#[test]
fn a_note_published_after_the_rejected_one_or_never_rejected_still_stays() {
    let home = tempfile::tempdir().unwrap();
    let exec = local_exec(home.path());
    let slot = crate::note_tool::new_submission_slot();
    let memory = new_collected_note_memory();

    // Rejected, then republished by the repair turn, which then failed.
    publish(home.path(), 1, BROKEN_PIKCHR);
    collect_turn_into_slot(&exec, "sess-1", "run-1", &slot, &memory).unwrap();
    publish(home.path(), 7, FIXED_PIKCHR);
    let clear = remote_outbox_clear_at_run_end(
        unread_after(&memory),
        &CompletionReason::Crashed,
        Some("usage limit reached"),
        RunEndForOutbox::default(),
    );
    assert!(matches!(
        clear,
        RunEndOutboxClear::ClearUnlessRepublished(_)
    ));
    clear_remote_outbox_at_run_end(&exec, "sess-1", clear);
    let (recovery, ingested) = recovered_by_the_next_run(home.path());
    assert_eq!(recovery, OutboxRecovery::Ingested);
    assert_eq!(ingested, vec![FIXED_PIKCHR.to_string()]);

    // A valid note the run read (the watchdog's probe found it) before its
    // turn went on and failed unread: not rejected, so the marker decides.
    let memory = new_collected_note_memory();
    publish(home.path(), 9, FIXED_PIKCHR);
    collect_turn_into_slot(&exec, "sess-1", "run-1", &slot, &memory).unwrap();
    let outbox_use = unread_after(&memory);
    assert_eq!(outbox_use.rejected_note, None);
    let clear = remote_outbox_clear_at_run_end(
        outbox_use,
        &CompletionReason::RemoteAgentLost,
        Some("agent process gone"),
        RunEndForOutbox::default(),
    );
    assert_eq!(clear, RunEndOutboxClear::ClearUnlessPublished);
    clear_remote_outbox_at_run_end(&exec, "sess-1", clear);
    assert!(matches!(
        collect(&exec, "sess-1", None).unwrap(),
        OutboxState::Delivered(submission) if submission.body == FIXED_PIKCHR
    ));

    // A run that read no note at all rejected none.
    assert_eq!(rejected_remote_note(&new_collected_note_memory()), None);
}

/// A rejected note never outranks the states that keep the outbox or clear
/// it outright.
#[test]
fn the_rejected_note_only_changes_the_unread_ending() {
    let home = tempfile::tempdir().unwrap();
    let exec = local_exec(home.path());
    let slot = crate::note_tool::new_submission_slot();
    let memory = new_collected_note_memory();
    publish(home.path(), 1, BROKEN_PIKCHR);
    collect_turn_into_slot(&exec, "sess-1", "run-1", &slot, &memory).unwrap();
    let unread = unread_after(&memory);

    // Read after the last turn (the repair loop ran out): cleared outright.
    assert_eq!(
        remote_outbox_clear_at_run_end(
            RemoteOutboxUse {
                read_after_last_turn: true,
                ..unread
            },
            &CompletionReason::Crashed,
            Some("Pikchr validation failed after 2 repair attempts"),
            RunEndForOutbox::default()
        ),
        RunEndOutboxClear::Clear
    );
    // A Stop discards the turn's output whatever it was.
    assert_eq!(
        remote_outbox_clear_at_run_end(
            unread,
            &CompletionReason::Interrupted,
            None,
            RunEndForOutbox {
                user_stopped: true,
                ..RunEndForOutbox::default()
            }
        ),
        RunEndOutboxClear::Clear
    );
    // A row the sweep still polls keeps everything.
    let awaiting = awaiting_remote_note_message("ws-1", Some("incoming transport closed"));
    assert_eq!(
        remote_outbox_clear_at_run_end(
            unread,
            &CompletionReason::Detached,
            Some(&awaiting),
            RunEndForOutbox::default()
        ),
        RunEndOutboxClear::Keep
    );
}

/// A valid note the run collected for a row that then did not complete (a
/// quit's cancel between the read and the terminal state) was read, but
/// neither stored nor rejected: the clear leaves it for the next run. A
/// Stop the user asked for still discards it.
#[test]
fn a_valid_note_the_hooks_never_saw_is_not_cleared_as_read() {
    let hooks_skipped = RunEndForOutbox {
        collected_note_hooks_skipped: true,
        ..RunEndForOutbox::default()
    };
    assert_eq!(
        remote_outbox_clear_at_run_end(
            RemoteOutboxUse {
                own_note_collected: true,
                ..SETTLED_RUN
            },
            &CompletionReason::AppQuit,
            None,
            hooks_skipped
        ),
        RunEndOutboxClear::Keep
    );
    assert_eq!(
        remote_outbox_clear_at_run_end(
            RemoteOutboxUse {
                own_note_collected: true,
                ..SETTLED_RUN
            },
            &CompletionReason::Interrupted,
            None,
            RunEndForOutbox {
                user_stopped: true,
                ..hooks_skipped
            }
        ),
        RunEndOutboxClear::Clear
    );
}

/// What the collect script prints once `write_note` published `body` in the
/// run whose turn id is `run-1`.
fn published(body: &str) -> String {
    let meta = serde_json::json!({ "title": "Findings", "turn": "run-1", "writtenAt": 1 });
    format!(
        "done\n{}\n{}\ndone\n",
        b64(meta.to_string().as_bytes()),
        b64(body.as_bytes())
    )
}

const NOTHING_PUBLISHED: &str = "pending\n\n\npending\n";
const READ_FAILED: &str =
    "failed to read the remote note outbox: ssh: connect to host ws-1: Operation timed out";

fn unreachable(args: &[&str]) -> Result<String, String> {
    let _ = args;
    Err("ssh: connect to host ws-1: Operation timed out".to_string())
}

/// What a completed run does after its loop when the read after its last
/// turn failed, in the runner's order: it reads the outbox once more, into
/// its slot, asks whether the user pressed Stop while that read was out
/// (`stopped_during_read`), and settles from the two. Gives whether the
/// outbox was read and how the run ends. A run that does not see the failed
/// read reads nothing more and completes with what its slot holds.
fn second_read(
    exec: remote_staged_mcp::RemoteExec,
    session_id: &str,
    slot: &NoteSubmissionSlot,
    stopped_during_read: &dyn Fn() -> bool,
) -> (bool, UnreadOutboxSettlement) {
    let Some(read_error) =
        completed_run_outbox_unread(&Ok(AgentRunOutcome::Completed), false, Some(READ_FAILED))
    else {
        return (false, UnreadOutboxSettlement::Complete);
    };
    let retried = collect_turn_into_slot(
        exec,
        session_id,
        "run-1",
        slot,
        &new_collected_note_memory(),
    );
    (
        retried.is_ok(),
        unread_outbox_settlement(&read_error, &retried, stopped_during_read()),
    )
}

/// The settlement of a run whose second read decided it from the outbox.
fn from_outbox(settlement: UnreadOutboxSettlement) -> OutboxSettlement {
    match settlement {
        UnreadOutboxSettlement::FromOutbox(detail) => OutboxSettlement::TurnEndedUnread(detail),
        other => panic!("the run settles from the outbox, not {other:?}"),
    }
}

/// Only a run that completed, unstopped, with its last turn's read failed is
/// in the state.
#[test]
fn only_a_completed_run_whose_last_read_failed_has_an_unread_outbox() {
    let completed = Ok(AgentRunOutcome::Completed);
    assert_eq!(
        completed_run_outbox_unread(&completed, false, Some(READ_FAILED)).as_deref(),
        Some(READ_FAILED)
    );
    assert_eq!(
        completed_run_outbox_unread(&completed, false, None),
        None,
        "the outbox was read"
    );
    assert_eq!(
        completed_run_outbox_unread(&completed, true, Some(READ_FAILED)),
        None,
        "a Stop discards the turn's output"
    );
    for other in [
        Ok(AgentRunOutcome::Cancelled),
        Err(AgentRunError::Other("auth required".to_string())),
    ] {
        assert_eq!(
            completed_run_outbox_unread(&other, false, Some(READ_FAILED)),
            None,
            "{other:?} does not store a note"
        );
    }
}

/// Review warning 2. The workstation did not answer the read after the
/// run's last turn, nor the second read after the run. The run used to
/// complete and store an empty note. It now parks the row for the detached
/// sweep, keeps the outbox, and the sweep stores the published note once
/// the workstation answers.
#[test]
fn a_completed_run_that_cannot_read_its_outbox_is_parked_for_the_sweep() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    let slot = crate::note_tool::new_submission_slot();

    let (read, settlement) = second_read(&unreachable, &session_id, &slot, &|| false);
    assert!(!read);
    // An outbox nobody could read is not judged empty.
    let settlement = from_outbox(settlement);
    assert!(crate::note_tool::current_submission(&slot).is_none());

    // The terminal state `finish_session_run` writes for that settlement.
    let (status, error, reason) = settlement.terminal_state("ws-1", None);
    assert_eq!(
        (status, reason.clone()),
        ("error", CompletionReason::Detached)
    );
    let message = error.expect("a parked row says why");
    // Its own wording: the turn ended and no connection was lost. The
    // polling marker is still the message's tail.
    assert!(
        message.starts_with("This session's turn ended on ws-1")
            && message.contains(UNREAD_OUTBOX_DETAIL_MARKER)
            && message.contains("Operation timed out")
            && message.ends_with(AWAITING_REMOTE_NOTE_SUFFIX)
            && !message.contains("lost the connection"),
        "{message}"
    );
    assert!(detached_note_poll_is_pending(&reason, Some(&message)));
    assert_eq!(
        turn_artifact_hooks(
            terminal_state_completed_successfully(status, &reason),
            &reason
        ),
        TurnArtifactHooks::Skip,
        "no empty note is stored"
    );
    // The outbox is kept: the read never happened.
    assert_eq!(
        remote_outbox_clear_at_run_end(
            RemoteOutboxUse {
                read_after_last_turn: read,
                ..SETTLED_RUN
            },
            &reason,
            Some(&message),
            RunEndForOutbox::default()
        ),
        RunEndOutboxClear::Keep
    );

    // The row as the run leaves it, and the sweep's next visit with the
    // workstation back: the published note is stored and the row completes.
    assert!(store
        .transition_from_running(
            &session_id,
            SessionStatus::Error,
            Some(&message),
            Some(&reason)
        )
        .unwrap());
    let row = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(
        detached_note_poll_action(&row, row.updated_at),
        DetachedNotePollAction::Poll
    );
    let output = published("Published before the workstation dropped.");
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);
    let polled = poll_detached_remote_note_session_once(
        &store,
        &session_id,
        "ws-1",
        &exec,
        &mut |submission| {
            run_post_completion_hooks(
                &session_id,
                std::path::Path::new("."),
                None,
                None,
                None,
                &store,
                0,
                Some(submission),
            )
            .note_stored
        },
        &ObservedDetachedRow::of(&row),
    );
    assert!(
        matches!(
            &polled,
            DetachedNotePoll::Settled(outcome) if outcome.status == SessionStatus::Completed
        ),
        "{polled:?}"
    );
    let note = store.get_note(&note_id).unwrap().unwrap();
    assert_eq!(note.title, "Findings");
    assert_eq!(note.content, "Published before the workstation dropped.");
    assert_eq!(
        log.borrow().last().unwrap()[2],
        clear_outbox_script(&session_id)
    );
}

/// The second read going through settles the run on what it found: a valid
/// note is stored, a broken one is rejected and cleared, and an outbox read
/// as empty lets the turn complete as one that delivered nothing.
#[test]
fn a_second_read_that_goes_through_decides_the_completed_run() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    let log = RefCell::new(Vec::new());
    let read_use = RemoteOutboxUse {
        own_note_collected: true,
        ..SETTLED_RUN
    };

    // A valid note: the session completes from it.
    let output = published("Published; the first read failed.");
    let slot = crate::note_tool::new_submission_slot();
    let (read, settlement) =
        second_read(&recording_exec(&output, &log), &session_id, &slot, &|| {
            false
        });
    assert!(read);
    // A note found after the loop is checked before it is stored.
    let settlement = from_outbox(settlement);
    let submission = crate::note_tool::current_submission(&slot).unwrap();
    let (status, error, reason) = settlement.terminal_state("ws-1", Some(&submission));
    assert_eq!(
        (status, error, reason.clone()),
        ("completed", None, CompletionReason::TurnComplete)
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
    assert_eq!(
        store.get_note(&note_id).unwrap().unwrap().content,
        "Published; the first read failed."
    );
    assert_eq!(
        remote_outbox_clear_at_run_end(read_use, &reason, None, RunEndForOutbox::default()),
        RunEndOutboxClear::Clear
    );

    // A note with a broken diagram never went through the loop's check: it
    // is not stored, the row says so, and the note is cleared.
    let output = published(BROKEN_PIKCHR);
    let slot = crate::note_tool::new_submission_slot();
    let (_, settlement) = second_read(&recording_exec(&output, &log), &session_id, &slot, &|| {
        false
    });
    let submission = crate::note_tool::current_submission(&slot).unwrap();
    let (status, error, reason) = from_outbox(settlement).terminal_state("ws-1", Some(&submission));
    let message = error.unwrap();
    assert_eq!(
        (status, reason.clone()),
        ("error", CompletionReason::Detached)
    );
    assert!(
        message.starts_with("This session's turn ended on ws-1")
            && message.contains("does not parse")
            && !message.contains("lost the connection")
            && !message.ends_with(AWAITING_REMOTE_NOTE_SUFFIX),
        "{message}"
    );
    assert_eq!(
        remote_outbox_clear_at_run_end(
            read_use,
            &reason,
            Some(&message),
            RunEndForOutbox::default()
        ),
        RunEndOutboxClear::Clear
    );

    // Nothing published: the outbox was read, so the run completes as a
    // turn that delivered nothing does.
    let slot = crate::note_tool::new_submission_slot();
    let (read, settlement) = second_read(
        &recording_exec(NOTHING_PUBLISHED, &log),
        &session_id,
        &slot,
        &|| false,
    );
    assert!(read);
    assert_eq!(settlement, UnreadOutboxSettlement::Complete);
    assert_eq!(
        remote_outbox_clear_at_run_end(
            SETTLED_RUN,
            &CompletionReason::TurnComplete,
            None,
            RunEndForOutbox::default()
        ),
        RunEndOutboxClear::Clear
    );
    assert!(clear_unpublished_outbox_script(&session_id, None).contains("done"));
}

/// Review of d395ca77, warning 1. The stamp only recognises the note the
/// run rejected. A repair turn that publishes again, with a diagram that is
/// still broken, and then ends with no read (a usage limit, expired auth)
/// leaves a note with a new `writtenAt`: the guarded clear keeps it as one
/// nobody has read, and the next Resume's start-of-run recovery used to
/// store it with no Pikchr check. The recovery now checks the note it finds
/// and discards one that does not pass. Against a real shell.
#[test]
fn a_still_broken_note_the_repair_turn_republished_is_never_stored_unchecked() {
    const STILL_BROKEN: &str = "Body, second try\n```pikchr\nboxx \"still bad\"\n```";
    // A different body, and the very same body published again a few
    // seconds later (`meta.json` is part of the stamp).
    for republished in [STILL_BROKEN, BROKEN_PIKCHR] {
        let home = tempfile::tempdir().unwrap();
        let exec = local_exec(home.path());
        let outbox = home.path().join(".staged/sessions/sess-1");
        let slot = crate::note_tool::new_submission_slot();
        let memory = new_collected_note_memory();

        // Turn 1: published, read by the loop, and rejected.
        publish(home.path(), 1, BROKEN_PIKCHR);
        collect_turn_into_slot(&exec, "sess-1", "run-1", &slot, &memory).unwrap();
        // The repair turn republishes, then fails before the run reads it.
        publish(home.path(), 7, republished);
        let clear = remote_outbox_clear_at_run_end(
            unread_after(&memory),
            &CompletionReason::Crashed,
            Some("usage limit reached"),
            RunEndForOutbox::default(),
        );
        clear_remote_outbox_at_run_end(&exec, "sess-1", clear);
        assert!(
            outbox.join("done").exists(),
            "a note published since the rejected one is kept as unread"
        );

        // The Resume: the recovery finds the note, checks it, and does not
        // hand it to the hooks.
        let (recovery, ingested) = recovered_by_the_next_run(home.path());
        assert_eq!(recovery, OutboxRecovery::Discarded, "{republished:?}");
        assert_eq!(ingested, Vec::<String>::new(), "{republished:?}");
        // Discarded means cleared: the run that is starting does not trip
        // over it, and nothing later reads it.
        for file in ["done", "note.md", "meta.json", "draft.md"] {
            assert!(!outbox.join(file).exists(), "{file} is gone");
        }
    }
}

/// Review of d395ca77, warning 2. The Stop was sampled before the second
/// read of a completed run's unread outbox, and that read is a round trip
/// to a workstation that had just failed to answer, with the row still
/// `running`. A Stop pressed meanwhile was recorded by nobody, and the
/// settlement from the read outranked the cancel: the note was stored, or
/// the row was parked for the sweep to store it later. The Stop now wins
/// whatever the read returned.
#[test]
fn a_stop_during_the_second_read_ends_the_run_as_stopped() {
    let registry = SessionRegistry::new();
    let valid = published("Published; the first read failed.");
    let broken = published(BROKEN_PIKCHR);
    for (session_id, output) in [
        ("sess-valid", Ok(valid.clone())),
        ("sess-broken", Ok(broken)),
        ("sess-unreachable", Err("ssh: timed out".to_string())),
    ] {
        let (token, _) = registry.register(session_id);
        assert!(!user_stop_requested(&token, &registry, session_id));
        // The user presses Stop while the read is out.
        let exec = |_: &[&str]| -> Result<String, String> {
            assert!(registry.cancel(session_id));
            output.clone()
        };
        let slot = crate::note_tool::new_submission_slot();
        let (_, settlement) = second_read(&exec, session_id, &slot, &|| {
            user_stop_requested(&token, &registry, session_id)
        });
        assert_eq!(
            settlement,
            UnreadOutboxSettlement::Stopped,
            "{session_id}: the run ends as a stopped one"
        );
    }
    // The stopped run ends `cancelled` with no hooks, and its clear discards
    // the outbox, a read and valid note included.
    assert_eq!(
        remote_outbox_clear_at_run_end(
            RemoteOutboxUse {
                own_note_collected: true,
                ..SETTLED_RUN
            },
            &CompletionReason::Interrupted,
            None,
            RunEndForOutbox {
                user_stopped: true,
                collected_note_hooks_skipped: true,
                ..RunEndForOutbox::default()
            },
        ),
        RunEndOutboxClear::Clear
    );

    // A quit that lands during the read is not a Stop: the run settles from
    // the read, and the sweep recovers a parked row after the relaunch.
    let (token, _) = registry.register("sess-quit");
    let exec = |_: &[&str]| -> Result<String, String> {
        assert!(registry.cancel_with_completion_reason("sess-quit", CompletionReason::AppQuit));
        Ok(valid.clone())
    };
    let slot = crate::note_tool::new_submission_slot();
    let (_, settlement) = second_read(&exec, "sess-quit", &slot, &|| {
        user_stop_requested(&token, &registry, "sess-quit")
    });
    assert!(token.is_cancelled());
    assert!(
        matches!(settlement, UnreadOutboxSettlement::FromOutbox(_)),
        "{settlement:?}"
    );
}
