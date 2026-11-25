use super::*;
use crate::agent::MessageWriter;
use crate::note_tool::{
    new_submission_slot, store_submission, NoteSubmission, NoteSubmissionSource,
};
use crate::store::Session;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// The two rules shared with the chat pane
// ---------------------------------------------------------------------------

/// The case table the chat pane's tests run too (`noteDoneMessage.test.ts`
/// and `noteToolCall.test.ts`).
const SHARED_CASES: &str =
    include_str!("../../../src/lib/features/sessions/testStubs/noteDone.cases.json");

/// The `[label, input]` rows under `rule.verdict` in the shared table.
fn shared_cases(rule: &str, verdict: &str) -> Vec<(String, String)> {
    let table: serde_json::Value = serde_json::from_str(SHARED_CASES).unwrap();
    let rows = table[rule][verdict]
        .as_array()
        .unwrap_or_else(|| panic!("{rule}.{verdict} is missing from the shared cases"));
    assert!(!rows.is_empty(), "{rule}.{verdict} has no cases");
    rows.iter()
        .map(|row| {
            let field = |index: usize| {
                row[index]
                    .as_str()
                    .unwrap_or_else(|| panic!("{rule}.{verdict}: {row} is not [label, input]"))
                    .to_string()
            };
            (field(0), field(1))
        })
        .collect()
}

#[test]
fn done_text_matches_the_chat_pane_on_every_shared_case() {
    for (label, text) in shared_cases("doneText", "accept") {
        assert!(is_note_done_text(&text), "should accept {label}: {text:?}");
    }
    for (label, text) in shared_cases("doneText", "reject") {
        assert!(!is_note_done_text(&text), "should reject {label}: {text:?}");
    }
}

#[test]
fn write_note_tool_name_matches_the_chat_pane_on_every_shared_case() {
    for (label, name) in shared_cases("writeNoteToolName", "accept") {
        assert!(
            is_write_note_tool_name(&name),
            "should accept {label}: {name:?}"
        );
    }
    for (label, name) in shared_cases("writeNoteToolName", "reject") {
        assert!(
            !is_write_note_tool_name(&name),
            "should reject {label}: {name:?}"
        );
    }
}

#[test]
fn the_word_the_tool_result_asks_for_is_a_bare_done() {
    assert_eq!(NOTE_DONE_WORD, "Done");
    assert!(is_note_done_text(NOTE_DONE_WORD));
    // The saved result quotes the word: the request and the check cannot
    // drift apart inside the backend either.
    let result = crate::note_tool::saved_message("Plan", 10);
    assert!(
        result.contains(&format!("send a simple \"{NOTE_DONE_WORD}\" message")),
        "{result}"
    );
}

// ---------------------------------------------------------------------------
// The drift decision
// ---------------------------------------------------------------------------

const CLAUDE_CALL: &str = r#"{"name":"mcp__staged__write_note","input":{"title":"Plan"}}"#;
const GOOSE_CALL: &str = r#"{"name":"staged: write note","input":{"title":"Plan"}}"#;
const CODEX_CALL: &str = r#"{"name":"mcp.staged.write_note","input":{"title":"Plan"}}"#;
const SHELL_CALL: &str = r#"{"name":"Bash","input":{"command":"git status"}}"#;
const SAVED: &str = "Saved note \"Plan\" (10 bytes from the draft file).";

fn user(content: &str) -> TurnRow<'_> {
    TurnRow {
        role: MessageRole::User,
        content,
    }
}

fn assistant(content: &str) -> TurnRow<'_> {
    TurnRow {
        role: MessageRole::Assistant,
        content,
    }
}

fn call(content: &str) -> TurnRow<'_> {
    TurnRow {
        role: MessageRole::ToolCall,
        content,
    }
}

fn result(content: &str) -> TurnRow<'_> {
    TurnRow {
        role: MessageRole::ToolResult,
        content,
    }
}

/// A drift of `messages` messages whose last is `last_message_chars` long.
fn text_drift(messages: usize, last_message_chars: usize) -> NoteDoneDrift {
    NoteDoneDrift {
        messages,
        last_message_chars,
        trailing_done_line: false,
        tool_call_after_text: false,
    }
}

const NO_CLOSING_MESSAGE: NoteDoneDrift = NoteDoneDrift {
    messages: 0,
    last_message_chars: 0,
    trailing_done_line: false,
    tool_call_after_text: false,
};

#[test]
fn a_turn_that_ends_in_a_bare_done_is_not_drift() {
    for (bridge, note_call) in [
        ("Claude Code", CLAUDE_CALL),
        ("Goose", GOOSE_CALL),
        ("Codex", CODEX_CALL),
        ("a row that holds only the name", "mcp.staged.write_note"),
    ] {
        let rows = [
            user("Write the note"),
            assistant("I will look at the code first."),
            call(SHELL_CALL),
            result("clean"),
            call(note_call),
            result(SAVED),
            assistant("Done"),
        ];
        assert_eq!(note_done_drift(true, &rows), None, "{bridge}");
    }
}

