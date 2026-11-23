//! The `write_note` MCP tool: a note session's deliverable as a tool call.
//!
//! A note session used to hand its note back as assistant text after a `---`
//! separator, parsed on the laptop once the turn settled. That contract is
//! fragile (stray rules in prose, unterminated fences, "first block wins" for
//! the next-steps JSON) and ties the deliverable to the transcript. Instead the
//! agent writes the note body into its per-session draft file (`note_draft`)
//! and calls `write_note` with the title and structured suggested next steps;
//! the call publishes the draft. The call is the deliverable; the assistant's
//! own text is not saved, and a draft that changed without a call is not
//! stored either.
//!
//! Locally the tool is served by an in-process HTTP MCP server
//! ([`start_note_mcp_server`]) bound to loopback, exactly like the project and
//! pikchr servers. Each call replaces the whole note and the last call within a
//! turn wins; the result tells the agent the note is saved and to end its
//! turn. The submission lands in a per-turn [`NoteSubmissionSlot`] the
//! post-completion hooks read instead of parsing the transcript.
//!
//! Remote (Blox) sessions cannot reach a laptop-side server, so they get the
//! same tool from the Staged MCP on Blox, a stdio server installed on the
//! workstation (`remote_staged_mcp`), which reads `draft.md` from the
//! session's scratch directory and whose note outbox the laptop collects into
//! the same slot after the turn (`remote_staged_mcp::note_outbox`). The tool
//! contract — name, schema, result text — is shared so prompts do not care
//! which transport served the call.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData, ServerHandler};
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use crate::note_draft::DraftError;

/// MCP server name the tool is registered under, on both transports. Agents
/// that namespace tools see it as `mcp__staged__write_note`; prompts refer to
/// the bare `write_note`.
pub(crate) const NOTE_MCP_SERVER_NAME: &str = "staged";

/// The tool's name as the agent sees it. The `#[tool]` attribute below and the
/// Staged MCP on Blox spell it out; `tool_router_advertises_the_shared_contract`
/// keeps them in step.
#[cfg(test)]
pub(crate) const WRITE_NOTE_TOOL_NAME: &str = "write_note";

/// Tool description shared by the HTTP server (the `#[tool]` attribute) and
/// the Staged MCP on Blox; tests compare both against this copy.
#[cfg(test)]
pub(crate) const WRITE_NOTE_DESCRIPTION: &str = "Publish the note this session is producing. This call is the deliverable: \
Staged stores the current contents of your note draft file (the path is in your instructions) as the note body, \
under the title you pass here, and does not read your assistant text. Write the whole note body into the draft \
file first; calling again republishes the file, and the last call of the turn wins. After the result says the \
note is saved, send a simple \"Done\" message with no other text to end your turn, rather than summarising the \
work done. If the user asked you a question, answer it before making this call.";

/// Structured suggested next steps, the same shape as the former
/// `suggested-next-steps` JSON block (camelCase keys). Both fields are
/// nullable; `None` means no clear next action of that kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedNextSteps {
    /// A few words describing a commit the user might want next (for example
    /// "Implement this plan"), or null.
    pub suggested_next_commit_step: Option<String>,
    /// A few words describing a follow-up note the user might want next (for
    /// example "Plan a fix for this bug"), or null.
    pub suggested_next_note_step: Option<String>,
}

impl SuggestedNextSteps {
    /// Trim both fields and drop empty strings, so a tool call passing `""`
    /// stores `None` like a JSON `null` would.
    fn normalized(self) -> Self {
        fn clean(value: Option<String>) -> Option<String> {
            value
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        }
        Self {
            suggested_next_commit_step: clean(self.suggested_next_commit_step),
            suggested_next_note_step: clean(self.suggested_next_note_step),
        }
    }
}

/// Arguments of `write_note`. There is no `body`: the body is the draft
/// file. The schema is derived from this struct, so the Staged MCP on Blox's
/// hand-written schema must stay in step with it (see the
/// `write_note_schema_matches_the_rust_tool_schema` test in
/// `remote_staged_mcp::note_outbox`).
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct WriteNoteParams {
    // Optional for the handler only. A call without `title` takes the title
    // from the draft's first line (`NoteSubmission::from_draft`); a call
    // with one behaves as it did when the field was required. The schema
    // agents are shown is not changed yet: `with = "String"` keeps `title`
    // a required string there, and the doc comment below is its
    // description.
    /// The note title, shown as its heading. Plain text, no markdown, no
    /// leading `#`.
    #[schemars(with = "String")]
    pub title: Option<String>,
    /// What the user might want to do next. Keep both fields to a few words
    /// and use null when there is no clear next action.
    #[serde(default)]
    pub suggested_next_steps: Option<SuggestedNextSteps>,
}

