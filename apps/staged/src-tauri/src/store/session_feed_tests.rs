//! The session feed, and the mark each session writer leaves on it.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use serde_json::json;

use super::models::*;
use super::{SessionDirty, SessionFeed, Store};

// =============================================================================
// Helpers
// =============================================================================

/// A store with an active feed attached.
fn store_with_feed() -> (Store, Arc<SessionFeed>) {
    let feed = Arc::new(SessionFeed::default());
    feed.set_active(true);
    let store = Store::in_memory().unwrap().with_session_feed(feed.clone());
    (store, feed)
}

/// A running session, with the feed drained of anything its setup marked.
fn running_session(store: &Store, feed: &SessionFeed) -> Session {
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    feed.drain();
    session
}

fn queued_session(store: &Store, feed: &SessionFeed) -> Session {
    let session = Session::new_queued("work");
    store.create_session(&session).unwrap();
    feed.drain();
    session
}

/// A running session moved to `status` by a plain status write.
fn session_with_status(store: &Store, feed: &SessionFeed, status: SessionStatus) -> Session {
    let session = running_session(store, feed);
    store
        .update_session_status(&session.id, status, None, None)
        .unwrap();
    feed.drain();
    session
}

/// A session the dead-owner sweep has claimed: `error`/`detached`.
fn detached_session(store: &Store, feed: &SessionFeed) -> Session {
    let session = running_session(store, feed);
    assert!(store
        .transition_from_running(
            &session.id,
            SessionStatus::Error,
            Some("polling"),
            Some(&CompletionReason::Detached),
        )
        .unwrap());
    feed.drain();
    session
}

/// A session paused on a rate limit.
fn rate_limited_session(store: &Store, feed: &SessionFeed) -> Session {
    let session = running_session(store, feed);
    assert!(store
        .transition_running_to_rate_limited(&session.id, Some("limited"), None, None)
        .unwrap());
    feed.drain();
    session
}

/// A session with one follow-up queued behind it.
fn session_with_queued_message(
    store: &Store,
    feed: &SessionFeed,
) -> (Session, QueuedSessionMessage) {
    let session = running_session(store, feed);
    let queued = store
        .add_queued_session_message(&session.id, "next", &[], None)
        .unwrap();
    feed.drain();
    (session, queued)
}

/// [`session_with_queued_message`] with the turn over and the follow-up
/// claimed for sending.
fn session_with_claimed_message(
    store: &Store,
    feed: &SessionFeed,
) -> (Session, QueuedSessionMessage) {
    let (session, queued) = session_with_queued_message(store, feed);
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    store
        .claim_queued_session_message(&queued.id)
        .unwrap()
        .unwrap();
    feed.drain();
    (session, queued)
}

fn updated_at(store: &Store, session: &Session) -> i64 {
    store.get_session(&session.id).unwrap().unwrap().updated_at
}

fn metadata(kind: &str) -> AcpMessageMetadata {
    AcpMessageMetadata {
        acp_event_kind: Some(kind.to_string()),
        ..Default::default()
    }
}

fn rows(ids: &[i64]) -> SessionDirty {
    SessionDirty {
        rows: BTreeSet::from_iter(ids.iter().copied()),
        ..Default::default()
    }
}

fn metadata_rows(ids: &[i64]) -> SessionDirty {
    SessionDirty {
        metadata_rows: BTreeSet::from_iter(ids.iter().copied()),
        ..Default::default()
    }
}

fn session_row() -> SessionDirty {
    SessionDirty {
        session: true,
        ..Default::default()
    }
}

fn queue() -> SessionDirty {
    SessionDirty {
        queue: true,
        ..Default::default()
    }
}

/// Drain the feed and require that exactly `session` was marked, with
/// exactly `expected`.
#[track_caller]
fn assert_marked(feed: &SessionFeed, session: &Session, expected: SessionDirty) {
    assert_eq!(
        feed.drain(),
        HashMap::from([(session.id.clone(), expected)])
    );
}

#[track_caller]
fn assert_unmarked(feed: &SessionFeed) {
    assert_eq!(feed.drain(), HashMap::new());
}

/// Whether a wake-up is waiting to be taken, without waiting for one.
async fn wake_pending(feed: &SessionFeed) -> bool {
    tokio::select! {
        biased;
        _ = feed.marked() => true,
        _ = std::future::ready(()) => false,
    }
}

// =============================================================================
// The feed
// =============================================================================

