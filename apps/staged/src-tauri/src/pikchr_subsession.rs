//! Internal agent sub-session that turns a natural-language description into
//! validated Pikchr source, used by the `generate_pikchr` MCP tool.
//!
//! The specialist owns the whole iteration loop inside its own ACP session: it
//! drafts Pikchr, calls the `render_pikchr` MCP tool (served per-call by
//! [`crate::pikchr_mcp`]) to render and analyze each candidate, inspects the
//! returned image and layout report, and revises until satisfied. Every
//! successful render overwrites the shared [`LastRenderSlot`]; the specialist
//! accepts by ending its reply with [`ACCEPT_SENTINEL`] as the final line.
//! Acceptance points at the slot's validated contents rather than carrying
//! source in text, so unvalidated source can never reach the caller no matter
//! what the reply says. The host loop here only checks for the sentinel line
//! and re-prompts on protocol misses — a reply without it, or acceptance
//! before anything rendered — bounded by [`MAX_ATTEMPTS`].
//!
//! The sub-session is persisted as a normal `sessions` row. Each prompt and
//! assistant reply is written into that child session so the parent tool call
//! can later link to the specialist transcript.

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::agent::{AgentDriver, MessageWriter};
use crate::session_commands::PIKCHR_GRAMMAR_URL;
use crate::store::{CompletionReason, MessageRole, SessionStatus, Store};

/// Total sub-agent turns before giving up. The specialist iterates on the
/// diagram *within* a turn via `render_pikchr`, so this only bounds protocol
/// misses — a reply without the sentinel, or acceptance before anything
/// rendered successfully — not design iteration. Exhaustion is the error case.
const MAX_ATTEMPTS: usize = 3;

/// Token the specialist ends its turn with to accept the last successful
/// render. It only counts when it is the reply's final line (see
/// [`reply_accepts_last_render`]): the token appears verbatim in the prompts
/// and every `render_pikchr` result, so a model echoing the instructions
/// mid-prose ("I'll end with AcceptLastRender once…") must not read as
/// acceptance — a contains-check would.
pub(crate) const ACCEPT_SENTINEL: &str = "AcceptLastRender";

/// Explanation used when the cancellation token fired without a recorded
/// reason: nothing inside the worker arms the token, so the parent MCP request
/// must have been dropped — the caller cancelled, crashed, or hit its own
/// client-side timeout.
const ABANDONED_CANCEL_MESSAGE: &str = "The generate_pikchr call was abandoned by its caller \
(cancelled, or timed out on the caller's side) before the specialist finished, so the diagram \
run was cancelled.";

/// Why the sub-session's cancellation token was armed, recorded by the
/// initiator before it cancels (the pikchr worker's wall-clock timeout, or a
/// user Stop on the child session forwarded from its registered token). Read
/// when the run winds down so the cancelled child session — and the error
/// handed back to the caller — can say what killed the run instead of a bare
/// "cancelled". No recorded reason resolves to [`ABANDONED_CANCEL_MESSAGE`].
pub(crate) struct CancelReason(Mutex<Option<String>>);

impl CancelReason {
    pub(crate) fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// Record why the token is about to be cancelled. The first reason wins:
    /// a timeout that fires while the parent is already tearing down should
    /// not have its message replaced.
    pub(crate) fn record(&self, reason: String) {
        let mut slot = self.0.lock().unwrap();
        if slot.is_none() {
            *slot = Some(reason);
        }
    }

    pub(crate) fn resolve(&self) -> String {
        self.0
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| ABANDONED_CANCEL_MESSAGE.to_string())
    }
}

/// Result of a `generate_pikchr` sub-session: the render the specialist
/// accepted.
pub(crate) struct GenOutcome {
    /// The validated Pikchr source (no fences) — drop it into a ```pikchr block.
    pub(crate) source: String,
    /// Rendered PNG preview, if rasterization succeeded.
    pub(crate) png: Option<Vec<u8>>,
}