/// Where a note submission came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteSubmissionSource {
    /// The in-process HTTP `write_note` tool (local and project sessions).
    Tool,
    /// The workstation outbox written by the Staged MCP on Blox.
    Outbox,
}

/// A note the agent delivered through `write_note`, ready for the
/// post-completion hooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSubmission {
    pub title: String,
    pub body: String,
    /// `None` when the call omitted the argument; the hooks then keep the
    /// stored pair, as a rewriting turn without the JSON block used to.
    pub suggested_next_steps: Option<SuggestedNextSteps>,
    pub source: NoteSubmissionSource,
}

/// Why `write_note` refused a draft it could read
/// ([`NoteSubmission::from_params`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SubmissionError {
    /// The call passed `title` and the title or the body failed
    /// [`NoteSubmission::from_parts`]; this is its reason.
    Arguments(String),
    /// The call passed no `title` and the draft does not open with one
    /// ([`crate::note_draft::split_title`]).
    Draft(DraftError),
}

impl SubmissionError {
    /// The `write_note` error text. A draft error names `draft_path` so the
    /// agent fixes the file within the turn.
    pub(crate) fn describe(&self, draft_path: &str) -> String {
        match self {
            Self::Arguments(reason) => format!(
                "write_note rejected: {reason}. Call it again with a non-empty one-line title."
            ),
            Self::Draft(error) => error.describe(draft_path),
        }
    }
}

impl NoteSubmission {
    /// Validate and normalize tool arguments plus the draft
    /// (`note_draft::read_draft`'s text) into a submission.
    ///
    /// - With `title`: the title is the argument and the whole draft is the
    ///   body, under [`Self::from_parts`]'s rules. A `# Heading` at the top
    ///   of the draft is body text, as it was before `title` became
    ///   optional.
    /// - Without `title`: the draft's first line is the title and the rest
    ///   is the body ([`Self::from_draft`]).
    pub(crate) fn from_params(
        params: WriteNoteParams,
        draft: &str,
        source: NoteSubmissionSource,
    ) -> Result<Self, SubmissionError> {
        match params.title {
            Some(title) => Self::from_parts(&title, draft, params.suggested_next_steps, source)
                .map_err(SubmissionError::Arguments),
            None => Self::from_draft(draft, params.suggested_next_steps, source)
                .map_err(SubmissionError::Draft),
        }
    }

    /// A submission whose title is the draft's first line, a `# Title`
    /// heading, and whose body is everything below it
    /// ([`crate::note_draft::split_title`]). The heading line is not part
    /// of the stored body.
    ///
    /// The title is kept as written: unlike [`Self::from_parts`], a `#` the
    /// title text itself starts with is not stripped, so `# #12 follow-up`
    /// is titled `#12 follow-up`. `split_title` already returns a one-line,
    /// non-empty title and a non-empty body, both trimmed.
    pub(crate) fn from_draft(
        draft: &str,
        suggested_next_steps: Option<SuggestedNextSteps>,
        source: NoteSubmissionSource,
    ) -> Result<Self, DraftError> {
        let (title, body) = crate::note_draft::split_title(draft)?;
        Ok(Self {
            title,
            body,
            suggested_next_steps: suggested_next_steps.map(SuggestedNextSteps::normalized),
            source,
        })
    }

    /// Validate and normalize a title, a body and suggested next steps into
    /// a submission, whatever carried them: the tool's `title` argument and
    /// the draft locally ([`Self::from_params`]), or the outbox's `meta.json`
    /// and `note.md` remotely (`remote_staged_mcp::note_outbox`). The outbox
    /// reader calls this directly, so it does not depend on the shape of
    /// [`WriteNoteParams`].
    ///
    /// The title and body must both be non-empty after trimming; a title that
    /// was written as a markdown H1 anyway has its `# ` stripped so the stored
    /// title stays plain text.
    pub(crate) fn from_parts(
        title: &str,
        body: &str,
        suggested_next_steps: Option<SuggestedNextSteps>,
        source: NoteSubmissionSource,
    ) -> Result<Self, String> {
        let title = title.trim();
        let title = title.strip_prefix('#').unwrap_or(title).trim();
        if title.is_empty() {
            return Err("`title` must not be empty".to_string());
        }
        if title.contains('\n') {
            return Err("`title` must be a single line".to_string());
        }
        let body = body.trim();
        if body.is_empty() {
            return Err("the note body must not be empty".to_string());
        }
        Ok(Self {
            title: title.to_string(),
            body: body.to_string(),
            suggested_next_steps: suggested_next_steps.map(SuggestedNextSteps::normalized),
            source,
        })
    }

    /// The note as the `---` contract would have delivered it: an H1 title
    /// followed by the body. The post-completion hooks feed this to the
    /// unchanged title resolution and media ingestion.
    pub(crate) fn as_note_content(&self) -> String {
        format!("# {}\n{}", self.title, self.body)
    }

