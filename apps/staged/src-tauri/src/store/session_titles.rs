//! Display titles for sessions that haven't produced their artifact yet.
//!
//! The branch timeline and the child-session badges under a project session
//! both show in-flight sessions, and they must name them the same way: these
//! helpers are the single statement of each rule.
//!
//! Also here: the rule for which agent-pushed titles are worth keeping at
//! all ([`title_echoes_input`]).

use rusqlite::params;

use super::{Store, StoreError};

/// What an agent appends to a title it cut short.
const TRUNCATION_MARKERS: [&str; 2] = ["…", "..."];

/// Whether an agent-pushed session title is only the start of `input`, a
/// message Staged sent the agent, rather than a title the agent wrote.
///
/// Some agents name a session after the opening of its first message. Every
/// Staged message opens with the instructions Staged puts ahead of the user's
/// text, so such a title is the first words of those instructions and says
/// nothing about the session.
///
/// The title has to be a literal prefix of the input, compared the way an
/// agent derives one: each run of whitespace (newlines included) counts as a
/// single space, whitespace around either text is ignored, and the title may
/// end in a truncation marker (`…` or `...`) where it was cut, mid-word or
/// not. Nothing else is forgiven: case, punctuation and markup must match,
/// so a title the agent wrote is an echo only if it is word for word how
/// the input starts. A blank title is never one.
pub fn title_echoes_input(title: &str, input: &str) -> bool {
    let title = title.trim();
    // The whole title first: an input can itself contain `...` at the cut.
    is_collapsed_prefix(title, input)
        || TRUNCATION_MARKERS.iter().any(|marker| {
            title
                .strip_suffix(marker)
                .is_some_and(|cut| is_collapsed_prefix(cut, input))
        })
}

/// Whether `prefix` is non-blank and `text` starts with it, both read with
/// their whitespace collapsed (see [`collapsed`]).
fn is_collapsed_prefix(prefix: &str, text: &str) -> bool {
    let mut prefix = collapsed(prefix).peekable();
    if prefix.peek().is_none() {
        return false;
    }
    let mut text = collapsed(text);
    prefix.all(|c| text.next() == Some(c))
}

/// The characters of `text` with each run of whitespace as one space and
/// none at either end.
fn collapsed(text: &str) -> impl Iterator<Item = char> + '_ {
    text.split_whitespace()
        .enumerate()
        .flat_map(|(index, word)| (index > 0).then_some(' ').into_iter().chain(word.chars()))
}

