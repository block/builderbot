//! The `write_note` MCP tool: a note session's deliverable as a tool call.
//!
//! A note session used to hand its note back as assistant text after a `---`
//! separator, parsed on the laptop once the turn settled. That contract is
//! fragile (stray rules in prose, unterminated fences, "first block wins" for
//! the next-steps JSON) and ties the deliverable to the transcript. Instead the
//! agent writes the note into its per-session draft file (`note_draft`), the
//! title as a `# ` heading on the first line and the body below it, and calls
//! `write_note` with structured suggested next steps; the call publishes the
//! draft. The call is the deliverable; the assistant's own text is not saved,
//! and a draft that changed without a call is not stored either.
//!
//! The title is not an argument. The handler takes it from the draft's first
//! line and stores the rest as the body, so title and body stay separate in
//! the store. A `title` argument is still read, though no schema or prompt
//! names it, for a session resumed with the earlier instructions in its
//! context: see [`WriteNoteParams`].
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
Staged stores the current contents of your note draft file (the path is in your instructions) as the note, \
and does not read your assistant text. The file's first line must be the note's title as a level-1 markdown \
heading (`# Title`), and everything below that line is the note body. Write the whole note into the draft \
file first. Calling again republishes the file, and the last call of the turn wins. After the result says the \
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

/// Arguments of `write_note`. There is no `body` and no `title`: both are
/// the draft file, whose first line is the title. The schema is derived from
/// this struct, so the Staged MCP on Blox's hand-written schema must stay in
/// step with it (see the `write_note_schema_matches_the_rust_tool_schema`
/// test in `remote_staged_mcp::note_outbox`).
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
pub struct WriteNoteParams {
    /// What the user might want to do next. Keep both fields to a few words
    /// and use null when there is no clear next action.
    #[serde(default)]
    pub suggested_next_steps: Option<SuggestedNextSteps>,
    // The argument `title` was until the title moved into the draft. It is
    // in no schema and no prompt (`schemars(skip)`), and is read only so
    // that a session resumed with the earlier instructions in its context
    // is answered, not silently overruled: a `title` that agrees with the
    // draft's title line is accepted, one that does not is rejected
    // (`stale_title_agrees`, `SubmissionError::StaleTitle`). Absent and
    // null are the normal call. Any JSON value deserializes, so a `title`
    // that is not a string is rejected by the same rule and in the same
    // words as on the Staged MCP on Blox, which reads `args.title` as it
    // comes. To be removed once sessions started under the earlier
    // contract are unlikely to be resumed.
    #[serde(default)]
    #[schemars(skip)]
    pub title: Option<serde_json::Value>,
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
    /// The draft does not open with its title, or has nothing below it
    /// ([`crate::note_draft::split_title`]).
    Draft(DraftError),
    /// The call passed the former `title` argument and it names a different
    /// title than the draft's first line does, which is `heading_title`
    /// ([`stale_title_agrees`]).
    StaleTitle { heading_title: String },
}

impl SubmissionError {
    /// The `write_note` error text. It names `draft_path` so the agent
    /// fixes the file within the turn.
    pub(crate) fn describe(&self, draft_path: &str) -> String {
        match self {
            Self::Draft(error) => error.describe(draft_path),
            Self::StaleTitle { heading_title } => format!(
                "write_note rejected: `title` is no longer an argument. The note's title is \
                 the first line of the draft file `{draft_path}`, currently \
                 \"{heading_title}\", which differs from the `title` you passed. Edit that \
                 line if the title should change, then call write_note again without `title`."
            ),
        }
    }
}