    /// The text returned to the agent for a saved note.
    pub(crate) fn saved_message(&self) -> String {
        saved_message(&self.title, self.body.len())
    }
}

/// The `write_note` result text. Shared with the Staged MCP on Blox's wording so
/// the agent sees the same instruction on both transports. `body_len` is the
/// body's UTF-8 length, so the text says bytes.
///
/// The text asks the agent to close the turn with the one word "Done" in
/// place of a recap of the note, unless the user asked a question it has not
/// answered yet. The chat pane hides that word after a saved call
/// (`noteDoneMessage.ts`), so the quoted word here and the word the filter
/// accepts have to stay the same; `noteDoneWording.test.ts` reads this
/// function to check it.
pub(crate) fn saved_message(title: &str, body_len: usize) -> String {
    format!(
        "Saved note \"{title}\" ({body_len} bytes from the draft file). If the user asked you a \
question this turn and you have not answered it in chat yet, answer it now, briefly. Otherwise \
send a simple \"Done\" message with no other text to end your turn, rather than summarising the \
work done."
    )
}

/// The sentence the `staged` server puts in its `initialize` instructions on
/// both transports, naming the draft `write_note` publishes. The tool's
/// description says the path "is in your instructions"; this is what makes
/// that true even for a session whose prompt names no path. The Staged MCP
/// on Blox spells the same sentence out
/// (`write_note_reads_the_same_drafts_on_both_transports` compares them).
pub(crate) fn draft_instructions(draft_path: &str) -> String {
    format!(
        "Your note draft file is `{draft_path}`. This file is your note: write it, edit it, and \
when it is ready call write_note (title and suggested next steps) to publish it; your assistant \
text is not saved."
    )
}

/// Per-turn slot the `write_note` tool fills and the post-completion hooks
/// drain. Last call wins: each call overwrites the previous submission.
pub type NoteSubmissionSlot = Arc<Mutex<Option<NoteSubmission>>>;

pub(crate) fn new_submission_slot() -> NoteSubmissionSlot {
    Arc::new(Mutex::new(None))
}

/// Store a submission, replacing any earlier one in the turn.
pub(crate) fn store_submission(slot: &NoteSubmissionSlot, submission: NoteSubmission) {
    *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(submission);
}

/// Whether the turn has delivered a note through the tool so far.
pub(crate) fn has_submission(slot: &NoteSubmissionSlot) -> bool {
    slot.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some()
}

/// A copy of the current submission, if any.
pub(crate) fn current_submission(slot: &NoteSubmissionSlot) -> Option<NoteSubmission> {
    slot.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Handler for the local `write_note` tool. One per session; every HTTP
/// connection to the server shares the session's slot and draft path.
#[derive(Clone)]
struct NoteToolsHandler {
    session_id: String,
    slot: NoteSubmissionSlot,
    draft_path: PathBuf,
}

impl NoteToolsHandler {
    fn new(session_id: String, slot: NoteSubmissionSlot, draft_path: PathBuf) -> Self {
        Self {
            session_id,
            slot,
            draft_path,
        }
    }
}

#[tool_router]
impl NoteToolsHandler {
    #[tool(
        description = "Publish the note this session is producing. This call is the deliverable: \
Staged stores the current contents of your note draft file (the path is in your instructions) as the note body, \
under the title you pass here, and does not read your assistant text. Write the whole note body into the draft \
file first; calling again republishes the file, and the last call of the turn wins. After the result says the \
note is saved, send a simple \"Done\" message with no other text to end your turn, rather than summarising the \
work done. If the user asked you a question, answer it before making this call."
    )]
    async fn write_note(
        &self,
        Parameters(params): Parameters<WriteNoteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(handle_write_note(
            &self.session_id,
            &self.slot,
            &self.draft_path,
            params,
        ))
    }
}