#[test]
fn feed_starts_inactive_and_records_nothing() {
    let feed = SessionFeed::default();
    assert!(!feed.is_active());

    feed.mark_row("s", 1);
    feed.mark_metadata_row("s", 2);
    feed.mark_session("s");
    feed.mark_queue("s");

    assert_eq!(feed.drain(), HashMap::new());
}

#[test]
fn marks_coalesce_per_session_and_a_drain_takes_them() {
    let feed = SessionFeed::default();
    feed.set_active(true);

    feed.mark_row("a", 2);
    feed.mark_row("a", 1);
    feed.mark_row("a", 2);
    feed.mark_metadata_row("a", 3);
    feed.mark_session("a");
    feed.mark_queue("b");

    assert_eq!(
        feed.drain(),
        HashMap::from([
            (
                "a".to_string(),
                SessionDirty {
                    rows: BTreeSet::from([1, 2]),
                    metadata_rows: BTreeSet::from([3]),
                    session: true,
                    queue: false,
                },
            ),
            ("b".to_string(), queue()),
        ])
    );
    assert_eq!(feed.drain(), HashMap::new());
}

#[test]
fn deactivating_discards_undrained_marks() {
    let feed = SessionFeed::default();
    feed.set_active(true);
    feed.mark_session("a");

    feed.set_active(false);
    feed.mark_session("b");
    feed.set_active(true);

    assert_eq!(feed.drain(), HashMap::new());
}

#[tokio::test]
async fn a_mark_leaves_one_wake_up_for_the_draining_task() {
    let feed = SessionFeed::default();
    feed.set_active(true);
    assert!(!wake_pending(&feed).await);

    // Marked before anyone waits, and more than once: one wake-up is kept.
    feed.mark_row("a", 1);
    feed.mark_row("a", 2);
    assert!(wake_pending(&feed).await);
    assert!(!wake_pending(&feed).await);
}

#[tokio::test]
async fn an_inactive_feed_wakes_nobody() {
    let feed = SessionFeed::default();
    feed.mark_session("a");
    assert!(!wake_pending(&feed).await);
}

#[test]
fn writers_leave_no_mark_while_the_feed_is_inactive() {
    let feed = Arc::new(SessionFeed::default());
    let store = Store::in_memory().unwrap().with_session_feed(feed.clone());
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let message_id = store
        .add_session_message(&session.id, MessageRole::Assistant, "hello")
        .unwrap();
    store
        .update_message_content(message_id, "hello there")
        .unwrap();
    store
        .update_message_acp_metadata(message_id, &metadata("tool_call"))
        .unwrap();
    store
        .add_acp_metadata_message(&session.id, &metadata("plan_update"))
        .unwrap();
    store
        .update_session_acp_title(&session.id, Some("Title"))
        .unwrap();
    store
        .add_queued_session_message(&session.id, "next", &[], None)
        .unwrap();
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    assert_unmarked(&feed);

    // Only what is written once the feed is active is marked.
    feed.set_active(true);
    store
        .update_message_content(message_id, "hello again")
        .unwrap();
    assert_marked(&feed, &session, rows(&[message_id]));
}

#[test]
fn writers_work_with_no_feed_attached() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let message_id = store
        .add_session_message(&session.id, MessageRole::Assistant, "hello")
        .unwrap();
    store
        .update_message_content(message_id, "hello there")
        .unwrap();
    store
        .update_message_acp_metadata(message_id, &metadata("tool_call"))
        .unwrap();
    store
        .add_acp_metadata_message(&session.id, &metadata("plan_update"))
        .unwrap();
    store
        .update_session_acp_title(&session.id, Some("Title"))
        .unwrap();
    let queued = store
        .add_queued_session_message(&session.id, "next", &[], None)
        .unwrap();
    assert!(store
        .transition_from_running(&session.id, SessionStatus::Completed, None, None)
        .unwrap());
    store
        .claim_queued_session_message(&queued.id)
        .unwrap()
        .unwrap();
    assert!(store
        .release_queued_session_message(&queued.id, None)
        .unwrap());

    let messages = store.get_session_messages(&session.id).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "hello there");
}

// =============================================================================
// Message writers
// =============================================================================

#[test]
fn add_session_message_with_images_marks_the_row() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    let id = store
        .add_session_message_with_images(&session.id, MessageRole::User, "hi", &[])
        .unwrap();

    assert_marked(&feed, &session, rows(&[id]));
}

