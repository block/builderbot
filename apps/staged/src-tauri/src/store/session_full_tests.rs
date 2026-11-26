//! `Store::read_session_full`: the read behind the session stream's full
//! level, held to `poll_session`, whose response it is.

use std::path::Path;

use serde_json::{json, Value};

use super::models::*;
use super::{in_flight_tool_row_len, SessionPollRequest, Store, DEFERRED_TOOL_BODY_MIN_BYTES};

fn store_with_session() -> (Store, Session) {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("work", Path::new("/wt/feature"));
    store.create_session(&session).unwrap();
    (store, session)
}

fn add(store: &Store, session: &Session, role: MessageRole, content: &str) -> i64 {
    store
        .add_session_message(&session.id, role, content)
        .unwrap()
}

fn tool(status: &str, output: Value) -> AcpMessageMetadata {
    AcpMessageMetadata {
        acp_event_kind: Some("tool_call".to_string()),
        acp_tool_call_id: Some("tc-1".to_string()),
        acp_tool_status: Some(status.to_string()),
        acp_raw_output: Some(output),
        ..Default::default()
    }
}

fn wire(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}

fn ids(rows: &[SessionMessage]) -> Vec<i64> {
    rows.iter().map(|row| row.id).collect()
}

#[test]
fn with_both_parts_it_is_the_poll() {
    let (store, session) = store_with_session();
    store
        .add_queued_session_message(&session.id, "a follow-up", &[], None)
        .unwrap();
    add(&store, &session, MessageRole::User, "the prompt");
    let assistant = add(&store, &session, MessageRole::Assistant, "Taking a look");
    let call = add(&store, &session, MessageRole::ToolCall, "Read notes.md");
    store
        .update_message_acp_metadata(call, &tool("in_progress", json!("partial")))
        .unwrap();
    add(&store, &session, MessageRole::Assistant, "Done.");
    store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("plan_update".to_string()),
                acp_content: Some(json!({ "entries": [] })),
                ..Default::default()
            },
        )
        .unwrap();

    for request in [
        SessionPollRequest::default(),
        SessionPollRequest {
            since_id: call,
            refetch_ids: vec![assistant],
            metadata_since_id: call,
            metadata_refetch_ids: vec![call],
        },
    ] {
        let poll = store.poll_session(&session.id, &request).unwrap();
        let full = store
            .read_session_full(&session.id, &request, true, true)
            .unwrap();

        assert_eq!(wire(&full.session), wire(&poll.session));
        assert_eq!(wire(&full.queued_messages), wire(&poll.queued_messages));
        assert_eq!(wire(&full.messages), wire(&poll.messages));
        assert_eq!(
            wire(&full.acp_metadata_messages),
            wire(&poll.acp_metadata_messages)
        );
        // And on the wire it has the poll's four names.
        assert_eq!(wire(&full), wire(&poll));
    }
}

#[test]
fn a_part_that_is_not_asked_for_is_not_read_or_sent() {
    let (store, session) = store_with_session();
    store
        .add_queued_session_message(&session.id, "a follow-up", &[], None)
        .unwrap();
    let row = add(&store, &session, MessageRole::Assistant, "hello");

    let full = store
        .read_session_full(&session.id, &SessionPollRequest::default(), false, false)
        .unwrap();

    assert!(full.session.is_none());
    assert!(full.queued_messages.is_none());
    assert_eq!(ids(&full.messages), vec![row]);
    let sent = wire(&full);
    assert!(sent.get("session").is_none(), "{sent}");
    assert!(sent.get("queuedMessages").is_none(), "{sent}");
    assert!(!full.is_empty());

    // An empty queue that was asked for is sent, as empty.
    let other = Session::new_running("other", Path::new("/tmp"));
    store.create_session(&other).unwrap();
    let full = store
        .read_session_full(&other.id, &SessionPollRequest::default(), false, true)
        .unwrap();
    assert_eq!(wire(&full)["queuedMessages"], json!([]));
    assert!(!full.is_empty());
}

#[test]
fn rows_read_by_id_are_what_the_poll_returns_for_them() {
    let (store, session) = store_with_session();
    let prompt = add(&store, &session, MessageRole::User, "the prompt");
    let assistant = add(&store, &session, MessageRole::Assistant, "Taking a look");
    let call = add(&store, &session, MessageRole::ToolCall, "Read notes.md");
    store
        .update_message_acp_metadata(call, &tool("in_progress", json!("partial")))
        .unwrap();
    let tail = add(&store, &session, MessageRole::Assistant, "Done.");
    // A hidden row of a kind the transcript does not read, and one it does.
    let usage = store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("usage_update".to_string()),
                acp_usage: Some(json!({ "used": 1 })),
                ..Default::default()
            },
        )
        .unwrap();
    let plan = store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("plan_update".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

    let request = SessionPollRequest::by_ids(vec![assistant, usage, 9_999], vec![call, plan]);
    let full = store
        .read_session_full(&session.id, &request, false, false)
        .unwrap();

    // Only the rows named: nothing is past either cursor. A hidden row is
    // not a visible one whoever asks, and an id with no row is no error.
    assert_eq!(ids(&full.messages), vec![assistant]);
    assert_eq!(ids(&full.acp_metadata_messages), vec![call, plan]);
    let poll = store.poll_session(&session.id, &request).unwrap();
    assert_eq!(wire(&full.messages), wire(&poll.messages));
    assert_eq!(
        wire(&full.acp_metadata_messages),
        wire(&poll.acp_metadata_messages)
    );
    // The rows that were not named are not read.
    assert!(!ids(&full.messages).contains(&prompt));
    assert!(!ids(&full.messages).contains(&tail));
}

