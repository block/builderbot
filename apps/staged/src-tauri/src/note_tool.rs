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
note is saved, end your turn without repeating the note.";

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
    /// The note title, shown as its heading. Plain text, no markdown, no
    /// leading `#`.
    pub title: String,
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

impl NoteSubmission {
    /// Validate and normalize tool arguments plus the body read from the
    /// draft (locally) or the outbox's `note.md` (remotely) into a
    /// submission.
    ///
    /// The title and body must both be non-empty after trimming; a title that
    /// was written as a markdown H1 anyway has its `# ` stripped so the stored
    /// title stays plain text.
    pub(crate) fn from_params(
        params: WriteNoteParams,
        body: &str,
        source: NoteSubmissionSource,
    ) -> Result<Self, String> {
        let title = params.title.trim();
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
            suggested_next_steps: params
                .suggested_next_steps
                .map(SuggestedNextSteps::normalized),
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
/// the agent sees the same instruction on both transports.
pub(crate) fn saved_message(title: &str, body_len: usize) -> String {
    format!(
        "Saved note \"{title}\" ({body_len} characters from the draft file). The note is stored; \
end your turn now without repeating it in your reply."
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
note is saved, end your turn without repeating the note."
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
pub(crate) fn handle_write_note(
    session_id: &str,
    slot: &NoteSubmissionSlot,
    draft_path: &Path,
    params: WriteNoteParams,
) -> CallToolResult {
    let body = match crate::note_draft::read_draft(draft_path) {
        Ok(body) => body,
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
    match NoteSubmission::from_params(params, &body, NoteSubmissionSource::Tool) {
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
        Err(reason) => CallToolResult::error(vec![ContentBlock::text(format!(
            "write_note rejected: {reason}. Call it again with a non-empty one-line title."
        ))]),
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
            title: title.to_string(),
            suggested_next_steps: None,
        }
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
        assert!(
            NoteSubmission::from_params(params(""), "body", NoteSubmissionSource::Tool)
                .unwrap_err()
                .contains("title")
        );
        assert!(
            NoteSubmission::from_params(params("# "), "body", NoteSubmissionSource::Tool)
                .unwrap_err()
                .contains("title")
        );
        assert!(
            NoteSubmission::from_params(params("a\nb"), "body", NoteSubmissionSource::Tool)
                .unwrap_err()
                .contains("single line")
        );
        assert!(
            NoteSubmission::from_params(params("Title"), "  \n", NoteSubmissionSource::Tool)
                .unwrap_err()
                .contains("body")
        );
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
    }
}
