//! The full level: what an open chat is sent. A full frame is the pane's
//! poll sent on change, so most of these hold a frame to what
//! `Store::poll_session` returns.

use std::time::Duration;

use serde_json::{json, Value};

use crate::store::{
    AcpMessageMetadata, MessageRole, SessionPollRequest, SessionStatus,
    DEFERRED_TOOL_BODY_MIN_BYTES,
};

use super::super::full::{FULL_INTERVAL, LARGE_ROW_BYTES};
use super::super::hub::GATHER;
use super::super::SessionStreamFrameKind::{Delta, Snapshot};
use super::super::SessionStreamLevel::{Summary, Title};
use super::super::{SessionInterest, SessionStreamInterest};
use super::*;

/// A tool call's metadata, with `output` as its result so far.
pub(super) fn tool(status: &str, output: Value) -> AcpMessageMetadata {
    AcpMessageMetadata {
        acp_event_kind: Some("tool_call".to_string()),
        acp_tool_call_id: Some("tc-1".to_string()),
        acp_tool_status: Some(status.to_string()),
        acp_raw_output: Some(output),
        ..Default::default()
    }
}

/// A result too large to send often, told apart by `n`.
pub(super) fn large_output(n: usize) -> Value {
    json!(format!("{n}:{}", "x".repeat(LARGE_ROW_BYTES)))
}

fn row(session: &Session, level: SessionStreamLevel) -> SessionInterest {
    SessionInterest {
        session_id: session.id.clone(),
        level,
        cursor: None,
    }
}

/// Long enough after a full frame that the cadence no longer holds the next.
pub(super) async fn past_the_full_interval() {
    advance(FULL_INTERVAL).await;
}

/// An open chat on `session`, holding nothing, with its snapshot taken.
pub(super) async fn open_chat(h: &Harness, session: &Session) -> Arc<Recorder> {
    let client = h.client("a");
    h.want_chats("a", 1, vec![chat(session, from_the_start())]);
    settle().await;
    let frame = only(client.take());
    assert_eq!(frame.kind, Snapshot);
    past_the_full_interval().await;
    client
}

