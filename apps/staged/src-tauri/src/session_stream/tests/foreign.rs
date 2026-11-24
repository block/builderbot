//! A second Staged process on the same database. Its writes mark nothing on
//! this process's feed, and the hub finds them by the database's
//! `data_version`, which it reads only while a client tracks a session this
//! process is not running.
//!
//! The other process is a second store on the same database file. One thing
//! about it cannot be had from the store's own writers: they stamp
//! `owner_pid` with the pid of the process they run in, which in a test is
//! this one. Where the other instance has to own a row, the row is created
//! with another pid or claimed with SQL.

use std::path::Path;

use rusqlite::params;
use serde_json::json;
use tokio::time::Instant;

use crate::store::{MessageRole, Session, SessionStatus, Store};

use super::super::watch::FOREIGN_CHECK;
use super::super::SessionStreamLevel::{Summary, Title};
use super::super::{SessionStreamFrame, SessionStreamFrameKind, SessionStreamInterest};
use super::*;

pub(super) fn another_pid() -> u32 {
    std::process::id().wrapping_add(1)
}

/// A session the other instance is running. Not written anywhere yet.
pub(super) fn theirs() -> Session {
    let mut session = Session::new_running("work", Path::new("/tmp"));
    session.owner_pid = Some(another_pid());
    session
}

/// A session the other instance has started.
pub(super) fn running_elsewhere(other: &Store) -> Session {
    let session = theirs();
    other.create_session(&session).unwrap();
    session
}

/// The other instance takes the row as `running`, as its claim of a queued
/// row or its resume of an ended one would.
pub(super) fn claim_elsewhere(other: &Store, session: &Session) {
    let changed = other
        .hold_connection()
        .execute(
            "UPDATE sessions SET status = 'running', owner_pid = ?1 WHERE id = ?2",
            params![another_pid(), session.id],
        )
        .unwrap();
    assert_eq!(changed, 1);
}

