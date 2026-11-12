//! A turn whose `sq` proxy went while bloxlet still ran the agent is picked
//! back up inside the run's turn loop, so its end is an ordinary turn end: a
//! remote note session's published note is collected from the workstation
//! outbox, checked, and stored by the hooks.
//!
//! The lost note this guards against: the reattach used to run after the
//! loop, where the submission slot had already been read and nothing
//! collected the outbox again. The run then stored an empty note and cleared
//! the published one off the workstation.

use super::detached_recovery::{b64, is_collect, recording_exec, remote_note_session};
use super::*;
use crate::remote_staged_mcp::note_outbox::{
    clear_outbox_script, collect_turn_into_slot, new_collected_note_memory,
};
use std::cell::RefCell;

/// The error of a connection whose proxy went while bloxlet still runs the
/// agent.
const PROXY_WENT: &str = "Remote agent detached: the `sq blox acp` proxy exited, but bloxlet \
                          still runs the agent";

/// The connection settled failed, as it does when its proxy goes.
const FAILED: Option<SessionSettleReason> = Some(SessionSettleReason::Failed);

/// What the collect script prints once `write_note` published `body` in the
/// run whose turn id is `turn`.
fn published(turn: &str, title: &str, body: &str) -> String {
    let meta = serde_json::json!({ "title": title, "turn": turn, "writtenAt": 1 });
    format!(
        "done\n{}\n{}\ndone\n",
        b64(meta.to_string().as_bytes()),
        b64(body.as_bytes())
    )
}

/// A reattach that must not be asked for.
async fn no_reattach(error: String) -> Option<ReattachEnd> {
    panic!("a turn that ended with {error:?} is not picked back up")
}

#[tokio::test]
async fn a_turn_whose_proxy_went_ends_as_its_reattach_ended() {
    let asked = RefCell::new(Vec::new());
    let picked_up = pick_up_detached_turn(Err(PROXY_WENT.to_string()), false, FAILED, |error| {
        asked.borrow_mut().push(error);
        async { Some(ReattachEnd::of(Ok(AgentRunOutcome::Completed))) }
    })
    .await;

    assert_eq!(picked_up.result, Ok(AgentRunOutcome::Completed));
    assert_eq!(*asked.borrow(), vec![PROXY_WENT.to_string()]);
    let settlement = picked_up
        .reattached
        .expect("the reattach's settlement replaces the dead connection's");
    assert_eq!(
        settlement.settle_reason, None,
        "the failed settle of the connection that went is not the turn's"
    );
    assert!(settlement.transport_loss.is_none());
    assert!(!settlement.cancel_unacknowledged && !settlement.background_work_left);
    // So the turn loop goes on to its collection and checks.
    assert!(should_validate_pikchr_after_turn(&picked_up.result, false));
}

#[tokio::test]
async fn a_reattach_that_ends_otherwise_hands_the_run_that_ending() {
    // A Stop the agent may not have had, while the reattach waited.
    let picked_up = pick_up_detached_turn(Err(PROXY_WENT.to_string()), false, FAILED, |_| async {
        Some(ReattachEnd {
            result: Ok(AgentRunOutcome::Cancelled),
            cancel_unacknowledged: true,
        })
    })
    .await;
    assert_eq!(picked_up.result, Ok(AgentRunOutcome::Cancelled));
    assert!(picked_up.reattached.unwrap().cancel_unacknowledged);

    // The reattach lost the agent again: its error is the run's.
    let picked_up = pick_up_detached_turn(Err(PROXY_WENT.to_string()), false, FAILED, |_| async {
        Some(ReattachEnd::of(
            Err("Incoming transport closed".to_string()),
        ))
    })
    .await;
    assert_eq!(
        picked_up.result,
        Err("Incoming transport closed".to_string())
    );
    assert!(picked_up.reattached.is_some());
}

