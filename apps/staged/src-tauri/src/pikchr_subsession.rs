//! Internal agent sub-session that turns a natural-language description into
//! validated Pikchr source, used by the `generate_pikchr` MCP tool.
//!
//! A focused ACP sub-agent is asked for a single fenced ```pikchr block, whose
//! source is rendered through the internal [`crate::pikchr_mcp::run_preview`]
//! path. On a parse error the sub-agent is re-prompted with the specific
//! failure, resuming the *same* sub-session so the grammar and prior attempts
//! stay in context — a diagram that doesn't render is useless to hand back. The
//! same loop now re-prompts on layout warnings too — overlapping elements or
//! elements extending beyond the diagram bounds — so the calling note agent
//! only receives the final source and preview path. The loop is bounded by
//! [`MAX_ATTEMPTS`].
//!
//! The sub-session is persisted as a normal `sessions` row. Each attempted
//! prompt and assistant reply is written into that child session so the parent
//! tool call can later link to the specialist transcript.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::agent::{AgentDriver, MessageWriter};
use crate::pikchr_mcp::run_preview;
use crate::store::{CompletionReason, MessageRole, SessionStatus, Store};

/// Total sub-agent turns before giving up. Each parse error or empty reply
/// consumes one, and each renderable candidate flagged with layout warnings
/// (overlaps, out-of-bounds elements) also consumes one. 5 leaves room for a
/// couple of repair rounds without letting a hopeless request run the provider
/// subprocess forever.
const MAX_ATTEMPTS: usize = 5;
/// Result of a `generate_pikchr` sub-session.
pub(crate) struct GenOutcome {
    /// The validated Pikchr source (no fences) — drop it into a ```pikchr block.
    pub(crate) source: String,
    /// Rendered PNG preview, if rasterization succeeded.
    pub(crate) png: Option<Vec<u8>>,
}

/// Drive the sub-agent to produce validated Pikchr for `description`.
///
/// Generic over [`AgentDriver`] so it can be unit-tested with a fake driver
/// instead of spawning a real provider subprocess.
pub(crate) async fn generate_pikchr_source<D: AgentDriver + ?Sized>(
    driver: &D,
    store: Arc<Store>,
    session_id: &str,
    grammar_reference: &str,
    description: &str,
    previous_pikchr: Option<&str>,
    scale: f32,
    cancel_token: &CancellationToken,
) -> Result<GenOutcome, String> {
    let result = generate_pikchr_source_inner(
        driver,
        Arc::clone(&store),
        session_id,
        grammar_reference,
        description,
        previous_pikchr,
        scale,
        cancel_token,
    )
    .await;

    let status_result = match &result {
        Ok(_) => store.update_session_status(
            session_id,
            SessionStatus::Completed,
            None,
            Some(&CompletionReason::TurnComplete),
        ),
        Err(GenerationError::Cancelled) => store.update_session_status(
            session_id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::Interrupted),
        ),
        Err(GenerationError::Failed(message)) => store.update_session_status(
            session_id,
            SessionStatus::Error,
            Some(message),
            Some(&CompletionReason::Crashed),
        ),
    };

    match (result, status_result) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        // The diagram was generated successfully; a status-bookkeeping failure
        // shouldn't discard it. The session stays Running until dead-session
        // recovery on the next launch.
        (Ok(outcome), Err(e)) => {
            log::warn!(
                "[pikchr_subsession] failed to mark Pikchr session {session_id} completed: {e}"
            );
            Ok(outcome)
        }
        (Err(e), Ok(())) => Err(e.to_string()),
        (Err(e), Err(status_error)) => Err(format!(
            "{}; additionally failed to update Pikchr session status: {status_error}",
            e
        )),
    }
}

