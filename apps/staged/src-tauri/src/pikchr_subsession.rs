//! Internal agent sub-session that turns a natural-language description into
//! validated Pikchr source, used by the `generate_pikchr` MCP tool.
//!
//! A focused ACP sub-agent is asked for a single fenced ```pikchr block, whose
//! source is rendered through the internal [`crate::pikchr_mcp::run_preview`]
//! path. On a parse error the sub-agent is re-prompted with the specific
//! failure, resuming the *same* sub-session so the grammar and prior attempts
//! stay in context — a diagram that doesn't render is useless to hand back. The
//! loop is bounded by [`MAX_ATTEMPTS`]. Overlaps are *not* repaired here: the
//! first diagram that renders is returned as-is, its summary carrying any
//! overlap warnings for the calling note agent to review and decide on.
//!
//! The sub-session is deliberately **not** persisted: it uses in-memory `Store`
//! and `MessageWriter` stubs so its transcript never pollutes the user's
//! session messages. The store stub captures the agent session id so it can be
//! threaded into the next turn for resumption; the writer stub just accumulates
//! assistant text so the loop can read the candidate diagram back.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::agent::AgentDriver;
use crate::pikchr_mcp::run_preview;

/// Total sub-agent turns before giving up. Each parse error or empty reply
/// consumes one. 5 leaves room for a couple of repair rounds without letting a
/// hopeless request run the provider subprocess forever.
const MAX_ATTEMPTS: usize = 5;
/// Synthetic session id for the sub-session. The in-memory store keys nothing
/// meaningful on it; any stable string is fine.
const SUBSESSION_ID: &str = "pikchr-subsession";

/// Result of a `generate_pikchr` sub-session.
pub(crate) struct GenOutcome {
    /// The validated Pikchr source (no fences) — drop it into a ```pikchr block.
    pub(crate) source: String,
    /// Rendered PNG preview, if rasterization succeeded.
    pub(crate) png: Option<Vec<u8>>,
    /// Render summary (dimensions + any overlap warnings) for the returned
    /// source. Overlaps are advisory: the caller reviews the summary and the
    /// PNG and decides whether to keep the diagram or re-call to adjust it.
    pub(crate) summary: String,
}

