//! What a frame carries and when it is sent: snapshots, coalescing, the
//! cadence, and what each level hears.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::json;

use crate::store::{AcpMessageMetadata, MessageRole, SessionStatus, SummaryRow};

use super::super::hub::{GATHER, SUMMARY_INTERVAL};
use super::super::SessionStreamLevel::{Summary, Title};
use super::super::{SessionStreamFrameKind, SessionStreamInterest};
use super::*;

#[tokio::test(start_paused = true)]
async fn a_snapshot_carries_what_its_level_and_window_ask_for() {
    let h = Harness::new();
    let session = h.session();
    let prompt = h.say(&session, MessageRole::User, "a long opening prompt");
    let reply = h.say(&session, MessageRole::Assistant, "On it.");
    let result = h.say(&session, MessageRole::ToolResult, "file contents");
    let last = h.say(&session, MessageRole::Assistant, "Done.");
    let wide = h.client("wide");
    let narrow = h.client("narrow");
    let title = h.client("title");

    h.hub
        .set_interest(
            "wide",
            SessionStreamInterest {
                summary_window: 10,
                ..interest(7, &[(&session, Summary)])
            },
        )
        .unwrap();
    h.hub
        .set_interest(
            "narrow",
            SessionStreamInterest {
                summary_window: 2,
                ..interest(8, &[(&session, Summary)])
            },
        )
        .unwrap();
    h.want("title", 9, &[(&session, Title)]);
    settle().await;

    let frame = only(wide.take());
    assert_eq!((frame.generation, frame.seq), (7, 1));
    assert_eq!(frame.kind, SessionStreamFrameKind::Snapshot);
    let wide_summary = summary(&frame);
    assert_eq!(wide_summary.status, SessionStatus::Running);
    assert_eq!(wide_summary.working_dir.as_deref(), Some("/tmp"));
    assert_eq!(wide_summary.opening_prompt_id, Some(prompt));
    let row = |id, role, content: Option<&str>| SummaryRow {
        id,
        role,
        content: content.map(str::to_string),
        image_ids: Vec::new(),
    };
    assert_eq!(
        wide_summary.rows,
        vec![
            // Neither the opening prompt's text nor a tool result's is sent.
            row(prompt, MessageRole::User, None),
            row(reply, MessageRole::Assistant, Some("On it.")),
            row(result, MessageRole::ToolResult, None),
            row(last, MessageRole::Assistant, Some("Done.")),
        ]
    );

    // The same read, cut to this client's window. The opening prompt is
    // named though its row is outside it.
    let frame = only(narrow.take());
    assert_eq!((frame.generation, frame.seq), (8, 1));
    assert_eq!(row_ids(&frame), vec![result, last]);
    assert_eq!(summary(&frame).opening_prompt_id, Some(prompt));

    let frame = only(title.take());
    assert_eq!((frame.generation, frame.seq), (9, 1));
    assert_eq!(frame.kind, SessionStreamFrameKind::Snapshot);
    let title_summary = summary(&frame);
    assert_eq!(title_summary.status, SessionStatus::Running);
    assert_eq!(title_summary.working_dir, None);
    assert_eq!(title_summary.opening_prompt_id, None);
    assert!(title_summary.rows.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_burst_of_writes_is_one_delta() {
    let h = Harness::new();
    let session = h.session();
    let prompt = h.say(&session, MessageRole::User, "the prompt");
    let client = h.client("a");
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    assert_eq!(shape(&client.take()), vec![snapshot(1, &session)]);
    past_the_interval().await;

    let first = h.say(&session, MessageRole::Assistant, "one");
    let second = h.say(&session, MessageRole::ToolCall, "{}");
    h.store.update_message_content(first, "one, two").unwrap();
    let third = h.say(&session, MessageRole::ToolResult, "output");

    // Nothing until the burst has had its time to gather.
    advance(GATHER - Duration::from_millis(1)).await;
    assert!(client.take().is_empty());
    advance(Duration::from_millis(2)).await;

    let frame = only(client.take());
    assert_eq!(shape(&[frame.clone()]), vec![delta(2, &session)]);
    assert_eq!(row_ids(&frame), vec![first, second, third]);
    let rows = &summary(&frame).rows;
    // The row written twice comes once, as it is now.
    assert_eq!(rows[0].content.as_deref(), Some("one, two"));
    assert_eq!(rows[2].content, None);
    assert_eq!(summary(&frame).opening_prompt_id, Some(prompt));

    // And nothing is owed afterwards.
    advance(Duration::from_secs(2)).await;
    assert!(client.take().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_busy_session_gets_one_frame_per_interval() {
    let h = Harness::new();
    let session = h.session();
    let client = h.client("a");
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;

    // A second of writes, one every 20 ms.
    let mut written = BTreeSet::new();
    for _ in 0..50 {
        written.insert(h.say(&session, MessageRole::Assistant, "chunk"));
        advance(Duration::from_millis(20)).await;
    }
    advance(SUMMARY_INTERVAL + GATHER).await;

    let frames = client.take_timed();
    // The snapshot, then a delta per interval: 1.3 s leaves room for four.
    assert!((4..=5).contains(&frames.len()), "{} frames", frames.len());
    for pair in frames.windows(2) {
        assert!(
            pair[1].0 - pair[0].0 >= SUMMARY_INTERVAL,
            "frames {} and {} are {:?} apart",
            pair[0].1.seq,
            pair[1].1.seq,
            pair[1].0 - pair[0].0
        );
    }
    let seqs: Vec<u64> = frames.iter().map(|(_, frame)| frame.seq).collect();
    assert_eq!(seqs, (1..=frames.len() as u64).collect::<Vec<_>>());
    // Every row arrived all the same.
    let delivered: BTreeSet<i64> = frames
        .iter()
        .flat_map(|(_, frame)| row_ids(frame))
        .collect();
    assert_eq!(delivered, written);
}

#[tokio::test(start_paused = true)]
async fn rows_reach_the_summary_level_and_session_changes_reach_both() {
    let h = Harness::new();
    let session = h.session();
    let call = h.say(&session, MessageRole::ToolCall, "{\"name\":\"Read\"}");
    let titles = h.client("titles");
    let summaries = h.client("summaries");
    h.want("titles", 1, &[(&session, Title)]);
    h.want("summaries", 1, &[(&session, Summary)]);
    settle().await;
    titles.take();
    summaries.take();
    past_the_interval().await;

    // A row: the summary level only.
    let reply = h.say(&session, MessageRole::Assistant, "On it.");
    past_the_gather().await;
    assert!(titles.take().is_empty());
    let frame = only(summaries.take());
    assert_eq!(shape(&[frame.clone()]), vec![delta(2, &session)]);
    assert_eq!(row_ids(&frame), vec![reply]);
    past_the_interval().await;

    // The queue and a row's metadata belong to the full level: nobody.
    h.store
        .add_queued_session_message(&session.id, "next", &[], None)
        .unwrap();
    h.store
        .update_message_acp_metadata(
            call,
            &AcpMessageMetadata {
                acp_event_kind: Some("tool_call_update".to_string()),
                acp_tool_status: Some("completed".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    advance(Duration::from_secs(1)).await;
    assert!(titles.take().is_empty());
    assert!(summaries.take().is_empty());

    // The title: both, each with its own level's fields and no rows.
    h.store
        .update_session_acp_title(&session.id, Some("Fix the flake"))
        .unwrap();
    past_the_gather().await;
    let frame = only(titles.take());
    assert_eq!(shape(&[frame.clone()]), vec![delta(2, &session)]);
    assert_eq!(summary(&frame).acp_title.as_deref(), Some("Fix the flake"));
    assert_eq!(summary(&frame).working_dir, None);
    assert!(summary(&frame).rows.is_empty());
    let frame = only(summaries.take());
    assert_eq!(shape(&[frame.clone()]), vec![delta(3, &session)]);
    assert_eq!(summary(&frame).acp_title.as_deref(), Some("Fix the flake"));
    assert_eq!(summary(&frame).working_dir.as_deref(), Some("/tmp"));
    assert!(summary(&frame).rows.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_session_write_the_level_does_not_carry_sends_nothing() {
    let h = Harness::new();
    let session = h.session();
    let titles = h.client("titles");
    let summaries = h.client("summaries");
    h.want("titles", 1, &[(&session, Title)]);
    h.want("summaries", 1, &[(&session, Summary)]);
    settle().await;
    titles.take();
    summaries.take();
    past_the_interval().await;

    // Fields of the session row that neither level sends.
    h.store.set_agent_session_id(&session.id, "acp-1").unwrap();
    h.store.set_session_provider(&session.id, "claude").unwrap();
    h.store
        .merge_session_provider_metadata(&session.id, &json!({ "rateLimit": { "used": 3 } }))
        .unwrap();
    advance(Duration::from_secs(1)).await;
    assert!(titles.take().is_empty());
    assert!(summaries.take().is_empty());

    // The working directory: the summary level alone.
    h.store
        .prepare_queued_session(&session.id, "/wt/feature", "the full prompt")
        .unwrap();
    past_the_gather().await;
    assert!(titles.take().is_empty());
    let frame = only(summaries.take());
    assert_eq!(shape(&[frame.clone()]), vec![delta(2, &session)]);
    assert_eq!(summary(&frame).working_dir.as_deref(), Some("/wt/feature"));
    past_the_interval().await;

    // The status: both.
    h.store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    past_the_gather().await;
    let frame = only(titles.take());
    assert_eq!(shape(&[frame.clone()]), vec![delta(2, &session)]);
    assert_eq!(summary(&frame).status, SessionStatus::Completed);
    let frame = only(summaries.take());
    assert_eq!(shape(&[frame.clone()]), vec![delta(3, &session)]);
    assert_eq!(summary(&frame).status, SessionStatus::Completed);
}

#[tokio::test(start_paused = true)]
async fn a_session_gets_its_snapshot_before_any_delta() {
    let h = Harness::new();
    let session = h.session();
    let prompt = h.say(&session, MessageRole::User, "the prompt");
    let client = h.client("a");
    h.want("a", 1, &[(&session, Summary)]);
    // Written before the hub has read anything for the interest.
    let reply = h.say(&session, MessageRole::Assistant, "already here");
    advance(Duration::from_secs(1)).await;

    // The snapshot holds the row, and no delta repeats it.
    let frame = only(client.take());
    assert_eq!(shape(&[frame.clone()]), vec![snapshot(1, &session)]);
    assert_eq!(row_ids(&frame), vec![prompt, reply]);

    // A session added to the interest while it is being written to.
    let other = h.session();
    let opening = h.say(&other, MessageRole::User, "another prompt");
    h.want("a", 1, &[(&session, Summary), (&other, Summary)]);
    let answer = h.say(&other, MessageRole::Assistant, "mid-burst");
    advance(Duration::from_secs(1)).await;

    let frame = only(client.take());
    assert_eq!(shape(&[frame.clone()]), vec![snapshot(2, &other)]);
    assert_eq!(row_ids(&frame), vec![opening, answer]);
}

#[tokio::test(start_paused = true)]
async fn raising_a_level_snapshots_and_lowering_it_drops_the_rows() {
    let h = Harness::new();
    let session = h.session();
    let prompt = h.say(&session, MessageRole::User, "the prompt");
    let client = h.client("a");

    h.want("a", 1, &[(&session, Title)]);
    settle().await;
    let frame = only(client.take());
    assert_eq!(shape(&[frame.clone()]), vec![snapshot(1, &session)]);
    assert!(summary(&frame).rows.is_empty());

    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    let frame = only(client.take());
    assert_eq!(shape(&[frame.clone()]), vec![snapshot(2, &session)]);
    assert_eq!(row_ids(&frame), vec![prompt]);
    assert_eq!(summary(&frame).working_dir.as_deref(), Some("/tmp"));

    // Lowered: the client already holds more than the title level sends.
    h.want("a", 1, &[(&session, Title)]);
    settle().await;
    assert!(client.take().is_empty());
    past_the_interval().await;
    h.say(&session, MessageRole::Assistant, "not for the title level");
    advance(Duration::from_secs(1)).await;
    assert!(client.take().is_empty());

    h.store
        .update_session_acp_title(&session.id, Some("Fix the flake"))
        .unwrap();
    past_the_gather().await;
    let frame = only(client.take());
    assert_eq!(shape(&[frame.clone()]), vec![delta(3, &session)]);
    assert!(summary(&frame).rows.is_empty());
    assert_eq!(summary(&frame).working_dir, None);
}
