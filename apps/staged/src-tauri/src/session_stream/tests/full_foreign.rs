//! The full level beside a second Staged process on the same database (see
//! `foreign.rs` for how the tests stand one up).
//!
//! The rows a chat holds are too many to compare as the summary level
//! compares its window, so a chat on a session this process is not running
//! is asked to resync. Of a session this process *is* running, another
//! process can write the session row alone, and that row is read again.

use crate::store::{MessageRole, SessionPollRequest, SessionStatus};

use super::super::full::FULL_INTERVAL;
use super::super::watch::FOREIGN_CHECK;
use super::super::SessionStreamFrameKind::{Delta, Resync, Snapshot};
use super::super::SessionStreamInterest;
use super::super::SessionStreamLevel::Summary;
use super::foreign::{running_elsewhere, say};
use super::*;

fn since(since_id: i64) -> Option<SessionPollRequest> {
    Some(SessionPollRequest {
        since_id,
        ..Default::default()
    })
}

#[tokio::test(start_paused = true)]
async fn a_chat_on_another_instances_session_is_asked_to_resync() {
    let (h, other) = Harness::beside_another_instance();
    let session = running_elsewhere(&other);
    let prompt = say(&other, &session, MessageRole::User, "the prompt");
    let client = h.client("a");
    h.want_chats("a", 1, vec![chat(&session, from_the_start())]);
    settle().await;
    let frame = only(client.take());
    assert_eq!((frame.seq, frame.kind), (1, Snapshot));
    assert_eq!(full_ids(&frame).0, vec![prompt]);

    // Nothing is written: the version is read and nothing is asked.
    advance(FOREIGN_CHECK * 3).await;
    assert!(client.take().is_empty());

    // The other instance writes a row. Which row is not known here, so the
    // client is told to ask, and is sent nothing to merge.
    let wrote = Instant::now();
    let reply = say(&other, &session, MessageRole::Assistant, "Looking");
    advance(FOREIGN_CHECK).await;
    let mut frames = client.take_timed();
    assert_eq!(frames.len(), 1, "expected one frame, got {frames:?}");
    let (at, frame) = frames.remove(0);
    assert!(at - wrote <= FOREIGN_CHECK);
    assert_eq!((frame.seq, frame.kind), (2, Resync));
    assert!(frame.summary.is_none() && frame.full.is_none());

    // It is asked once, however much more is written before it answers.
    let more = say(&other, &session, MessageRole::Assistant, "at the tests");
    other
        .update_session_acp_title(&session.id, Some("Fix the flake"))
        .unwrap();
    advance(FOREIGN_CHECK * 5).await;
    assert!(client.take().is_empty());

    // Its answer is its cursor, and the snapshot from it holds everything
    // the other instance wrote.
    h.want_chats("a", 1, vec![chat(&session, since(prompt))]);
    settle().await;
    let frame = only(client.take());
    assert_eq!((frame.seq, frame.kind), (3, Snapshot));
    // (A cursor's `since_id` is the pane's newest row, which is read again.)
    assert_eq!(full_ids(&frame).0, vec![prompt, reply, more]);
    assert_eq!(
        full(&frame).session.as_ref().unwrap().acp_title.as_deref(),
        Some("Fix the flake")
    );

    // Up to date again, it is asked again only after another write.
    advance(FOREIGN_CHECK * 3).await;
    assert!(client.take().is_empty());
    other.update_message_content(more, "at the tests.").unwrap();
    advance(FOREIGN_CHECK).await;
    assert_eq!(only(client.take()).kind, Resync);
}

