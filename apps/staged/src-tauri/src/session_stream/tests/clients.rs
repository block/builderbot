//! Clients coming, going and changing their minds: when the feed is on,
//! pauses, generations, slow and closed sinks, reconnects, sessions with no
//! row, and the browser queue.

use std::time::Duration;

use serde_json::{json, Value};

use crate::store::{MessageRole, Session, SessionPollRequest};

use super::super::hub::SINK_RETRY;
use super::super::web::{web_queue, WEB_QUEUE_FRAMES};
use super::super::SessionStreamLevel::{Full, Summary, Title};
use super::super::{SessionStreamFrameKind, SessionStreamInterest};
use super::*;

#[tokio::test(start_paused = true)]
async fn the_feed_is_on_only_while_someone_would_be_sent_frames() {
    let h = Harness::new();
    let session = h.session();
    let paused = |paused| SessionStreamInterest {
        paused,
        ..interest(1, &[(&session, Summary)])
    };

    // No client: writers record nothing and the hub is not woken.
    h.say(&session, MessageRole::User, "the prompt");
    settle().await;
    assert!(!h.feed.is_active());
    assert!(h.feed.drain().is_empty());

    // A stream with nothing asked of it, or only a hidden window's.
    let client = h.client("a");
    assert!(!h.feed.is_active());
    h.want("a", 1, &[]);
    assert!(!h.feed.is_active());
    h.hub.set_interest("a", paused(true)).unwrap();
    assert!(!h.feed.is_active());
    h.say(&session, MessageRole::Assistant, "unheard");
    settle().await;
    assert!(h.feed.drain().is_empty());
    assert!(client.take().is_empty());

    h.hub.set_interest("a", paused(false)).unwrap();
    assert!(h.feed.is_active());
    h.hub.set_interest("a", paused(true)).unwrap();
    assert!(!h.feed.is_active());

    h.hub.set_interest("a", paused(false)).unwrap();
    assert!(h.feed.is_active());
    h.hub.disconnect("a", None);
    assert!(!h.feed.is_active());
}

#[tokio::test(start_paused = true)]
async fn a_paused_client_keeps_its_interest_and_gets_snapshots_when_it_returns() {
    let h = Harness::new();
    let session = h.session();
    let prompt = h.say(&session, MessageRole::User, "the prompt");
    let client = h.client("a");
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    assert_eq!(shape(&client.take()), vec![snapshot(1, &session)]);

    h.hub
        .set_interest(
            "a",
            SessionStreamInterest {
                paused: true,
                ..interest(1, &[(&session, Summary)])
            },
        )
        .unwrap();
    let first = h.say(&session, MessageRole::Assistant, "while hidden");
    let second = h.say(&session, MessageRole::Assistant, "still hidden");
    advance(Duration::from_secs(2)).await;
    assert!(client.take().is_empty());
    // The stream is still open in the same generation.
    assert!(h.hub.check("a", 1, 1));

    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    let frame = only(client.take());
    assert_eq!(shape(&[frame.clone()]), vec![snapshot(2, &session)]);
    assert_eq!(row_ids(&frame), vec![prompt, first, second]);
}