#[tokio::test]
async fn a_turn_that_is_not_picked_up_keeps_its_result() {
    // A quit in progress, or a prompt that never reached the agent: the
    // reattach declines and the connection's error stands.
    let picked_up = pick_up_detached_turn(Err(PROXY_WENT.to_string()), false, FAILED, |_| async {
        None
    })
    .await;
    assert_eq!(picked_up.result, Err(PROXY_WENT.to_string()));
    assert!(picked_up.reattached.is_none());

    for (result, stopped, settle_reason) in [
        // The turn ended on its own connection.
        (Ok(AgentRunOutcome::Completed), false, None),
        (Ok(AgentRunOutcome::Cancelled), true, None),
        // The agent itself was lost, not just the proxy.
        (Err("Incoming transport closed".to_string()), false, FAILED),
        // A Stop ends the turn.
        (Err(PROXY_WENT.to_string()), true, FAILED),
        // A quit's detach leaves it running.
        (
            Err(PROXY_WENT.to_string()),
            false,
            Some(SessionSettleReason::Detached),
        ),
    ] {
        let picked_up =
            pick_up_detached_turn(result.clone(), stopped, settle_reason, no_reattach).await;
        assert_eq!(picked_up.result, result);
        assert!(picked_up.reattached.is_none());
    }
}

/// Why the note was lost: a connection whose proxy went is not a lost agent,
/// so the collection the run makes after its loop never read the outbox for
/// it. Only the turn loop's own collection can, which is why the reattach
/// has to end inside the loop.
#[test]
fn a_detached_proxy_is_not_a_lost_agent_the_post_run_collection_rescues() {
    assert_eq!(
        lost_remote_agent_detail(
            &Err(AgentRunError::Other(PROXY_WENT.to_string())),
            FAILED,
            false,
            None,
            UnacknowledgedCancelContext {
                stop_requested: false,
                agent_idle: None,
            },
        ),
        None
    );
}

/// Scenario 9 of the live run, with the steps the turn loop and the end of
/// the run take in order: the agent published its note, the proxy died, the
/// reattach saw the turn end. The note is stored and only then cleared.
#[tokio::test]
async fn a_note_published_before_the_proxy_died_is_stored_when_the_reattached_turn_ends() {
    let store = Arc::new(Store::in_memory().unwrap());
    let (session_id, note_id) = remote_note_session(&store);
    let output = published("run-1", "Findings", "Published before the proxy died.");
    let log = RefCell::new(Vec::new());
    let exec = recording_exec(&output, &log);
    let slot = crate::note_tool::new_submission_slot();
    let memory = new_collected_note_memory();

    // The turn's connection ends with the proxy gone; the reattach follows
    // the turn to its end.
    let picked_up = pick_up_detached_turn(Err(PROXY_WENT.to_string()), false, FAILED, |_| async {
        Some(ReattachEnd::of(Ok(AgentRunOutcome::Completed)))
    })
    .await;
    assert!(
        log.borrow().is_empty(),
        "nothing has read the outbox yet, so nothing may clear it"
    );

    // The loop's collection for a completed turn, into the run's slot.
    assert!(should_validate_pikchr_after_turn(&picked_up.result, false));
    assert!(matches!(
        collect_turn_into_slot(&exec, &session_id, "run-1", &slot, &memory),
        Ok(OutboxState::Delivered(_))
    ));
    assert!(validate_submitted_note_pikchr(&slot).is_none());
    assert!(note_turn_delivered(&store, &session_id, 0, &slot));

    // The end of the run: the hooks store what the slot holds, and the
    // outbox is cleared because it was read after the turn ended.
    let hooks = run_post_completion_hooks(
        &session_id,
        std::path::Path::new("."),
        None,
        None,
        None,
        &store,
        0,
        crate::note_tool::current_submission(&slot),
    );
    assert!(hooks.note_stored);
    let note = store.get_note(&note_id).unwrap().unwrap();
    assert_eq!(note.title, "Findings");
    assert_eq!(note.content, "Published before the proxy died.");
    assert!(note.completed_at.is_some());

    let clear = remote_outbox_clear_at_run_end(
        RemoteOutboxUse {
            staged_mcp_attached: true,
            recovery_settled: true,
            own_note_collected: true,
            read_after_last_turn: true,
        },
        &CompletionReason::TurnComplete,
        None,
        RunEndForOutbox::default(),
    );
    assert_eq!(clear, RunEndOutboxClear::Clear);
    clear_remote_outbox_at_run_end(&exec, &session_id, clear);
    let commands = log.borrow();
    assert_eq!(commands.len(), 2, "collect, then clear: {commands:?}");
    assert!(is_collect(
        &commands[0].iter().map(String::as_str).collect::<Vec<_>>()
    ));
    assert_eq!(commands[1][2], clear_outbox_script(&session_id));
}