/// The last *successful* render produced through the specialist's
/// `render_pikchr` tool. The tool server overwrites it on every successful
/// render (last write wins; parse failures leave it untouched) and the host
/// loop takes it when the specialist ends its reply with [`ACCEPT_SENTINEL`]
/// — the render gate is the only way anything reaches this slot.
pub(crate) struct LastRenderSlot(Mutex<Option<GenOutcome>>);

impl LastRenderSlot {
    pub(crate) fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// Overwrite the slot with a new successful render.
    pub(crate) fn store(&self, outcome: GenOutcome) {
        *self.0.lock().unwrap() = Some(outcome);
    }

    /// Take the accepted render out of the slot, leaving it empty.
    pub(crate) fn take(&self) -> Option<GenOutcome> {
        self.0.lock().unwrap().take()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.lock().unwrap().is_none()
    }
}

/// Drive the sub-agent to produce validated Pikchr for `description`.
///
/// Generic over [`AgentDriver`] so it can be unit-tested with a fake driver
/// instead of spawning a real provider subprocess.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn generate_pikchr_source<D: AgentDriver + ?Sized>(
    driver: &D,
    store: Arc<Store>,
    session_id: &str,
    grammar: Option<&str>,
    description: &str,
    previous_pikchr: Option<&str>,
    slot: &LastRenderSlot,
    cancel_token: &CancellationToken,
    cancel_reason: &CancelReason,
) -> Result<GenOutcome, String> {
    let result = generate_pikchr_source_inner(
        driver,
        Arc::clone(&store),
        session_id,
        grammar,
        description,
        previous_pikchr,
        slot,
        cancel_token,
    )
    .await;

    // A token cancellation is externally imposed — the wall-clock timeout or
    // the caller abandoning the MCP request — so resolve the recorded reason
    // once and tell the same story on the child session's status row and in
    // the error handed back to the caller.
    let cancel_message =
        matches!(&result, Err(GenerationError::Cancelled)).then(|| cancel_reason.resolve());

    // Terminal status goes through `transition_from_running`: a user cancel
    // normally fires this run's registered token (and lands here as
    // `Cancelled`), but one arriving before the worker registers the child
    // session takes `cancel_session`'s fallback path and writes Cancelled
    // straight to the store — an unconditional write here would silently
    // clobber it.
    let status_result = match &result {
        Ok(_) => store.transition_from_running(
            session_id,
            SessionStatus::Completed,
            None,
            Some(&CompletionReason::TurnComplete),
        ),
        Err(GenerationError::Cancelled) => store.transition_from_running(
            session_id,
            SessionStatus::Cancelled,
            cancel_message.as_deref(),
            Some(&CompletionReason::Interrupted),
        ),
        Err(GenerationError::Failed(message)) => store.transition_from_running(
            session_id,
            SessionStatus::Error,
            Some(message),
            Some(&CompletionReason::Crashed),
        ),
    };

    match (result, status_result) {
        (Ok(outcome), Ok(transitioned)) => {
            if !transitioned {
                // A concurrent transition (e.g. a user cancel) won the race;
                // its status stands, but the generated diagram still goes back
                // to the caller.
                log::info!(
                    "[pikchr_subsession] Pikchr session {session_id} already left Running; \
not marking it completed"
                );
            }
            Ok(outcome)
        }
        // The diagram was generated successfully; a status-bookkeeping failure
        // shouldn't discard it. The session stays Running until dead-session
        // recovery on the next launch.
        (Ok(outcome), Err(e)) => {
            log::warn!(
                "[pikchr_subsession] failed to mark Pikchr session {session_id} completed: {e}"
            );
            Ok(outcome)
        }
        (Err(e), Ok(_)) => Err(generation_error_text(e, cancel_message)),
        (Err(e), Err(status_error)) => Err(format!(
            "{}; additionally failed to update Pikchr session status: {status_error}",
            generation_error_text(e, cancel_message)
        )),
    }
}