#[tokio::test(start_paused = true)]
async fn a_full_snapshot_is_the_poll_for_the_clients_cursor() {
    let h = Harness::new();
    let session = h.session();
    h.store
        .add_queued_session_message(&session.id, "a follow-up", &[], None)
        .unwrap();
    h.say(&session, MessageRole::User, "the prompt");
    let assistant = h.say(&session, MessageRole::Assistant, "Taking a look");
    let call = h.say(&session, MessageRole::ToolCall, "Read notes.md");
    h.store
        .update_message_acp_metadata(call, &tool("in_progress", json!("partial")))
        .unwrap();
    let tail = h.say(&session, MessageRole::Assistant, "Done.");
    // What a pane that loaded up to the tool call would send.
    let cursor = SessionPollRequest {
        since_id: call,
        refetch_ids: vec![assistant],
        metadata_since_id: 0,
        metadata_refetch_ids: vec![call],
    };
    let client = h.client("a");

    h.want_chats("a", 7, vec![chat(&session, Some(cursor.clone()))]);
    settle().await;

    let frame = only(client.take());
    assert_eq!((frame.generation, frame.seq, frame.kind), (7, 1, Snapshot));
    assert!(frame.summary.is_none());
    let poll = h.store.poll_session(&session.id, &cursor).unwrap();
    assert_eq!(wire(full(&frame)), wire(&poll));
    assert_eq!(full_ids(&frame), (vec![assistant, call, tail], vec![call]));
    assert_eq!(
        full(&frame).session.as_ref().unwrap().status,
        SessionStatus::Running
    );
    assert_eq!(full(&frame).queued_messages.as_ref().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_full_delta_is_the_poll_for_the_rows_that_were_written() {
    let h = Harness::new();
    let session = h.session();
    h.say(&session, MessageRole::User, "the prompt");
    let assistant = h.say(&session, MessageRole::Assistant, "Taking a");
    let call = h.say(&session, MessageRole::ToolCall, "Read notes.md");
    h.store
        .update_message_acp_metadata(call, &tool("in_progress", json!("partial")))
        .unwrap();
    let client = open_chat(&h, &session).await;

    // One burst: the assistant row grows behind the tool call, a new tail
    // row and a plan arrive, the tool call gets more output, and a row the
    // transcript never reads is written for replay.
    h.store
        .update_message_content(assistant, "Taking a look")
        .unwrap();
    let tail = h.say(&session, MessageRole::Assistant, "Done.");
    let plan = h
        .store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("plan_update".to_string()),
                acp_content: Some(json!({ "entries": [] })),
                ..Default::default()
            },
        )
        .unwrap();
    h.store
        .update_message_acp_metadata(call, &tool("in_progress", json!("partial, and more")))
        .unwrap();
    h.store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("usage_update".to_string()),
                acp_usage: Some(json!({ "used": 1 })),
                ..Default::default()
            },
        )
        .unwrap();
    past_the_gather().await;

    let frame = only(client.take());
    assert_eq!((frame.seq, frame.kind), (2, Delta));
    assert!(frame.summary.is_none());
    // The rows the writes marked, and what a poll returns for exactly them.
    let dirty = SessionPollRequest::by_ids(vec![assistant, tail], vec![call, plan]);
    let poll = h.store.poll_session(&session.id, &dirty).unwrap();
    assert_eq!(full_ids(&frame), (vec![assistant, tail], vec![call, plan]));
    assert_eq!(wire(&full(&frame).messages), wire(&poll.messages));
    assert_eq!(
        wire(&full(&frame).acp_metadata_messages),
        wire(&poll.acp_metadata_messages)
    );
    assert_eq!(full(&frame).messages[0].content, "Taking a look");
    // Neither the session row nor the queue was written, so neither is
    // read or sent.
    let sent = wire(&frame);
    assert!(sent["full"].get("session").is_none(), "{sent}");
    assert!(sent["full"].get("queuedMessages").is_none(), "{sent}");

    // And nothing is owed afterwards.
    advance(Duration::from_secs(2)).await;
    assert!(client.take().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_session_row_and_the_queue_come_only_when_they_are_written() {
    let h = Harness::new();
    let session = h.session();
    h.say(&session, MessageRole::User, "the prompt");
    let client = open_chat(&h, &session).await;

    h.store
        .update_session_acp_title(&session.id, Some("Fix the flake"))
        .unwrap();
    past_the_gather().await;
    let frame = only(client.take());
    assert_eq!(frame.kind, Delta);
    assert_eq!(
        wire(&full(&frame).session),
        wire(&h.store.get_session(&session.id).unwrap())
    );
    assert!(full(&frame).queued_messages.is_none());
    assert_eq!(full_ids(&frame), (vec![], vec![]));
    past_the_full_interval().await;

    let queued = h
        .store
        .add_queued_session_message(&session.id, "next", &[], None)
        .unwrap();
    past_the_gather().await;
    let frame = only(client.take());
    assert!(full(&frame).session.is_none());
    let queue = full(&frame).queued_messages.as_ref().unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].id, queued.id);
    past_the_full_interval().await;

    // The queue emptied is sent as an empty queue.
    h.store.delete_queued_session_message(&queued.id).unwrap();
    past_the_gather().await;
    let frame = only(client.take());
    assert_eq!(wire(&frame)["full"]["queuedMessages"], json!([]));
}

#[tokio::test(start_paused = true)]
async fn a_tool_row_that_settles_arrives_with_markers() {
    let h = Harness::new();
    let session = h.session();
    h.say(&session, MessageRole::User, "the prompt");
    let call = h.say(&session, MessageRole::ToolCall, "Read notes.md");
    let body = json!("x".repeat(DEFERRED_TOOL_BODY_MIN_BYTES + 1));
    h.store
        .update_message_acp_metadata(call, &tool("in_progress", body.clone()))
        .unwrap();
    let client = h.client("a");
    h.want_chats("a", 1, vec![chat(&session, from_the_start())]);
    settle().await;

    // In flight, the row ships whole.
    let frame = only(client.take());
    assert_eq!(full_ids(&frame).1, vec![call]);
    assert_eq!(
        full(&frame).acp_metadata_messages[0].acp.acp_raw_output,
        Some(body.clone())
    );
    past_the_full_interval().await;

    h.store
        .update_message_acp_metadata(call, &tool("completed", body))
        .unwrap();
    past_the_gather().await;

    // Settled, a visible tool row is no longer among the metadata rows a
    // transcript load returns. The update that settled it is sent all the
    // same, so the pane sees it settle, and its body is now a marker.
    let loaded = h
        .store
        .get_session_acp_metadata_messages(&session.id)
        .unwrap();
    assert!(loaded.is_empty());
    let frame = only(client.take());
    assert_eq!(frame.kind, Delta);
    assert_eq!(full_ids(&frame), (vec![], vec![call]));
    let settled = &full(&frame).acp_metadata_messages[0];
    assert_eq!(settled.acp.acp_tool_status.as_deref(), Some("completed"));
    assert_eq!(
        settled.acp.acp_raw_output,
        Some(json!({
            "$deferred": {
                "messageId": call,
                "field": "output",
                "bytes": DEFERRED_TOOL_BODY_MIN_BYTES + 3,
            }
        }))
    );
}