#[test]
fn add_authored_session_message_marks_the_row() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    let id = store
        .add_authored_session_message(
            &session.id,
            MessageRole::Assistant,
            "Resumed.",
            "staged_notice",
        )
        .unwrap();

    assert_marked(&feed, &session, rows(&[id]));
}

#[test]
fn add_session_message_from_queue_marks_the_row_and_the_queue() {
    let (store, feed) = store_with_feed();
    let (session, queued) = session_with_claimed_message(&store, &feed);

    let id = store
        .add_session_message_with_images_from_queue(
            &session.id,
            MessageRole::User,
            "next",
            &[],
            &queued.id,
        )
        .unwrap();

    assert_marked(
        &feed,
        &session,
        SessionDirty {
            queue: true,
            ..rows(&[id])
        },
    );
}

#[test]
fn add_session_message_from_queue_marks_nothing_when_the_claim_is_gone() {
    let (store, feed) = store_with_feed();
    let (session, queued) = session_with_queued_message(&store, &feed);

    // Never claimed, so the insert is rolled back with the failed update.
    store
        .add_session_message_with_images_from_queue(
            &session.id,
            MessageRole::User,
            "next",
            &[],
            &queued.id,
        )
        .unwrap_err();

    assert_unmarked(&feed);
    assert!(store.get_session_messages(&session.id).unwrap().is_empty());
}

#[test]
fn update_message_content_marks_the_row_under_its_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);
    let other = running_session(&store, &feed);
    let id = store
        .add_session_message(&session.id, MessageRole::Assistant, "Hel")
        .unwrap();
    store
        .add_session_message(&other.id, MessageRole::Assistant, "Other")
        .unwrap();
    feed.drain();

    store.update_message_content(id, "Hello").unwrap();

    assert_marked(&feed, &session, rows(&[id]));
    let messages = store.get_session_messages(&session.id).unwrap();
    assert_eq!(messages[0].content, "Hello");
}

#[test]
fn update_message_content_for_a_missing_row_is_not_an_error() {
    let (store, feed) = store_with_feed();
    running_session(&store, &feed);

    store.update_message_content(9_999, "nobody").unwrap();

    assert_unmarked(&feed);
}

#[test]
fn update_message_acp_metadata_marks_the_metadata_row() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);
    let id = store
        .add_session_message(&session.id, MessageRole::ToolCall, "{\"name\":\"Read\"}")
        .unwrap();
    feed.drain();

    store
        .update_message_acp_metadata(
            id,
            &AcpMessageMetadata {
                acp_tool_call_id: Some("call-1".to_string()),
                acp_tool_status: Some("completed".to_string()),
                ..metadata("tool_call_update")
            },
        )
        .unwrap();

    assert_marked(&feed, &session, metadata_rows(&[id]));
    let stored = &store.get_session_messages(&session.id).unwrap()[0];
    assert_eq!(stored.acp.acp_tool_status.as_deref(), Some("completed"));
    assert_eq!(stored.content, "{\"name\":\"Read\"}");
}

#[test]
fn update_message_acp_metadata_for_a_missing_row_is_not_an_error() {
    let (store, feed) = store_with_feed();
    running_session(&store, &feed);

    store
        .update_message_acp_metadata(9_999, &metadata("tool_call_update"))
        .unwrap();

    assert_unmarked(&feed);
}

#[test]
fn add_acp_metadata_message_marks_a_row_the_transcript_reads() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    let plan = store
        .add_acp_metadata_message(&session.id, &metadata("plan_update"))
        .unwrap();
    let user_side = store
        .add_acp_metadata_message_with_role(
            &session.id,
            MessageRole::User,
            &metadata("available_commands_update"),
        )
        .unwrap();

    assert_marked(&feed, &session, metadata_rows(&[plan, user_side]));
}

#[test]
fn add_acp_metadata_message_leaves_chunk_rows_unmarked() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_message_id: Some("message-1".to_string()),
                acp_content: Some(json!({"type": "text", "text": "Hel"})),
                ..metadata("agent_message_chunk")
            },
        )
        .unwrap();
    store
        .add_acp_metadata_message(&session.id, &AcpMessageMetadata::default())
        .unwrap();

    assert_unmarked(&feed);
}

