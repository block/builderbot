//! `Store::read_session_summary`: the session fields and transcript rows the
//! session stream sends at its summary level.

use std::path::Path;

use super::models::*;
use super::{Store, SummaryRow, SummaryRowsQuery};

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

fn newest(count: u32) -> SummaryRowsQuery {
    SummaryRowsQuery {
        newest: count,
        ..Default::default()
    }
}

fn row(id: i64, role: MessageRole, content: Option<&str>) -> SummaryRow {
    SummaryRow {
        id,
        role,
        content: content.map(str::to_string),
        image_ids: Vec::new(),
    }
}

#[test]
fn a_session_with_no_row_reads_as_none() {
    let store = Store::in_memory().unwrap();

    assert!(store
        .read_session_summary("missing", None)
        .unwrap()
        .is_none());
    assert!(store
        .read_session_summary("missing", Some(&newest(5)))
        .unwrap()
        .is_none());
}

#[test]
fn without_a_rows_query_only_the_session_fields_are_read() {
    let (store, session) = store_with_session();
    add(&store, &session, MessageRole::User, "the prompt");
    store
        .update_session_acp_title(&session.id, Some("Fix the flake"))
        .unwrap();
    store
        .update_session_pipeline(&session.id, &PipelineExecution::from_steps(&[]))
        .unwrap();

    let summary = store
        .read_session_summary(&session.id, None)
        .unwrap()
        .unwrap();

    assert_eq!(summary.status, SessionStatus::Running);
    assert_eq!(summary.acp_title.as_deref(), Some("Fix the flake"));
    assert_eq!(summary.working_dir.as_deref(), Some("/wt/feature"));
    assert!(summary.pipeline.is_some());
    assert_eq!(summary.opening_prompt_id, None);
    assert!(summary.rows.is_empty());
}

#[test]
fn the_newest_visible_rows_come_in_id_order_without_the_texts_nobody_reads() {
    let (store, session) = store_with_session();
    let prompt = add(&store, &session, MessageRole::User, "a very long prompt");
    let reply = add(&store, &session, MessageRole::Assistant, "On it.");
    let call = add(
        &store,
        &session,
        MessageRole::ToolCall,
        "{\"name\":\"Read\"}",
    );
    let result = add(&store, &session, MessageRole::ToolResult, "file contents");
    let follow_up = add(&store, &session, MessageRole::User, "and the tests?");

    let summary = store
        .read_session_summary(&session.id, Some(&newest(10)))
        .unwrap()
        .unwrap();

    assert_eq!(summary.opening_prompt_id, Some(prompt));
    assert_eq!(
        summary.rows,
        vec![
            // The opening prompt and a tool result keep their place and
            // lose their text; a later user row is a reply and keeps it.
            row(prompt, MessageRole::User, None),
            row(reply, MessageRole::Assistant, Some("On it.")),
            row(call, MessageRole::ToolCall, Some("{\"name\":\"Read\"}")),
            row(result, MessageRole::ToolResult, None),
            row(follow_up, MessageRole::User, Some("and the tests?")),
        ]
    );

    let last_two = store
        .read_session_summary(&session.id, Some(&newest(2)))
        .unwrap()
        .unwrap();
    assert_eq!(last_two.opening_prompt_id, Some(prompt));
    assert_eq!(
        last_two.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![result, follow_up]
    );
}

#[test]
fn hidden_rows_are_neither_listed_nor_taken_for_the_opening_prompt() {
    let (store, session) = store_with_session();
    let chunk = AcpMessageMetadata {
        acp_event_kind: Some("user_message_chunk".to_string()),
        ..Default::default()
    };
    let hidden = store
        .add_acp_metadata_message_with_role(&session.id, MessageRole::User, &chunk)
        .unwrap();
    let prompt = add(&store, &session, MessageRole::User, "the prompt");

    let summary = store
        .read_session_summary(
            &session.id,
            Some(&SummaryRowsQuery {
                newest: 10,
                ids: vec![hidden],
                opening_prompt_id: None,
            }),
        )
        .unwrap()
        .unwrap();

    assert_eq!(summary.opening_prompt_id, Some(prompt));
    assert_eq!(summary.rows, vec![row(prompt, MessageRole::User, None)]);
}

