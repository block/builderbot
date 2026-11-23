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

/// How the instructions Staged puts ahead of a request open.
const STAGED_INSTRUCTIONS_OPENING: &str = "<action>";

/// Whether `input`, a message Staged sent an agent, opens with Staged's own
/// instructions: the `<action>` block Staged puts ahead of the user's text
/// in a session's first prompt, whatever its kind, and at the top of the
/// requests it writes by itself (a note update, a note or Pikchr follow-up,
/// a pull request or commit action).
///
/// Everything else reads as typed. That is a chat follow-up, a queued or
/// mid-turn message and the "continue" that resumes a paused session, which
/// reach the agent exactly as written, and also the few plain-text messages
/// Staged writes with no block (a pipeline step's failure hand-off, the
/// Pikchr sub-agent's prompt). Taking those for typed errs towards keeping
/// a title: see [`title_echoes_input`].
pub fn opens_with_staged_instructions(input: &str) -> bool {
    input
        .trim_start_matches(is_title_whitespace)
        .starts_with(STAGED_INSTRUCTIONS_OPENING)
}

/// Whether an agent-pushed session title only repeats `input`, a message
/// Staged sent the agent, rather than being a title the agent wrote.
///
/// Some agents name a session after the opening of its first message. For a
/// prompt Staged built that is the first words of Staged's instructions, and
/// for any message it is the user's request cut to length: neither is a name
/// for the session.
///
/// The title is an echo when it is one of:
/// - the whole input;
/// - the start of the input, ending in a truncation marker (`…` or `...`)
///   where it was cut, mid-word or not;
/// - the start of the input with no marker, when the input opens with
///   Staged's instructions ([`opens_with_staged_instructions`]). No agent
///   writes a title that begins the way those do. A message sent as typed is
///   different: "Fix the login flow" is a fair title for "Fix the login flow
///   so expired tokens are refreshed", so there a bare start is taken for a
///   title the agent wrote.
///
/// "Start" is literal, compared the way an agent derives such a title: each
/// run of whitespace (newlines included) counts as a single space and
/// whitespace around either text is ignored. Nothing else is forgiven: case,
/// punctuation and markup must match. A blank title is never an echo.
pub fn title_echoes_input(title: &str, input: &str) -> bool {
    let title = title.trim_matches(is_title_whitespace);
    let cut_at_a_marker = TRUNCATION_MARKERS.iter().any(|marker| {
        title
            .strip_suffix(marker)
            .is_some_and(|cut| collapsed_prefix(cut, input).is_some())
    });
    // The title as it stands too: an input can have its own `...` there.
    cut_at_a_marker
        || match collapsed_prefix(title, input) {
            Some(Covers::All) => true,
            Some(Covers::Start) => opens_with_staged_instructions(input),
            None => false,
        }
}

/// How much of a text one of its prefixes covers.
enum Covers {
    All,
    Start,
}

/// Whether `prefix` is non-blank and `text` starts with it, and if so
/// whether it is all of `text`; both read with their whitespace collapsed
/// (see [`collapsed`]).
fn collapsed_prefix(prefix: &str, text: &str) -> Option<Covers> {
    let mut prefix = collapsed(prefix).peekable();
    prefix.peek()?;
    let mut text = collapsed(text);
    if !prefix.all(|c| text.next() == Some(c)) {
        return None;
    }
    Some(match text.next() {
        None => Covers::All,
        Some(_) => Covers::Start,
    })
}

/// The characters of `text` with each run of whitespace as one space and
/// none at either end.
fn collapsed(text: &str) -> impl Iterator<Item = char> + '_ {
    text.split(is_title_whitespace)
        .filter(|word| !word.is_empty())
        .enumerate()
        .flat_map(|(index, word)| (index > 0).then_some(' ').into_iter().chain(word.chars()))
}