#[tokio::test(start_paused = true)]
async fn a_session_held_as_a_row_and_as_a_chat_is_served_both_ways() {
    let h = Harness::new();
    let session = h.session();
    let prompt = h.say(&session, MessageRole::User, "the prompt");
    let client = h.client("a");

    h.want_chats(
        "a",
        1,
        vec![row(&session, Summary), chat(&session, from_the_start())],
    );
    settle().await;

    // A snapshot for each, numbered on the client's one sequence.
    let mut frames = client.take();
    frames.sort_by_key(|frame| frame.full.is_some());
    assert_eq!(frames.len(), 2);
    let mut seqs: Vec<u64> = frames.iter().map(|frame| frame.seq).collect();
    seqs.sort_unstable();
    assert_eq!(seqs, vec![1, 2]);
    assert!(frames.iter().all(|frame| frame.kind == Snapshot));
    assert_eq!(row_ids(&frames[0]), vec![prompt]);
    assert!(frames[0].full.is_none());
    assert_eq!(full_ids(&frames[1]).0, vec![prompt]);
    assert!(frames[1].summary.is_none());
    past_the_interval().await;

    // A row: a delta for each level, each in its own shape.
    let reply = h.say(&session, MessageRole::Assistant, "On it.");
    past_the_gather().await;
    let mut frames = client.take();
    frames.sort_by_key(|frame| frame.full.is_some());
    assert_eq!(frames.len(), 2);
    assert!(frames.iter().all(|frame| frame.kind == Delta));
    assert_eq!(row_ids(&frames[0]), vec![reply]);
    assert_eq!(full_ids(&frames[1]).0, vec![reply]);
    past_the_interval().await;

    // A tool call's metadata and the queue are the chat's alone.
    h.store
        .update_message_acp_metadata(reply, &tool("in_progress", json!("partial")))
        .unwrap();
    h.store
        .add_queued_session_message(&session.id, "next", &[], None)
        .unwrap();
    past_the_gather().await;
    let frame = only(client.take());
    assert_eq!(full_ids(&frame), (vec![], vec![reply]));
    assert!(full(&frame).queued_messages.is_some());

    // Closing the chat leaves the row as it was: no snapshot, and deltas.
    past_the_interval().await;
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    assert!(client.take().is_empty());
    let last = h.say(&session, MessageRole::Assistant, "Done.");
    past_the_gather().await;
    let frame = only(client.take());
    assert_eq!(row_ids(&frame), vec![last]);
    assert!(frame.full.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_chat_alone_is_sent_nothing_at_the_other_levels() {
    let h = Harness::new();
    let session = h.session();
    let other = h.session();
    h.say(&session, MessageRole::User, "the prompt");
    let client = h.client("a");

    h.want_chats(
        "a",
        1,
        vec![chat(&session, from_the_start()), row(&other, Title)],
    );
    settle().await;

    let frames = client.take();
    assert_eq!(frames.len(), 2);
    for frame in &frames {
        // One level a frame, by which session it is for.
        assert_eq!(frame.full.is_some(), frame.session_id == session.id);
        assert_eq!(frame.summary.is_some(), frame.session_id == other.id);
    }
}

#[tokio::test(start_paused = true)]
async fn a_cursor_asks_for_a_snapshot_and_none_keeps_the_deltas_coming() {
    let h = Harness::new();
    let session = h.session();
    let prompt = h.say(&session, MessageRole::User, "the prompt");
    let first = h.say(&session, MessageRole::Assistant, "one");
    let client = h.client("a");
    let since = |since_id| {
        Some(SessionPollRequest {
            since_id,
            ..Default::default()
        })
    };

    h.want_chats("a", 1, vec![chat(&session, since(first))]);
    settle().await;
    let frame = only(client.take());
    assert_eq!((frame.seq, frame.kind), (1, Snapshot));
    assert_eq!(full_ids(&frame).0, vec![first]);
    past_the_full_interval().await;

    // Declared again without a cursor, as when some other session is added
    // to the interest: nothing is read again.
    let other = h.session();
    h.want_chats("a", 1, vec![chat(&session, None), row(&other, Title)]);
    settle().await;
    let frame = only(client.take());
    assert_eq!(frame.session_id, other.id);
    let second = h.say(&session, MessageRole::Assistant, "two");
    past_the_gather().await;
    let frame = only(client.take());
    assert_eq!((frame.seq, frame.kind), (3, Delta));
    assert_eq!(full_ids(&frame).0, vec![second]);
    past_the_full_interval().await;

    // A cursor, as from a second pane that holds less: a snapshot from it,
    // though the session was held already.
    h.want_chats("a", 1, vec![chat(&session, since(prompt))]);
    settle().await;
    let frame = only(client.take());
    assert_eq!((frame.seq, frame.kind), (4, Snapshot));
    assert_eq!(full_ids(&frame).0, vec![prompt, first, second]);
    assert!(full(&frame).session.is_some());
    assert!(full(&frame).queued_messages.is_some());

    // The stream starting over without a cursor reads from the last one
    // given.
    h.want_chats("a", 2, vec![chat(&session, None)]);
    settle().await;
    let frame = only(client.take());
    assert_eq!((frame.generation, frame.seq, frame.kind), (2, 1, Snapshot));
    assert_eq!(full_ids(&frame).0, vec![prompt, first, second]);

    // And a chat that never gave one is read from the start.
    let fresh = h.client("b");
    h.want_chats("b", 1, vec![chat(&session, None)]);
    settle().await;
    let frame = only(fresh.take());
    let poll = h
        .store
        .poll_session(&session.id, &SessionPollRequest::default())
        .unwrap();
    assert_eq!(wire(full(&frame)), wire(&poll));
}

#[tokio::test(start_paused = true)]
async fn a_cursor_given_while_a_delta_is_owed_does_not_lose_it() {
    let h = Harness::new();
    let session = h.session();
    h.say(&session, MessageRole::User, "the prompt");
    let reply = h.say(&session, MessageRole::Assistant, "Taking a");
    let call = h.say(&session, MessageRole::ToolCall, "Read notes.md");
    h.store
        .update_message_acp_metadata(call, &tool("in_progress", json!("partial")))
        .unwrap();
    let tail = h.say(&session, MessageRole::Assistant, "Next");
    let client = open_chat(&h, &session).await;

    // The assistant row grows behind the tool call and the tool call
    // settles. The hub has taken both marks and is waiting out the gather
    // when a second pane joins with a cursor that covers neither row.
    h.store
        .update_message_content(reply, "Taking a look")
        .unwrap();
    h.store
        .update_message_acp_metadata(call, &tool("completed", json!("all of it")))
        .unwrap();
    advance(GATHER / 2).await;
    assert!(client.take().is_empty());
    let cursor = SessionPollRequest {
        since_id: tail,
        metadata_since_id: tail,
        ..Default::default()
    };
    h.want_chats("a", 1, vec![chat(&session, Some(cursor.clone()))]);
    advance(Duration::from_secs(1)).await;

    // The first pane was owed them, and the snapshot holds them.
    let frame = only(client.take());
    assert_eq!(frame.kind, Snapshot);
    assert_eq!(full_ids(&frame), (vec![reply, tail], vec![call]));
    assert_eq!(full(&frame).messages[0].content, "Taking a look");
    assert_eq!(
        full(&frame).acp_metadata_messages[0]
            .acp
            .acp_tool_status
            .as_deref(),
        Some("completed")
    );

    // The same with a write the hub has not yet taken the mark of when the
    // cursor arrives.
    h.store
        .update_message_content(reply, "Taking a look.")
        .unwrap();
    h.want_chats("a", 1, vec![chat(&session, Some(cursor))]);
    advance(Duration::from_secs(1)).await;
    let frame = only(client.take());
    assert_eq!(frame.kind, Snapshot);
    assert_eq!(full_ids(&frame), (vec![reply, tail], vec![]));
    assert_eq!(full(&frame).messages[0].content, "Taking a look.");
}

#[tokio::test(start_paused = true)]
async fn a_write_before_the_snapshot_is_in_it_and_one_after_is_a_delta() {
    let h = Harness::new();
    let session = h.session();
    let prompt = h.say(&session, MessageRole::User, "the prompt");
    let client = h.client("a");
    h.want_chats("a", 1, vec![chat(&session, from_the_start())]);
    // Written before the hub has read anything for the interest.
    let reply = h.say(&session, MessageRole::Assistant, "already here");
    advance(Duration::from_secs(1)).await;

    // The snapshot holds the row, and no delta repeats it.
    let frame = only(client.take());
    assert_eq!((frame.seq, frame.kind), (1, Snapshot));
    assert_eq!(full_ids(&frame).0, vec![prompt, reply]);
}

#[tokio::test(start_paused = true)]
async fn a_paused_chat_is_sent_nothing_and_gets_a_snapshot_when_it_returns() {
    let h = Harness::new();
    let session = h.session();
    let prompt = h.say(&session, MessageRole::User, "the prompt");
    let client = open_chat(&h, &session).await;
    let since = |since_id| SessionPollRequest {
        since_id,
        ..Default::default()
    };

    let paused = SessionStreamInterest {
        paused: true,
        ..chats(1, vec![chat(&session, None)])
    };
    h.hub.set_interest("a", paused).unwrap();
    settle().await;
    assert!(!h.feed.is_active());
    let reply = h.say(&session, MessageRole::Assistant, "while hidden");
    advance(Duration::from_secs(5)).await;
    assert!(client.take().is_empty());

    // Back, with the cursor of what the pane holds.
    h.want_chats("a", 1, vec![chat(&session, Some(since(prompt)))]);
    settle().await;
    let frame = only(client.take());
    assert_eq!(frame.kind, Snapshot);
    assert_eq!(full_ids(&frame).0, vec![prompt, reply]);
}

#[tokio::test(start_paused = true)]
async fn a_chat_on_a_session_with_no_row_waits_for_it() {
    let h = Harness::new();
    let session = Session::new_running("work", Path::new("/tmp"));
    let client = h.client("a");
    h.want_chats("a", 1, vec![chat(&session, from_the_start())]);
    advance(Duration::from_secs(5)).await;
    assert!(client.take().is_empty());
    assert!(h.hub.check("a", 1, 0));

    // Written at last: that write brings the snapshot.
    h.store.create_session(&session).unwrap();
    let prompt = h.say(&session, MessageRole::User, "the prompt");
    settle().await;
    let frame = only(client.take());
    assert_eq!((frame.seq, frame.kind), (1, Snapshot));
    assert_eq!(full_ids(&frame).0, vec![prompt]);
}

#[tokio::test(start_paused = true)]
async fn two_chats_on_one_session_share_a_read_and_get_their_own_rows() {
    let h = Harness::new();
    let session = h.session();
    h.say(&session, MessageRole::User, "the prompt");
    let early = h.client("early");
    let late = h.client("late");
    h.want_chats("early", 1, vec![chat(&session, from_the_start())]);
    settle().await;
    early.take();
    past_the_full_interval().await;

    // One client is owed a row the other joins too late to be owed.
    let first = h.say(&session, MessageRole::Assistant, "one");
    advance(GATHER - Duration::from_millis(5)).await;
    h.want_chats("late", 1, vec![chat(&session, from_the_start())]);
    settle().await;
    assert_eq!(full_ids(&only(late.take())).0.last(), Some(&first));
    past_the_full_interval().await;
    assert_eq!(full_ids(&only(early.take())).0, vec![first]);
    assert!(late.take().is_empty());

    // A write both are owed is read once for the two of them.
    let rounds = h.rounds();
    let second = h.say(&session, MessageRole::Assistant, "two");
    past_the_gather().await;
    assert_eq!(h.rounds(), rounds + 1);
    for client in [&early, &late] {
        let frame = only(client.take());
        assert_eq!(frame.kind, Delta);
        assert_eq!(full_ids(&frame).0, vec![second]);
    }
}

#[test]
fn a_full_frame_goes_out_under_the_polls_names() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .add_session_message(&session.id, MessageRole::Assistant, "hello")
        .unwrap();
    let whole = store
        .read_session_full(&session.id, &SessionPollRequest::default(), true, true)
        .unwrap();
    let frame = |kind, full| SessionStreamFrame {
        generation: 2,
        seq: 7,
        session_id: session.id.clone(),
        kind,
        summary: None,
        full: Some(full),
    };

    let sent = wire(&frame(Snapshot, whole.clone()));
    assert_eq!(sent["kind"], "snapshot");
    assert!(sent.get("summary").is_none());
    let mut parts: Vec<&str> = sent["full"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    parts.sort_unstable();
    assert_eq!(
        parts,
        vec![
            "acpMetadataMessages",
            "messages",
            "queuedMessages",
            "session"
        ]
    );

    // A delta of rows alone leaves the other two parts out.
    let rows = SessionFull {
        session: None,
        queued_messages: None,
        ..whole
    };
    let sent = wire(&frame(Delta, rows));
    let mut parts: Vec<&str> = sent["full"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    parts.sort_unstable();
    assert_eq!(parts, vec!["acpMetadataMessages", "messages"]);
}
