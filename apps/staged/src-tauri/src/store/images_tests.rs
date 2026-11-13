//! Tests for the image-reference checks in `images.rs`: what counts as a chat
//! reference, and that the check reads the partial index from migration 0049
//! instead of scanning `session_messages`.

use std::path::Path;

use rusqlite::{params, Connection};

use super::images::CHAT_REFERENCE_SQL;
use super::models::*;
use super::Store;

const IMAGE_IDS_INDEX: &str = "idx_session_messages_image_ids";

/// The `detail` column of each `EXPLAIN QUERY PLAN` row for the chat
/// reference check, in plan order.
fn chat_reference_plan(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {CHAT_REFERENCE_SQL}"))
        .unwrap();
    let details = stmt
        .query_map(params!["\"some-image\""], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    details
}

/// Assert the chat reference check reads `session_messages` only through the
/// partial index, and as a covering index, so it never touches a table row.
///
/// Plan details read `SCAN <table> ...` or `SEARCH <table> ...`; the table is
/// matched as a whole word because `queued_session_messages` contains the
/// other table's name.
pub(super) fn assert_chat_reference_check_reads_the_index(conn: &Connection) {
    let plan = chat_reference_plan(conn);
    let message_steps: Vec<&str> = plan
        .iter()
        .map(String::as_str)
        .filter(|detail| detail.split_whitespace().nth(1) == Some("session_messages"))
        .collect();
    assert_eq!(
        message_steps.len(),
        1,
        "expected one step over session_messages in {plan:?}"
    );
    assert!(
        message_steps[0].contains(&format!("USING COVERING INDEX {IMAGE_IDS_INDEX}")),
        "the chat reference check should read {IMAGE_IDS_INDEX}, not scan \
         session_messages; plan: {plan:?}"
    );
}

fn running_session(store: &Store) -> Session {
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    session
}

#[test]
fn test_chat_reference_check_reads_the_image_ids_index() {
    let store = Store::in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    // The app never runs ANALYZE, so this is the plan a real store gets.
    assert_chat_reference_check_reads_the_index(&conn);
}

#[test]
fn test_chat_reference_check_keeps_the_index_once_the_table_has_statistics() {
    let store = Store::in_memory().unwrap();
    let session = running_session(&store);
    // Mostly messages without attachments, as in a real transcript.
    for n in 0..600 {
        let images: Vec<String> = if n % 100 == 0 {
            vec![format!("image-{n}")]
        } else {
            Vec::new()
        };
        store
            .add_session_message_with_images(
                &session.id,
                MessageRole::User,
                "a message long enough to give the table wider rows than the index",
                &images,
            )
            .unwrap();
    }

    let conn = store.conn.lock().unwrap();
    conn.execute_batch("ANALYZE;").unwrap();
    assert_chat_reference_check_reads_the_index(&conn);

    // Only the attaching messages are in the index.
    let indexed: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM session_messages INDEXED BY {IMAGE_IDS_INDEX}
                 WHERE image_ids IS NOT NULL"
            ),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 6);
}

#[test]
fn test_image_has_chat_references_matches_sent_and_queued_attachments_only() {
    let store = Store::in_memory().unwrap();
    let session = running_session(&store);

    let sent = "11111111-aaaa-4aaa-8aaa-000000000001".to_string();
    let also_sent = "11111111-aaaa-4aaa-8aaa-000000000002".to_string();
    let queued = "22222222-bbbb-4bbb-8bbb-000000000001".to_string();
    let mentioned = "33333333-cccc-4ccc-8ccc-000000000001";

    store
        .add_session_message_with_images(
            &session.id,
            MessageRole::User,
            "two attachments",
            &[sent.clone(), also_sent.clone()],
        )
        .unwrap();
    // No attachments: `image_ids` is NULL, and naming an id in the text is
    // not a reference.
    store
        .add_session_message(&session.id, MessageRole::User, mentioned)
        .unwrap();
    store
        .add_queued_session_message(&session.id, "later", std::slice::from_ref(&queued), None)
        .unwrap();

    assert!(store.image_has_chat_references(&sent).unwrap());
    assert!(store.image_has_chat_references(&also_sent).unwrap());
    assert!(store.image_has_chat_references(&queued).unwrap());
    assert!(!store.image_has_chat_references(mentioned).unwrap());
    // The needle is the quoted id, so part of an attached id does not match.
    assert!(!store
        .image_has_chat_references(&sent[..sent.len() - 1])
        .unwrap());
    assert!(!store
        .image_has_chat_references("44444444-dddd-4ddd-8ddd-000000000001")
        .unwrap());
}