/// Whether a `title` argument passed under the earlier contract names the
/// title the draft's first line holds (`heading_title`, as
/// [`crate::note_draft::split_title`] returns it).
///
/// It does when it is a string that, trimmed, is the heading title as
/// written, or is that title behind one `#`: the earlier contract stripped
/// one leading `#` from the argument, so a call that passed `# Plan` meant
/// the title `Plan`. Nothing more is stripped. `## Plan` therefore agrees
/// with a draft that opens `# # Plan` or `# ## Plan` and not with one that
/// opens `# Plan`. Anything else, an empty string and a value that is not a
/// string included, does not agree.
///
/// Under the earlier contract the two transports disagreed about an
/// argument with more than one `#` (the laptop stripped a second one from
/// a remote session's title, so `## Plan` was stored as `# Plan` locally
/// and as `Plan` remotely). This rule is the only one now, and
/// `staged-mcp.cjs` (`staleTitleAgrees`) applies it with the same trim;
/// `write_note_reads_the_same_drafts_on_both_transports` in
/// `remote_staged_mcp::note_outbox` runs both over the same calls.
fn stale_title_agrees(argument: &serde_json::Value, heading_title: &str) -> bool {
    let Some(argument) = argument.as_str() else {
        return false;
    };
    let argument = argument.trim();
    argument == heading_title
        || argument
            .strip_prefix('#')
            .is_some_and(|rest| rest.trim() == heading_title)
}