#[test]
fn a_settled_tool_row_is_still_read_by_id_and_has_its_bodies_deferred() {
    let (store, session) = store_with_session();
    let call = add(&store, &session, MessageRole::ToolCall, "Read notes.md");
    let body = json!("x".repeat(DEFERRED_TOOL_BODY_MIN_BYTES + 1));
    store
        .update_message_acp_metadata(call, &tool("in_progress", body.clone()))
        .unwrap();
    let request = SessionPollRequest::by_ids(vec![call], vec![call]);

    // In flight it ships whole, in both lists.
    let full = store
        .read_session_full(&session.id, &request, false, false)
        .unwrap();
    assert_eq!(full.messages[0].acp.acp_raw_output, Some(body.clone()));
    assert_eq!(
        full.acp_metadata_messages[0].acp.acp_raw_output,
        Some(body.clone())
    );
    assert!(in_flight_tool_row_len(&full.messages[0]).is_some());

    store
        .update_message_acp_metadata(call, &tool("completed", body))
        .unwrap();

    // Settled, a visible tool row is outside the transcript's metadata
    // filter: a read past the cursor no longer returns it.
    let past_the_cursor = store
        .read_session_full(&session.id, &SessionPollRequest::default(), false, false)
        .unwrap();
    assert!(past_the_cursor.acp_metadata_messages.is_empty());
    // By id it is read all the same, with a marker where the body was.
    let full = store
        .read_session_full(&session.id, &request, false, false)
        .unwrap();
    let marker = json!({
        "$deferred": { "messageId": call, "field": "output", "bytes": DEFERRED_TOOL_BODY_MIN_BYTES + 3 }
    });
    assert_eq!(ids(&full.acp_metadata_messages), vec![call]);
    assert_eq!(
        full.acp_metadata_messages[0].acp.acp_raw_output,
        Some(marker.clone())
    );
    assert_eq!(full.messages[0].acp.acp_raw_output, Some(marker));
    assert_eq!(in_flight_tool_row_len(&full.messages[0]), None);
}

#[test]
fn only_a_tool_row_in_flight_has_an_in_flight_size() {
    let (store, session) = store_with_session();
    let text = add(&store, &session, MessageRole::Assistant, "plain text");
    let call = add(&store, &session, MessageRole::ToolCall, "Read notes.md");
    store
        .update_message_acp_metadata(call, &tool("in_progress", json!("partial")))
        .unwrap();

    let full = store
        .read_session_full(&session.id, &SessionPollRequest::default(), false, false)
        .unwrap();
    let row = |id| full.messages.iter().find(|row| row.id == id).unwrap();

    assert_eq!(in_flight_tool_row_len(row(text)), None);
    assert_eq!(
        in_flight_tool_row_len(row(call)),
        Some(serde_json::to_vec(row(call)).unwrap().len())
    );
}

#[test]
fn more_ids_than_one_statement_takes_are_read_in_order_and_once() {
    let (store, session) = store_with_session();
    let rows: Vec<i64> = (0..1_250)
        .map(|n| {
            add(
                &store,
                &session,
                MessageRole::Assistant,
                &format!("row {n}"),
            )
        })
        .collect();

    // By id alone.
    let full = store
        .read_session_full(
            &session.id,
            &SessionPollRequest::by_ids(rows.clone(), Vec::new()),
            false,
            false,
        )
        .unwrap();
    assert_eq!(ids(&full.messages), rows);

    // With a cursor that the ids overlap: the rows past it come once.
    let request = SessionPollRequest {
        since_id: rows[1_200],
        refetch_ids: rows.clone(),
        ..Default::default()
    };
    let full = store
        .read_session_full(&session.id, &request, false, false)
        .unwrap();
    assert_eq!(ids(&full.messages), rows);
}

#[test]
fn a_session_with_no_row_reads_as_nothing() {
    let store = Store::in_memory().unwrap();

    let full = store
        .read_session_full("missing", &SessionPollRequest::default(), true, true)
        .unwrap();

    assert!(full.session.is_none());
    assert!(full.queued_messages.is_some_and(|queue| queue.is_empty()));
    assert!(full.messages.is_empty());
    assert!(full.acp_metadata_messages.is_empty());
}