/// The Rust list of marked kinds and the SQL filter the transcript reads
/// through are written separately. This holds them together: an insert is
/// marked exactly when the row it wrote is one the transcript would ship.
#[test]
fn metadata_inserts_are_marked_exactly_when_the_transcript_reads_them() {
    let kinds = [
        // Read by the transcript.
        "tool_call",
        "tool_call_update",
        "plan_update",
        "available_commands_update",
        "config_options_update",
        "pikchr_session_started",
        // Kept for replay only.
        "agent_message_chunk",
        "agent_thought_chunk",
        "user_message_chunk",
        "usage_update",
        "session_info_update",
        "session_mode_state",
        "current_mode_update",
        "initialize",
        "prompt_response",
        "response_result",
    ];
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    let mut marked = Vec::new();
    for kind in kinds {
        let id = store
            .add_acp_metadata_message(&session.id, &metadata(kind))
            .unwrap();
        if feed.drain().remove(&session.id) == Some(metadata_rows(&[id])) {
            marked.push(kind);
        } else {
            assert_unmarked(&feed);
        }
    }

    let shipped: Vec<String> = store
        .get_session_acp_metadata_messages(&session.id)
        .unwrap()
        .into_iter()
        .filter_map(|message| message.acp.acp_event_kind)
        .collect();
    assert_eq!(marked, shipped);
    assert_eq!(marked, &kinds[..6]);
}

/// The four writers that became `UPDATE ... RETURNING` are read back through
/// a second connection: the write has to be committed once the method
/// returns, as it was when each was a plain `UPDATE`.
#[test]
fn returning_updates_are_committed_when_the_writer_returns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.db");
    let feed = Arc::new(SessionFeed::default());
    feed.set_active(true);
    let store = Store::new(&path).unwrap().with_session_feed(feed.clone());
    let (session, released) = session_with_claimed_message(&store, &feed);
    let sent = store
        .add_queued_session_message_held_for_transfer(&session.id, "second", &[], None)
        .unwrap();
    store
        .claim_queued_session_message(&sent.id)
        .unwrap()
        .unwrap();
    let message_id = store
        .add_session_message(&session.id, MessageRole::ToolCall, "{}")
        .unwrap();

    store
        .update_message_content(message_id, "{\"name\":\"Read\"}")
        .unwrap();
    store
        .update_message_acp_metadata(
            message_id,
            &AcpMessageMetadata {
                acp_tool_status: Some("completed".to_string()),
                ..metadata("tool_call_update")
            },
        )
        .unwrap();
    assert!(store
        .mark_queued_session_message_sent(&sent.id, message_id)
        .unwrap());
    assert!(store
        .release_queued_session_message(&released.id, None)
        .unwrap());

    let other = rusqlite::Connection::open(&path).unwrap();
    let (content, tool_status): (String, Option<String>) = other
        .query_row(
            "SELECT content, acp_tool_status FROM session_messages WHERE id = ?1",
            [message_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(content, "{\"name\":\"Read\"}");
    assert_eq!(tool_status.as_deref(), Some("completed"));
    let queue_status = |id: &str| -> String {
        other
            .query_row(
                "SELECT status FROM queued_session_messages WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(queue_status(&sent.id), "sent");
    assert_eq!(queue_status(&released.id), "queued");
}

// =============================================================================
// Session row writers
// =============================================================================

#[test]
fn update_session_acp_title_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    store
        .update_session_acp_title(&session.id, Some("Fix the flake"))
        .unwrap();

    assert_marked(&feed, &session, session_row());
}

#[test]
fn update_session_pipeline_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    store
        .update_session_pipeline(&session.id, &PipelineExecution::from_steps(&[]))
        .unwrap();

    assert_marked(&feed, &session, session_row());
}

#[test]
fn update_session_status_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();

    assert_marked(&feed, &session, session_row());
}

#[test]
fn transition_from_running_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    assert!(store
        .transition_from_running(&session.id, SessionStatus::Completed, None, None)
        .unwrap());

    assert_marked(&feed, &session, session_row());
}

#[test]
fn finish_detached_recovery_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = detached_session(&store, &feed);

    assert!(store
        .finish_detached_recovery(&session.id, SessionStatus::Completed, None, None)
        .unwrap());

    assert_marked(&feed, &session, session_row());
}

#[test]
fn finish_detached_recovery_observed_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = detached_session(&store, &feed);
    let observed = store.get_session(&session.id).unwrap().unwrap();

    assert!(store
        .finish_detached_recovery_observed(
            &session.id,
            observed.updated_at,
            observed.error_message.as_deref(),
            SessionStatus::Completed,
            None,
            None,
        )
        .unwrap()
        .is_some());

    assert_marked(&feed, &session, session_row());
}