/// The tool body, separated from the rmcp plumbing so tests can drive it.
///
/// Reads the draft first: a missing, empty, oversized, non-UTF-8 or
/// non-regular draft is a tool error naming the path, and the slot is left
/// untouched so the agent fixes the file and calls again within the turn.
///
/// The title then comes from the call when it passes `title` (the whole
/// draft is the body) and from the draft's first line when it does not (a
/// `# Title` heading, with the body below it). A draft that has no such
/// line, or nothing under it, is rejected the same way as an unreadable
/// one. See [`NoteSubmission::from_params`].
pub(crate) fn handle_write_note(
    session_id: &str,
    slot: &NoteSubmissionSlot,
    draft_path: &Path,
    params: WriteNoteParams,
) -> CallToolResult {
    let draft = match crate::note_draft::read_draft(draft_path) {
        Ok(draft) => draft,
        Err(error) => {
            log::warn!(
                "Session {session_id}: write_note could not read the draft {}: {error:?}",
                draft_path.display()
            );
            return CallToolResult::error(vec![ContentBlock::text(
                error.describe(&draft_path.to_string_lossy()),
            )]);
        }
    };
    match NoteSubmission::from_params(params, &draft, NoteSubmissionSource::Tool) {
        Ok(submission) => {
            let message = submission.saved_message();
            log::info!(
                "Session {session_id}: write_note saved \"{}\" ({} chars from {})",
                submission.title,
                submission.body.len(),
                draft_path.display()
            );
            store_submission(slot, submission);
            CallToolResult::success(vec![ContentBlock::text(message)])
        }
        Err(error) => {
            log::warn!(
                "Session {session_id}: write_note rejected the call for the draft {}: {error:?}",
                draft_path.display()
            );
            CallToolResult::error(vec![ContentBlock::text(
                error.describe(&draft_path.to_string_lossy()),
            )])
        }
    }
}

#[tool_handler]
impl ServerHandler for NoteToolsHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(draft_instructions(&self.draft_path.to_string_lossy()))
    }
}

/// Start the local MCP HTTP server exposing `write_note` for one session,
/// publishing the draft at `draft_path`.
///
/// Returns the bound port and a `JoinHandle`. The server runs until the handle
/// (and its parent `LocalSet`) is dropped. Submissions land in `slot`.
pub async fn start_note_mcp_server(
    session_id: String,
    slot: NoteSubmissionSlot,
    draft_path: PathBuf,
) -> Result<(u16, JoinHandle<()>), String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind note MCP listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to get local address: {e}"))?
        .port();

    let service = StreamableHttpService::new(
        move || {
            Ok(NoteToolsHandler::new(
                session_id.clone(),
                Arc::clone(&slot),
                draft_path.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let router = Router::new().route_service("/mcp", service);

    log::debug!("[note_tool] HTTP server bound on port {port}");

    let handle = tokio::task::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            log::error!("[note_tool] HTTP server error: {e}");
        }
    });

    Ok((port, handle))
}