#[test]
fn every_spelling_the_filter_hides_is_not_drift() {
    for (label, text) in shared_cases("doneText", "accept") {
        let rows = [user("Write the note"), call(CLAUDE_CALL), assistant(&text)];
        assert_eq!(note_done_drift(true, &rows), None, "{label}: {text:?}");
    }
}

#[test]
fn every_text_the_filter_shows_is_drift() {
    for (label, text) in shared_cases("doneText", "reject") {
        let rows = [user("Write the note"), call(CLAUDE_CALL), assistant(&text)];
        let drift = note_done_drift(true, &rows)
            .unwrap_or_else(|| panic!("{label} should be drift: {text:?}"));
        assert_eq!(drift.messages, 1, "{label}");
        assert_eq!(
            drift.last_message_chars,
            trim_js(&text).chars().count(),
            "{label}"
        );
        assert!(!drift.tool_call_after_text, "{label}");
    }
}

#[test]
fn a_recap_after_the_saved_call_is_drift_with_its_length() {
    let recap = "  I wrote the note. It covers the root cause and the fix.\n";
    let rows = [
        user("Write the note"),
        call(CLAUDE_CALL),
        result(SAVED),
        assistant(recap),
    ];
    assert_eq!(
        note_done_drift(true, &rows),
        Some(text_drift(1, recap.trim().chars().count()))
    );
}

#[test]
fn the_length_counts_characters_not_bytes() {
    let rows = [
        user("Write the note"),
        call(CLAUDE_CALL),
        assistant("Fertig ✅ é"),
    ];
    assert_eq!(note_done_drift(true, &rows), Some(text_drift(1, 10)));
}

#[test]
fn a_turn_that_ends_at_the_call_is_drift_with_no_message() {
    // Text before the call is not the closing message.
    let rows = [
        user("Write the note"),
        assistant("Saving the note now."),
        call(CLAUDE_CALL),
        result(SAVED),
    ];
    assert_eq!(note_done_drift(true, &rows), Some(NO_CLOSING_MESSAGE));
}

#[test]
fn an_empty_slot_is_never_drift() {
    // The call was rejected, or the outbox held nothing: no note was saved,
    // so the result never asked for the word.
    let rows = [
        user("Write the note"),
        call(CLAUDE_CALL),
        result("The draft file is empty."),
        assistant("The draft was empty, so I could not save the note."),
    ];
    assert_eq!(note_done_drift(false, &rows), None);
}