#[test]
fn transition_from_running_observed_at_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    assert!(store
        .transition_from_running_observed_at(
            &session.id,
            updated_at(&store, &session),
            SessionStatus::Cancelled,
            None,
            None,
        )
        .unwrap());

    assert_marked(&feed, &session, session_row());
}

#[test]
fn transition_running_to_rate_limited_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    assert!(store
        .transition_running_to_rate_limited_with_provider_metadata(
            &session.id,
            Some("limited"),
            None,
            None,
            Some(&json!({"staged": {"rateLimitAutoRecovery": {"attempt": 1}}})),
        )
        .unwrap());

    assert_marked(&feed, &session, session_row());
}

#[test]
fn transition_from_owned_active_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    assert!(store
        .transition_from_owned_active(
            &session.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::AppQuit),
            std::process::id(),
        )
        .unwrap());

    assert_marked(&feed, &session, session_row());
}

/// Not a status change, but the row's owner is part of what a reader of the
/// session sees, and the write sits among the status transitions.
#[test]
fn adopt_running_session_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    assert!(store
        .adopt_running_session(&session.id, Some(std::process::id()))
        .unwrap());

    assert_marked(&feed, &session, session_row());
}

#[test]
fn transition_from_queued_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = queued_session(&store, &feed);

    assert!(store
        .transition_from_queued(&session.id, SessionStatus::Cancelled, None, None)
        .unwrap());

    assert_marked(&feed, &session, session_row());
}

#[test]
fn transition_from_disconnected_observed_at_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = session_with_status(&store, &feed, SessionStatus::Disconnected);

    assert!(store
        .transition_from_disconnected_observed_at(
            &session.id,
            updated_at(&store, &session),
            SessionStatus::Completed,
            None,
            None,
        )
        .unwrap());

    assert_marked(&feed, &session, session_row());
}

#[test]
fn claim_disconnected_session_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = session_with_status(&store, &feed, SessionStatus::Disconnected);

    assert!(store
        .claim_disconnected_session(&session.id, updated_at(&store, &session))
        .unwrap());

    assert_marked(&feed, &session, session_row());
}

#[test]
fn transition_queued_to_running_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = queued_session(&store, &feed);

    assert!(store.transition_queued_to_running(&session.id).unwrap());

    assert_marked(&feed, &session, session_row());
}

#[test]
fn transition_to_running_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = session_with_status(&store, &feed, SessionStatus::Completed);

    assert!(store
        .transition_to_running_clearing_agent_id(&session.id, None)
        .unwrap());

    assert_marked(&feed, &session, session_row());
}

#[test]
fn transition_app_quit_to_queued_marks_the_session() {
    let (store, feed) = store_with_feed();
    let project = Project::new("owner/repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let mut session = Session::new_running("work", Path::new("/tmp"));
    session.branch_id = Some(branch.id.clone());
    store.create_session(&session).unwrap();
    store
        .update_session_status(
            &session.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::AppQuit),
        )
        .unwrap();
    feed.drain();

    assert!(store.transition_app_quit_to_queued(&session.id).unwrap());

    assert_marked(&feed, &session, session_row());
}

#[test]
fn transition_to_running_with_provider_metadata_marks_the_session() {
    let (store, feed) = store_with_feed();
    let session = rate_limited_session(&store, &feed);

    assert!(store
        .transition_to_running_with_provider_metadata(&session.id, None)
        .unwrap());

    assert_marked(&feed, &session, session_row());
}

#[test]
fn a_session_write_that_changes_no_row_marks_nothing() {
    let (store, feed) = store_with_feed();
    let finished = session_with_status(&store, &feed, SessionStatus::Completed);

    // Guards that do not match the row.
    assert!(!store
        .transition_from_running(&finished.id, SessionStatus::Cancelled, None, None)
        .unwrap());
    assert!(!store
        .transition_from_queued(&finished.id, SessionStatus::Cancelled, None, None)
        .unwrap());
    assert!(!store.transition_queued_to_running(&finished.id).unwrap());
    assert!(!store
        .finish_detached_recovery(&finished.id, SessionStatus::Completed, None, None)
        .unwrap());
    assert!(store
        .finish_detached_recovery_observed(
            &finished.id,
            updated_at(&store, &finished),
            None,
            SessionStatus::Completed,
            None,
            None,
        )
        .unwrap()
        .is_none());
    assert!(!store.adopt_running_session(&finished.id, None).unwrap());

    // Unguarded writes to a session that does not exist.
    store
        .update_session_status("missing", SessionStatus::Completed, None, None)
        .unwrap();
    store
        .update_session_acp_title("missing", Some("Title"))
        .unwrap();
    store
        .update_session_pipeline("missing", &PipelineExecution::from_steps(&[]))
        .unwrap();

    assert_unmarked(&feed);
}