impl NoteSubmission {
    /// Turn a call's arguments and the draft (`note_draft::read_draft`'s
    /// text) into a submission.
    ///
    /// The draft's first line is the title and the rest is the body
    /// ([`Self::from_draft`]); a draft without that line, or with nothing
    /// under it, is refused. A call that also passes the former `title`
    /// argument is accepted when the argument agrees with the draft's title
    /// and refused when it does not ([`stale_title_agrees`]): ignoring it
    /// would silently drop a rename asked for by an agent whose context
    /// still holds the earlier instructions. The draft is checked first, so
    /// such a call on a draft with no title line is told to add the line.
    pub(crate) fn from_params(
        params: WriteNoteParams,
        draft: &str,
        source: NoteSubmissionSource,
    ) -> Result<Self, SubmissionError> {
        let submission = Self::from_draft(draft, params.suggested_next_steps, source)
            .map_err(SubmissionError::Draft)?;
        match &params.title {
            Some(stale) if !stale_title_agrees(stale, &submission.title) => {
                Err(SubmissionError::StaleTitle {
                    heading_title: submission.title,
                })
            }
            _ => Ok(submission),
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
    /// non-empty title and a non-empty body, both trimmed. A remote session
    /// gets the same title: the script splits the draft by the same rule
    /// and the outbox reader takes its title through
    /// [`Self::from_parts_as_written`].
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

    /// Validate and normalize a title, a body and suggested next steps that
    /// arrived apart into a submission, for a title that was a `title`
    /// argument of the tool. The outbox reader uses this for an outbox
    /// written by a script from before version 8, whose `meta.json` holds
    /// that argument and whose `note.md` holds the whole draft
    /// (`remote_staged_mcp::note_outbox`). A turn that outlives its
    /// connection keeps the script it started with, so such outboxes are
    /// still written.
    ///
    /// The title and body must both be non-empty after trimming; a title that
    /// was written as a markdown H1 anyway has its `# ` stripped so the stored
    /// title stays plain text.
    ///
    /// That strip is for a title that was a tool argument. A title that was
    /// parsed from the draft's heading must not pass through here: see
    /// [`Self::from_parts_as_written`].
    pub(crate) fn from_parts(
        title: &str,
        body: &str,
        suggested_next_steps: Option<SuggestedNextSteps>,
        source: NoteSubmissionSource,
    ) -> Result<Self, String> {
        let title = title.trim();
        let title = title.strip_prefix('#').unwrap_or(title);
        Self::from_parts_as_written(title, body, suggested_next_steps, source)
    }

    /// [`Self::from_parts`] for a title that is already the one to store:
    /// it is trimmed and checked, and a `#` it starts with is kept.
    ///
    /// The outbox reader uses this for an outbox whose writer settled the
    /// title (the Staged MCP on Blox from version 8). Such a title may be
    /// the draft's heading text, which [`Self::from_draft`] keeps as written
    /// for a local session, so `# #12 follow-up` is titled `#12 follow-up`
    /// on both transports. The rejection reasons are `from_parts`' own.
    pub(crate) fn from_parts_as_written(
        title: &str,
        body: &str,
        suggested_next_steps: Option<SuggestedNextSteps>,
        source: NoteSubmissionSource,
    ) -> Result<Self, String> {
        let title = title.trim();
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
when it is ready call write_note (suggested next steps) to publish it; your assistant text is \
not saved. Its first line must be the note's title as a level-1 markdown heading (`# Title`)."
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
Staged stores the current contents of your note draft file (the path is in your instructions) as the note, \
and does not read your assistant text. The file's first line must be the note's title as a level-1 markdown \
heading (`# Title`), and everything below that line is the note body. Write the whole note into the draft \
file first. Calling again republishes the file, and the last call of the turn wins. After the result says the \
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
/// The title then comes from the draft's first line, a `# Title` heading,
/// and the body is what lies below it. A draft that has no such line, or
/// nothing under it, is rejected the same way as an unreadable one, and so
/// is a call whose former `title` argument names another title than that
/// line. See [`NoteSubmission::from_params`].
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

    /// The normal call: no arguments beyond the optional next steps.
    fn no_title() -> WriteNoteParams {
        WriteNoteParams::default()
    }

    /// A call that still passes the former `title` argument.
    fn stale_title(title: &str) -> WriteNoteParams {
        WriteNoteParams {
            title: Some(serde_json::Value::String(title.to_string())),
            suggested_next_steps: None,
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

    /// The stale-`title` rejection for a draft at `path` titled `heading`.
    fn stale_title_text(path: &str, heading: &str) -> String {
        format!(
            "write_note rejected: `title` is no longer an argument. The note's title is the \
first line of the draft file `{path}`, currently \"{heading}\", which differs from the `title` \
you passed. Edit that line if the title should change, then call write_note again without \
`title`."
        )
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

    /// The title is the draft's first line, kept as written: it is trimmed
    /// and nothing is stripped from it, and the body is the rest, trimmed.
    #[test]
    fn submission_takes_the_title_from_the_draft_heading_as_written() {
        let submission = NoteSubmission::from_params(
            no_title(),
            "#   Design  \n\nBody text.\n",
            NoteSubmissionSource::Tool,
        )
        .unwrap();
        assert_eq!(submission.title, "Design");
        assert_eq!(submission.body, "Body text.");
        assert_eq!(submission.as_note_content(), "# Design\nBody text.");
        assert_eq!(submission.source, NoteSubmissionSource::Tool);

        // A `#` the title text starts with is the title's own.
        let title_of = |draft: &str| {
            NoteSubmission::from_params(no_title(), draft, NoteSubmissionSource::Tool)
                .unwrap()
                .title
        };
        assert_eq!(title_of("# # Design\nBody"), "# Design");
        assert_eq!(title_of("# #12 follow-up\nBody"), "#12 follow-up");
    }

    /// What used to be a bad `title` argument is now a bad first line of
    /// the draft: no title text, or no body under it.
    #[test]
    fn submission_rejects_a_draft_without_a_title_line_or_without_a_body() {
        let reject = |draft: &str| {
            NoteSubmission::from_params(no_title(), draft, NoteSubmissionSource::Tool).unwrap_err()
        };
        for draft in ["body", "# \nbody", "#\nbody", "## Title\nbody", "a\nb"] {
            assert_eq!(
                reject(draft),
                SubmissionError::Draft(DraftError::MissingTitle),
                "{draft:?}"
            );
        }
        for draft in ["# Title", "# Title\n  \n"] {
            assert_eq!(
                reject(draft),
                SubmissionError::Draft(DraftError::TitleOnly),
                "{draft:?}"
            );
        }
    }

    /// `from_parts` is the reader of an outbox from before version 8, whose
    /// title was a `title` argument: trimmed, one leading `#` stripped,
    /// empty and multi-line titles and empty bodies refused in these words.
    #[test]
    fn from_parts_normalizes_and_validates_an_argument_title() {
        let steps = || {
            Some(SuggestedNextSteps {
                suggested_next_commit_step: Some("  Fix it ".to_string()),
                suggested_next_note_step: Some("   ".to_string()),
            })
        };
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
        // The source and the absence of steps are kept.
        let plain =
            NoteSubmission::from_parts("Plain title", "Body", None, NoteSubmissionSource::Tool)
                .unwrap();
        assert_eq!(plain.source, NoteSubmissionSource::Tool);
        assert_eq!(plain.suggested_next_steps, None);
        // One `#` only, and the whole body, a heading at its top included.
        let deeper = NoteSubmission::from_parts(
            "## Deeper",
            "# Section\n\nText",
            None,
            NoteSubmissionSource::Outbox,
        )
        .unwrap();
        assert_eq!(deeper.title, "# Deeper");
        assert_eq!(deeper.body, "# Section\n\nText");

        let reject = |title: &str, body: &str| {
            NoteSubmission::from_parts(title, body, None, NoteSubmissionSource::Outbox).unwrap_err()
        };
        assert_eq!(reject("", "body"), "`title` must not be empty");
        assert_eq!(reject("# ", "body"), "`title` must not be empty");
        assert_eq!(reject("a\nb", "body"), "`title` must be a single line");
        assert_eq!(reject("Title", "  \n"), "the note body must not be empty");
        assert_eq!(reject("", ""), "`title` must not be empty");
    }

    /// `from_parts_as_written` is `from_parts` without the `#` strip: the
    /// same submission for a title that does not start with `#`, the same
    /// rejections, and for a draft that opens with its title the submission
    /// `from_draft` builds from the draft itself. That last equality is what
    /// gives a note the same title whether the laptop split the draft (a
    /// local session) or the script did and the outbox carried the parts.
    #[test]
    fn from_parts_as_written_keeps_a_leading_hash_and_matches_from_draft() {
        let as_written = |title: &str, body: &str| {
            NoteSubmission::from_parts_as_written(title, body, None, NoteSubmissionSource::Outbox)
        };
        let title_of = |title: &str| as_written(title, "Body").unwrap().title;
        assert_eq!(title_of("#12 follow-up"), "#12 follow-up");
        assert_eq!(title_of("  # Design  "), "# Design");
        assert_eq!(title_of("## Deeper"), "## Deeper");
        assert_eq!(title_of("#"), "#");

        // Nothing to strip: the two agree, steps and body included.
        let steps = Some(SuggestedNextSteps {
            suggested_next_commit_step: Some("  Fix it ".to_string()),
            suggested_next_note_step: Some("   ".to_string()),
        });
        for (title, body) in [
            ("Plain title", "Body"),
            ("  Padded  ", "\nBody text.\n"),
            ("Ends with a hash #", "# Section\n\nText"),
            ("", "body"),
            ("  ", "body"),
            ("a\nb", "body"),
            ("Title", "  \n"),
        ] {
            for source in [NoteSubmissionSource::Tool, NoteSubmissionSource::Outbox] {
                assert_eq!(
                    NoteSubmission::from_parts_as_written(title, body, steps.clone(), source),
                    NoteSubmission::from_parts(title, body, steps.clone(), source),
                    "title {title:?}, body {body:?}"
                );
            }
        }
        assert_eq!(
            as_written("", "body").unwrap_err(),
            "`title` must not be empty"
        );
        assert_eq!(
            as_written("a\nb", "body").unwrap_err(),
            "`title` must be a single line"
        );
        assert_eq!(
            as_written("Title", " \n").unwrap_err(),
            "the note body must not be empty"
        );

        // The parts of a split draft, carried separately, give the
        // submission the draft itself gives.
        for draft in [
            "# Plan\n\nBody",
            "# #12 follow-up\nBody",
            "# # Spaced\n\n# Section\n\nText",
            "# Use `foo` for [bar](x) #\nBody",
            "#\t Tabbed \t\r\n\r\nBody",
        ] {
            let (title, body) = crate::note_draft::split_title(draft).unwrap();
            assert_eq!(
                NoteSubmission::from_parts_as_written(
                    &title,
                    &body,
                    steps.clone(),
                    NoteSubmissionSource::Outbox
                )
                .unwrap(),
                NoteSubmission::from_draft(draft, steps.clone(), NoteSubmissionSource::Outbox)
                    .unwrap(),
                "{draft:?}"
            );
        }
    }

    #[test]
    fn suggested_next_steps_use_the_legacy_camel_case_keys_and_normalize_blanks() {
        let steps: SuggestedNextSteps = serde_json::from_str(
            r#"{"suggestedNextCommitStep": "  Fix it ", "suggestedNextNoteStep": "   "}"#,
        )
        .unwrap();
        let mut p = no_title();
        p.suggested_next_steps = Some(steps);
        let submission =
            NoteSubmission::from_params(p, "# T\nB", NoteSubmissionSource::Tool).unwrap();
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

        let path = draft.write("\n\n# First\n\nFirst body ![Shot](staged-media://abc.png)\n");
        let first = handle_write_note("s1", &slot, &path, no_title());
        assert_ne!(first.is_error, Some(true));
        let text = serde_json::to_string(&first.content).unwrap();
        assert!(text.contains("Saved note \\\"First\\\""), "{text}");
        assert!(text.contains("from the draft file"), "{text}");
        assert!(text.contains("end your turn"), "{text}");
        // The handler's text is `saved_message` for the stored title and
        // body: the count leaves the title line out.
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
        assert_eq!(current.title, "First");
        assert_eq!(current.body, "First body ![Shot](staged-media://abc.png)");

        // The agent edits the draft, its title line included, and
        // republishes: the file wins, trimmed.
        draft.write("# Second\n\nSecond body");
        handle_write_note("s1", &slot, &path, no_title());
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
            let result = handle_write_note("s1", &slot, &path, no_title());
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

        // A readable draft that is only a body is still a rejection with no
        // store, and so is one that is only a title.
        std::fs::remove_dir(&path).unwrap();
        draft.write("body");
        expect_rejected("does not start with the note's title");
        draft.write("# Title");
        expect_rejected("holds only the title line");

        // The missing and empty texts, word for word: they ask for the
        // whole note, title line first.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            result_text(&handle_write_note("s1", &slot, &path, no_title())),
            format!(
                "write_note rejected: the draft file `{shown}` does not exist. This file is \
your note: write the full note into it as markdown, starting with the title as a `# ` heading \
on the first line, then call write_note again."
            )
        );
        draft.write("\n \n");
        assert_eq!(
            result_text(&handle_write_note("s1", &slot, &path, no_title())),
            format!(
                "write_note rejected: the draft file `{shown}` is empty. Write the full note \
into it as markdown, starting with the title as a `# ` heading on the first line, then call \
write_note again."
            )
        );
    }

    /// The title comes from the draft's first line and the stored body is
    /// the draft without that line.
    #[test]
    fn handler_takes_the_title_and_body_from_the_draft() {
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

    /// A call's JSON may leave `title` out or pass null, and its suggested
    /// next steps reach the submission either way.
    #[test]
    fn a_call_deserializes_without_a_title_and_keeps_its_suggested_next_steps() {
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
        // A stale `title` deserializes whatever its JSON type, so the
        // handler, not the argument parser, answers it.
        for stale in [
            serde_json::json!("T"),
            serde_json::json!(12),
            serde_json::json!(["T"]),
            serde_json::json!({"text": "T"}),
            serde_json::json!(false),
        ] {
            let params: WriteNoteParams =
                serde_json::from_value(serde_json::json!({ "title": stale })).unwrap();
            assert_eq!(params.title, Some(stale));
        }

        // `from_draft` keeps the source it is given.
        let submission =
            NoteSubmission::from_draft("# Plan\n\nBody", None, NoteSubmissionSource::Outbox)
                .unwrap();
        assert_eq!(submission.source, NoteSubmissionSource::Outbox);
        assert_eq!(submission.suggested_next_steps, None);
    }

    /// A draft that does not open with a `# Title` line, or has nothing
    /// under it, is rejected in `DraftError`'s words and the slot keeps
    /// what it held.
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

        // The draft comes before a stale `title`: a call that still passes
        // one is told about the draft, in the same words, and the argument
        // does not stand in for the missing line.
        for (text, error) in &cases {
            draft.write(text);
            let result = handle_write_note("s1", &slot, &path, stale_title("Title"));
            assert_eq!(result_text(&result), error.describe(&shown), "{text:?}");
        }
        std::fs::remove_file(&path).unwrap();
        let without = handle_write_note("s1", &slot, &path, no_title());
        let with = handle_write_note("s1", &slot, &path, stale_title("Title"));
        assert_eq!(result_text(&without), DraftError::Missing.describe(&shown));
        assert_eq!(result_text(&without), result_text(&with));
        draft.write("  \n\t\n");
        let without = handle_write_note("s1", &slot, &path, no_title());
        let with = handle_write_note("s1", &slot, &path, stale_title("Title"));
        assert_eq!(result_text(&without), DraftError::Empty.describe(&shown));
        assert_eq!(result_text(&without), result_text(&with));
        assert_eq!(current_submission(&slot).as_ref(), Some(&kept));
    }

    /// The rule for a `title` argument left over from the earlier contract:
    /// it agrees when, trimmed, it is the draft's title as written or that
    /// title behind one `#`.
    #[test]
    fn a_stale_title_agrees_only_with_the_heading_title_or_that_title_behind_one_hash() {
        let agrees = |argument: &str, heading: &str| {
            stale_title_agrees(&serde_json::Value::String(argument.to_string()), heading)
        };
        // (argument, heading title, agrees)
        let cases = [
            ("Plan", "Plan", true),
            ("  Plan \n", "Plan", true),
            // The earlier contract stripped one `#` from the argument.
            ("# Plan", "Plan", true),
            ("#Plan", "Plan", true),
            ("  #   Plan  ", "Plan", true),
            // One only: a second `#` is part of the title that was meant.
            ("## Plan", "Plan", false),
            ("# # Plan", "Plan", false),
            ("### Plan", "Plan", false),
            ("## Plan", "# Plan", true),
            ("# # Plan", "# Plan", true),
            ("## Plan", "## Plan", true),
            ("### Plan", "## Plan", true),
            ("### Plan", "# Plan", false),
            // A heading title that starts with `#` agrees as written, and
            // with one more `#` in front, never with one fewer.
            ("#12 follow-up", "#12 follow-up", true),
            ("##12 follow-up", "#12 follow-up", true),
            ("# #12 follow-up", "#12 follow-up", true),
            ("12 follow-up", "#12 follow-up", false),
            ("# Plan", "# Plan", true),
            ("Plan", "# Plan", false),
            // Different text, case and inner spacing.
            ("Plan B", "Plan", false),
            ("plan", "Plan", false),
            ("Plan  B", "Plan B", false),
            ("Plan\nB", "Plan B", false),
            // Nothing passed as text.
            ("", "Plan", false),
            ("   ", "Plan", false),
            ("#", "Plan", false),
            ("#", "#", true),
            ("##", "#", true),
            // Rust's whitespace is trimmed: U+3000 and U+0085 are, a byte
            // order mark is text.
            ("\u{3000}Plan\u{85}", "Plan", true),
            ("#\u{3000}Plan", "Plan", true),
            ("\u{feff}Plan", "Plan", false),
            ("\u{feff}Plan", "\u{feff}Plan", true),
        ];
        for (argument, heading, expected) in cases {
            assert_eq!(
                agrees(argument, heading),
                expected,
                "argument {argument:?}, heading title {heading:?}"
            );
        }
        // A value that is not a string never agrees.
        for value in [
            serde_json::json!(12),
            serde_json::json!(true),
            serde_json::json!(["Plan"]),
            serde_json::json!({"title": "Plan"}),
        ] {
            assert!(!stale_title_agrees(&value, "Plan"), "{value}");
            assert!(!stale_title_agrees(&value, &value.to_string()), "{value}");
        }
    }

    /// A call that still passes `title`: accepted when it agrees with the
    /// draft's title line, with the heading as the stored title either way,
    /// and rejected with the text that names the heading when it does not.
    /// A rejected call leaves the slot as it was.
    #[test]
    fn a_stale_title_that_matches_is_accepted_and_one_that_differs_is_rejected() {
        let draft = Draft::new();
        let path = draft.path();
        let shown = path.to_string_lossy().into_owned();

        // (draft, `title` argument, stored title, stored body)
        let accepted = [
            ("# Plan\n\nBody", "Plan", "Plan", "Body"),
            ("# Plan\n\nBody", "  Plan  ", "Plan", "Body"),
            // The heading is the title that is stored, not the argument's
            // spelling of it.
            ("# Plan\n\nBody", "# Plan", "Plan", "Body"),
            (
                "# #12 follow-up\nBody",
                "#12 follow-up",
                "#12 follow-up",
                "Body",
            ),
            ("# # Plan\n\nBody", "## Plan", "# Plan", "Body"),
            // The body is the draft without the title line, as for a call
            // with no `title`: the heading is not kept in it.
            (
                "# Same\n\n# Section\n\nText",
                "Same",
                "Same",
                "# Section\n\nText",
            ),
        ];
        for (text, argument, title, body) in accepted {
            let slot = new_submission_slot();
            draft.write(text);
            let result = handle_write_note("s1", &slot, &path, stale_title(argument));
            assert_ne!(result.is_error, Some(true), "{argument:?}: {result:?}");
            assert_eq!(
                result_text(&result),
                saved_message(title, body.len()),
                "{argument:?}"
            );
            // The same submission the call makes without the argument.
            let with = current_submission(&slot).unwrap();
            assert_eq!(
                (with.title.as_str(), with.body.as_str()),
                (title, body),
                "{argument:?}"
            );
            let slot = new_submission_slot();
            handle_write_note("s1", &slot, &path, no_title());
            assert_eq!(current_submission(&slot), Some(with), "{argument:?}");
        }

        // (draft, `title` argument, the heading title the text quotes)
        let rejected = [
            ("# Plan\n\nBody", "Another title", "Plan"),
            ("# Plan\n\nBody", "plan", "Plan"),
            ("# Plan\n\nBody", "", "Plan"),
            ("# Plan\n\nBody", "#", "Plan"),
            ("# Plan\n\nBody", "Two\nlines", "Plan"),
            // More than one `#` is not stripped down to the heading.
            ("# Plan\n\nBody", "## Plan", "Plan"),
            ("# Plan\n\nBody", "# # Plan", "Plan"),
            // A `#` of the heading title's own is not optional.
            ("# #12 follow-up\nBody", "12 follow-up", "#12 follow-up"),
            // The quoted title is the heading as written.
            (
                "# Use `foo` for \"bar\"\nBody",
                "Use foo for bar",
                "Use `foo` for \"bar\"",
            ),
        ];
        let slot = new_submission_slot();
        for (text, argument, heading) in rejected {
            draft.write(text);
            let result = handle_write_note("s1", &slot, &path, stale_title(argument));
            assert_eq!(result.is_error, Some(true), "{argument:?}");
            assert_eq!(
                result_text(&result),
                stale_title_text(&shown, heading),
                "{argument:?}"
            );
            assert!(
                !has_submission(&slot),
                "{argument:?} must not fill the slot"
            );
        }

        // A `title` that is not a string is rejected in the same words.
        draft.write("# Plan\n\nBody");
        for value in [serde_json::json!(12), serde_json::json!(["Plan"])] {
            let params: WriteNoteParams =
                serde_json::from_value(serde_json::json!({ "title": value })).unwrap();
            let result = handle_write_note("s1", &slot, &path, params);
            assert_eq!(result.is_error, Some(true), "{value}");
            assert_eq!(
                result_text(&result),
                stale_title_text(&shown, "Plan"),
                "{value}"
            );
        }

        // The text, word for word.
        assert_eq!(
            result_text(&handle_write_note(
                "s1",
                &slot,
                &path,
                stale_title("Renamed plan")
            )),
            format!(
                "write_note rejected: `title` is no longer an argument. The note's title is \
the first line of the draft file `{shown}`, currently \"Plan\", which differs from the `title` \
you passed. Edit that line if the title should change, then call write_note again without \
`title`."
            )
        );

        // A note an earlier call of the turn saved is kept, and the call
        // the text asks for then goes through.
        handle_write_note("s1", &slot, &path, no_title());
        let kept = current_submission(&slot).unwrap();
        draft.write("# Plan\n\nEdited body");
        let result = handle_write_note("s1", &slot, &path, stale_title("Renamed plan"));
        assert_eq!(result.is_error, Some(true));
        assert_eq!(current_submission(&slot).as_ref(), Some(&kept));
        draft.write("# Renamed plan\n\nEdited body");
        let result = handle_write_note("s1", &slot, &path, no_title());
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let renamed = current_submission(&slot).unwrap();
        assert_eq!(
            (renamed.title.as_str(), renamed.body.as_str()),
            ("Renamed plan", "Edited body")
        );
    }

    /// The tool advertises no `title` and requires nothing: the one
    /// property is the optional next steps.
    #[test]
    fn input_schema_names_only_the_steps_and_requires_nothing() {
        let schema = write_note_input_schema();
        let text = schema.to_string();
        assert!(!text.contains("\"body\""), "{text}");
        assert!(text.contains("suggestedNextCommitStep"), "{text}");
        assert!(text.contains("suggestedNextNoteStep"), "{text}");
        let properties: Vec<&str> = schema["properties"]
            .as_object()
            .expect("an object schema")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(properties, ["suggested_next_steps"], "{text}");
        // The only `title` left is the schema's own name for the struct.
        assert_eq!(text.matches("\"title\"").count(), 1, "{text}");
        assert_eq!(schema["title"], "WriteNoteParams", "{text}");
        let required = schema
            .get("required")
            .and_then(|required| required.as_array())
            .cloned()
            .unwrap_or_default();
        assert!(required.is_empty(), "no required fields: {text}");
    }

    /// Every text the tool itself shows the agent describes the new
    /// contract: the title is the draft's first line, not an argument.
    #[test]
    fn the_tool_texts_put_the_title_in_the_draft_and_not_in_the_call() {
        assert!(
            WRITE_NOTE_DESCRIPTION.contains(
                "The file's first line must be the note's title as a level-1 markdown heading \
(`# Title`), and everything below that line is the note body."
            ),
            "{WRITE_NOTE_DESCRIPTION}"
        );
        let instructions = draft_instructions("/tmp/draft.md");
        assert_eq!(
            instructions,
            "Your note draft file is `/tmp/draft.md`. This file is your note: write it, edit \
it, and when it is ready call write_note (suggested next steps) to publish it; your assistant \
text is not saved. Its first line must be the note's title as a level-1 markdown heading \
(`# Title`)."
        );
        for text in [WRITE_NOTE_DESCRIPTION, instructions.as_str()] {
            for stale in ["title you pass", "title and suggested", "without the title"] {
                assert!(!text.contains(stale), "{stale}: {text}");
            }
        }
    }
}
