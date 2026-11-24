//! The full level's timing: its cadence, the spacing of a large tool row
//! that is still in flight, and what a frame carrying the session row may
//! not leave behind.

use std::time::Duration;

use serde_json::Value;

use crate::store::{MessageRole, SessionStatus};

use super::super::full::{FULL_INTERVAL, LARGE_ROW_INTERVAL};
use super::super::hub::GATHER;
use super::super::SessionStreamFrameKind::Delta;
use super::full::{large_output, open_chat, tool};
use super::*;

#[tokio::test(start_paused = true)]
async fn a_terminal_status_never_arrives_before_the_final_text() {
    let h = Harness::new();
    let session = h.session();
    h.say(&session, MessageRole::User, "the prompt");
    let reply = h.say(&session, MessageRole::Assistant, "Work");
    let client = open_chat(&h, &session).await;

    // A turn streams text every 20 ms, and ends as the runner ends one:
    // the last text, then the status.
    let mut text = String::from("Work");
    for _ in 0..40 {
        text.push_str("ing");
        h.store.update_message_content(reply, &text).unwrap();
        advance(Duration::from_millis(20)).await;
    }
    text.push_str(" done.");
    h.store.update_message_content(reply, &text).unwrap();
    h.store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    advance(Duration::from_secs(1)).await;

    let frames = client.take();
    let status_of = |frame: &SessionStreamFrame| {
        full(frame)
            .session
            .as_ref()
            .map(|session| session.status.clone())
    };
    let ended = frames
        .iter()
        .position(|frame| status_of(frame) == Some(SessionStatus::Completed))
        .expect("the terminal status arrives");
    // The frame that reports the end holds the final text.
    assert_eq!(full(&frames[ended]).messages[0].content, text);
    // No frame before it reported a status at all, and it is the last.
    assert!(frames[..ended]
        .iter()
        .all(|frame| status_of(frame).is_none()));
    assert_eq!(ended, frames.len() - 1);
}