// =============================================================================
// Queue writers
// =============================================================================

#[test]
fn add_queued_session_message_marks_the_queue() {
    let (store, feed) = store_with_feed();
    let session = running_session(&store, &feed);

    store
        .add_queued_session_message(&session.id, "next", &[], None)
        .unwrap();

    assert_marked(&feed, &session, queue());
}

#[test]
fn add_queued_session_message_held_for_transfer_marks_the_queue() {
    let (store, feed) = store_with_feed();
    let session = session_with_status(&store, &feed, SessionStatus::Completed);

    store
        .add_queued_session_message_held_for_transfer(&session.id, "later", &[], None)
        .unwrap();

    assert_marked(&feed, &session, queue());
}

#[test]
fn delete_queued_session_message_marks_the_queue() {
    let (store, feed) = store_with_feed();
    let (session, queued) = session_with_queued_message(&store, &feed);

    assert!(store.delete_queued_session_message(&queued.id).unwrap());

    assert_marked(&feed, &session, queue());
}

#[test]
fn claim_queued_session_message_marks_the_queue() {
    let (store, feed) = store_with_feed();
    let (session, queued) = session_with_queued_message(&store, &feed);
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    feed.drain();

    let claimed = store.claim_queued_session_message(&queued.id).unwrap();

    assert_eq!(claimed.map(|message| message.id), Some(queued.id));
    assert_marked(&feed, &session, queue());
}

#[test]
fn claim_oldest_queued_session_message_marks_the_queue() {
    let (store, feed) = store_with_feed();
    let (session, queued) = session_with_queued_message(&store, &feed);
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    feed.drain();

    let claimed = store
        .claim_oldest_queued_session_message(&session.id)
        .unwrap();

    assert_eq!(claimed.map(|message| message.id), Some(queued.id));
    assert_marked(&feed, &session, queue());
}

#[test]
fn mark_queued_session_message_sent_marks_the_queue() {
    let (store, feed) = store_with_feed();
    let (session, queued) = session_with_claimed_message(&store, &feed);
    let message_id = store
        .add_session_message(&session.id, MessageRole::User, "next")
        .unwrap();
    feed.drain();

    assert!(store
        .mark_queued_session_message_sent(&queued.id, message_id)
        .unwrap());

    assert_marked(&feed, &session, queue());
    assert!(store
        .list_queued_session_messages(&session.id)
        .unwrap()
        .is_empty());
}

#[test]
fn release_queued_session_message_marks_the_queue() {
    let (store, feed) = store_with_feed();
    let (session, queued) = session_with_claimed_message(&store, &feed);

    assert!(store
        .release_queued_session_message(&queued.id, Some("try again"))
        .unwrap());

    assert_marked(&feed, &session, queue());
    let listed = store.list_queued_session_messages(&session.id).unwrap();
    assert_eq!(listed[0].status, QueuedSessionMessageStatus::Queued);
    assert_eq!(listed[0].last_error.as_deref(), Some("try again"));
}

#[test]
fn a_queue_write_that_changes_no_row_marks_nothing() {
    let (store, feed) = store_with_feed();
    // The session is still running, so its follow-up cannot be claimed, and
    // an unclaimed follow-up can be neither sent nor released.
    let (session, queued) = session_with_queued_message(&store, &feed);

    assert!(store
        .claim_queued_session_message(&queued.id)
        .unwrap()
        .is_none());
    assert!(store
        .claim_oldest_queued_session_message(&session.id)
        .unwrap()
        .is_none());
    assert!(!store
        .mark_queued_session_message_sent(&queued.id, 1)
        .unwrap());
    assert!(!store
        .release_queued_session_message(&queued.id, None)
        .unwrap());
    assert!(!store.delete_queued_session_message("missing").unwrap());
    store
        .add_queued_session_message("missing", "next", &[], None)
        .unwrap_err();
    let finished = session_with_status(&store, &feed, SessionStatus::Completed);
    store
        .add_queued_session_message(&finished.id, "next", &[], None)
        .unwrap_err();

    assert_unmarked(&feed);
}