async fn generate_pikchr_source_inner<D: AgentDriver + ?Sized>(
    driver: &D,
    store: Arc<Store>,
    session_id: &str,
    grammar_reference: &str,
    description: &str,
    previous_pikchr: Option<&str>,
    scale: f32,
    cancel_token: &CancellationToken,
) -> Result<GenOutcome, GenerationError> {
    // The sub-agent needs no repo access; the grammar path is absolute.
    let working_dir = std::env::temp_dir();
    let store_dyn: Arc<dyn acp_client::Store> = store.clone();

    let mut prompt = initial_prompt(grammar_reference, description, previous_pikchr);
    let mut agent_session_id: Option<String> = None;

    for _ in 0..MAX_ATTEMPTS {
        if cancel_token.is_cancelled() {
            return Err(GenerationError::Cancelled);
        }

        let prompt_message_id = store
            .add_session_message(session_id, MessageRole::User, &prompt)
            .map_err(|e| {
                GenerationError::Failed(format!("Failed to persist Pikchr prompt: {e}"))
            })?;
        let writer = Arc::new(MessageWriter::new(
            session_id.to_string(),
            Arc::clone(&store),
        ));
        let writer_dyn: Arc<dyn acp_client::MessageWriter> = writer.clone();

        let run_outcome = driver
            .run(
                session_id,
                &prompt,
                &[],
                &working_dir,
                &store_dyn,
                &writer_dyn,
                cancel_token,
                agent_session_id.as_deref(),
                &[],
            )
            .await;
        writer_dyn.finalize().await;

        let run_outcome = run_outcome.map_err(GenerationError::Failed)?;
        if run_outcome == acp_client::AgentRunOutcome::Cancelled || cancel_token.is_cancelled() {
            return Err(GenerationError::Cancelled);
        }

        // Resume the same sub-session next turn so context is retained. The
        // real store implementation persists this when the driver creates the
        // ACP session on the first turn.
        if let Some(id) = stored_agent_session_id(&store, session_id)? {
            agent_session_id = Some(id);
        }

        let reply = latest_assistant_reply_since(&store, session_id, prompt_message_id)?;
        let source = extract_pikchr_source(&reply);
        if source.trim().is_empty() {
            prompt = empty_reply_prompt();
            continue;
        }

        let preview = run_preview(&source, scale);
        if preview.is_error {
            prompt = parse_error_prompt(&preview.summary);
            continue;
        }

        if preview.has_overlaps || preview.has_out_of_bounds {
            prompt = layout_warning_prompt(&source, &preview.summary);
            continue;
        }

        // It renders cleanly, with no layout warnings for the caller to
        // interpret.
        return Ok(GenOutcome {
            source,
            png: preview.png,
        });
    }

    Err(GenerationError::Failed(
        "The Pikchr specialist could not produce a diagram that renders cleanly.".to_string(),
    ))
}

#[derive(Debug)]
enum GenerationError {
    Failed(String),
    Cancelled,
}

impl std::fmt::Display for GenerationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(message) => write!(f, "{message}"),
            Self::Cancelled => write!(f, "The Pikchr specialist was cancelled."),
        }
    }
}

fn stored_agent_session_id(
    store: &Store,
    session_id: &str,
) -> Result<Option<String>, GenerationError> {
    store
        .get_session(session_id)
        .map(|session| session.and_then(|s| s.agent_id))
        .map_err(|e| GenerationError::Failed(format!("Failed to load Pikchr child session: {e}")))
}

fn latest_assistant_reply_since(
    store: &Store,
    session_id: &str,
    since_id: i64,
) -> Result<String, GenerationError> {
    store
        .get_session_messages_since(session_id, since_id)
        .map_err(|e| GenerationError::Failed(format!("Failed to load Pikchr assistant reply: {e}")))
        .map(|messages| {
            messages
                .into_iter()
                .rev()
                .find(|message| message.role == MessageRole::Assistant)
                .map(|message| message.content)
                .unwrap_or_default()
        })
}

/// Pull the Pikchr source out of a sub-agent reply. Prefers a real
/// ```pikchr / ~~~pikchr fence; falls back to stripping a generic code fence
/// (or using the trimmed reply as-is when the agent skipped fences entirely).
fn extract_pikchr_source(reply: &str) -> String {
    if let Some(block) = crate::pikchr_validation::extract_pikchr_blocks(reply)
        .into_iter()
        .next()
    {
        return block.source;
    }
    acp_client::strip_code_fences(reply).trim().to_string()
}

// =============================================================================
// Prompt templates
// =============================================================================