#[tokio::test(start_paused = true)]
async fn a_full_sink_is_skipped_without_loss() {
    let h = Harness::new();
    let session = h.session();
    let slow = h.client("slow");
    let quick = h.client("quick");
    h.want("slow", 1, &[(&session, Summary)]);
    h.want("quick", 1, &[(&session, Summary)]);
    settle().await;
    slow.take();
    quick.take();
    slow.set_room(0);

    past_the_interval().await;
    let first = h.say(&session, MessageRole::Assistant, "one");
    past_the_gather().await;
    past_the_interval().await;
    let second = h.say(&session, MessageRole::Assistant, "two");
    past_the_gather().await;

    // The other client is sent each change as it comes.
    let frames = quick.take();
    assert_eq!(shape(&frames), vec![delta(2, &session), delta(3, &session)]);
    assert_eq!(row_ids(&frames[0]), vec![first]);
    assert_eq!(row_ids(&frames[1]), vec![second]);
    assert!(slow.take().is_empty());

    // With room again, the slow one gets everything it missed as one frame,
    // and its sequence has no gap.
    slow.set_room(usize::MAX);
    advance(SINK_RETRY).await;
    let frame = only(slow.take());
    assert_eq!(shape(&[frame.clone()]), vec![delta(2, &session)]);
    assert_eq!(row_ids(&frame), vec![first, second]);
    assert!(quick.take().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_new_generation_restarts_the_sequence_with_snapshots() {
    let h = Harness::new();
    let (one, two, three) = (h.session(), h.session(), h.session());
    let client = h.client("a");
    let sessions = |frames: &[SessionStreamFrame]| {
        let mut ids: Vec<String> = frames.iter().map(|f| f.session_id.clone()).collect();
        ids.sort_unstable();
        ids
    };
    let sorted = |sessions: &[&Session]| {
        let mut ids: Vec<String> = sessions.iter().map(|s| s.id.clone()).collect();
        ids.sort_unstable();
        ids
    };
    let seqs = |frames: &[SessionStreamFrame]| frames.iter().map(|f| f.seq).collect::<Vec<_>>();

    h.want("a", 1, &[(&one, Summary), (&two, Summary)]);
    settle().await;
    let frames = client.take();
    assert_eq!(seqs(&frames), vec![1, 2]);
    assert_eq!(sessions(&frames), sorted(&[&one, &two]));

    // The same generation with another list: only the added session gets a
    // snapshot, and the dropped one is no longer heard.
    h.want("a", 1, &[(&two, Summary), (&three, Summary)]);
    settle().await;
    assert_eq!(shape(&client.take()), vec![snapshot(3, &three)]);
    past_the_interval().await;
    h.say(&one, MessageRole::Assistant, "not wanted any more");
    advance(Duration::from_secs(1)).await;
    assert!(client.take().is_empty());

    h.want("a", 2, &[(&two, Summary), (&three, Summary)]);
    settle().await;
    let frames = client.take();
    assert_eq!(seqs(&frames), vec![1, 2]);
    assert_eq!(sessions(&frames), sorted(&[&two, &three]));
    assert!(frames
        .iter()
        .all(|f| f.generation == 2 && f.kind == SessionStreamFrameKind::Snapshot));
}

#[tokio::test(start_paused = true)]
async fn a_session_with_no_row_is_dropped_quietly() {
    let h = Harness::new();
    let live = h.session();
    let unborn = Session::new_queued("later");
    let never = Session::new_queued("never created");
    let client = h.client("a");

    h.want(
        "a",
        1,
        &[(&live, Summary), (&unborn, Summary), (&never, Title)],
    );
    advance(Duration::from_secs(5)).await;
    assert_eq!(shape(&client.take()), vec![snapshot(1, &live)]);

    // The row is written after all: its insert brings the snapshot.
    h.store.create_session(&unborn).unwrap();
    settle().await;
    assert_eq!(shape(&client.take()), vec![snapshot(2, &unborn)]);

    // Deleted while a change to it is still owed.
    past_the_interval().await;
    h.say(&unborn, MessageRole::Assistant, "last words");
    h.store.delete_session(&unborn.id).unwrap();
    h.say(&live, MessageRole::Assistant, "still here");
    past_the_gather().await;
    assert_eq!(shape(&client.take()), vec![delta(3, &live)]);
    advance(Duration::from_secs(5)).await;
    assert!(client.take().is_empty());

    // Asked for again once it is gone.
    h.want("a", 2, &[(&live, Summary), (&unborn, Summary)]);
    advance(Duration::from_secs(5)).await;
    assert_eq!(shape(&client.take()), vec![snapshot(1, &live)]);

    // The hub is still serving.
    assert!(!h.task.is_finished());
    assert!(h.hub.check("a", 2, 1));
    past_the_interval().await;
    h.say(&live, MessageRole::Assistant, "and still here");
    past_the_gather().await;
    assert_eq!(shape(&client.take()), vec![delta(2, &live)]);
}

#[tokio::test(start_paused = true)]
async fn nothing_is_sent_while_there_is_no_store_to_read() {
    let h = Harness::without_store();
    let session = h.session();
    let client = h.client("a");

    h.want("a", 1, &[(&session, Summary)]);
    advance(Duration::from_secs(5)).await;

    assert!(client.take().is_empty());
    assert!(!h.task.is_finished());
}

#[tokio::test(start_paused = true)]
async fn check_compares_the_last_sequence_number_sent() {
    let h = Harness::new();
    let session = h.session();
    let _client = h.client("a");

    // An open stream with no interest yet has no generation.
    assert!(!h.hub.check("a", 1, 0));

    h.want("a", 4, &[(&session, Title)]);
    settle().await;

    assert!(h.hub.check("a", 4, 1));
    assert!(!h.hub.check("a", 4, 0));
    assert!(!h.hub.check("a", 4, 2));
    assert!(!h.hub.check("a", 3, 1));
    assert!(!h.hub.check("nobody", 4, 1));
}

#[tokio::test(start_paused = true)]
async fn interest_is_refused_without_a_stream() {
    let h = Harness::new();
    let session = h.session();

    let err = h
        .hub
        .set_interest("nobody", interest(1, &[(&session, Summary)]))
        .unwrap_err();
    assert!(err.contains("No session stream is open"), "{err}");
    assert!(!h.feed.is_active());

    // And once it has one, at any level.
    let client = h.client("a");
    h.want(
        "a",
        1,
        &[(&session, Title), (&session, Summary), (&session, Full)],
    );
    settle().await;
    assert_eq!(client.take().len(), 2);
    assert!(h.feed.is_active());
}

#[tokio::test(start_paused = true)]
async fn a_closed_sink_removes_its_client() {
    let h = Harness::new();
    let session = h.session();
    let client = h.client("a");
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    client.take();

    client.close();
    past_the_interval().await;
    h.say(&session, MessageRole::Assistant, "to nobody");
    past_the_gather().await;

    assert!(!h.feed.is_active());
    assert!(!h.hub.check("a", 1, 1));
    assert!(h
        .hub
        .set_interest("a", interest(2, &[(&session, Summary)]))
        .is_err());
}

#[tokio::test(start_paused = true)]
async fn a_hub_whose_task_has_ended_holds_no_stream_and_takes_no_interest() {
    let h = Harness::new();
    let session = h.session();
    let client = h.client("a");
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    assert_eq!(shape(&client.take()), vec![snapshot(1, &session)]);
    assert!(h.hub.check("a", 1, 1));

    // The task never returns, so this is what a panic in it leaves behind.
    h.task.abort();
    settle().await;

    // The client's health check finds no stream, and its interest is
    // refused whether or not it opens a new one.
    assert!(!h.hub.check("a", 1, 1));
    assert!(h
        .hub
        .set_interest("a", interest(2, &[(&session, Summary)]))
        .is_err());
    h.client("a");
    assert!(h
        .hub
        .set_interest("a", interest(2, &[(&session, Summary)]))
        .is_err());

    // And writers are back to recording nothing.
    assert!(!h.feed.is_active());
    h.say(&session, MessageRole::Assistant, "unheard");
    assert!(h.feed.drain().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_late_disconnect_does_not_remove_a_reconnected_client() {
    let h = Harness::new();
    let session = h.session();
    let old = Arc::new(Recorder::default());
    let old_connection = h.hub.connect("a", old.clone());
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    assert_eq!(shape(&old.take()), vec![snapshot(1, &session)]);

    // The client reconnects before its old socket is seen to close. The
    // interest went with the old sink, until the client declares it again.
    let new = Arc::new(Recorder::default());
    let new_connection = h.hub.connect("a", new.clone());
    assert!(!h.feed.is_active());
    past_the_interval().await;
    h.say(&session, MessageRole::Assistant, "between streams");
    advance(Duration::from_secs(1)).await;
    assert!(old.take().is_empty());
    assert!(new.take().is_empty());

    h.want("a", 2, &[(&session, Summary)]);
    settle().await;
    assert_eq!(shape(&new.take()), vec![snapshot(1, &session)]);

    // The old socket's close arrives now, and is not this stream's.
    h.hub.disconnect("a", Some(old_connection));
    assert!(h.hub.check("a", 2, 1));
    past_the_interval().await;
    h.say(&session, MessageRole::Assistant, "after the late close");
    past_the_gather().await;
    assert_eq!(shape(&new.take()), vec![delta(2, &session)]);
    assert!(old.take().is_empty());

    h.hub.disconnect("a", Some(new_connection));
    assert!(!h.hub.check("a", 2, 2));
    assert!(!h.feed.is_active());
}

#[tokio::test(start_paused = true)]
async fn a_browser_frame_is_an_event_envelope() {
    let h = Harness::new();
    let session = h.session();
    let prompt = h.say(&session, MessageRole::User, "a long opening prompt");
    let reply = h.say(&session, MessageRole::Assistant, "On it.");
    let (sink, mut frames) = web_queue();
    h.hub.connect("web-1", sink);

    h.want("web-1", 3, &[(&session, Summary)]);
    settle().await;

    let text = frames.try_recv().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!({
            "event": "session-stream",
            "payload": {
                "generation": 3,
                "seq": 1,
                "sessionId": session.id,
                "kind": "snapshot",
                "summary": {
                    "status": "running",
                    "acpTitle": null,
                    "workingDir": "/tmp",
                    "openingPromptId": prompt,
                    "rows": [
                        { "id": prompt, "role": "user" },
                        { "id": reply, "role": "assistant", "content": "On it." },
                    ],
                },
            },
        })
    );
    assert!(frames.try_recv().is_err());

    // The socket goes away: the next frame finds the queue closed.
    drop(frames);
    past_the_interval().await;
    h.say(&session, MessageRole::Assistant, "to nobody");
    past_the_gather().await;
    assert!(!h.hub.check("web-1", 3, 1));
    assert!(!h.feed.is_active());
}

#[tokio::test(start_paused = true)]
async fn a_browser_queue_that_fills_delays_frames_and_loses_none() {
    let h = Harness::new();
    let extra = 6;
    let sessions: Vec<Session> = (0..WEB_QUEUE_FRAMES + extra).map(|_| h.session()).collect();
    let wanted: Vec<(&Session, SessionStreamLevel)> =
        sessions.iter().map(|session| (session, Title)).collect();
    let (sink, mut frames) = web_queue();
    h.hub.connect("web-1", sink);

    // Nobody reads the socket for a while.
    h.want("web-1", 1, &wanted);
    advance(Duration::from_secs(2)).await;

    let seq = |text: String| {
        serde_json::from_str::<Value>(&text).unwrap()["payload"]["seq"]
            .as_u64()
            .unwrap()
    };
    let mut seqs = Vec::new();
    while let Ok(text) = frames.try_recv() {
        seqs.push(seq(text));
    }
    assert_eq!(seqs.len(), WEB_QUEUE_FRAMES);

    // Once it is read, the sessions still owed their snapshot get it.
    advance(SINK_RETRY).await;
    while let Ok(text) = frames.try_recv() {
        seqs.push(seq(text));
    }
    assert_eq!(seqs, (1..=sessions.len() as u64).collect::<Vec<_>>());
    assert!(h.hub.check("web-1", 1, sessions.len() as u64));
}

#[test]
fn an_interest_reads_from_the_wire_names_the_frontend_sends() {
    let interest: SessionStreamInterest = serde_json::from_value(json!({
        "generation": 5,
        "summaryWindow": 20,
        "sessions": [
            { "sessionId": "s-1", "level": "title" },
            { "sessionId": "s-2", "level": "summary" },
            { "sessionId": "s-3", "level": "full" },
            {
                "sessionId": "s-4",
                "level": "full",
                "cursor": {
                    "sinceId": 41,
                    "refetchIds": [38],
                    "metadataSinceId": 40,
                    "metadataRefetchIds": [39, 40],
                },
            },
            // The lists of a cursor may be left out, as in a poll request.
            { "sessionId": "s-5", "level": "full", "cursor": { "sinceId": 0, "metadataSinceId": 0 } },
        ],
    }))
    .unwrap();

    assert_eq!(interest.generation, 5);
    assert!(!interest.paused);
    assert_eq!(interest.summary_window, 20);
    let levels: Vec<(&str, SessionStreamLevel)> = interest
        .sessions
        .iter()
        .map(|session| (session.session_id.as_str(), session.level))
        .collect();
    assert_eq!(
        levels,
        vec![
            ("s-1", Title),
            ("s-2", Summary),
            ("s-3", Full),
            ("s-4", Full),
            ("s-5", Full)
        ]
    );
    let cursors: Vec<Option<&SessionPollRequest>> = interest
        .sessions
        .iter()
        .map(|session| session.cursor.as_ref())
        .collect();
    assert_eq!(
        cursors,
        vec![
            None,
            None,
            None,
            Some(&SessionPollRequest {
                since_id: 41,
                refetch_ids: vec![38],
                metadata_since_id: 40,
                metadata_refetch_ids: vec![39, 40],
            }),
            Some(&SessionPollRequest::default()),
        ]
    );
}
