//! The mark the remaining `sessions` writers leave on the session feed: the
//! insert, and the fields the session stream's frames carry beside the ones
//! `session_feed_tests.rs` covers. The bulk writers (a branch move, a project
//! transfer, a note move) are tested beside their own fixtures.

use std::path::Path;

use serde_json::json;

use super::models::*;
use super::session_feed_tests::{
    assert_marked, assert_unmarked, queued_session, running_session, session_row,
    session_with_status, store_with_feed, updated_at,
};

#[test]
fn create_session_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = Session::new_queued("work");

    store.create_session(&session).unwrap();

    assert_marked(&feed, &session, session_row());
}

#[test]
fn prepare_queued_session_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = queued_session(&store, &feed);

    store
        .prepare_queued_session(&session.id, "/wt/feature", "the full prompt")
        .unwrap();

    assert_marked(&feed, &session, session_row());
    let stored = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(stored.working_dir, "/wt/feature");
}

#[test]
fn prepare_immediate_branch_session_marks_the_session_it_claims() {
    let (store, feed) = store_with_feed();
    let session = Session::new_running("work", Path::new("/tmp")).with_agent("agent-1");
    store.create_session(&session).unwrap();
    feed.drain();

    // Another agent id: the row is not this caller's, and is not written.
    assert!(!store
        .prepare_immediate_branch_session(&session.id, "agent-2", "/wt/feature", "prompt")
        .unwrap());
    assert_unmarked(&feed);

    assert!(store
        .prepare_immediate_branch_session(&session.id, "agent-1", "/wt/feature", "prompt")
        .unwrap());
    assert_marked(&feed, &session, session_row());
}

#[test]
fn set_agent_session_id_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    store.set_agent_session_id(&session.id, "acp-1").unwrap();

    assert_marked(&feed, &session, session_row());
}

#[test]
fn set_session_provider_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    store.set_session_provider(&session.id, "claude").unwrap();

    assert_marked(&feed, &session, session_row());
}

#[test]
fn set_session_pre_head_sha_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    store
        .set_session_pre_head_sha(&session.id, Some("abc123"))
        .unwrap();

    assert_marked(&feed, &session, session_row());
}

#[test]
fn set_session_blox_command_id_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    store
        .set_session_blox_command_id(&session.id, Some("cmd-1"))
        .unwrap();

    assert_marked(&feed, &session, session_row());
}

#[test]
fn set_session_acp_config_selection_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    store
        .set_session_acp_config_selection(&session.id, Some(&AcpConfigSelection::default()))
        .unwrap();

    assert_marked(&feed, &session, session_row());
}

#[test]
fn merge_session_provider_metadata_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    store
        .merge_session_provider_metadata(&session.id, &json!({ "rateLimit": { "used": 3 } }))
        .unwrap();

    assert_marked(&feed, &session, session_row());
}

#[test]
fn mark_completion_effects_ran_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = session_with_status(&store, &feed, SessionStatus::Completed);

    store.mark_completion_effects_ran(&session.id).unwrap();

    assert_marked(&feed, &session, session_row());
}

#[test]
fn mark_hold_stopped_note_recovery_observed_marks_the_session_it_writes() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);
    store
        .update_session_status(
            &session.id,
            SessionStatus::Completed,
            None,
            Some(&CompletionReason::HoldStopped),
        )
        .unwrap();
    feed.drain();
    let observed = updated_at(&store, &session);

    // The row is not as the caller read it: nothing is written.
    assert!(store
        .mark_hold_stopped_note_recovery_observed(&session.id, observed, Some("moved on"), None)
        .unwrap()
        .is_none());
    assert_unmarked(&feed);

    assert!(store
        .mark_hold_stopped_note_recovery_observed(&session.id, observed, None, None)
        .unwrap()
        .is_some());
    assert_marked(&feed, &session, session_row());
}

#[test]
fn a_field_write_to_a_missing_session_marks_nothing() {
    let (store, feed) = store_with_feed();

    store
        .prepare_queued_session("missing", "/wt/feature", "prompt")
        .unwrap();
    store.set_agent_session_id("missing", "acp-1").unwrap();
    store.set_session_provider("missing", "claude").unwrap();
    store
        .set_session_pre_head_sha("missing", Some("abc123"))
        .unwrap();
    store
        .set_session_blox_command_id("missing", Some("cmd-1"))
        .unwrap();
    store
        .set_session_acp_config_selection("missing", None)
        .unwrap();
    store
        .merge_session_provider_metadata("missing", &json!({ "a": 1 }))
        .unwrap();
    store.mark_completion_effects_ran("missing").unwrap();

    assert_unmarked(&feed);
}