/// The error text handed back to the `generate_pikchr` caller. A cancellation
/// reports its resolved reason (always `Some` when the error is `Cancelled`)
/// so a timeout reads as a timeout rather than a generic cancel.
fn generation_error_text(error: GenerationError, cancel_message: Option<String>) -> String {
    match error {
        GenerationError::Failed(message) => message,
        GenerationError::Cancelled => {
            cancel_message.unwrap_or_else(|| ABANDONED_CANCEL_MESSAGE.to_string())
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn generate_pikchr_source_inner<D: AgentDriver + ?Sized>(
    driver: &D,
    store: Arc<Store>,
    session_id: &str,
    grammar: Option<&str>,
    description: &str,
    previous_pikchr: Option<&str>,
    slot: &LastRenderSlot,
    cancel_token: &CancellationToken,
) -> Result<GenOutcome, GenerationError> {
    // The sub-agent needs no repo access; the grammar is inlined in the prompt.
    let working_dir = std::env::temp_dir();
    let store_dyn: Arc<dyn acp_client::Store> = store.clone();

    let mut prompt = initial_prompt(grammar, description, previous_pikchr);
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
        if reply_accepts_last_render(&reply) {
            // Acceptance points at the slot, not at the reply text: only a
            // render that passed through `render_pikchr` can be handed back.
            match slot.take() {
                Some(outcome) => return Ok(outcome),
                None => {
                    prompt = accepted_without_render_prompt();
                    continue;
                }
            }
        }
        prompt = missing_sentinel_prompt();
    }

    Err(GenerationError::Failed(
        "The Pikchr specialist did not accept a rendered diagram.".to_string(),
    ))
}

#[derive(Debug)]
enum GenerationError {
    Failed(String),
    /// The cancellation token fired; the "why" lives in [`CancelReason`] and
    /// is resolved by [`generation_error_text`].
    Cancelled,
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

/// The whole reply must end with [`ACCEPT_SENTINEL`] as its own line.
/// Case, surrounding backticks (the prompts quote the token in backticks),
/// and trailing sentence punctuation are forgiven; extra words on the line
/// are not.
fn reply_accepts_last_render(reply: &str) -> bool {
    reply.trim_end().lines().next_back().is_some_and(|line| {
        line.trim_matches(|c: char| c.is_whitespace() || matches!(c, '`' | '.' | '!'))
            .eq_ignore_ascii_case(ACCEPT_SENTINEL)
    })
}

// =============================================================================
// Prompt templates
// =============================================================================

/// `grammar` is the full grammar text to inline into the message — the
/// sub-session has no repo access, so a file path would be a dead reference.
/// `None` (bundled grammar missing or unreadable) falls back to naming the
/// public grammar URL instead.
fn initial_prompt(
    grammar: Option<&str>,
    description: &str,
    previous_pikchr: Option<&str>,
) -> String {
    let grammar_line = match grammar {
        Some(_) => {
            "Consult the full Pikchr grammar reference included below for exact syntax.".to_string()
        }
        None => format!(
            "The Pikchr grammar reference is at {PIKCHR_GRAMMAR_URL}; consult it for exact syntax."
        ),
    };
    let mut prompt = format!(
        "You are a Pikchr diagram specialist. {grammar_line}\n\
\n\
Workflow: draft the diagram source, then call the `render_pikchr` tool with it (no code fences). \
Inspect the returned image and layout analysis, then revise and render again until the diagram \
renders cleanly, the analysis reports no warnings (unless a warning is intentional), and the image \
matches the request. When you are satisfied, accept the render: your whole message must end with \
`{ACCEPT_SENTINEL}` as its own line. The accepted diagram is your last successful render — the \
rest of your reply text is ignored."
    );
    if let Some(grammar) = grammar {
        prompt.push_str(&format!(
            "\n\nPikchr grammar reference:\n\n{}",
            grammar.trim_end()
        ));
    }
    if let Some(previous) = previous_pikchr {
        prompt.push_str(&format!(
            "\n\nHere is the current diagram to modify:\n```pikchr\n{previous}\n```\n\
Revise it per the request below."
        ));
    }
    prompt.push_str(&format!("\n\nDiagram to produce: {description}"));
    prompt
}

fn accepted_without_render_prompt() -> String {
    format!(
        "Nothing has been rendered successfully yet, so there is no render to accept. Call the \
`render_pikchr` tool with your Pikchr source; once you are satisfied with a successful render, \
end your message with `{ACCEPT_SENTINEL}` as its own line."
    )
}

fn missing_sentinel_prompt() -> String {
    format!(
        "Iterate with the `render_pikchr` tool; when you are satisfied with a successful render, \
accept it by ending your message with `{ACCEPT_SENTINEL}` as its own line — it must be the final \
line of the whole message. Pikchr source sent as reply text is ignored — only rendered output can \
be accepted."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use async_trait::async_trait;

    use crate::store::{Session, Store};

    const CLEAN_SOURCE: &str = r#"box "Clean" fit"#;

    /// One scripted specialist turn: optionally store a render into the slot
    /// (as a real `render_pikchr` call would mid-turn), then reply with `reply`.
    struct FakeTurn {
        store_render: Option<GenOutcome>,
        reply: String,
    }

    fn turn(store_render: Option<GenOutcome>, reply: &str) -> FakeTurn {
        FakeTurn {
            store_render,
            reply: reply.to_string(),
        }
    }

    fn render(source: &str) -> GenOutcome {
        GenOutcome {
            source: source.to_string(),
            png: Some(vec![1, 2, 3]),
        }
    }

    /// Scripted driver that replays canned turns (writing the shared slot the
    /// way the `render_pikchr` tool would) and records the `agent_session_id`
    /// it was handed each turn (to assert resumption).
    struct FakeDriver {
        slot: Arc<LastRenderSlot>,
        turns: Mutex<Vec<FakeTurn>>,
        calls: Mutex<usize>,
        seen_session_ids: Mutex<Vec<Option<String>>>,
        prompts: Mutex<Vec<String>>,
    }

    impl FakeDriver {
        fn new(slot: Arc<LastRenderSlot>, turns: Vec<FakeTurn>) -> Self {
            Self {
                slot,
                turns: Mutex::new(turns),
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
            *self.calls.lock().unwrap() += 1;
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

            let turn = {
                let mut turns = self.turns.lock().unwrap();
                if turns.is_empty() {
                    turn(None, "")
                } else {
                    turns.remove(0)
                }
            };
            if let Some(outcome) = turn.store_render {
                self.slot.store(outcome);
            }
            writer.append_text(&turn.reply).await;
            writer.finalize().await;
            Ok(acp_client::AgentRunOutcome::Completed)
        }
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

    async fn run_generation<D: AgentDriver>(
        driver: &D,
        store: &Arc<Store>,
        session_id: &str,
        slot: &LastRenderSlot,
        cancel: &CancellationToken,
    ) -> Result<GenOutcome, String> {
        run_generation_with_reason(
            driver,
            store,
            session_id,
            slot,
            cancel,
            &CancelReason::new(),
        )
        .await
    }

    async fn run_generation_with_reason<D: AgentDriver>(
        driver: &D,
        store: &Arc<Store>,
        session_id: &str,
        slot: &LastRenderSlot,
        cancel: &CancellationToken,
        cancel_reason: &CancelReason,
    ) -> Result<GenOutcome, String> {
        generate_pikchr_source(
            driver,
            Arc::clone(store),
            session_id,
            Some("test grammar body"),
            "a friendly box",
            None,
            slot,
            cancel,
            cancel_reason,
        )
        .await
    }

    #[tokio::test]
    async fn accepts_last_render_in_one_turn() {
        let slot = Arc::new(LastRenderSlot::new());
        let driver = FakeDriver::new(
            Arc::clone(&slot),
            vec![turn(Some(render(CLEAN_SOURCE)), ACCEPT_SENTINEL)],
        );
        let (store, session_id) = child_session();

        let outcome = run_generation(
            &driver,
            &store,
            &session_id,
            &slot,
            &CancellationToken::new(),
        )
        .await
        .expect("should accept the rendered diagram");

        assert_eq!(outcome.source, CLEAN_SOURCE);
        assert!(outcome.png.is_some());
        assert_eq!(*driver.calls.lock().unwrap(), 1);
        assert!(slot.is_empty(), "acceptance takes the slot");

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
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert!(messages[0].content.contains("render_pikchr"));
        assert!(messages[0]
            .content
            .contains("Diagram to produce: a friendly box"));
        assert_eq!(messages[1].role, MessageRole::Assistant);
        assert_eq!(messages[1].content, ACCEPT_SENTINEL);
    }

    #[test]
    fn sentinel_must_be_the_final_line() {
        for reply in [
            ACCEPT_SENTINEL,
            "acceptlastrender",
            "Looks good.\nAcceptLastRender",
            "Overlap fixed, boxes aligned.\n\n`AcceptLastRender`",
            "Done.\nACCEPTLASTRENDER.",
            "`AcceptLastRender`.",
            "AcceptLastRender\n\n",
        ] {
            assert!(reply_accepts_last_render(reply), "should accept {reply:?}");
        }
        for reply in [
            "",
            "Looks good. AcceptLastRender",
            "I'll end with AcceptLastRender once the overlap is fixed.",
            "AcceptLastRender\nOne more tweak first.",
            "AcceptLastRenders",
        ] {
            assert!(!reply_accepts_last_render(reply), "should reject {reply:?}");
        }
    }

    #[tokio::test]
    async fn accepts_sentinel_as_final_line_after_prose() {
        for reply in [
            "Looks good.\nAcceptLastRender",
            "Done — the boxes no longer overlap.\n\n`acceptlastrender`",
        ] {
            let slot = Arc::new(LastRenderSlot::new());
            let driver = FakeDriver::new(
                Arc::clone(&slot),
                vec![turn(Some(render(CLEAN_SOURCE)), reply)],
            );
            let (store, session_id) = child_session();

            let outcome = run_generation(
                &driver,
                &store,
                &session_id,
                &slot,
                &CancellationToken::new(),
            )
            .await
            .unwrap_or_else(|e| panic!("reply {reply:?} should accept, got {e}"));

            assert_eq!(outcome.source, CLEAN_SOURCE);
            assert_eq!(*driver.calls.lock().unwrap(), 1);
        }
    }

    #[tokio::test]
    async fn reprompts_when_the_sentinel_is_only_echoed_in_prose() {
        // Models echo instructions; a mid-sentence mention is not acceptance.
        // The render stays in the slot for the turn that actually ends with
        // the sentinel line.
        let slot = Arc::new(LastRenderSlot::new());
        let driver = FakeDriver::new(
            Arc::clone(&slot),
            vec![
                turn(
                    Some(render(CLEAN_SOURCE)),
                    "I'll end with AcceptLastRender once the overlap is fixed.",
                ),
                turn(None, ACCEPT_SENTINEL),
            ],
        );
        let (store, session_id) = child_session();

        let outcome = run_generation(
            &driver,
            &store,
            &session_id,
            &slot,
            &CancellationToken::new(),
        )
        .await
        .expect("should accept on the turn that ends with the sentinel");

        assert_eq!(outcome.source, CLEAN_SOURCE);
        assert_eq!(*driver.calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn reprompts_when_accepting_before_any_render() {
        let slot = Arc::new(LastRenderSlot::new());
        let driver = FakeDriver::new(
            Arc::clone(&slot),
            vec![
                turn(None, ACCEPT_SENTINEL),
                turn(Some(render(CLEAN_SOURCE)), ACCEPT_SENTINEL),
            ],
        );
        let (store, session_id) = child_session();

        let outcome = run_generation(
            &driver,
            &store,
            &session_id,
            &slot,
            &CancellationToken::new(),
        )
        .await
        .expect("should succeed on the second turn");

        assert_eq!(outcome.source, CLEAN_SOURCE);
        assert_eq!(*driver.calls.lock().unwrap(), 2);

        let prompts = driver.prompts.lock().unwrap();
        assert!(prompts[1].contains("Nothing has been rendered successfully yet"));
        assert!(prompts[1].contains("render_pikchr"));

        // First turn starts a session; the re-prompt resumes it.
        let seen = driver.seen_session_ids.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], None);
        assert_eq!(seen[1].as_deref(), Some("fake-agent-session"));
    }

    #[tokio::test]
    async fn reprompts_when_reply_lacks_the_sentinel() {
        // A stray fenced reply — the old protocol's habit — is not acceptance,
        // but the render already in the slot survives to the next turn.
        let slot = Arc::new(LastRenderSlot::new());
        let driver = FakeDriver::new(
            Arc::clone(&slot),
            vec![
                turn(
                    Some(render(CLEAN_SOURCE)),
                    "```pikchr\nbox \"Clean\" fit\n```",
                ),
                turn(None, ACCEPT_SENTINEL),
            ],
        );
        let (store, session_id) = child_session();

        let outcome = run_generation(
            &driver,
            &store,
            &session_id,
            &slot,
            &CancellationToken::new(),
        )
        .await
        .expect("should accept the carried-over render on the second turn");

        assert_eq!(outcome.source, CLEAN_SOURCE);
        assert_eq!(*driver.calls.lock().unwrap(), 2);

        let prompts = driver.prompts.lock().unwrap();
        assert!(prompts[1].contains(ACCEPT_SENTINEL));
    }

    #[tokio::test]
    async fn errors_when_the_specialist_never_accepts() {
        let slot = Arc::new(LastRenderSlot::new());
        let turns = (0..MAX_ATTEMPTS + 2)
            .map(|_| turn(None, "Here is some prose without the token."))
            .collect();
        let driver = FakeDriver::new(Arc::clone(&slot), turns);
        let (store, session_id) = child_session();

        let result = run_generation(
            &driver,
            &store,
            &session_id,
            &slot,
            &CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        // The loop is bounded by MAX_ATTEMPTS even when every reply misses.
        assert_eq!(*driver.calls.lock().unwrap(), MAX_ATTEMPTS);

        let session = store
            .get_session(&session_id)
            .expect("load session")
            .expect("session exists");
        assert_eq!(session.status, SessionStatus::Error);
        assert_eq!(
            session.error_message.as_deref(),
            Some("The Pikchr specialist did not accept a rendered diagram.")
        );
    }

    #[tokio::test]
    async fn cancellation_without_a_recorded_reason_reads_as_abandoned() {
        let slot = Arc::new(LastRenderSlot::new());
        let driver = FakeDriver::new(Arc::clone(&slot), vec![]);
        let (store, session_id) = child_session();
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = run_generation(&driver, &store, &session_id, &slot, &cancel).await;

        assert_eq!(result.err().as_deref(), Some(ABANDONED_CANCEL_MESSAGE));
        assert_eq!(*driver.calls.lock().unwrap(), 0);

        let session = store
            .get_session(&session_id)
            .expect("load session")
            .expect("session exists");
        assert_eq!(session.status, SessionStatus::Cancelled);
        assert_eq!(
            session.error_message.as_deref(),
            Some(ABANDONED_CANCEL_MESSAGE),
            "the cancelled child session should explain why it ended"
        );
        assert_eq!(
            session.completion_reason.as_ref(),
            Some(&CompletionReason::Interrupted)
        );
    }

    #[tokio::test]
    async fn recorded_cancel_reason_lands_on_the_session_and_the_error() {
        let slot = Arc::new(LastRenderSlot::new());
        let driver = FakeDriver::new(Arc::clone(&slot), vec![]);
        let (store, session_id) = child_session();
        let cancel = CancellationToken::new();
        let reason = CancelReason::new();
        reason.record("generate_pikchr hit its 10-minute time limit.".to_string());
        // A later initiator must not overwrite the recorded reason.
        reason.record("some competing reason".to_string());
        cancel.cancel();

        let result =
            run_generation_with_reason(&driver, &store, &session_id, &slot, &cancel, &reason).await;

        assert_eq!(
            result.err().as_deref(),
            Some("generate_pikchr hit its 10-minute time limit.")
        );

        let session = store
            .get_session(&session_id)
            .expect("load session")
            .expect("session exists");
        assert_eq!(session.status, SessionStatus::Cancelled);
        assert_eq!(
            session.error_message.as_deref(),
            Some("generate_pikchr hit its 10-minute time limit.")
        );
        assert_eq!(
            session.completion_reason.as_ref(),
            Some(&CompletionReason::Interrupted)
        );
    }

    /// Simulates `cancel_session`'s fallback path: a user cancel that lands
    /// before the worker registers the child session in the SessionRegistry
    /// writes Cancelled straight to the store while the specialist turn is
    /// still running.
    struct CancelRacingDriver {
        inner: FakeDriver,
        store: Arc<Store>,
    }

    #[async_trait(?Send)]
    impl AgentDriver for CancelRacingDriver {
        async fn run(
            &self,
            session_id: &str,
            prompt: &str,
            images: &[(String, String)],
            working_dir: &Path,
            store: &Arc<dyn acp_client::Store>,
            writer: &Arc<dyn acp_client::MessageWriter>,
            cancel_token: &CancellationToken,
            agent_session_id: Option<&str>,
            config_options: &[acp_client::AcpSessionConfigOptionSelection],
        ) -> Result<acp_client::AgentRunOutcome, String> {
            self.store
                .update_session_status(
                    session_id,
                    SessionStatus::Cancelled,
                    None,
                    Some(&CompletionReason::Interrupted),
                )
                .expect("write concurrent cancel");
            self.inner
                .run(
                    session_id,
                    prompt,
                    images,
                    working_dir,
                    store,
                    writer,
                    cancel_token,
                    agent_session_id,
                    config_options,
                )
                .await
        }
    }

    #[tokio::test]
    async fn late_completion_does_not_clobber_concurrent_cancel() {
        let slot = Arc::new(LastRenderSlot::new());
        let (store, session_id) = child_session();
        let driver = CancelRacingDriver {
            inner: FakeDriver::new(
                Arc::clone(&slot),
                vec![turn(Some(render(CLEAN_SOURCE)), ACCEPT_SENTINEL)],
            ),
            store: Arc::clone(&store),
        };

        let outcome = run_generation(
            &driver,
            &store,
            &session_id,
            &slot,
            &CancellationToken::new(),
        )
        .await
        .expect("the generated diagram still goes back to the caller");
        assert_eq!(outcome.source, CLEAN_SOURCE);

        let session = store
            .get_session(&session_id)
            .expect("load session")
            .expect("session exists");
        assert_eq!(session.status, SessionStatus::Cancelled);
        assert!(
            session.error_message.is_none(),
            "a user cancel carries no explanation and must not gain one"
        );
        assert_eq!(
            session.completion_reason.as_ref(),
            Some(&CompletionReason::Interrupted)
        );
    }

    #[test]
    fn slot_overwrites_and_takes() {
        let slot = LastRenderSlot::new();
        assert!(slot.is_empty());
        slot.store(render("box \"first\""));
        slot.store(render("box \"second\""));
        assert!(!slot.is_empty());
        assert_eq!(slot.take().expect("stored render").source, "box \"second\"");
        assert!(slot.take().is_none());
        assert!(slot.is_empty());
    }

    #[test]
    fn initial_prompt_embeds_previous_source_when_revising() {
        let prompt = initial_prompt(Some("GRAMMAR BODY"), "add a box", Some("box \"old\""));
        assert!(prompt.contains("Pikchr grammar reference:\n\nGRAMMAR BODY"));
        assert!(prompt.contains("render_pikchr"));
        assert!(prompt.contains(ACCEPT_SENTINEL));
        assert!(prompt.contains("current diagram to modify"));
        assert!(prompt.contains("box \"old\""));
        assert!(prompt.contains("add a box"));
    }

    #[test]
    fn initial_prompt_omits_revision_block_for_fresh_diagram() {
        let prompt = initial_prompt(Some("GRAMMAR BODY"), "a fresh box", None);
        assert!(!prompt.contains("current diagram to modify"));
        assert!(prompt.contains("render_pikchr"));
        assert!(prompt.contains(ACCEPT_SENTINEL));
        assert!(prompt.contains("a fresh box"));
    }

    #[test]
    fn initial_prompt_falls_back_to_grammar_url_without_bundled_grammar() {
        let prompt = initial_prompt(None, "a fresh box", None);
        assert!(prompt.contains(PIKCHR_GRAMMAR_URL));
        assert!(!prompt.contains("Pikchr grammar reference:\n"));
    }
}