/// Drive the sub-agent to produce validated Pikchr for `description`.
///
/// Generic over [`AgentDriver`] so it can be unit-tested with a fake driver
/// instead of spawning a real provider subprocess.
pub(crate) async fn generate_pikchr_source<D: AgentDriver + ?Sized>(
    driver: &D,
    grammar_reference: &str,
    description: &str,
    previous_pikchr: Option<&str>,
    scale: f32,
    cancel_token: &CancellationToken,
) -> Result<GenOutcome, String> {
    // The sub-agent needs no repo access; the grammar path is absolute.
    let working_dir = std::env::temp_dir();
    let store_capture = Arc::new(SessionIdCapture::default());
    let store_dyn: Arc<dyn acp_client::Store> = store_capture.clone();

    let mut prompt = initial_prompt(grammar_reference, description, previous_pikchr);
    let mut agent_session_id: Option<String> = None;

    for _ in 0..MAX_ATTEMPTS {
        if cancel_token.is_cancelled() {
            break;
        }

        // Fresh writer per turn: it only accumulates and never clears on
        // finalize, so we read the whole turn's text back after `run` returns.
        let writer = Arc::new(CapturingWriter::default());
        let writer_dyn: Arc<dyn acp_client::MessageWriter> = writer.clone();

        driver
            .run(
                SUBSESSION_ID,
                &prompt,
                &[],
                &working_dir,
                &store_dyn,
                &writer_dyn,
                cancel_token,
                agent_session_id.as_deref(),
            )
            .await?;

        // Resume the same sub-session next turn so context is retained. The
        // driver only sets this on the first (new-session) turn.
        if let Some(id) = store_capture.agent_session_id() {
            agent_session_id = Some(id);
        }

        // A cancelled run returns Ok with partial text; don't treat that as a
        // real attempt.
        if cancel_token.is_cancelled() {
            break;
        }

        let source = extract_pikchr_source(&writer.text());
        if source.trim().is_empty() {
            prompt = empty_reply_prompt();
            continue;
        }

        let preview = run_preview(&source, scale);
        if preview.is_error {
            prompt = parse_error_prompt(&preview.summary);
            continue;
        }

        // It renders — return it as-is. Overlaps are advisory warnings carried
        // in the summary; the calling agent decides on them, so we don't loop
        // or re-prompt here.
        return Ok(GenOutcome {
            source,
            png: preview.png,
            summary: preview.summary,
        });
    }

    Err("The Pikchr specialist could not produce a diagram that renders successfully.".to_string())
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

// =============================================================================
// In-memory Store + MessageWriter stubs
// =============================================================================

/// Captures the ACP agent session id set on the first (new-session) turn so it
/// can be threaded into later turns for resumption. `get_session_messages`
/// keeps the trait default (empty), so no user transcript is replayed.
#[derive(Default)]
struct SessionIdCapture {
    agent_session_id: Mutex<Option<String>>,
}

impl SessionIdCapture {
    fn agent_session_id(&self) -> Option<String> {
        self.agent_session_id.lock().unwrap().clone()
    }
}

impl acp_client::Store for SessionIdCapture {
    fn set_agent_session_id(
        &self,
        _session_id: &str,
        agent_session_id: &str,
    ) -> Result<(), String> {
        *self.agent_session_id.lock().unwrap() = Some(agent_session_id.to_string());
        Ok(())
    }
}

/// Accumulates the assistant text for one turn. Unlike the DB-backed writer,
/// `finalize` does **not** clear the buffer — the loop reads the whole turn's
/// text after `run` returns — and a fresh writer is created for each turn.
#[derive(Default)]
struct CapturingWriter {
    text: Mutex<String>,
}

impl CapturingWriter {
    fn text(&self) -> String {
        self.text.lock().unwrap().clone()
    }
}

#[async_trait]
impl acp_client::MessageWriter for CapturingWriter {
    async fn append_text(&self, text: &str) {
        self.text.lock().unwrap().push_str(text);
    }

    async fn finalize(&self) {}

    async fn record_tool_call(
        &self,
        _tool_call_id: &str,
        _title: &str,
        _raw_input: Option<&serde_json::Value>,
    ) {
    }

    async fn update_tool_call_title(
        &self,
        _tool_call_id: &str,
        _title: Option<&str>,
        _raw_input: Option<&serde_json::Value>,
    ) {
    }

    async fn record_tool_result(&self, _content: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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

    /// Scripted driver that replays canned replies turn by turn and records the
    /// `agent_session_id` it was handed each turn (to assert resumption).
    struct FakeDriver {
        replies: Vec<String>,
        calls: Mutex<usize>,
        seen_session_ids: Mutex<Vec<Option<String>>>,
    }

    impl FakeDriver {
        fn new(replies: Vec<String>) -> Self {
            Self {
                replies,
                calls: Mutex::new(0),
                seen_session_ids: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait(?Send)]
    impl AgentDriver for FakeDriver {
        async fn run(
            &self,
            session_id: &str,
            _prompt: &str,
            _images: &[(String, String)],
            _working_dir: &Path,
            store: &Arc<dyn acp_client::Store>,
            writer: &Arc<dyn acp_client::MessageWriter>,
            _cancel_token: &CancellationToken,
            agent_session_id: Option<&str>,
        ) -> Result<(), String> {
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
            Ok(())
        }
    }

    fn fenced(source: &str) -> String {
        format!("```pikchr\n{source}\n```")
    }

    #[tokio::test]
    async fn returns_immediately_on_overlapping_render() {
        let driver = FakeDriver::new(vec![fenced(OVERLAPPING_SOURCE)]);

        let outcome = generate_pikchr_source(
            &driver,
            "/tmp/grammar.md",
            "a busy diagram",
            None,
            2.0,
            &CancellationToken::new(),
        )
        .await
        .expect("should return the first renderable diagram");

        // Renders with overlaps — returned as-is, no retries spent on overlap.
        assert_eq!(outcome.source, OVERLAPPING_SOURCE);
        assert!(outcome.png.is_some());
        assert!(outcome.summary.contains("overlapping pair"));
        assert_eq!(*driver.calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn repairs_parse_error_then_returns_overlapping_render() {
        let driver = FakeDriver::new(vec![
            fenced("box \"unterminated"), // parse error, repaired
            fenced(OVERLAPPING_SOURCE),   // renders with overlaps, kept
        ]);

        let outcome = generate_pikchr_source(
            &driver,
            "/tmp/grammar.md",
            "a friendly box",
            None,
            2.0,
            &CancellationToken::new(),
        )
        .await
        .expect("should repair the parse error and return the overlapping render");

        // Parse error repaired; overlap left for the caller to decide on.
        assert_eq!(outcome.source, OVERLAPPING_SOURCE);
        assert!(outcome.png.is_some());
        assert!(outcome.summary.contains("overlapping pair"));
        assert_eq!(*driver.calls.lock().unwrap(), 2);

        // First turn starts a session; the repair turn resumes it.
        let seen = driver.seen_session_ids.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], None);
        assert_eq!(seen[1].as_deref(), Some("fake-agent-session"));
    }

    #[tokio::test]
    async fn errors_when_nothing_ever_renders() {
        let driver = FakeDriver::new(vec![fenced("box \"unterminated"); MAX_ATTEMPTS + 2]);

        let result = generate_pikchr_source(
            &driver,
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
