//! Whether a note session's turn ended the way the `write_note` result asks.
//!
//! A saved `write_note` call tells the agent to close its turn with the one
//! word "Done" (`note_tool::saved_message`), and the chat pane hides that
//! message (`hiddenNoteDoneMessageIds` in
//! `src/lib/features/sessions/noteDoneMessage.ts`). Nothing forces the agent
//! to comply: a model that stops following the wording goes back to writing a
//! recap under the note card, and nothing fails. So each settled turn that
//! saved a note and did not end in a bare "Done" is logged, as one line with
//! lengths only. A rising count of those lines means the wording has stopped
//! working.
//!
//! The decision is [`note_done_drift`], a pure function over the turn's
//! stored rows. It follows the chat pane's rule, so a logged turn is one
//! where the pane shows text after the saved call, or where the agent sent
//! no closing message at all:
//!
//! - what counts as "Done" is [`is_note_done_text`], the Rust copy of the
//!   pane's `isNoteDoneText`;
//! - the call is found by tool name with [`is_write_note_tool_name`], the
//!   Rust copy of the pane's `isWriteNoteToolName`.
//!
//! Both live in `note_done/rules.rs`. Each pair is held together by the case
//! table both test suites run,
//! `src/lib/features/sessions/testStubs/noteDone.cases.json`.

mod rules;

use std::fmt;

use crate::note_tool::NoteSubmissionSlot;
use crate::store::{MessageRole, Store};

use rules::{is_note_done_text, is_write_note_tool_name, trim_js, NOTE_DONE_WORD};

/// One stored transcript row, as the drift check reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnRow<'a> {
    pub role: MessageRole,
    pub content: &'a str,
}

/// How a turn that saved a note ended, when it did not end in a bare "Done".
/// It holds counts and lengths only, never the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NoteDoneDrift {
    /// Assistant messages after the last `write_note` call, to the end of
    /// the turn. Zero when the turn ended at the call.
    pub messages: usize,
    /// Length in characters of the last of those messages, trimmed. Zero
    /// when there is none.
    pub last_message_chars: usize,
    /// The last message is longer text whose final line is a bare "Done":
    /// an answer the agent closed with the word. The pane shows it whole.
    pub trailing_done_line: bool,
    /// A tool call came after the first of those messages. The pane then
    /// shows even a bare "Done", since the turn did not end there.
    pub tool_call_after_text: bool,
}

impl fmt::Display for NoteDoneDrift {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.messages == 0 {
            return f.write_str("no message after the saved write_note call");
        }
        write!(
            f,
            "{} {} after the saved write_note call, the last {} chars",
            self.messages,
            if self.messages == 1 {
                "message"
            } else {
                "messages"
            },
            self.last_message_chars
        )?;
        if self.trailing_done_line {
            write!(f, ", ending in a \"{NOTE_DONE_WORD}\" line")?;
        }
        if self.tool_call_after_text {
            f.write_str(", with a tool call after the text")?;
        }
        Ok(())
    }
}