/// Whitespace as the normalisation behind such a title reads it. codex-acp,
/// which collapses its input this way, is JavaScript, whose `\s` and `trim`
/// also take U+FEFF (a byte-order mark); `char::is_whitespace` does not.
fn is_title_whitespace(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

impl Store {
    /// Whether `matches` holds for any message Staged sent in this session,
    /// oldest first: the stored user rows, each holding a message as it was
    /// built or typed (a prompt Staged built has its instructions in the
    /// row). Hidden ACP rows that replay a user message have no content and
    /// are passed over.
    ///
    /// `matches` runs with the store locked, so it must not use the store.
    pub fn any_session_input(
        &self,
        session_id: &str,
        mut matches: impl FnMut(&str) -> bool,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT content FROM session_messages
             WHERE session_id = ?1 AND role = 'user' AND acp_event_kind IS NULL
             ORDER BY id",
        )?;
        let mut rows = stmt.query(params![session_id])?;
        while let Some(row) = rows.next()? {
            // Borrowed: a prompt can be long and often only its start is read.
            let input = row.get_ref(0)?.as_str().unwrap_or_default();
            if matches(input) {
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

    /// A message sent as typed, with no instructions ahead of it: a chat
    /// follow-up.
    const TYPED: &str = "Fix the login flow so expired tokens are refreshed";

    #[test]
    fn staged_instructions_are_recognised_by_how_they_open() {
        assert!(opens_with_staged_instructions(INPUT));
        assert!(opens_with_staged_instructions(
            "\n\u{feff}  <action>\nDo it"
        ));
        for typed in [
            TYPED,
            "Continue where you left off.",
            "",
            "<Action>\nDo it",
            "Do it <action>",
            "action> Do it",
        ] {
            assert!(!opens_with_staged_instructions(typed), "{typed:?}");
        }
    }

    #[test]
    fn title_that_starts_instructions_staged_wrote_is_an_echo() {
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
    fn title_that_starts_a_typed_message_is_one_the_agent_wrote() {
        for title in [
            "Fix the login flow",
            "Fix",
            "Fix the login flow so expired tokens are",
            // Cut mid-word, but with nothing to say it was cut.
            "Fix the login flow so exp",
            "  Fix the\nlogin flow  ",
        ] {
            assert!(!title_echoes_input(title, TYPED), "{title:?}");
        }
        assert!(!title_echoes_input(
            "Continue",
            "Continue where you left off."
        ));
    }

    #[test]
    fn whole_or_marked_as_cut_typed_message_is_an_echo() {
        for title in [
            // All of it, however the whitespace fell.
            TYPED,
            "  Fix the login flow so expired\ntokens are   refreshed ",
            // Cut and marked, at a word and mid-word.
            "Fix the login flow…",
            "Fix the login flow...",
            "Fix the login flow so exp…",
            "Fix the login flow …",
        ] {
            assert!(title_echoes_input(title, TYPED), "{title:?}");
        }
        assert!(title_echoes_input(
            "Continue where you left off.",
            "Continue where you left off."
        ));
        // All of it and a marker is not the message cut short.
        assert!(!title_echoes_input(
            "Fix the login flow so expired tokens are refreshed, and more…",
            TYPED
        ));
    }

    #[test]
    fn byte_order_mark_reads_as_whitespace() {
        // JavaScript's `\s` takes U+FEFF, so an agent's title has a space,
        // or nothing at an end, where the input has one.
        assert!(title_echoes_input(
            "<action> The user is requesting…",
            "\u{feff}<action>\u{feff}The user\u{feff}\u{feff} is requesting a note"
        ));
        assert!(title_echoes_input(
            "Fix the login flow",
            "\u{feff}Fix the\u{feff}login flow\u{feff}"
        ));
        assert!(title_echoes_input(
            "\u{feff}Fix the login…\u{feff}",
            "Fix the login flow"
        ));
        // It is not dropped: no whitespace in the title, none assumed.
        assert!(!title_echoes_input(
            "Fix thelogin flow",
            "Fix the\u{feff}login flow"
        ));
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
        assert!(title_echoes_input(
            "Wait... what broke?",
            "Wait... what broke?"
        ));
        assert!(title_echoes_input(
            "<action> Wait...",
            "<action>\nWait... what broke?\n</action>"
        ));
        assert!(title_echoes_input("Wait...", "Wait..."));
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
                .any_session_input(&session.id, |input| title_echoes_input(title, input))
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

        // A chat follow-up, stored as typed.
        store
            .add_session_message(&session.id, MessageRole::User, "Now cover it with tests")
            .unwrap();

        assert!(echoes("<action> The user is requesting…"));
        assert!(echoes("<action> Follow-up instructions."));
        // The typed message whole or marked as cut, but not its bare start.
        assert!(echoes("Now cover it with tests"));
        assert!(echoes("Now cover it…"));
        assert!(!echoes("Now cover it"));
        // What the agent said, another session's input, a written title.
        assert!(!echoes("Done: tokens refresh"));
        assert!(!echoes("Rename the module"));
        assert!(!echoes("Refresh expired login tokens"));
        assert!(!echoes("Also log the refresh"));

        // The rows are offered oldest first, until one matches.
        let mut seen = Vec::new();
        let found = store
            .any_session_input(&session.id, |input| {
                seen.push(input.chars().take(8).collect::<String>());
                input.starts_with("<action>\nFollow-up")
            })
            .unwrap();
        assert!(found);
        assert_eq!(seen, ["<action>", "<action>"]);
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