impl Store {
    /// Whether `title` echoes any message Staged sent in this session (see
    /// [`title_echoes_input`]): the stored user rows, which hold each prompt
    /// as built, instructions included. Hidden ACP rows that replay a user
    /// message have no content and are passed over.
    pub fn session_title_echoes_input(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT content FROM session_messages
             WHERE session_id = ?1 AND role = 'user' AND acp_event_kind IS NULL
             ORDER BY id",
        )?;
        let mut rows = stmt.query(params![session_id])?;
        while let Some(row) = rows.next()? {
            // Borrowed: a prompt can be long and only its start is read.
            let input = row.get_ref(0)?.as_str().unwrap_or_default();
            if title_echoes_input(title, input) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// The agent-pushed ACP session title, when set and non-empty.
///
/// Applied at read time only, as an interim display name for in-progress
/// pending commits and untitled reviews. Notes always use their stored titles.
pub fn non_empty_acp_title(acp_title: Option<&str>) -> Option<String> {
    acp_title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// The subject shown for a commit whose session hasn't produced a sha yet:
/// the live ACP title, else the session's prompt, else a static placeholder.
pub fn pending_commit_subject(acp_title: Option<&str>, prompt: Option<&str>) -> String {
    non_empty_acp_title(acp_title)
        .or_else(|| prompt.map(str::to_string))
        .unwrap_or_else(|| "Pending commit".to_string())
}

/// A review's displayed title. A stored title always wins; while the review's
/// session is still running and the title is unset, the ACP title stands in,
/// so an untitled running review isn't a static placeholder.
pub fn review_title(
    stored: Option<String>,
    session_status: Option<&str>,
    acp_title: Option<&str>,
) -> Option<String> {
    match stored {
        None if session_status == Some("running") => non_empty_acp_title(acp_title),
        title => title,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_title_is_trimmed_and_blank_is_none() {
        assert_eq!(
            non_empty_acp_title(Some("  Fix it  ")).as_deref(),
            Some("Fix it")
        );
        assert_eq!(non_empty_acp_title(Some("   ")), None);
        assert_eq!(non_empty_acp_title(None), None);
    }

    /// A prompt shaped like the ones Staged sends: instructions, then the
    /// user's text.
    const INPUT: &str = "<action>\nThe user is requesting you make a commit based on the \
        prompt below. Make the necessary\ncode changes, then create a commit.\n\n\
        Project information:\n  You are working in project \"Login\".\n</action>\n\n\
        Fix the login flow so expired tokens are refreshed";

    #[test]
    fn title_that_starts_the_input_is_an_echo() {
        assert!(title_echoes_input("<action>", INPUT));
        assert!(title_echoes_input(
            "<action>\nThe user is requesting you make a commit",
            INPUT
        ));
        // Cut mid-word, as a character limit cuts.
        assert!(title_echoes_input("<action>\nThe user is reque", INPUT));
        // The whole input.
        assert!(title_echoes_input(INPUT, INPUT));
    }

    #[test]
    fn echo_is_recognised_through_whitespace_changes() {
        // Collapsed to one line, as an agent's own normalisation leaves it.
        assert!(title_echoes_input(
            "<action> The user is requesting you make a commit based on the prompt below. \
             Make the necessary code changes",
            INPUT
        ));
        assert!(title_echoes_input(
            "  \n<action>\tThe  user\n\nis requesting\u{a0}you  ",
            INPUT
        ));
        // Whitespace the input has and the title lost only at its ends.
        assert!(title_echoes_input(
            "<action> The user",
            "\n\n  <action>\nThe user asks"
        ));
    }

    #[test]
    fn echo_is_recognised_through_a_truncation_marker() {
        for title in [
            "<action> The user is requesting…",
            "<action> The user is requesting...",
            "<action> The user is requesting …",
            "<action> The user is reque…",
            "  <action>\nThe user is requesting... ",
        ] {
            assert!(title_echoes_input(title, INPUT), "{title:?}");
        }
        // An input with its own `...` where the title stops.
        assert!(title_echoes_input("Wait...", "Wait... what broke?"));
        assert!(title_echoes_input("Wait......", "Wait... what broke?"));
    }

    #[test]
    fn written_title_is_not_an_echo() {
        for title in [
            "Fix the login flow",
            "Refresh expired login tokens",
            // The user's own words are not how the input starts.
            "Fix the login flow so expired tokens are refreshed",
            // Words of the instructions, but not from their start.
            "The user is requesting you make a commit",
            "Make the necessary code changes…",
            // Case and punctuation are not forgiven.
            "<Action> The user is requesting",
            "<action> the user is requesting",
            "<action>: The user is requesting",
            // Longer than the input.
            "<action> The user is requesting you make a commit, and more",
        ] {
            assert!(!title_echoes_input(title, INPUT), "{title:?}");
        }
        assert!(!title_echoes_input("Fix it now", "Fix it"));
        // A marker is only dropped from the end, and only a whole one.
        assert!(!title_echoes_input("Fix… it", "Fix it"));
        assert!(!title_echoes_input("Fix it.", "Fix it; then ship"));
        assert!(!title_echoes_input("Fix it..", "Fix it; then ship"));
    }

    #[test]
    fn blank_title_is_not_an_echo() {
        for title in ["", "   ", "\n\t", "…", "...", "  …  "] {
            assert!(!title_echoes_input(title, INPUT), "{title:?}");
            assert!(!title_echoes_input(title, ""), "{title:?}");
        }
        assert!(!title_echoes_input("Fix it", ""));
        assert!(!title_echoes_input("Fix it", "  \n "));
    }

    #[test]
    fn session_title_echo_reads_every_message_staged_sent() {
        use crate::store::{AcpMessageMetadata, MessageRole, Session};
        use std::path::Path;

        let store = Store::in_memory().unwrap();
        let session = Session::new_running("Fix the login flow", Path::new("."));
        store.create_session(&session).unwrap();
        let other = Session::new_running("Other", Path::new("."));
        store.create_session(&other).unwrap();

        let echoes = |title: &str| {
            store
                .session_title_echoes_input(&session.id, title)
                .unwrap()
        };
        // Nothing sent yet.
        assert!(!echoes("<action> The user is requesting"));

        store
            .add_session_message(&session.id, MessageRole::User, INPUT)
            .unwrap();
        store
            .add_session_message(&session.id, MessageRole::Assistant, "Done: tokens refresh")
            .unwrap();
        store
            .add_session_message(
                &session.id,
                MessageRole::User,
                "<action>\nFollow-up instructions.\n</action>\n\nAlso log the refresh",
            )
            .unwrap();
        store
            .add_session_message(&other.id, MessageRole::User, "Rename the module")
            .unwrap();
        // A user message the agent replayed: a hidden row, never an input.
        store
            .add_acp_metadata_message_with_role(
                &session.id,
                MessageRole::User,
                &AcpMessageMetadata {
                    acp_event_kind: Some("user_message_chunk".to_string()),
                    acp_content: Some(serde_json::json!({"text": "Replayed text"})),
                    ..Default::default()
                },
            )
            .unwrap();

        assert!(echoes("<action> The user is requesting…"));
        assert!(echoes("<action> Follow-up instructions."));
        // What the agent said, another session's input, a written title.
        assert!(!echoes("Done: tokens refresh"));
        assert!(!echoes("Rename the module"));
        assert!(!echoes("Refresh expired login tokens"));
        assert!(!echoes("Also log the refresh"));
    }

    #[test]
    fn pending_commit_prefers_acp_title_then_prompt_then_placeholder() {
        assert_eq!(
            pending_commit_subject(Some("Live title"), Some("prompt")),
            "Live title"
        );
        assert_eq!(pending_commit_subject(Some(" "), Some("prompt")), "prompt");
        assert_eq!(pending_commit_subject(None, None), "Pending commit");
    }

    #[test]
    fn review_title_uses_acp_title_only_while_running_and_untitled() {
        assert_eq!(
            review_title(Some("Stored".into()), Some("running"), Some("Live")).as_deref(),
            Some("Stored")
        );
        assert_eq!(
            review_title(None, Some("running"), Some("Live")).as_deref(),
            Some("Live")
        );
        assert_eq!(review_title(None, Some("queued"), Some("Live")), None);
        assert_eq!(review_title(None, Some("running"), None), None);
    }
}