#[tokio::test(start_paused = true)]
async fn a_frame_with_the_session_row_holds_no_row_back() {
    let h = Harness::new();
    let session = h.session();
    h.say(&session, MessageRole::User, "the prompt");
    let call = h.say(&session, MessageRole::ToolCall, "Run the tests");
    h.store
        .update_message_acp_metadata(call, &tool("in_progress", large_output(0)))
        .unwrap();
    let client = open_chat(&h, &session).await;

    // A large row was just sent, so its next copy is being held back.
    h.store
        .update_message_acp_metadata(call, &tool("in_progress", large_output(1)))
        .unwrap();
    past_the_gather().await;
    assert!(client.take().is_empty());

    // The turn is stopped with the tool call still in flight.
    h.store
        .update_message_acp_metadata(call, &tool("in_progress", large_output(2)))
        .unwrap();
    h.store
        .update_session_status(&session.id, SessionStatus::Cancelled, None, None)
        .unwrap();
    past_the_gather().await;

    // The frame that reports the end carries the row as it was left.
    let frame = only(client.take());
    assert_eq!(
        full(&frame).session.as_ref().unwrap().status,
        SessionStatus::Cancelled
    );
    assert_eq!(full_ids(&frame).1, vec![call]);
    assert_eq!(
        full(&frame).acp_metadata_messages[0].acp.acp_raw_output,
        Some(large_output(2))
    );
    advance(Duration::from_secs(2)).await;
    assert!(client.take().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_large_tool_row_in_flight_is_sent_twice_a_second_at_most() {
    let h = Harness::new();
    let session = h.session();
    h.say(&session, MessageRole::User, "the prompt");
    let call = h.say(&session, MessageRole::ToolCall, "Run the tests");
    h.store
        .update_message_acp_metadata(call, &tool("in_progress", large_output(0)))
        .unwrap();
    let client = open_chat(&h, &session).await;

    // Two seconds of output, rewritten whole every 50 ms, with a line of
    // assistant text beside each.
    let mut texts = Vec::new();
    for n in 1..=40 {
        h.store
            .update_message_acp_metadata(call, &tool("in_progress", large_output(n)))
            .unwrap();
        texts.push((
            Instant::now(),
            h.say(&session, MessageRole::Assistant, "a line"),
        ));
        advance(Duration::from_millis(50)).await;
    }
    advance(LARGE_ROW_INTERVAL).await;

    let frames = client.take_timed();
    let copies: Vec<(Instant, &Value)> = frames
        .iter()
        .flat_map(|(at, frame)| {
            full(frame)
                .acp_metadata_messages
                .iter()
                .map(move |row| (*at, row.acp.acp_raw_output.as_ref().unwrap()))
        })
        .collect();
    // Two seconds and a bit leave room for five copies; unspaced, the
    // cadence alone would have sent about fourteen.
    assert!((4..=5).contains(&copies.len()), "{} copies", copies.len());
    for pair in copies.windows(2) {
        assert!(
            pair[1].0 - pair[0].0 >= LARGE_ROW_INTERVAL,
            "copies {:?} apart",
            pair[1].0 - pair[0].0
        );
    }
    // The last copy is the row as it was left.
    assert_eq!(copies.last().unwrap().1, &large_output(40));
    // The text beside it was not held up: every line arrived within the
    // gather and one interval of being written.
    for (written, id) in texts {
        let (arrived, _) = frames
            .iter()
            .find(|(_, frame)| full_ids(frame).0.contains(&id))
            .expect("every line arrives");
        assert!(
            *arrived - written <= GATHER + FULL_INTERVAL,
            "a line took {:?}",
            *arrived - written
        );
    }
}

#[tokio::test(start_paused = true)]
async fn the_update_that_settles_a_large_tool_row_goes_at_once() {
    let h = Harness::new();
    let session = h.session();
    h.say(&session, MessageRole::User, "the prompt");
    let call = h.say(&session, MessageRole::ToolCall, "Run the tests");
    h.store
        .update_message_acp_metadata(call, &tool("in_progress", large_output(0)))
        .unwrap();
    let client = open_chat(&h, &session).await;

    // Both copies of the row are spaced: the one among the visible rows,
    // which a content write marks, and the one among the metadata rows.
    h.store
        .update_message_content(call, "Run the tests (12 passed)")
        .unwrap();
    h.store
        .update_message_acp_metadata(call, &tool("in_progress", large_output(1)))
        .unwrap();
    past_the_gather().await;
    assert!(client.take().is_empty());

    // It settles while they wait. Settled, its bodies are markers and it is
    // small, so it is sent now and not when the half second is up.
    let settled_at = Instant::now();
    h.store
        .update_message_acp_metadata(call, &tool("completed", large_output(2)))
        .unwrap();
    past_the_gather().await;
    let mut frames = client.take_timed();
    assert_eq!(frames.len(), 1, "expected one frame, got {frames:?}");
    let (at, frame) = frames.remove(0);
    assert!(at - settled_at <= GATHER);
    assert_eq!(full_ids(&frame), (vec![call], vec![call]));
    for row in [
        &full(&frame).messages[0],
        &full(&frame).acp_metadata_messages[0],
    ] {
        assert_eq!(row.acp.acp_tool_status.as_deref(), Some("completed"));
        assert!(wire(&row.acp.acp_raw_output).get("$deferred").is_some());
    }
    assert_eq!(
        full(&frame).messages[0].content,
        "Run the tests (12 passed)"
    );

    // Nothing is left waiting for the half second.
    advance(LARGE_ROW_INTERVAL * 2).await;
    assert!(client.take().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_busy_chat_gets_one_frame_per_full_interval() {
    let h = Harness::new();
    let session = h.session();
    let reply = h.say(&session, MessageRole::Assistant, "");
    let client = open_chat(&h, &session).await;

    // A second of streamed text, flushed every 20 ms.
    let mut text = String::new();
    for _ in 0..50 {
        text.push_str("word ");
        h.store.update_message_content(reply, &text).unwrap();
        advance(Duration::from_millis(20)).await;
    }
    advance(FULL_INTERVAL + GATHER).await;

    let frames = client.take_timed();
    // A second leaves room for six or seven, where the summary level's
    // interval would allow four.
    assert!((6..=8).contains(&frames.len()), "{} frames", frames.len());
    for pair in frames.windows(2) {
        assert!(
            pair[1].0 - pair[0].0 >= FULL_INTERVAL,
            "frames {:?} apart",
            pair[1].0 - pair[0].0
        );
    }
    // Each is the row as it was then, and the last is the row as it is.
    assert_eq!(full(&frames.last().unwrap().1).messages[0].content, text);
}

#[tokio::test(start_paused = true)]
async fn a_full_sink_delays_a_chat_and_loses_nothing() {
    let h = Harness::new();
    let session = h.session();
    h.say(&session, MessageRole::User, "the prompt");
    let client = open_chat(&h, &session).await;

    client.set_room(0);
    let first = h.say(&session, MessageRole::Assistant, "one");
    advance(Duration::from_secs(1)).await;
    let second = h.say(&session, MessageRole::Assistant, "two");
    h.store
        .update_session_acp_title(&session.id, Some("Fix the flake"))
        .unwrap();
    advance(Duration::from_secs(1)).await;
    assert!(client.take().is_empty());

    client.set_room(usize::MAX);
    advance(Duration::from_secs(1)).await;
    let frame = only(client.take());
    assert_eq!((frame.seq, frame.kind), (2, Delta));
    assert_eq!(full_ids(&frame).0, vec![first, second]);
    assert_eq!(
        full(&frame).session.as_ref().unwrap().acp_title.as_deref(),
        Some("Fix the flake")
    );
}