fn initial_prompt(reference: &str, description: &str, previous_pikchr: Option<&str>) -> String {
    let mut prompt = format!(
        "You are a Pikchr diagram specialist. Reply with ONLY the diagram as a single fenced \
```pikchr code block — no prose, no explanation. The Pikchr grammar reference is at `{reference}`; \
consult it for exact syntax."
    );
    if let Some(previous) = previous_pikchr {
        prompt.push_str(&format!(
            "\n\nHere is the current diagram to modify:\n```pikchr\n{previous}\n```\n\
Revise it per the request below."
        ));
    }
    prompt.push_str(&format!("\n\nDiagram to produce: {description}"));
    prompt
}

fn empty_reply_prompt() -> String {
    "Your reply did not contain a Pikchr diagram. Resend ONLY a single fenced ```pikchr code block \
containing the diagram — no prose."
        .to_string()
}

fn parse_error_prompt(error: &str) -> String {
    format!(
        "That failed to render. Pikchr reported:\n{error}\n\
Fix it and resend ONLY the ```pikchr code block."
    )
}

fn layout_warning_prompt(source: &str, summary: &str) -> String {
    format!(
        "That diagram rendered, but the preview analysis found layout warnings:\n{summary}\n\
\n\
Current source:\n```pikchr\n{source}\n```\n\
Fix the reported layout issues while preserving the requested diagram content. Resend ONLY the \
corrected ```pikchr code block."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    use async_trait::async_trait;

    use crate::store::{Session, Store};

    /// A diagram known to render with overlapping boxes (percentage-length
    /// arrows between large `fit` boxes with no explicit flow direction).
    const OVERLAPPING_SOURCE: &str = r#"linerad = 4px
box "goose-internal (OPEN SOURCE)" "typed OTel catalog → TelemetrySink facade" fit fill 0xeef6ff
arrow down 35%
box "Sink = OTLP exporter (Block build)" "via Tauri native export_otel_logs (CORS)" fit fill 0xfff3d6
arrow right 60% "OTLP /v1/logs + auth" above
box "Block OTel Collector" "OTel→UAP mapping (from CDF manifest)" fit fill 0xffe6e6
arrow right 50%
box "unifiedevents/batch" "→ Snowflake (UAP unchanged)" fit fill 0xffd6d6
arrow down 30% from 1st box.s
box "Sink = NO-OP (default / external clone)" "no socket, no Block deps → builds anywhere" fit fill 0xe8f5e9"#;

    const CLEAN_SOURCE: &str = r#"box "Clean" fit"#;

    /// Renders fine, but a negative margin shrinks Pikchr's computed canvas
    /// below its content, so the box geometry (font-independent) crosses the
    /// diagram edges.
    const OUT_OF_BOUNDS_SOURCE: &str = "margin = -0.2in\nbox \"Out\"";

    /// Scripted driver that replays canned replies turn by turn and records the
    /// `agent_session_id` it was handed each turn (to assert resumption).
    struct FakeDriver {
        replies: Vec<String>,
        calls: Mutex<usize>,
        seen_session_ids: Mutex<Vec<Option<String>>>,
        prompts: Mutex<Vec<String>>,
    }

    impl FakeDriver {
        fn new(replies: Vec<String>) -> Self {
            Self {
                replies,
                calls: Mutex::new(0),
                seen_session_ids: Mutex::new(Vec::new()),
                prompts: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait(?Send)]
    impl AgentDriver for FakeDriver {
        async fn run(
            &self,
            session_id: &str,
            prompt: &str,
            _images: &[(String, String)],
            _working_dir: &Path,
            store: &Arc<dyn acp_client::Store>,
            writer: &Arc<dyn acp_client::MessageWriter>,
            _cancel_token: &CancellationToken,
            agent_session_id: Option<&str>,
            _config_options: &[acp_client::AcpSessionConfigOptionSelection],
        ) -> Result<acp_client::AgentRunOutcome, String> {
            let idx = {
                let mut calls = self.calls.lock().unwrap();
                let idx = *calls;
                *calls += 1;
                idx
            };
            self.seen_session_ids
                .lock()
                .unwrap()
                .push(agent_session_id.map(str::to_string));
            self.prompts.lock().unwrap().push(prompt.to_string());

            // Mimic a new-session turn: register an agent session id so the
            // loop resumes it next time.
            if agent_session_id.is_none() {
                store
                    .set_agent_session_id(session_id, "fake-agent-session")
                    .unwrap();
            }

            let reply = self.replies.get(idx).cloned().unwrap_or_default();
            writer.append_text(&reply).await;
            writer.finalize().await;
            Ok(acp_client::AgentRunOutcome::Completed)
        }
    }

    fn fenced(source: &str) -> String {
        format!("```pikchr\n{source}\n```")
    }

    fn child_session() -> (Arc<Store>, String) {
        let store = Arc::new(Store::in_memory().expect("in-memory store"));
        let session = Session::new_running("Generate Pikchr diagram", &std::env::temp_dir())
            .with_provider("fake-agent");
        let session_id = session.id.clone();
        store
            .create_session(&session)
            .expect("create child session");
        (store, session_id)
    }

    #[tokio::test]
    async fn repairs_overlapping_render_before_returning() {
        let driver = FakeDriver::new(vec![fenced(OVERLAPPING_SOURCE), fenced(CLEAN_SOURCE)]);
        let (store, session_id) = child_session();

        let outcome = generate_pikchr_source(
            &driver,
            Arc::clone(&store),
            &session_id,
            "/tmp/grammar.md",
            "a busy diagram",
            None,
            2.0,
            &CancellationToken::new(),
        )
        .await
        .expect("should repair the overlapping render");

        assert_eq!(outcome.source, CLEAN_SOURCE);
        assert!(outcome.png.is_some());
        assert_eq!(*driver.calls.lock().unwrap(), 2);

        let prompts = driver.prompts.lock().unwrap();
        assert!(prompts[1].contains("layout warnings"));
        assert!(prompts[1].contains("overlapping pair"));
        assert!(prompts[1].contains(OVERLAPPING_SOURCE));
    }

    #[tokio::test]
    async fn repairs_out_of_bounds_render_before_returning() {
        let driver = FakeDriver::new(vec![fenced(OUT_OF_BOUNDS_SOURCE), fenced(CLEAN_SOURCE)]);

        let outcome = generate_pikchr_source(
            &driver,
            "/tmp/grammar.md",
            "a cramped diagram",
            None,
            2.0,
            &CancellationToken::new(),
        )
        .await
        .expect("should repair the out-of-bounds render");

        assert_eq!(outcome.source, CLEAN_SOURCE);
        assert_eq!(*driver.calls.lock().unwrap(), 2);

        let prompts = driver.prompts.lock().unwrap();
        assert!(prompts[1].contains("layout warnings"));
        assert!(prompts[1].contains("beyond the diagram bounds"));
        assert!(prompts[1].contains(OUT_OF_BOUNDS_SOURCE));
    }

    #[tokio::test]
    async fn repairs_parse_error_then_overlap_before_returning() {
        let driver = FakeDriver::new(vec![
            fenced("box \"unterminated"), // parse error, repaired
            fenced(OVERLAPPING_SOURCE),   // renders with overlaps, repaired
            fenced(CLEAN_SOURCE),
        ]);
        let (store, session_id) = child_session();

        let outcome = generate_pikchr_source(
            &driver,
            Arc::clone(&store),
            &session_id,
            "/tmp/grammar.md",
            "a friendly box",
            None,
            2.0,
            &CancellationToken::new(),
        )
        .await
        .expect("should repair the parse error and overlap");

        assert_eq!(outcome.source, CLEAN_SOURCE);
        assert!(outcome.png.is_some());
        assert_eq!(*driver.calls.lock().unwrap(), 3);

        let prompts = driver.prompts.lock().unwrap();
        assert!(prompts[1].contains("failed to render"));
        assert!(prompts[2].contains("overlapping pair"));

        // First turn starts a session; repair turns resume it.
        let seen = driver.seen_session_ids.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0], None);
        assert_eq!(seen[1].as_deref(), Some("fake-agent-session"));
        assert_eq!(seen[2].as_deref(), Some("fake-agent-session"));
    }

    #[tokio::test]
    async fn persists_prompts_assistant_messages_agent_id_and_terminal_status() {
        let driver = FakeDriver::new(vec![fenced("box \"unterminated"), fenced(CLEAN_SOURCE)]);
        let (store, session_id) = child_session();

        let outcome = generate_pikchr_source(
            &driver,
            Arc::clone(&store),
            &session_id,
            "/tmp/grammar.md",
            "a friendly box",
            None,
            2.0,
            &CancellationToken::new(),
        )
        .await
        .expect("should repair and complete");

        assert_eq!(outcome.source, CLEAN_SOURCE);

        let session = store
            .get_session(&session_id)
            .expect("load session")
            .expect("session exists");
        assert_eq!(session.status, SessionStatus::Completed);
        assert_eq!(session.provider.as_deref(), Some("fake-agent"));
        assert_eq!(session.agent_id.as_deref(), Some("fake-agent-session"));
        assert_eq!(
            session.completion_reason.as_ref(),
            Some(&CompletionReason::TurnComplete)
        );

        let messages = store
            .get_session_messages(&session_id)
            .expect("load messages");
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, MessageRole::User);
        assert!(messages[0]
            .content
            .contains("Diagram to produce: a friendly box"));
        assert_eq!(messages[1].role, MessageRole::Assistant);
        assert_eq!(messages[1].content, fenced("box \"unterminated"));
        assert_eq!(messages[2].role, MessageRole::User);
        assert!(messages[2].content.contains("failed to render"));
        assert_eq!(messages[3].role, MessageRole::Assistant);
        assert_eq!(messages[3].content, fenced(CLEAN_SOURCE));
    }

    #[tokio::test]
    async fn errors_when_nothing_ever_renders() {
        let driver = FakeDriver::new(vec![fenced("box \"unterminated"); MAX_ATTEMPTS + 2]);
        let (store, session_id) = child_session();

        let result = generate_pikchr_source(
            &driver,
            Arc::clone(&store),
            &session_id,
            "/tmp/grammar.md",
            "impossible",
            None,
            2.0,
            &CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        // The loop is bounded by MAX_ATTEMPTS even when every reply fails.
        assert_eq!(*driver.calls.lock().unwrap(), MAX_ATTEMPTS);

        let session = store
            .get_session(&session_id)
            .expect("load session")
            .expect("session exists");
        assert_eq!(session.status, SessionStatus::Error);
        assert_eq!(
            session.error_message.as_deref(),
            Some("The Pikchr specialist could not produce a diagram that renders cleanly.")
        );
    }

    #[tokio::test]
    async fn errors_when_overlaps_never_repair() {
        let driver = FakeDriver::new(vec![fenced(OVERLAPPING_SOURCE); MAX_ATTEMPTS + 2]);
        let (store, session_id) = child_session();

        let result = generate_pikchr_source(
            &driver,
            Arc::clone(&store),
            &session_id,
            "/tmp/grammar.md",
            "impossible",
            None,
            2.0,
            &CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(*driver.calls.lock().unwrap(), MAX_ATTEMPTS);
    }

    #[test]
    fn extract_prefers_pikchr_fence() {
        let reply = "Here you go:\n```pikchr\nbox \"A\"\n```\nhope that helps";
        assert_eq!(extract_pikchr_source(reply), "box \"A\"");
    }

    #[test]
    fn extract_falls_back_to_generic_fence() {
        let reply = "```\nbox \"B\"\n```";
        assert_eq!(extract_pikchr_source(reply), "box \"B\"");
    }

    #[test]
    fn extract_uses_raw_reply_without_fence() {
        assert_eq!(extract_pikchr_source("  box \"C\"  "), "box \"C\"");
    }

    #[test]
    fn initial_prompt_embeds_previous_source_when_revising() {
        let prompt = initial_prompt("/tmp/grammar.md", "add a box", Some("box \"old\""));
        assert!(prompt.contains("/tmp/grammar.md"));
        assert!(prompt.contains("current diagram to modify"));
        assert!(prompt.contains("box \"old\""));
        assert!(prompt.contains("add a box"));
    }

    #[test]
    fn initial_prompt_omits_revision_block_for_fresh_diagram() {
        let prompt = initial_prompt("/tmp/grammar.md", "a fresh box", None);
        assert!(!prompt.contains("current diagram to modify"));
        assert!(prompt.contains("a fresh box"));
    }
}