#[test]
fn rows_asked_for_by_id_join_the_newest_once_each() {
    let (store, session) = store_with_session();
    let prompt = add(&store, &session, MessageRole::User, "the prompt");
    let first = add(&store, &session, MessageRole::Assistant, "one");
    let second = add(&store, &session, MessageRole::Assistant, "two");
    let third = add(&store, &session, MessageRole::Assistant, "three");
    let other = Session::new_running("other", Path::new("/tmp"));
    store.create_session(&other).unwrap();
    let theirs = add(&store, &other, MessageRole::Assistant, "not this session's");

    let summary = store
        .read_session_summary(
            &session.id,
            Some(&SummaryRowsQuery {
                newest: 1,
                // In the newest already, twice over, another session's, and
                // one that does not exist.
                ids: vec![third, first, first, theirs, 9_999],
                opening_prompt_id: None,
            }),
        )
        .unwrap()
        .unwrap();

    assert_eq!(summary.opening_prompt_id, Some(prompt));
    assert_eq!(
        summary.rows,
        vec![
            row(first, MessageRole::Assistant, Some("one")),
            row(third, MessageRole::Assistant, Some("three")),
        ]
    );

    // By id alone, with nothing asked of the newest.
    let by_id = store
        .read_session_summary(
            &session.id,
            Some(&SummaryRowsQuery {
                newest: 0,
                ids: vec![second, prompt],
                opening_prompt_id: None,
            }),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        by_id.rows,
        vec![
            row(prompt, MessageRole::User, None),
            row(second, MessageRole::Assistant, Some("two")),
        ]
    );
}

#[test]
fn a_supplied_opening_prompt_id_is_used_as_given() {
    let (store, session) = store_with_session();
    let prompt = add(&store, &session, MessageRole::User, "the prompt");
    let reply = add(&store, &session, MessageRole::User, "a reply");

    let summary = store
        .read_session_summary(
            &session.id,
            Some(&SummaryRowsQuery {
                newest: 10,
                ids: Vec::new(),
                opening_prompt_id: Some(prompt),
            }),
        )
        .unwrap()
        .unwrap();

    assert_eq!(summary.opening_prompt_id, Some(prompt));
    assert_eq!(
        summary.rows,
        vec![
            row(prompt, MessageRole::User, None),
            row(reply, MessageRole::User, Some("a reply")),
        ]
    );
}

#[test]
fn image_ids_come_with_the_row() {
    let (store, session) = store_with_session();
    let prompt = store
        .add_session_message_with_images(
            &session.id,
            MessageRole::User,
            "look at this",
            &["img-1".to_string(), "img-2".to_string()],
        )
        .unwrap();

    let summary = store
        .read_session_summary(&session.id, Some(&newest(5)))
        .unwrap()
        .unwrap();

    assert_eq!(
        summary.rows,
        vec![SummaryRow {
            id: prompt,
            role: MessageRole::User,
            content: None,
            image_ids: vec!["img-1".to_string(), "img-2".to_string()],
        }]
    );
}

#[test]
fn a_session_with_no_transcript_has_no_opening_prompt() {
    let (store, session) = store_with_session();

    let summary = store
        .read_session_summary(&session.id, Some(&newest(5)))
        .unwrap()
        .unwrap();

    assert_eq!(summary.opening_prompt_id, None);
    assert!(summary.rows.is_empty());
}

#[test]
fn the_owner_is_read_and_never_serialized() {
    let (store, session) = store_with_session();
    let mut queued = Session::new_queued("later");
    queued.owner_pid = None;
    store.create_session(&queued).unwrap();

    let running = store
        .read_session_summary(&session.id, None)
        .unwrap()
        .unwrap();
    assert_eq!(running.owner_pid, Some(std::process::id()));
    let queued = store
        .read_session_summary(&queued.id, None)
        .unwrap()
        .unwrap();
    assert_eq!(queued.owner_pid, None);

    let json = serde_json::to_value(&running).unwrap();
    let fields: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert!(
        fields
            .iter()
            .all(|field| !field.to_lowercase().contains("pid")),
        "the owner went out on the wire: {fields:?}"
    );
}