/// The JSON schema of `write_note`'s arguments, as rmcp advertises it.
#[cfg(test)]
pub(crate) fn write_note_input_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(WriteNoteParams)).expect("schema serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_router_advertises_the_shared_contract() {
        let tools = NoteToolsHandler::tool_router().list_all();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name.as_ref(), WRITE_NOTE_TOOL_NAME);
        assert_eq!(
            tools[0].description.as_deref(),
            Some(WRITE_NOTE_DESCRIPTION),
            "the #[tool] description must match the shared constant"
        );
        // How the turn ends after a saved call, word for word.
        assert!(
            WRITE_NOTE_DESCRIPTION.ends_with(
                "After the result says the note is saved, send a simple \"Done\" message with no \
other text to end your turn, rather than summarising the work done. If the user asked you a \
question, answer it before making this call."
            ),
            "{WRITE_NOTE_DESCRIPTION}"
        );
    }

    #[test]
    fn initialize_instructions_name_the_draft_file() {
        let path = PathBuf::from("/tmp/staged-note-draft-s1.md");
        let handler = NoteToolsHandler::new("s1".to_string(), new_submission_slot(), path);
        let instructions = handler.get_info().instructions.expect("instructions");
        assert_eq!(
            instructions,
            draft_instructions("/tmp/staged-note-draft-s1.md")
        );
        assert!(
            instructions.starts_with("Your note draft file is `/tmp/staged-note-draft-s1.md`."),
            "{instructions}"
        );
    }

    fn params(title: &str) -> WriteNoteParams {
        WriteNoteParams {
            title: Some(title.to_string()),
            suggested_next_steps: None,
        }
    }

    /// A call that passes no `title`: the draft's first line names it.
    fn no_title() -> WriteNoteParams {
        WriteNoteParams {
            title: None,
            suggested_next_steps: None,
        }
    }

    /// The reason a call that passed `title` was refused.
    fn argument_reason(result: Result<NoteSubmission, SubmissionError>) -> String {
        match result {
            Err(SubmissionError::Arguments(reason)) => reason,
            other => panic!("expected an argument rejection, got {other:?}"),
        }
    }

    /// The text of a tool result's first content block.
    fn result_text(result: &CallToolResult) -> String {
        let content = serde_json::to_value(&result.content).unwrap();
        content[0]["text"]
            .as_str()
            .expect("a text result")
            .to_string()
    }

    /// A temp directory holding the session's draft at `draft.md`.
    struct Draft {
        dir: tempfile::TempDir,
    }

    impl Draft {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
            }
        }

        fn path(&self) -> PathBuf {
            self.dir.path().join("draft.md")
        }

        fn write(&self, body: &str) -> PathBuf {
            std::fs::write(self.path(), body).unwrap();
            self.path()
        }
    }

    #[test]
    fn submission_trims_and_strips_a_markdown_heading_from_the_title() {
        let submission = NoteSubmission::from_params(
            params("  # Design  "),
            "\nBody text.\n",
            NoteSubmissionSource::Tool,
        )
        .unwrap();
        assert_eq!(submission.title, "Design");
        assert_eq!(submission.body, "Body text.");
        assert_eq!(submission.as_note_content(), "# Design\nBody text.");
        assert_eq!(submission.source, NoteSubmissionSource::Tool);
    }

    #[test]
    fn submission_rejects_empty_or_multiline_titles_and_empty_bodies() {
        let reject = |title: &str, body: &str| {
            argument_reason(NoteSubmission::from_params(
                params(title),
                body,
                NoteSubmissionSource::Tool,
            ))
        };
        assert!(reject("", "body").contains("title"));
        assert!(reject("# ", "body").contains("title"));
        assert!(reject("a\nb", "body").contains("single line"));
        assert!(reject("Title", "  \n").contains("body"));
    }

    /// `from_params` with a `title` is `from_parts` over the arguments'
    /// fields: the same submission for valid input and the same reason for
    /// each rejection.
    #[test]
    fn from_parts_validates_and_normalizes_exactly_like_from_params() {
        let steps = || {
            Some(SuggestedNextSteps {
                suggested_next_commit_step: Some("  Fix it ".to_string()),
                suggested_next_note_step: Some("   ".to_string()),
            })
        };
        let cases = [
            ("  # Design  ", "\nBody text.\n"),
            ("Plain title", "Body"),
            ("## Deeper", "# Section\n\nText"),
            ("", "body"),
            ("# ", "body"),
            ("a\nb", "body"),
            ("Title", "  \n"),
            ("", ""),
        ];
        for (title, body) in cases {
            for source in [NoteSubmissionSource::Tool, NoteSubmissionSource::Outbox] {
                for steps in [None, steps()] {
                    let mut p = params(title);
                    p.suggested_next_steps = steps.clone();
                    assert_eq!(
                        NoteSubmission::from_parts(title, body, steps, source)
                            .map_err(SubmissionError::Arguments),
                        NoteSubmission::from_params(p, body, source),
                        "title {title:?}, body {body:?}"
                    );
                }
            }
        }

        let submission = NoteSubmission::from_parts(
            "  # Design  ",
            "\nBody text.\n",
            steps(),
            NoteSubmissionSource::Outbox,
        )
        .unwrap();
        assert_eq!(
            submission,
            NoteSubmission {
                title: "Design".to_string(),
                body: "Body text.".to_string(),
                suggested_next_steps: Some(SuggestedNextSteps {
                    suggested_next_commit_step: Some("Fix it".to_string()),
                    suggested_next_note_step: None,
                }),
                source: NoteSubmissionSource::Outbox,
            }
        );
        let reject = |title: &str, body: &str| {
            NoteSubmission::from_parts(title, body, None, NoteSubmissionSource::Outbox).unwrap_err()
        };
        assert_eq!(reject("", "body"), "`title` must not be empty");
        assert_eq!(reject("# ", "body"), "`title` must not be empty");
        assert_eq!(reject("a\nb", "body"), "`title` must be a single line");
        assert_eq!(reject("Title", "  \n"), "the note body must not be empty");
    }

    #[test]
    fn suggested_next_steps_use_the_legacy_camel_case_keys_and_normalize_blanks() {
        let steps: SuggestedNextSteps = serde_json::from_str(
            r#"{"suggestedNextCommitStep": "  Fix it ", "suggestedNextNoteStep": "   "}"#,
        )
        .unwrap();
        let mut p = params("T");
        p.suggested_next_steps = Some(steps);
        let submission = NoteSubmission::from_params(p, "B", NoteSubmissionSource::Tool).unwrap();
        assert_eq!(
            submission.suggested_next_steps,
            Some(SuggestedNextSteps {
                suggested_next_commit_step: Some("Fix it".to_string()),
                suggested_next_note_step: None,
            })
        );
    }

    #[test]
    fn handler_publishes_the_draft_and_tells_the_agent_to_end_the_turn() {
        let draft = Draft::new();
        let slot = new_submission_slot();
        assert!(!has_submission(&slot));

        let path = draft.write("\n\nFirst body ![Shot](staged-media://abc.png)\n");
        let first = handle_write_note("s1", &slot, &path, params("First"));
        assert_ne!(first.is_error, Some(true));
        let text = serde_json::to_string(&first.content).unwrap();
        assert!(text.contains("Saved note \\\"First\\\""), "{text}");
        assert!(text.contains("from the draft file"), "{text}");
        assert!(text.contains("end your turn"), "{text}");
        // The handler's text is `saved_message` for the stored title and body.
        let content = serde_json::to_value(&first.content).unwrap();
        assert_eq!(
            content[0]["text"],
            saved_message("First", "First body ![Shot](staged-media://abc.png)".len())
        );
        // The count is the body's UTF-8 length, and the text says so. The
        // rest asks for a bare "Done" unless a question is still unanswered.
        assert_eq!(
            saved_message("Né", "é→".len()),
            "Saved note \"Né\" (5 bytes from the draft file). If the user asked you a question \
this turn and you have not answered it in chat yet, answer it now, briefly. Otherwise send a \
simple \"Done\" message with no other text to end your turn, rather than summarising the work \
done."
        );
        let current = current_submission(&slot).unwrap();
        assert_eq!(current.body, "First body ![Shot](staged-media://abc.png)");

        // The agent edits the draft and republishes: the file wins, trimmed.
        draft.write("Second body");
        handle_write_note("s1", &slot, &path, params("Second"));
        let current = current_submission(&slot).unwrap();
        assert_eq!(current.title, "Second");
        assert_eq!(current.body, "Second body");
    }

    #[test]
    fn handler_rejects_a_bad_draft_naming_the_path_without_touching_the_slot() {
        let draft = Draft::new();
        let slot = new_submission_slot();
        let path = draft.path();
        let shown = path.to_string_lossy().into_owned();

        let expect_rejected = |what: &str| {
            let result = handle_write_note("s1", &slot, &path, params("Title"));
            assert_eq!(result.is_error, Some(true), "{what}");
            let text = serde_json::to_string(&result.content).unwrap();
            assert!(text.contains(&shown), "{what}: {text}");
            assert!(text.contains(what), "{what}: {text}");
            assert!(!has_submission(&slot), "{what} must not fill the slot");
        };

        expect_rejected("does not exist");
        draft.write("  \n\t\n");
        expect_rejected("is empty");
        std::fs::write(&path, [0xffu8, 0xfe, b'a']).unwrap();
        expect_rejected("not valid UTF-8");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(crate::note_draft::DRAFT_MAX_BYTES + 1)
            .unwrap();
        drop(file);
        expect_rejected("byte limit");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        expect_rejected("not a regular file");

        // A good draft but a bad title is still a rejection with no store.
        std::fs::remove_dir(&path).unwrap();
        draft.write("body");
        let result = handle_write_note("s1", &slot, &path, params(""));
        assert_eq!(result.is_error, Some(true));
        assert!(!has_submission(&slot));
    }

    /// A call with no `title` takes the title from the draft's first line
    /// and stores the body without that line.
    #[test]
    fn handler_takes_the_title_and_body_from_the_draft_when_the_call_has_no_title() {
        let draft = Draft::new();
        let slot = new_submission_slot();

        let path = draft.write(
            "\n\n# Design the outbox\n\nFirst body ![Shot](staged-media://abc.png)\n\n## Details\n",
        );
        let result = handle_write_note("s1", &slot, &path, no_title());
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let body = "First body ![Shot](staged-media://abc.png)\n\n## Details";
        // The result echoes the parsed title, and the count is the body
        // without the heading line.
        assert_eq!(
            result_text(&result),
            saved_message("Design the outbox", body.len())
        );
        let current = current_submission(&slot).unwrap();
        assert_eq!(
            current,
            NoteSubmission {
                title: "Design the outbox".to_string(),
                body: body.to_string(),
                suggested_next_steps: None,
                source: NoteSubmissionSource::Tool,
            }
        );
        // What the post-completion hooks read holds the title once.
        assert_eq!(
            current.as_note_content(),
            format!("# Design the outbox\n{body}")
        );

        // (draft, title, body)
        let cases = [
            // Only the first line is consumed: a body that opens with its
            // own H1 keeps it.
            ("# Title\n\n# Section\n\nText", "Title", "# Section\n\nText"),
            // The title text is kept as written, a `#` of its own included.
            ("# #12 follow-up\nBody", "#12 follow-up", "Body"),
            (
                "# Use `foo` for [bar](x) #\nBody",
                "Use `foo` for [bar](x) #",
                "Body",
            ),
            ("#\t Tabbed \t\nBody", "Tabbed", "Body"),
            // A byte order mark and blank lines above the heading are not
            // part of the draft the split sees.
            ("\u{feff}\n\n# Marked\r\n\r\nBody\r\n", "Marked", "Body"),
        ];
        for (text, title, body) in cases {
            draft.write(text);
            let result = handle_write_note("s1", &slot, &path, no_title());
            assert_ne!(result.is_error, Some(true), "{text:?}: {result:?}");
            assert_eq!(
                result_text(&result),
                saved_message(title, body.len()),
                "{text:?}"
            );
            let current = current_submission(&slot).unwrap();
            assert_eq!(
                (current.title.as_str(), current.body.as_str()),
                (title, body),
                "{text:?}"
            );
        }
    }

    /// The suggested next steps reach a submission the same way with and
    /// without `title`, and a call's JSON may leave `title` out or null.
    #[test]
    fn a_call_without_a_title_deserializes_and_keeps_its_suggested_next_steps() {
        let draft = Draft::new();
        let path = draft.write("# Plan\n\nBody");
        for arguments in [
            serde_json::json!({
                "suggested_next_steps": {
                    "suggestedNextCommitStep": "  Fix it ",
                    "suggestedNextNoteStep": "   ",
                }
            }),
            serde_json::json!({
                "title": null,
                "suggested_next_steps": {
                    "suggestedNextCommitStep": "Fix it",
                    "suggestedNextNoteStep": null,
                }
            }),
        ] {
            let params: WriteNoteParams = serde_json::from_value(arguments.clone())
                .unwrap_or_else(|e| panic!("{arguments}: {e}"));
            assert_eq!(params.title, None, "{arguments}");
            let slot = new_submission_slot();
            let result = handle_write_note("s1", &slot, &path, params);
            assert_ne!(result.is_error, Some(true), "{arguments}: {result:?}");
            assert_eq!(
                current_submission(&slot).unwrap(),
                NoteSubmission {
                    title: "Plan".to_string(),
                    body: "Body".to_string(),
                    suggested_next_steps: Some(SuggestedNextSteps {
                        suggested_next_commit_step: Some("Fix it".to_string()),
                        suggested_next_note_step: None,
                    }),
                    source: NoteSubmissionSource::Tool,
                },
                "{arguments}"
            );
        }

        let empty: WriteNoteParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(empty.title, None);
        assert_eq!(empty.suggested_next_steps, None);
        let titled: WriteNoteParams =
            serde_json::from_value(serde_json::json!({ "title": "T" })).unwrap();
        assert_eq!(titled.title.as_deref(), Some("T"));

        // `from_draft` keeps the source it is given.
        let submission =
            NoteSubmission::from_draft("# Plan\n\nBody", None, NoteSubmissionSource::Outbox)
                .unwrap();
        assert_eq!(submission.source, NoteSubmissionSource::Outbox);
        assert_eq!(submission.suggested_next_steps, None);
    }

    /// With no `title`, a draft that does not open with a `# Title` line, or
    /// has nothing under it, is rejected in `DraftError`'s words and the
    /// slot keeps what it held.
    #[test]
    fn handler_rejects_a_draft_with_no_title_line_and_leaves_the_slot_untouched() {
        let draft = Draft::new();
        let path = draft.path();
        let shown = path.to_string_lossy().into_owned();

        let cases = [
            ("Plain text\n\nBody", DraftError::MissingTitle),
            ("## Section\n\nBody", DraftError::MissingTitle),
            ("### Deeper\n\nBody", DraftError::MissingTitle),
            ("#\nBody", DraftError::MissingTitle),
            ("#   \nBody", DraftError::MissingTitle),
            ("#Title\n\nBody", DraftError::MissingTitle),
            ("#note:6c48d5ac\n\nBody", DraftError::MissingTitle),
            ("Title\n=====\n\nBody", DraftError::MissingTitle),
            ("---\ntitle: T\n---\n\nBody", DraftError::MissingTitle),
            ("Intro line\n# Title\n\nBody", DraftError::MissingTitle),
            ("# Title", DraftError::TitleOnly),
            ("\n# Title\n\n  \n", DraftError::TitleOnly),
        ];

        // An empty slot stays empty.
        let slot = new_submission_slot();
        for (text, error) in &cases {
            draft.write(text);
            let result = handle_write_note("s1", &slot, &path, no_title());
            assert_eq!(result.is_error, Some(true), "{text:?}");
            assert_eq!(result_text(&result), error.describe(&shown), "{text:?}");
            assert!(!has_submission(&slot), "{text:?} must not fill the slot");
        }

        // The texts, word for word.
        draft.write("## Section\n\nBody");
        assert_eq!(
            result_text(&handle_write_note("s1", &slot, &path, no_title())),
            format!(
                "write_note rejected: the draft file `{shown}` does not start with the note's \
title. Its first non-blank line must be a level-1 markdown heading: `# ` followed by the title \
on one line (not `##`, not a line underlined with `===`, and nothing above it). Add that line \
at the top and leave the rest of the note below it, then call write_note again."
            )
        );
        draft.write("# Title\n");
        assert_eq!(
            result_text(&handle_write_note("s1", &slot, &path, no_title())),
            format!(
                "write_note rejected: the draft file `{shown}` holds only the title line. \
Write the note body in markdown below the `# ` title line, then call write_note again."
            )
        );

        // A slot filled by an earlier call of the turn keeps that note.
        draft.write("# Kept\n\nKept body");
        handle_write_note("s1", &slot, &path, no_title());
        let kept = current_submission(&slot).unwrap();
        for (text, _) in &cases {
            draft.write(text);
            let result = handle_write_note("s1", &slot, &path, no_title());
            assert_eq!(result.is_error, Some(true), "{text:?}");
            assert_eq!(
                current_submission(&slot).as_ref(),
                Some(&kept),
                "{text:?} must not replace the earlier note"
            );
        }

        // Read errors come first and read the same with or without `title`.
        std::fs::remove_file(&path).unwrap();
        let without = handle_write_note("s1", &slot, &path, no_title());
        let with = handle_write_note("s1", &slot, &path, params("Title"));
        assert_eq!(result_text(&without), DraftError::Missing.describe(&shown));
        assert_eq!(result_text(&without), result_text(&with));
        draft.write("  \n\t\n");
        let without = handle_write_note("s1", &slot, &path, no_title());
        assert_eq!(result_text(&without), DraftError::Empty.describe(&shown));
        assert_eq!(current_submission(&slot).as_ref(), Some(&kept));
    }

    /// A call that still passes `title` is handled as before the argument
    /// became optional: the argument is the title, the whole draft is the
    /// body, and a heading at the top of the draft is neither required nor
    /// removed.
    #[test]
    fn a_call_that_passes_a_title_still_publishes_the_whole_draft_as_the_body() {
        let draft = Draft::new();
        let slot = new_submission_slot();
        let path = draft.path();

        // (draft, `title` argument, stored title, stored body)
        let cases = [
            // A draft that opens with a heading keeps it in the body, even
            // when it differs from the argument or repeats it.
            (
                "# Draft heading\n\nBody",
                "Passed title",
                "Passed title",
                "# Draft heading\n\nBody",
            ),
            ("# Same\n\nBody", "Same", "Same", "# Same\n\nBody"),
            // A heading with nothing under it is a body like any other.
            ("# Only a heading", "Passed", "Passed", "# Only a heading"),
            // Drafts the no-title path rejects are accepted with a title.
            ("Plain body", "Passed", "Passed", "Plain body"),
            (
                "## Section\n\nText",
                "Passed",
                "Passed",
                "## Section\n\nText",
            ),
            // The argument is still trimmed and loses one leading `#`.
            ("Body", "  # Design  ", "Design", "Body"),
        ];
        for (text, argument, title, body) in cases {
            draft.write(text);
            let result = handle_write_note("s1", &slot, &path, params(argument));
            assert_ne!(result.is_error, Some(true), "{text:?}: {result:?}");
            assert_eq!(
                result_text(&result),
                saved_message(title, body.len()),
                "{text:?}"
            );
            let current = current_submission(&slot).unwrap();
            assert_eq!(
                (current.title.as_str(), current.body.as_str()),
                (title, body),
                "{text:?}"
            );
        }

        // A bad `title` is rejected in the same words as before, whatever
        // the draft opens with: the heading does not stand in for it.
        let slot = new_submission_slot();
        draft.write("# Draft heading\n\nBody");
        for (argument, text) in [
            (
                "",
                "write_note rejected: `title` must not be empty. Call it again with a non-empty \
one-line title.",
            ),
            (
                "# ",
                "write_note rejected: `title` must not be empty. Call it again with a non-empty \
one-line title.",
            ),
            (
                "Two\nlines",
                "write_note rejected: `title` must be a single line. Call it again with a \
non-empty one-line title.",
            ),
        ] {
            let result = handle_write_note("s1", &slot, &path, params(argument));
            assert_eq!(result.is_error, Some(true), "{argument:?}");
            assert_eq!(result_text(&result), text, "{argument:?}");
            assert!(!has_submission(&slot), "{argument:?}");
        }
    }

    /// Making `title` optional for the handler does not change what the
    /// tool advertises: `title` is still a required string with the same
    /// description.
    #[test]
    fn input_schema_names_title_and_steps_only_with_camel_case_step_keys() {
        let schema = write_note_input_schema();
        let text = schema.to_string();
        assert!(text.contains("\"title\""), "{text}");
        assert!(!text.contains("\"body\""), "{text}");
        assert!(text.contains("\"suggested_next_steps\""), "{text}");
        assert!(text.contains("suggestedNextCommitStep"), "{text}");
        assert!(text.contains("suggestedNextNoteStep"), "{text}");
        let required = schema["required"].as_array().expect("required list");
        let required: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(required, ["title"]);
        assert_eq!(
            schema["properties"]["title"],
            serde_json::json!({
                // The field's doc comment, line break included.
                "description":
                    "The note title, shown as its heading. Plain text, no markdown, no\nleading `#`.",
                "type": "string",
            }),
            "{text}"
        );
    }
}