#[tokio::test(start_paused = true)]
async fn a_resync_is_at_most_one_a_second() {
    let (h, other) = Harness::beside_another_instance();
    let session = running_elsewhere(&other);
    let prompt = say(&other, &session, MessageRole::User, "the prompt");
    let reply = say(&other, &session, MessageRole::Assistant, "");
    let client = h.client("a");
    h.want_chats("a", 1, vec![chat(&session, from_the_start())]);
    settle().await;
    client.take();

    // The other instance streams text every 20 ms for three seconds, and
    // the client answers each resync as soon as it gets it.
    let mut resyncs = Vec::new();
    let mut text = String::new();
    for _ in 0..150 {
        text.push_str("word ");
        other.update_message_content(reply, &text).unwrap();
        advance(Duration::from_millis(20)).await;
        for (at, frame) in client.take_timed() {
            if frame.kind == Resync {
                resyncs.push(at);
                h.want_chats("a", 1, vec![chat(&session, since(prompt))]);
            } else {
                assert_eq!(frame.kind, Snapshot);
            }
        }
    }
    assert!(
        (2..=4).contains(&resyncs.len()),
        "{} resyncs",
        resyncs.len()
    );
    for pair in resyncs.windows(2) {
        assert!(
            pair[1] - pair[0] >= FOREIGN_CHECK,
            "resyncs {:?} apart",
            pair[1] - pair[0]
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_row_and_a_chat_on_another_instances_session_each_follow_it() {
    let (h, other) = Harness::beside_another_instance();
    let session = running_elsewhere(&other);
    say(&other, &session, MessageRole::User, "the prompt");
    let client = h.client("a");
    h.want_chats(
        "a",
        1,
        vec![
            SessionInterest {
                session_id: session.id.clone(),
                level: Summary,
                cursor: None,
            },
            chat(&session, from_the_start()),
        ],
    );
    settle().await;
    assert_eq!(client.take().len(), 2);

    // The row's few rows are compared and sent; the chat is asked.
    let reply = say(&other, &session, MessageRole::Assistant, "Looking");
    advance(FOREIGN_CHECK).await;
    let mut frames = client.take();
    frames.sort_by_key(|frame| frame.summary.is_none());
    assert_eq!(frames.len(), 2, "expected two frames, got {frames:?}");
    assert_eq!(frames[0].kind, Delta);
    assert_eq!(row_ids(&frames[0]), vec![reply]);
    assert_eq!(frames[1].kind, Resync);
    assert!(frames[1].full.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_stop_from_another_instance_reaches_a_chat_on_a_session_running_here() {
    let (h, other) = Harness::beside_another_instance();
    let session = h.session();
    h.say(&session, MessageRole::User, "the prompt");
    let reply = h.say(&session, MessageRole::Assistant, "Working");
    let client = h.client("a");
    h.want_chats("a", 1, vec![chat(&session, from_the_start())]);
    settle().await;
    assert_eq!(only(client.take()).kind, Snapshot);

    // The other instance's Stop writes the status and tells nobody here.
    let stopped = Instant::now();
    other
        .update_session_status(&session.id, SessionStatus::Cancelled, None, None)
        .unwrap();
    advance(FOREIGN_CHECK).await;

    // The session row is this process's to read, and it is all that is.
    // With it the session stops being one that runs here, so it is one
    // another instance may write from now on, and the client is asked once
    // for whatever that may have been.
    let mut frames = client.take_timed();
    assert_eq!(frames.len(), 2, "expected two frames, got {frames:?}");
    let (at, frame) = frames.remove(0);
    assert!(at - stopped <= FOREIGN_CHECK);
    assert_eq!((frame.seq, frame.kind), (2, Delta));
    assert_eq!(
        wire(&full(&frame).session),
        wire(&h.store.get_session(&session.id).unwrap())
    );
    assert_eq!(
        full(&frame).session.as_ref().unwrap().status,
        SessionStatus::Cancelled
    );
    assert_eq!(full_ids(&frame), (vec![], vec![]));
    assert!(full(&frame).queued_messages.is_none());
    assert_eq!((frames[0].1.seq, frames[0].1.kind), (3, Resync));

    // What this process still writes while its turn winds down is marked
    // as ever, asked or not.
    h.store
        .update_message_content(reply, "Working (stopped)")
        .unwrap();
    advance(GATHER + FULL_INTERVAL).await;
    let frame = only(client.take());
    assert_eq!(frame.kind, Delta);
    assert_eq!(full_ids(&frame).0, vec![reply]);

    // And it is not asked again before it answers.
    advance(FOREIGN_CHECK * 3).await;
    assert!(client.take().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_commit_elsewhere_that_leaves_a_session_running_here_alone_sends_nothing() {
    let (h, other) = Harness::beside_another_instance();
    let session = h.session();
    let bystander = running_elsewhere(&other);
    h.say(&session, MessageRole::User, "the prompt");
    let client = h.client("a");
    h.want_chats("a", 1, vec![chat(&session, from_the_start())]);
    settle().await;
    client.take();

    // The session row is read again, is as it was, and is not sent. No
    // resync either: the transcript is this process's own.
    for _ in 0..3 {
        say(&other, &bystander, MessageRole::Assistant, "elsewhere");
        advance(FOREIGN_CHECK).await;
    }
    assert!(client.take().is_empty());

    // And the stream is as it was: the next write here is a delta.
    let reply = h.say(&session, MessageRole::Assistant, "On it.");
    past_the_gather().await;
    let frame = only(client.take());
    assert_eq!((frame.seq, frame.kind), (2, Delta));
    assert_eq!(full_ids(&frame).0, vec![reply]);
    assert!(full(&frame).session.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_chat_on_an_ended_session_is_asked_only_when_another_instance_commits() {
    let (h, other) = Harness::beside_another_instance();
    let session = h.session();
    let prompt = h.say(&session, MessageRole::User, "the prompt");
    h.store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    let bystander = running_elsewhere(&other);
    let client = h.client("a");
    h.want_chats("a", 1, vec![chat(&session, from_the_start())]);
    settle().await;
    client.take();

    // Nobody runs it, and nobody else commits: nothing is sent.
    advance(FOREIGN_CHECK * 5).await;
    assert!(client.take().is_empty());

    // The other instance commits. It may have resumed the session, for all
    // the version says, so the client is asked.
    say(&other, &bystander, MessageRole::Assistant, "elsewhere");
    advance(FOREIGN_CHECK).await;
    assert_eq!(only(client.take()).kind, Resync);

    // The snapshot that answers has nothing new in it, and settles it.
    h.want_chats("a", 1, vec![chat(&session, since(prompt))]);
    settle().await;
    let frame = only(client.take());
    assert_eq!(frame.kind, Snapshot);
    assert_eq!(full_ids(&frame), (vec![prompt], vec![]));
    advance(FOREIGN_CHECK * 5).await;
    assert!(client.take().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_data_version_is_read_while_a_chat_is_open_whoever_runs_the_session() {
    let (h, _other) = Harness::beside_another_instance();
    let session = h.session();
    let client = h.client("a");
    let chat_open = |paused| SessionStreamInterest {
        paused,
        ..chats(1, vec![chat(&session, None)])
    };

    // As a row, a session running here costs nothing while it is quiet.
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    let rounds = h.rounds();
    advance(FOREIGN_CHECK * 10).await;
    assert_eq!(h.rounds(), rounds);

    // As a chat it costs one read of the version a second, which is what
    // finds another instance's Stop.
    h.hub.set_interest("a", chat_open(false)).unwrap();
    settle().await;
    let rounds = h.rounds();
    advance(FOREIGN_CHECK * 10).await;
    assert_eq!(h.rounds(), rounds + 10);

    // Not for a hidden window.
    h.hub.set_interest("a", chat_open(true)).unwrap();
    settle().await;
    let rounds = h.rounds();
    advance(FOREIGN_CHECK * 10).await;
    assert_eq!(h.rounds(), rounds);

    // And not once the chat is closed.
    h.hub.set_interest("a", chat_open(false)).unwrap();
    settle().await;
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    let rounds = h.rounds();
    advance(FOREIGN_CHECK * 10).await;
    assert_eq!(h.rounds(), rounds);

    assert!(client.take().iter().all(|frame| frame.kind == Snapshot));
}