#[test]
fn a_turn_with_no_write_note_call_is_never_drift() {
    // The slot outlives a turn. A Pikchr repair turn that never called the
    // tool, or a chat answer, did not save anything itself.
    let no_tool = [
        user("Why does the test pass?"),
        assistant("Because it mocks the page size."),
    ];
    assert_eq!(note_done_drift(true, &no_tool), None);

    let other_tools = [
        user("Fix the diagram"),
        call(SHELL_CALL),
        result("ok"),
        assistant("I fixed the diagram in the draft."),
    ];
    assert_eq!(note_done_drift(true, &other_tools), None);

    let other_server = [
        user("Write the note"),
        call(r#"{"name":"mcp__other__write_note","input":{}}"#),
        assistant("Saved it to the other server."),
    ];
    assert_eq!(note_done_drift(true, &other_server), None);
}

#[test]
fn only_the_last_turn_is_read() {
    // An earlier turn's recap is not this turn's ending.
    let rows = [
        user("Write the note"),
        call(CLAUDE_CALL),
        assistant("Here is a long recap of the note."),
        user("Add a section on the empty case"),
        call(CLAUDE_CALL),
        result(SAVED),
        assistant("Done"),
    ];
    assert_eq!(note_done_drift(true, &rows), None);

    // And an earlier turn's "Done" does not excuse this one.
    let rows = [
        user("Write the note"),
        call(CLAUDE_CALL),
        assistant("Done"),
        user("Add a section on the empty case"),
        call(CLAUDE_CALL),
        assistant("Added the section."),
    ];
    assert_eq!(note_done_drift(true, &rows), Some(text_drift(1, 18)));
}

#[test]
fn a_message_steered_in_after_the_call_starts_a_new_turn() {
    // What follows a steer answers the user. The pane shows it as a reply
    // under the user's message, and it is not checked.
    let rows = [
        user("Write the note"),
        call(CLAUDE_CALL),
        result(SAVED),
        user("Which line holds the bug?"),
        assistant("Line 42 of src/page.js."),
    ];
    assert_eq!(note_done_drift(true, &rows), None);
}

#[test]
fn rows_with_no_user_row_are_one_turn() {
    let rows = [call(CLAUDE_CALL), result(SAVED), assistant("All saved.")];
    assert_eq!(note_done_drift(true, &rows), Some(text_drift(1, 10)));
}

#[test]
fn other_tool_rows_beside_the_call_do_not_matter() {
    // Codex completes a review item after the call; Claude sometimes issues
    // a shell call in the same message.
    let rows = [
        user("Write the note"),
        call(CODEX_CALL),
        call(r#"{"name":"Guardian Review","input":{}}"#),
        result("approved"),
        result(SAVED),
        call(SHELL_CALL),
        result("clean"),
        assistant("Done"),
    ];
    assert_eq!(note_done_drift(true, &rows), None);
}

#[test]
fn a_second_done_from_a_background_wake_is_not_drift() {
    let rows = [
        user("Write the note"),
        call(CLAUDE_CALL),
        result(SAVED),
        assistant("Done"),
        assistant("Done."),
    ];
    assert_eq!(note_done_drift(true, &rows), None);
}

#[test]
fn a_recap_after_done_is_drift() {
    let rows = [
        user("Write the note"),
        call(CLAUDE_CALL),
        assistant("Done"),
        assistant("One more thing: the test is flaky."),
    ];
    assert_eq!(note_done_drift(true, &rows), Some(text_drift(2, 34)));

    // A reply between the call and the word shows both messages too. The
    // last one is the bare word, so its length is the word's.
    let rows = [
        user("Does the fix cover zero orders?"),
        call(CLAUDE_CALL),
        assistant("Yes, the empty page is returned."),
        assistant("Done"),
    ];
    assert_eq!(note_done_drift(true, &rows), Some(text_drift(2, 4)));
}

#[test]
fn an_answer_that_ends_in_a_done_line_is_drift_and_says_so() {
    let answer = "The bug is in `src/main.rs`.\n\nDone";
    let rows = [
        user("Where is the bug?"),
        call(CLAUDE_CALL),
        assistant(answer),
    ];
    assert_eq!(
        note_done_drift(true, &rows),
        Some(NoteDoneDrift {
            trailing_done_line: true,
            ..text_drift(1, answer.chars().count())
        })
    );

    // A wrapped or punctuated last line counts, as the bare message would.
    let rows = [
        user("Where is the bug?"),
        call(CLAUDE_CALL),
        assistant("It is on line 42.\n**Done.**\n"),
    ];
    assert!(note_done_drift(true, &rows).unwrap().trailing_done_line);

    // The word inside a last sentence does not.
    let rows = [
        user("Where is the bug?"),
        call(CLAUDE_CALL),
        assistant("It is on line 42.\nAll done"),
    ];
    assert!(!note_done_drift(true, &rows).unwrap().trailing_done_line);
}

#[test]
fn a_tool_call_after_the_closing_text_is_drift() {
    // The pane shows the word here: the turn did not end at it.
    let rows = [
        user("Write the note"),
        call(CLAUDE_CALL),
        result(SAVED),
        assistant("Done"),
        call(SHELL_CALL),
        result("clean"),
    ];
    assert_eq!(
        note_done_drift(true, &rows),
        Some(NoteDoneDrift {
            tool_call_after_text: true,
            ..text_drift(1, 4)
        })
    );

    // Text after that tool call is the turn's last message.
    let rows = [
        user("Write the note"),
        call(CLAUDE_CALL),
        assistant("Done"),
        call(SHELL_CALL),
        assistant("The tree is clean."),
    ];
    assert_eq!(
        note_done_drift(true, &rows),
        Some(NoteDoneDrift {
            tool_call_after_text: true,
            ..text_drift(2, 18)
        })
    );
}

#[test]
fn the_last_write_note_call_of_the_turn_is_the_one_that_counts() {
    // A recap between two calls is not the ending: the agent republished
    // and then closed the turn as asked.
    let rows = [
        user("Write the note"),
        call(CLAUDE_CALL),
        result(SAVED),
        assistant("I should add the edge case too."),
        call(CLAUDE_CALL),
        result(SAVED),
        assistant("Done"),
    ];
    assert_eq!(note_done_drift(true, &rows), None);

    // A rejected first call and a saved retry.
    let rows = [
        user("Write the note"),
        call(CLAUDE_CALL),
        result("The title is empty."),
        call(CLAUDE_CALL),
        result(SAVED),
        assistant("Saved on the second try."),
    ];
    assert_eq!(note_done_drift(true, &rows), Some(text_drift(1, 24)));
}

#[test]
fn the_log_line_names_what_followed_the_call() {
    assert_eq!(
        NO_CLOSING_MESSAGE.to_string(),
        "no message after the saved write_note call"
    );
    assert_eq!(
        text_drift(1, 742).to_string(),
        "1 message after the saved write_note call, the last 742 chars"
    );
    assert_eq!(
        NoteDoneDrift {
            trailing_done_line: true,
            ..text_drift(2, 90)
        }
        .to_string(),
        "2 messages after the saved write_note call, the last 90 chars, ending in a \"Done\" line"
    );
    assert_eq!(
        NoteDoneDrift {
            tool_call_after_text: true,
            ..text_drift(1, 4)
        }
        .to_string(),
        "1 message after the saved write_note call, the last 4 chars, with a tool call after \
         the text"
    );
}

// ---------------------------------------------------------------------------
// Read from the store, with rows the real writer stored
// ---------------------------------------------------------------------------

fn saved_slot() -> crate::note_tool::NoteSubmissionSlot {
    let slot = new_submission_slot();
    store_submission(
        &slot,
        NoteSubmission {
            title: "Plan".to_string(),
            body: "The body.".to_string(),
            suggested_next_steps: None,
            source: NoteSubmissionSource::Tool,
        },
    );
    slot
}

/// A session with one prompt row; returns the store, the session id and the
/// prompt's row id, which is where the run's first turn starts.
fn session_with_prompt() -> (Arc<Store>, String, i64) {
    let store = Arc::new(Store::in_memory().unwrap());
    let session = Session::new_running("write a note", std::path::Path::new("."));
    store.create_session(&session).unwrap();
    let turn_start = store
        .add_session_message(&session.id, MessageRole::User, "Write a note")
        .unwrap();
    (store, session.id, turn_start)
}

/// Store a `write_note` call and its saved result through the real writer,
/// titled as `title`, then `closing` as the assistant message after it.
async fn write_saved_turn(store: &Arc<Store>, session_id: &str, title: &str, closing: &str) {
    let writer = MessageWriter::new(session_id.to_string(), Arc::clone(store));
    writer.append_text("Writing the note.").await;
    let input = serde_json::json!({ "title": "Plan" });
    writer.record_tool_call("call-1", title, Some(&input)).await;
    writer.record_tool_result("call-1", SAVED).await;
    if !closing.is_empty() {
        writer.append_text(closing).await;
    }
    writer.finalize().await;
    acp_client::MessageWriter::wait_until_stored(&writer).await;
}

#[tokio::test]
async fn a_stored_turn_is_read_as_each_bridge_titles_the_call() {
    for title in [
        "mcp__staged__write_note",
        "staged: write note",
        "mcp.staged.write_note",
    ] {
        let (store, session_id, turn_start) = session_with_prompt();
        write_saved_turn(&store, &session_id, title, "Done").await;
        assert_eq!(
            settled_turn_note_done_drift(&store, &session_id, turn_start, &saved_slot()),
            None,
            "{title}"
        );

        let (store, session_id, turn_start) = session_with_prompt();
        write_saved_turn(
            &store,
            &session_id,
            title,
            "The note is saved. It covers the fix.",
        )
        .await;
        assert_eq!(
            settled_turn_note_done_drift(&store, &session_id, turn_start, &saved_slot()),
            Some(text_drift(1, 37)),
            "{title}"
        );

        let (store, session_id, turn_start) = session_with_prompt();
        write_saved_turn(&store, &session_id, title, "").await;
        assert_eq!(
            settled_turn_note_done_drift(&store, &session_id, turn_start, &saved_slot()),
            Some(NO_CLOSING_MESSAGE),
            "{title}"
        );
    }
}

#[tokio::test]
async fn a_stored_turn_with_an_empty_slot_reports_nothing() {
    let (store, session_id, turn_start) = session_with_prompt();
    write_saved_turn(&store, &session_id, "mcp__staged__write_note", "A recap.").await;
    assert_eq!(
        settled_turn_note_done_drift(&store, &session_id, turn_start, &new_submission_slot()),
        None
    );
}

#[tokio::test]
async fn a_later_turn_of_the_run_is_read_from_its_own_prompt() {
    // The run's first turn ended with a recap. The automatic follow-up (or a
    // Pikchr repair) is a stored user row, and its turn ended as asked. The
    // check still starts at the run's first prompt.
    let (store, session_id, turn_start) = session_with_prompt();
    write_saved_turn(
        &store,
        &session_id,
        "mcp__staged__write_note",
        "A long recap.",
    )
    .await;
    let slot = saved_slot();
    assert_eq!(
        settled_turn_note_done_drift(&store, &session_id, turn_start, &slot),
        Some(text_drift(1, 13))
    );

    store
        .add_session_message(
            &session_id,
            MessageRole::User,
            "Fix the diagram and republish",
        )
        .unwrap();
    write_saved_turn(&store, &session_id, "mcp__staged__write_note", "Done").await;
    assert_eq!(
        settled_turn_note_done_drift(&store, &session_id, turn_start, &slot),
        None
    );
}