/// How the turn at the end of `rows` drifted from the ending the
/// `write_note` result asks for, or `None` when it did not, or saved no note.
///
/// `rows` are a session's stored rows in order, ending with the turn that
/// just settled. The turn is everything after the last user row, as in the
/// chat pane, so a message steered into the turn starts a new one: what the
/// agent wrote after a steer answers the user, and is not checked.
///
/// A turn saved a note when the run's submission slot is filled
/// (`slot_filled`) and the turn holds a `write_note` call. The slot is the
/// backend's record of a saved note. It covers a remote call whose result
/// never arrived, which the pane resolves from the outbox. The call row
/// places the save in this turn: the slot outlives a turn, so on its own it
/// would also match a repair turn that never called the tool. The pane
/// reads the call's own status instead, so the two differ in one rare case:
/// a saved call followed by a rejected one in the same turn. The pane then
/// shows the closing word and this reports nothing.
///
/// From the last `write_note` call on, the rule is the pane's
/// (`hiddenNoteDoneMessageIds`):
///
/// 1. The tool rows directly after the call belong to its tool group and are
///    stepped over. Codex completes a review item there, and Claude
///    sometimes issues a shell call beside the note call.
/// 2. Then comes a run of assistant messages. Each must be a bare "Done"
///    ([`is_note_done_text`]), and there must be at least one.
/// 3. The turn must end with that run, not at another tool call.
///
/// Anything else is drift. An empty run is drift too: the pane has nothing
/// to hide, but the agent ended the turn without the message it was asked
/// for.
pub(crate) fn note_done_drift(slot_filled: bool, rows: &[TurnRow<'_>]) -> Option<NoteDoneDrift> {
    if !slot_filled {
        return None;
    }
    let turn = match rows.iter().rposition(|row| row.role == MessageRole::User) {
        Some(last_user) => &rows[last_user + 1..],
        None => rows,
    };
    let call = turn.iter().rposition(|row| {
        row.role == MessageRole::ToolCall
            && is_write_note_tool_name(&crate::agent::writer::stored_tool_call_title(row.content))
    })?;
    let after_call = &turn[call + 1..];
    let is_tool_row =
        |row: &TurnRow<'_>| matches!(row.role, MessageRole::ToolCall | MessageRole::ToolResult);
    let group_end = after_call
        .iter()
        .position(|row| !is_tool_row(row))
        .unwrap_or(after_call.len());
    let closing = &after_call[group_end..];
    let run_end = closing
        .iter()
        .position(|row| row.role != MessageRole::Assistant)
        .unwrap_or(closing.len());
    let (run, rest) = closing.split_at(run_end);

    let ends_in_done =
        !run.is_empty() && rest.is_empty() && run.iter().all(|row| is_note_done_text(row.content));
    if ends_in_done {
        return None;
    }

    let messages: Vec<&str> = closing
        .iter()
        .filter(|row| row.role == MessageRole::Assistant)
        .map(|row| row.content)
        .collect();
    let last = messages.last().copied().map(trim_js).unwrap_or_default();
    Some(NoteDoneDrift {
        messages: messages.len(),
        last_message_chars: last.chars().count(),
        trailing_done_line: !is_note_done_text(last)
            && last.lines().next_back().is_some_and(is_note_done_text),
        tool_call_after_text: !rest.is_empty(),
    })
}

/// [`note_done_drift`] for the turn of `session_id` that just settled, read
/// from the store. `turn_start_message_id` is the run's first prompt row; a
/// later turn of the run (the `write_note` follow-up, a Pikchr repair) starts
/// at its own stored prompt, which [`note_done_drift`] finds.
///
/// A failed read reports nothing: this is a diagnostic, and the run goes on.
pub(crate) fn settled_turn_note_done_drift(
    store: &Store,
    session_id: &str,
    turn_start_message_id: i64,
    slot: &NoteSubmissionSlot,
) -> Option<NoteDoneDrift> {
    if !crate::note_tool::has_submission(slot) {
        return None;
    }
    let messages = match store.get_session_messages_since(session_id, turn_start_message_id, &[]) {
        Ok(messages) => messages,
        Err(e) => {
            log::debug!(
                "Session {session_id}: could not read the turn for the note-done check: {e}"
            );
            return None;
        }
    };
    let rows: Vec<TurnRow<'_>> = messages
        .iter()
        .map(|message| TurnRow {
            role: message.role.clone(),
            content: &message.content,
        })
        .collect();
    note_done_drift(true, &rows)
}

/// Log one line when the turn of `session_id` that just settled saved a note
/// and did not end in a bare "Done" (see the module docs). The line starts
/// with a fixed phrase, `note-done drift`, so the lines can be counted.
pub(crate) fn log_settled_turn_note_done_drift(
    store: &Store,
    session_id: &str,
    turn_start_message_id: i64,
    slot: &NoteSubmissionSlot,
) {
    if let Some(drift) =
        settled_turn_note_done_drift(store, session_id, turn_start_message_id, slot)
    {
        log::info!(
            "Session {session_id}: note-done drift: the turn saved a note and did not end in a \
             bare \"{NOTE_DONE_WORD}\": {drift}"
        );
    }
}

#[cfg(test)]
mod tests;
