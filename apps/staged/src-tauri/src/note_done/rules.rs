//! The two string rules the backend shares with the chat pane: what counts
//! as the closing word of a saved note turn, and which tool name is the
//! `staged` server's `write_note`.
//!
//! They are the Rust copies of `isNoteDoneText`
//! (`src/lib/features/sessions/noteDoneMessage.ts`) and `isWriteNoteToolName`
//! (`src/lib/features/sessions/noteToolCall.ts`), which decide what the chat
//! pane hides. Each pair is kept equal by a shared case table,
//! `src/lib/features/sessions/testStubs/noteDone.cases.json`, that both test
//! suites run (see `note_done/tests.rs`).
//!
//! The file uses nothing outside `std` and has no tests of its own, on
//! purpose: the real-bridge test in
//! `crates/acp-client/src/driver/tests/real_bridge.rs` compiles this same
//! file by path, so a live agent is held to the rules the app ships. Keep it
//! that way, and update that `#[path]` if the file moves.

/// The closing message the `write_note` result asks for
/// (`note_tool::saved_message`, and `savedMessage` in `staged-mcp.cjs`).
pub(crate) const NOTE_DONE_WORD: &str = "Done";

/// Wrappers an agent may put around the word: Markdown emphasis, code,
/// quotes. `**` comes before `*`, so bold is taken off whole.
const WRAPPERS: [(&str, &str); 8] = [
    ("**", "**"),
    ("*", "*"),
    ("_", "_"),
    ("`", "`"),
    ("\"", "\""),
    ("'", "'"),
    ("“", "”"),
    ("‘", "’"),
];

/// Whether JavaScript counts `c` as white space, in `String.prototype.trim`
/// and in a regular expression's `\s`. That is Unicode `White_Space` without
/// U+0085 and with the byte order mark, U+FEFF.
fn is_js_whitespace(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// `text` without leading and trailing white space, as JavaScript's `trim`
/// removes it.
pub(crate) fn trim_js(text: &str) -> &str {
    text.trim_matches(is_js_whitespace)
}

/// Whether an assistant message's text is nothing but the closing word:
/// `Done`, in any case, optionally in one pair of wrappers and with one
/// trailing `.` or `!`. Anything more is real text: `Done ✅`, a sentence
/// that starts with the word, an answer whose last line is `Done`.
pub(crate) fn is_note_done_text(text: &str) -> bool {
    let mut word = trim_js(text);
    for (open, close) in WRAPPERS {
        if word.len() >= open.len() + close.len() && word.starts_with(open) && word.ends_with(close)
        {
            word = &word[open.len()..word.len() - close.len()];
            break;
        }
    }
    if let Some(stripped) = word.strip_suffix(['.', '!']) {
        word = stripped;
    }
    word.to_lowercase() == NOTE_DONE_WORD.to_lowercase()
}

/// Whether a tool call's title names the `staged` server's `write_note`, in
/// any of the spellings the bridges use: Claude Code's
/// `mcp__staged__write_note`, Goose's `staged: write note`, Codex's
/// `mcp.staged.write_note`, or a bare `staged.write_note`. The `mcp` prefix
/// is optional and the server must be `staged`, so another server's
/// `write_note` does not count.
pub(crate) fn is_write_note_tool_name(name: &str) -> bool {
    // Runs of white space and hyphens become one underscore.
    let mut normalized = String::with_capacity(name.len());
    let mut in_separator = false;
    for c in trim_js(name).to_lowercase().chars() {
        if is_js_whitespace(c) || c == '-' {
            if !in_separator {
                normalized.push('_');
            }
            in_separator = true;
        } else {
            normalized.push(c);
            in_separator = false;
        }
    }
    let unprefixed = normalized
        .strip_prefix("mcp__")
        .or_else(|| normalized.strip_prefix("mcp."))
        .unwrap_or(&normalized);
    unprefixed.strip_prefix("staged").is_some_and(|rest| {
        ["__", ":_", "_", "."]
            .iter()
            .any(|separator| rest.strip_prefix(separator) == Some("write_note"))
    })
}