pub(super) fn say(store: &Store, session: &Session, role: MessageRole, content: &str) -> i64 {
    store
        .add_session_message(&session.id, role, content)
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn another_instances_session_follows_within_a_second() {
    let (h, other) = Harness::beside_another_instance();
    let session = running_elsewhere(&other);
    let prompt = say(&other, &session, MessageRole::User, "the prompt");
    let client = h.client("a");
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    let frame = only(client.take());
    assert_eq!(
        shape(std::slice::from_ref(&frame)),
        vec![snapshot(1, &session)]
    );
    assert_eq!(row_ids(&frame), vec![prompt]);

    // A row it adds arrives alone: the prompt is held already.
    let wrote = Instant::now();
    let reply = say(&other, &session, MessageRole::Assistant, "Looking");
    advance(FOREIGN_CHECK).await;
    let mut frames = client.take_timed();
    assert_eq!(frames.len(), 1, "expected one frame, got {frames:?}");
    let (at, frame) = frames.remove(0);
    assert!(at - wrote <= FOREIGN_CHECK);
    assert_eq!(
        shape(std::slice::from_ref(&frame)),
        vec![delta(2, &session)]
    );
    assert_eq!(row_ids(&frame), vec![reply]);

    // A row it rewrites in place, as streamed text is.
    other
        .update_message_content(reply, "Looking at the tests")
        .unwrap();
    advance(FOREIGN_CHECK).await;
    let frame = only(client.take());
    assert_eq!(
        shape(std::slice::from_ref(&frame)),
        vec![delta(3, &session)]
    );
    assert_eq!(row_ids(&frame), vec![reply]);
    assert_eq!(
        summary(&frame).rows[0].content.as_deref(),
        Some("Looking at the tests")
    );

    // A write to the session row.
    other
        .update_session_acp_title(&session.id, Some("Fix the flake"))
        .unwrap();
    advance(FOREIGN_CHECK).await;
    let frame = only(client.take());
    assert_eq!(
        shape(std::slice::from_ref(&frame)),
        vec![delta(4, &session)]
    );
    assert!(row_ids(&frame).is_empty());
    assert_eq!(summary(&frame).acp_title.as_deref(), Some("Fix the flake"));
}

#[tokio::test(start_paused = true)]
async fn a_commit_that_leaves_the_session_alone_sends_nothing() {
    let (h, other) = Harness::beside_another_instance();
    let session = running_elsewhere(&other);
    let bystander = running_elsewhere(&other);
    say(&other, &session, MessageRole::User, "the prompt");
    let client = h.client("a");
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    assert_eq!(shape(&client.take()), vec![snapshot(1, &session)]);

    // The other instance writes a session nobody here asked for. The
    // version moves, the session is read again, and nothing differs.
    let rounds = h.rounds();
    say(&other, &bystander, MessageRole::Assistant, "elsewhere");
    advance(FOREIGN_CHECK * 3).await;
    assert!(client.take().is_empty());
    assert_eq!(
        h.rounds(),
        rounds + 4,
        "three version reads and one re-read"
    );

    // This process's own write to the session goes by its mark, at the
    // usual cadence, and does not move the version it reads.
    let here = h.say(&session, MessageRole::Assistant, "from here");
    past_the_gather().await;
    let frame = only(client.take());
    assert_eq!(
        shape(std::slice::from_ref(&frame)),
        vec![delta(2, &session)]
    );
    assert_eq!(row_ids(&frame), vec![here]);
    let rounds = h.rounds();
    advance(FOREIGN_CHECK * 3).await;
    assert!(client.take().is_empty());
    assert_eq!(h.rounds(), rounds + 3, "version reads only, no re-read");
}

#[tokio::test(start_paused = true)]
async fn a_write_made_here_does_not_hide_one_made_there_just_before() {
    let (h, other) = Harness::beside_another_instance();
    let session = running_elsewhere(&other);
    say(&other, &session, MessageRole::User, "the prompt");
    let client = h.client("a");
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    client.take();
    past_the_interval().await;

    // The other instance adds a row, and before the version is next read
    // this one writes the session's title. The frame for the title reads
    // the session fields and no row.
    let reply = say(&other, &session, MessageRole::Assistant, "Looking");
    h.store
        .update_session_acp_title(&session.id, Some("Fix the flake"))
        .unwrap();
    past_the_gather().await;
    let frame = only(client.take());
    assert_eq!(summary(&frame).acp_title.as_deref(), Some("Fix the flake"));
    assert!(row_ids(&frame).is_empty());

    // That read was not of the newest rows, so it does not count as having
    // caught up with the other instance: its row still arrives.
    advance(FOREIGN_CHECK * 2).await;
    let frame = only(client.take());
    assert_eq!(row_ids(&frame), vec![reply]);
}

#[tokio::test(start_paused = true)]
async fn only_the_newest_window_is_compared() {
    let (h, other) = Harness::beside_another_instance();
    let session = running_elsewhere(&other);
    let old = say(&other, &session, MessageRole::User, "the prompt");
    say(&other, &session, MessageRole::Assistant, "one");
    let client = h.client("a");
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    assert_eq!(shape(&client.take()), vec![snapshot(1, &session)]);

    // More rows than the window between two checks: the client would trim
    // the older ones away, so they are not sent.
    let written: Vec<i64> = (0..WINDOW + 2)
        .map(|n| {
            say(
                &other,
                &session,
                MessageRole::Assistant,
                &format!("more {n}"),
            )
        })
        .collect();
    advance(FOREIGN_CHECK).await;
    let frame = only(client.take());
    assert_eq!(row_ids(&frame), written[written.len() - WINDOW as usize..]);

    // A rewrite of a row that has left the window is not the client's to
    // hold either.
    other.update_message_content(old, "rewritten").unwrap();
    advance(FOREIGN_CHECK * 2).await;
    assert!(client.take().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_data_version_is_read_only_while_a_session_is_not_this_processs() {
    let (h, other) = Harness::beside_another_instance();
    let ours = h.session();
    let theirs = running_elsewhere(&other);
    let client = h.client("a");
    let paused = |paused| SessionStreamInterest {
        paused,
        ..interest(1, &[(&ours, Summary), (&theirs, Title)])
    };

    // Running here: the hub is parked and goes to the store for nothing.
    h.want("a", 1, &[(&ours, Summary)]);
    settle().await;
    let rounds = h.rounds();
    advance(FOREIGN_CHECK * 10).await;
    assert_eq!(h.rounds(), rounds);

    // Running elsewhere: one read of the version a second.
    h.hub.set_interest("a", paused(false)).unwrap();
    settle().await;
    let rounds = h.rounds();
    advance(FOREIGN_CHECK * 10).await;
    assert_eq!(h.rounds(), rounds + 10);

    // Not for a hidden window.
    h.hub.set_interest("a", paused(true)).unwrap();
    settle().await;
    let rounds = h.rounds();
    advance(FOREIGN_CHECK * 10).await;
    assert_eq!(h.rounds(), rounds);

    // And not once the session is let go of.
    h.hub.set_interest("a", paused(false)).unwrap();
    settle().await;
    h.want("a", 1, &[(&ours, Summary)]);
    settle().await;
    let rounds = h.rounds();
    advance(FOREIGN_CHECK * 10).await;
    assert_eq!(h.rounds(), rounds);

    // None of it sent anything but the snapshots.
    let kinds: Vec<_> = client.take().iter().map(|frame| frame.kind).collect();
    assert!(kinds
        .iter()
        .all(|kind| *kind == SessionStreamFrameKind::Snapshot));
}

#[tokio::test(start_paused = true)]
async fn a_session_that_ends_here_is_watched_until_it_runs_here_again() {
    let (h, other) = Harness::beside_another_instance();
    let session = h.session();
    h.say(&session, MessageRole::User, "the prompt");
    let client = h.client("a");
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    assert_eq!(shape(&client.take()), vec![snapshot(1, &session)]);

    h.store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    past_the_interval().await;
    let frame = only(client.take());
    assert_eq!(summary(&frame).status, SessionStatus::Completed);

    // Ended, it can be resumed by any instance, so it is watched now. With
    // nothing written the checks send nothing.
    let rounds = h.rounds();
    advance(FOREIGN_CHECK * 2).await;
    assert!(h.rounds() >= rounds + 2);
    assert!(client.take().is_empty());

    // The other instance resumes it.
    claim_elsewhere(&other, &session);
    let reply = say(
        &other,
        &session,
        MessageRole::Assistant,
        "Resumed elsewhere",
    );
    advance(FOREIGN_CHECK).await;
    let frame = only(client.take());
    assert_eq!(
        shape(std::slice::from_ref(&frame)),
        vec![delta(3, &session)]
    );
    assert_eq!(summary(&frame).status, SessionStatus::Running);
    assert_eq!(row_ids(&frame), vec![reply]);

    // It ends there and is resumed here: this process's marks cover it
    // again, and the version is left alone.
    other
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    advance(FOREIGN_CHECK).await;
    assert_eq!(
        summary(&only(client.take())).status,
        SessionStatus::Completed
    );
    assert!(h.store.transition_to_running(&session.id).unwrap());
    past_the_interval().await;
    assert_eq!(summary(&only(client.take())).status, SessionStatus::Running);
    let rounds = h.rounds();
    advance(FOREIGN_CHECK * 5).await;
    assert_eq!(h.rounds(), rounds);
}

#[tokio::test(start_paused = true)]
async fn a_queued_session_another_instance_starts_is_followed() {
    let (h, other) = Harness::beside_another_instance();
    // Queued rows have no owner: whichever instance drains the branch
    // claims it.
    let session = Session::new_queued("work");
    h.store.create_session(&session).unwrap();
    let client = h.client("a");
    h.want("a", 1, &[(&session, Summary)]);
    settle().await;
    let frame = only(client.take());
    assert_eq!(summary(&frame).status, SessionStatus::Queued);

    claim_elsewhere(&other, &session);
    let prompt = say(&other, &session, MessageRole::User, "the prompt");
    let reply = say(&other, &session, MessageRole::Assistant, "On it");
    advance(FOREIGN_CHECK).await;
    let frame = only(client.take());
    assert_eq!(
        shape(std::slice::from_ref(&frame)),
        vec![delta(2, &session)]
    );
    assert_eq!(summary(&frame).status, SessionStatus::Running);
    assert_eq!(row_ids(&frame), vec![prompt, reply]);
    assert_eq!(summary(&frame).opening_prompt_id, Some(prompt));
}

#[tokio::test(start_paused = true)]
async fn a_session_another_instance_creates_later_gets_its_snapshot() {
    let (h, other) = Harness::beside_another_instance();
    let session = theirs();
    let client = h.client("a");
    h.want("a", 1, &[(&session, Summary)]);
    advance(FOREIGN_CHECK * 2).await;
    assert!(client.take().is_empty());

    other.create_session(&session).unwrap();
    let prompt = say(&other, &session, MessageRole::User, "the prompt");
    advance(FOREIGN_CHECK).await;
    let frame = only(client.take());
    assert_eq!(
        shape(std::slice::from_ref(&frame)),
        vec![snapshot(1, &session)]
    );
    assert_eq!(row_ids(&frame), vec![prompt]);
}

#[tokio::test(start_paused = true)]
async fn the_title_level_follows_the_session_row_and_not_the_transcript() {
    let (h, other) = Harness::beside_another_instance();
    let session = running_elsewhere(&other);
    let client = h.client("a");
    h.want("a", 1, &[(&session, Title)]);
    settle().await;
    assert_eq!(shape(&client.take()), vec![snapshot(1, &session)]);

    say(&other, &session, MessageRole::Assistant, "Looking");
    advance(FOREIGN_CHECK * 2).await;
    assert!(client.take().is_empty());

    other
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    advance(FOREIGN_CHECK).await;
    let frame = only(client.take());
    assert_eq!(
        shape(std::slice::from_ref(&frame)),
        vec![delta(2, &session)]
    );
    assert_eq!(summary(&frame).status, SessionStatus::Completed);
    assert_eq!(summary(&frame).working_dir, None);
    assert!(row_ids(&frame).is_empty());
}

#[tokio::test(start_paused = true)]
async fn two_clients_are_each_sent_what_they_do_not_hold() {
    let (h, other) = Harness::beside_another_instance();
    let session = running_elsewhere(&other);
    let prompt = say(&other, &session, MessageRole::User, "the prompt");
    let early = h.client("early");
    h.want("early", 1, &[(&session, Summary)]);
    settle().await;
    assert_eq!(row_ids(&only(early.take())), vec![prompt]);

    // The second client's snapshot already holds the row the first has yet
    // to be sent.
    let reply = say(&other, &session, MessageRole::Assistant, "Looking");
    let late = h.client("late");
    h.want("late", 1, &[(&session, Summary)]);
    settle().await;
    assert_eq!(row_ids(&only(late.take())), vec![prompt, reply]);

    advance(FOREIGN_CHECK).await;
    assert_eq!(row_ids(&only(early.take())), vec![reply]);
    assert!(late.take().is_empty());
}

#[test]
fn a_resync_frame_names_its_kind_and_carries_nothing_else() {
    let frame = SessionStreamFrame {
        generation: 2,
        seq: 7,
        session_id: "s".to_string(),
        kind: SessionStreamFrameKind::Resync,
        summary: None,
        full: None,
    };

    assert_eq!(
        serde_json::to_value(&frame).unwrap(),
        json!({ "generation": 2, "seq": 7, "sessionId": "s", "kind": "resync" })
    );
}
