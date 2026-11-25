//! Protocol-agnostic message writer — streams agent output into the DB.
//!
//! [`MessageWriter`] accumulates streaming text in memory and flushes to
//! the DB at a throttled interval ([`FLUSH_INTERVAL`]). Tool calls and
//! results are written immediately.
//!
//! "Written" means queued for the store's writer thread (see
//! `store::write_queue`): the writer runs on the session's thread, inline
//! with the agent reader, so it never takes the store lock itself. Rows it
//! adds are named by [`RowSlot`]s, so it can queue updates to a row before
//! the row exists. What the writer has to know from the store (whether a
//! session title only echoes a prompt, whether the session delivers a note)
//! is worked out on the writer thread too, after the writes queued before it.
//! [`acp_client::MessageWriter::wait_until_stored`] waits for all of it.
//!
//! This module has **no** protocol-specific types. Any agent driver
//! (ACP, custom HTTP, mock, etc.) calls the same methods with plain
//! `&str` arguments.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::store::{title_echoes_input, AcpMessageMetadata, MessageRole, RowSlot, RowWrite, Store};

use acp_client::{
    autoapprove_permission_decision, strip_code_fences, AcpEventMetadata, AcpInitializeMetadata,
    AcpPermissionDecision, AcpPermissionRequest, AcpToolCallMetadata, PromptImage,
};
use agent_client_protocol::schema::MaybeUndefined;

/// ACP event kind tagging a visible transcript row Staged authored itself
/// rather than received from the agent: the announcement that a pinned config
/// value was unavailable and a substitute was applied. Rows carrying it are
/// excluded from replay boundaries — see
/// [`super::replay_boundaries_from_messages`].
pub(crate) const CONFIG_OPTION_FALLBACK_EVENT: &str = "config_option_fallback";

/// Minimum interval between DB flushes for streaming text. Chunks accumulate
/// in memory and are queued at most this often, reducing mutex contention
/// when many sessions stream concurrently; flushes the writer thread has not
/// reached yet collapse into one. [`MessageWriter::finalize`] always forces
/// an immediate flush regardless of this interval.
const FLUSH_INTERVAL: Duration = Duration::from_millis(150);

/// Streams agent output into the DB, one session at a time.
///
/// All methods are `&self` + async — the struct uses interior mutability
/// via `tokio::sync::Mutex` so it can be shared behind an `Arc`.
pub struct MessageWriter {
    session_id: String,
    store: Arc<Store>,
    /// The row of the current assistant message being streamed into.
    current_assistant_row: Mutex<Option<RowSlot>>,
    /// Accumulated text for the current assistant message (complete, not
    /// a delta). Flushed wholesale on each DB write.
    current_text: Mutex<String>,
    /// A tool call landed after the current message's text. The next append
    /// starts a new message unless the driver resumes this one first — see
    /// [`MessageWriter::resume_interrupted_text`].
    text_interrupted: Mutex<bool>,
    /// When we last wrote to the DB — used to throttle flush frequency.
    last_flush_at: Mutex<Instant>,
    /// Maps external tool-call IDs → (DB row, last-known title).
    tool_call_rows: Mutex<HashMap<String, (RowSlot, String)>>,
    /// Maps external tool-call IDs -> DB row for streaming tool results.
    ///
    /// ACP can send interleaved content updates for multiple tool calls; each
    /// tool call updates its own row instead of sharing one global result row.
    tool_result_rows: Mutex<HashMap<String, RowSlot>>,
    /// The session's `write_note` submission slot, when it is a note session.
    /// A filled slot means the turn's deliverable exists, whatever the
    /// assistant text says (see [`MessageWriter::should_finish_after_response`]).
    note_slot: Option<crate::note_tool::NoteSubmissionSlot>,
    /// Bumped on every event the driver hands this writer, for a watchdog
    /// that wants to know how long the agent has been silent.
    activity: Option<Arc<super::ActivityClock>>,
    /// The text of each prompt this writer's own connection has sent, as
    /// the agent received it (see `on_prompt_sent`): what a session title is
    /// checked against on top of the session's stored messages (see
    /// [`session_title_echoes_input`]).
    ///
    /// It starts empty for every connection and is never filled from an
    /// earlier one. Each run of a session has a writer of its own, so what
    /// an earlier run sent is known here only from its stored user row,
    /// which holds the prompt and not the text the connection added for
    /// attachments. That goes for every later connection: an ordinary
    /// resume of the session as much as an attach catching it up.
    sent_prompts: std::sync::Mutex<Vec<String>>,
}

/// The title of a tool call recorded from an update that names none (see
/// [`MessageWriter::update_tool_call_title`]); a later update's title
/// replaces it.
const UNTITLED_TOOL_CALL: &str = "Tool call";

/// The metadata a tool call's or tool result's row is inserted with: its
/// call's id, written in the insert itself (see [`RowWrite`]) so a read
/// between the writer's writes never finds a tool row it cannot tie to its
/// call. An update leaves the row's metadata alone.
fn tool_call_row_metadata(tool_call_id: &str) -> AcpMessageMetadata {
    AcpMessageMetadata {
        acp_tool_call_id: Some(tool_call_id.to_string()),
        ..Default::default()
    }
}

/// Strip backticks from agent-provided tool-call titles.
fn sanitize_title(title: &str) -> String {
    title.replace('`', "")
}

/// Format a tool call for storage. When `raw_input` is present, produces a JSON
/// object `{"name": title, "input": raw_input}` that the frontend can parse to
/// display structured tool call info. Without raw_input, falls back to the plain
/// title string.
fn format_tool_call_content(title: &str, raw_input: Option<&serde_json::Value>) -> String {
    match raw_input {
        Some(input) => serde_json::json!({ "name": title, "input": input }).to_string(),
        None => title.to_string(),
    }
}

/// The title a stored tool call row was written with: the inverse of
/// [`format_tool_call_content`].
pub(crate) fn stored_tool_call_title(content: &str) -> String {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|value| value.get("name")?.as_str().map(str::to_string))
        .unwrap_or_else(|| content.to_string())
}

fn acp_tool_call_metadata(metadata: AcpToolCallMetadata) -> AcpMessageMetadata {
    AcpMessageMetadata {
        acp_event_kind: metadata.event_kind,
        acp_message_id: metadata.message_id,
        acp_tool_call_id: metadata.tool_call_id,
        acp_tool_kind: metadata.tool_kind,
        acp_tool_status: metadata.tool_status,
        acp_raw_input: metadata.raw_input,
        acp_raw_output: metadata.raw_output,
        acp_content: metadata.content,
        acp_locations: metadata.locations,
        acp_origin: metadata.origin,
        ..Default::default()
    }
}

fn acp_event_metadata(metadata: AcpEventMetadata) -> AcpMessageMetadata {
    AcpMessageMetadata {
        acp_event_kind: metadata.event_kind,
        acp_message_id: metadata.message_id,
        acp_content: metadata.content,
        acp_usage: metadata.usage,
        acp_origin: metadata.origin,
        ..Default::default()
    }
}

fn acp_event_role(metadata: &AcpMessageMetadata) -> MessageRole {
    match metadata.acp_event_kind.as_deref() {
        Some("user_message_chunk") => MessageRole::User,
        _ => MessageRole::Assistant,
    }
}

/// Session-level provider metadata fragment for an ACP event, if the event
/// carries state that recovery logic reads back after the run ends (rate-limit
/// status from usage updates, structured prompt errors, stop reasons).
fn provider_metadata_for_acp_event(metadata: &AcpEventMetadata) -> Option<serde_json::Value> {
    let key = match metadata.event_kind.as_deref() {
        Some("usage_update") => "usageUpdate",
        Some("prompt_response") => "promptResponse",
        Some("prompt_error") => "promptError",
        _ => return None,
    };
    let content = metadata.content.clone()?;
    // Usage updates stream for the whole life of a run, but recovery only ever
    // reads the rate-limit fragment back out of the session row. Skip the
    // per-tick row write unless that fragment is present; prompt errors and
    // responses are once-per-turn and always persisted.
    if key == "usageUpdate"
        && content
            .get("_meta")
            .and_then(|meta| meta.get("_claude/rateLimit"))
            .is_none()
    {
        return None;
    }
    Some(serde_json::json!({ "acp": { key: content } }))
}

/// Whether an agent-provided session title only repeats a message Staged
/// sent in this session, which is how some agents name a session and never
/// a name worth showing (see [`title_echoes_input`] for the comparison,
/// which asks more of a title before calling it an echo of a message that
/// was sent as typed than of one that opens with Staged's instructions).
///
/// Every message of the session counts, not just its first: the stored user
/// rows cover earlier runs, follow-ups and a writer that is only catching
/// up, so an echo of the first prompt is still recognised when it arrives,
/// or is sent again, turns later, and an agent that renames the session
/// after a later message gets the same answer. `sent` is what this
/// connection sent (see [`MessageWriter::sent_prompts`]).
fn session_title_echoes_input(
    store: &Store,
    session_id: &str,
    sent: &[String],
    title: &str,
) -> bool {
    any_input(store, session_id, sent, |input| {
        title_echoes_input(title, input)
    })
}

/// [`session_title_echoes_input`] for the title a session has stored, which
/// went through [`sanitize_title`] on its way in: it is compared with each
/// input as the same sanitising leaves it.
fn stored_title_echoes_input(
    store: &Store,
    session_id: &str,
    sent: &[String],
    stored: &str,
) -> bool {
    any_input(store, session_id, sent, |input| {
        if input.contains('`') {
            title_echoes_input(stored, &sanitize_title(input))
        } else {
            title_echoes_input(stored, input)
        }
    })
}

/// Whether `matches` holds for any message Staged sent in this session: the
/// prompts of this connection as the agent received them, then the
/// session's stored user rows.
fn any_input(
    store: &Store,
    session_id: &str,
    sent: &[String],
    matches: impl Fn(&str) -> bool,
) -> bool {
    if sent.iter().any(|prompt| matches(prompt)) {
        return true;
    }
    match store.any_session_input(session_id, &matches) {
        Ok(found) => found,
        Err(e) => {
            log::warn!(
                "Session {session_id}: couldn't compare the ACP session title with the \
                 session's messages; keeping the title: {e}"
            );
            false
        }
    }
}

/// Clear the session's stored title when it is itself an echo of a message
/// Staged sent: one kept by a build that still stored them. Run when an echo
/// arrives, which is when an agent that names sessions this way sends the
/// same title again (on every reload of the session, for one), so such a
/// session goes back to its fallback name instead of keeping the echo for
/// good. A title the agent wrote is not an echo and stays.
fn clear_stored_echo_title(store: &Store, session_id: &str, sent: &[String]) {
    let stored = match store.get_session(session_id) {
        Ok(session) => session.and_then(|session| session.acp_title),
        Err(e) => {
            log::warn!("Session {session_id}: couldn't read the stored ACP session title: {e}");
            return;
        }
    };
    let Some(stored) = stored else {
        return;
    };
    if !stored_title_echoes_input(store, session_id, sent, &stored) {
        return;
    }
    match store.update_session_acp_title(session_id, None) {
        Ok(()) => log::info!(
            "Session {session_id}: cleared a stored ACP session title that repeats a message \
             Staged sent"
        ),
        Err(e) => log::error!("Failed to clear the stored ACP session title: {e}"),
    }
}

/// Mirror the agent-provided title onto the session row so it can be shown
/// as an interim session name while the session runs.
fn apply_session_title(
    store: &Store,
    session_id: &str,
    sent: &[String],
    title: &MaybeUndefined<String>,
) {
    match title {
        // A title that only repeats what Staged sent names nothing: the title
        // the agent wrote for the session, or what the session falls back to
        // without one, stands. A stored title that is such an echo itself
        // goes.
        MaybeUndefined::Value(title)
            if session_title_echoes_input(store, session_id, sent, title) =>
        {
            log::debug!(
                "Session {session_id}: ignoring an ACP session title that repeats a message \
                 Staged sent"
            );
            clear_stored_echo_title(store, session_id, sent);
        }
        MaybeUndefined::Value(title) => {
            let title = sanitize_title(title);
            let title = title.trim();
            if !title.is_empty() {
                if let Err(e) = store.update_session_acp_title(session_id, Some(title)) {
                    log::error!("Failed to persist ACP session title: {e}");
                }
            }
        }
        // The agent explicitly retracted the title.
        MaybeUndefined::Null => {
            if let Err(e) = store.update_session_acp_title(session_id, None) {
                log::error!("Failed to clear ACP session title: {e}");
            }
        }
        // Update was about `updated_at`/meta only — leave the title alone.
        MaybeUndefined::Undefined => {}
    }
}

impl MessageWriter {
    pub fn new(session_id: String, store: Arc<Store>) -> Self {
        Self {
            session_id,
            store,
            current_assistant_row: Mutex::new(None),
            current_text: Mutex::new(String::new()),
            text_interrupted: Mutex::new(false),
            last_flush_at: Mutex::new(Instant::now()),
            tool_call_rows: Mutex::new(HashMap::new()),
            tool_result_rows: Mutex::new(HashMap::new()),
            note_slot: None,
            activity: None,
            sent_prompts: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// A writer that carries on a transcript another writer started: an
    /// attach catching a session up after its connection was lost. It knows
    /// the session's stored tool call and result rows by tool-call id, so an
    /// update for a call the lost connection recorded (the one running at
    /// the disconnect, which the replay or the live stream then completes)
    /// updates that call's rows, as it would have on the original
    /// connection, instead of adding rows of its own.
    pub fn catching_up(session_id: String, store: Arc<Store>) -> Self {
        let mut tool_call_rows = HashMap::new();
        let mut tool_result_rows = HashMap::new();
        match store.get_session_replay_messages(&session_id) {
            Ok(messages) => {
                for message in messages {
                    let Some(tool_call_id) = message.acp.acp_tool_call_id else {
                        continue;
                    };
                    // Later rows win: the writer's own maps do the same.
                    match message.role {
                        MessageRole::ToolCall => {
                            let title = stored_tool_call_title(&message.content);
                            tool_call_rows
                                .insert(tool_call_id, (RowSlot::known(message.id), title));
                        }
                        MessageRole::ToolResult => {
                            tool_result_rows.insert(tool_call_id, RowSlot::known(message.id));
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => log::warn!(
                "Session {session_id}: couldn't read its stored tool calls; updates to them \
                 will add rows of their own: {e}"
            ),
        }
        let writer = Self::new(session_id, store);
        Self {
            tool_call_rows: Mutex::new(tool_call_rows),
            tool_result_rows: Mutex::new(tool_result_rows),
            ..writer
        }
    }

    /// Attach the session's `write_note` submission slot so a saved note
    /// counts as the finished deliverable.
    pub fn with_note_slot(mut self, slot: crate::note_tool::NoteSubmissionSlot) -> Self {
        self.note_slot = Some(slot);
        self
    }

    /// Have every driver event bump `clock`, so a watchdog on the run can
    /// tell how long the agent has been silent.
    pub fn with_activity_clock(mut self, clock: Arc<super::ActivityClock>) -> Self {
        self.activity = Some(clock);
        self
    }

    /// The agent was heard from: record it for the watchdog, if any.
    fn heard_from_agent(&self) {
        if let Some(clock) = &self.activity {
            clock.touch();
        }
    }

    // =====================================================================
    // Text streaming
    // =====================================================================

    /// Append a text chunk to the current assistant message.
    ///
    /// Flushes to the DB at most every [`FLUSH_INTERVAL`]; intermediate
    /// chunks accumulate in memory.
    pub async fn append_text(&self, text: &str) {
        if std::mem::take(&mut *self.text_interrupted.lock().await) {
            self.finalize().await;
        }
        {
            let mut current = self.current_text.lock().await;
            current.push_str(text);
        }
        self.maybe_flush_text().await;
    }

    /// Flush all buffered text and close the current message block.
    ///
    /// **Must** be called before the session ends (on success, error, and
    /// cancellation) to ensure no text is lost.
    pub async fn finalize(&self) {
        self.flush_text().await;
        self.current_assistant_row.lock().await.take();
        *self.current_text.lock().await = String::new();
        *self.text_interrupted.lock().await = false;
    }

    /// Keep extending the message the latest tool call(s) interrupted instead
    /// of starting a new one on the next append.
    pub async fn resume_interrupted_text(&self) {
        *self.text_interrupted.lock().await = false;
    }

    /// Persist a user message the ACP connection delivered itself — steered
    /// into the turn in flight, or sent as the prompt after an interrupt —
    /// as an ordinary `user` row.
    ///
    /// Finalizes first so the row lands after whatever the assistant had
    /// streamed so far and the next [`append_text`](Self::append_text) opens
    /// a fresh assistant row: the transcript reads assistant → user →
    /// assistant. The connection finalizes before calling too; doing it here
    /// as well costs nothing (finalize is idempotent) and keeps the ordering
    /// a property of this method rather than of its caller.
    ///
    /// The `images` are the blocks the connection sent; `image_ids` are the
    /// stored attachments they were built from, threaded through the
    /// connection's `TurnControlRequest` by the caller that loaded them. They
    /// are persisted on the row as its `image_ids` and the images are
    /// re-associated with this session (`set_images_session_id`), as
    /// `start_session` does for a first prompt. The two lists normally pair
    /// up one to one; a shortfall (an attachment whose file could not be
    /// read still keeps its id) is logged and otherwise harmless.
    pub async fn record_user_message(
        &self,
        text: &str,
        images: &[PromptImage],
        image_ids: &[String],
    ) {
        self.finalize().await;
        if images.len() != image_ids.len() {
            log::debug!(
                "Session {}: user message delivered mid-turn with {} image block(s) and {} \
                 stored attachment id(s)",
                self.session_id,
                images.len(),
                image_ids.len()
            );
        }
        let session_id = self.session_id.clone();
        let text = text.to_string();
        let image_ids = image_ids.to_vec();
        self.store.queue_write(move |store| {
            if let Err(e) = store.add_session_message_with_images(
                &session_id,
                MessageRole::User,
                &text,
                &image_ids,
            ) {
                log::error!("Failed to insert delivered user message: {e}");
                return;
            }
            // Same association `start_session` makes for the prompt's images,
            // so the attachments show under this session in the branch
            // timeline.
            if !image_ids.is_empty() {
                if let Err(e) = store.set_images_session_id(&image_ids, &session_id) {
                    log::warn!(
                        "Failed to associate images {image_ids:?} with session {session_id}: {e}"
                    );
                }
            }
        });
    }

    // =====================================================================
    // Tool calls
    // =====================================================================

    /// Record a tool call. Flushes any in-progress assistant text first so
    /// its row precedes the tool call, and marks it interrupted: the next
    /// append opens a new message unless the driver resumes this one (a
    /// background subagent's tool call can land mid-message).
    pub async fn record_tool_call(
        &self,
        tool_call_id: &str,
        title: &str,
        raw_input: Option<&serde_json::Value>,
    ) {
        self.flush_text().await;
        *self.text_interrupted.lock().await = true;

        let title = sanitize_title(title);
        let content = format_tool_call_content(&title, raw_input);

        // Some providers may resend ToolCall for the same ID while streaming.
        // Treat those as updates to the existing row.
        let mut rows = self.tool_call_rows.lock().await;
        let slot = match rows.get_mut(tool_call_id) {
            Some((slot, stored_title)) => {
                *stored_title = title;
                slot.clone()
            }
            None => {
                let slot = RowSlot::new();
                rows.insert(tool_call_id.to_string(), (slot.clone(), title));
                slot
            }
        };
        self.queue_row(
            slot,
            MessageRole::ToolCall,
            content,
            Some(tool_call_row_metadata(tool_call_id)),
        );
    }

    /// Update a previously recorded tool call's title and/or raw input.
    ///
    /// When `title` is `None`, the last-known title stored at recording time
    /// is reused so that a `raw_input`-only update doesn't blank the name.
    ///
    /// A call with no row yet — its `tool_call` never reached this session,
    /// as when a connection was lost and re-made around it — gets one here,
    /// from what the update carries, so the updates and result after it land
    /// on a titled call rather than on nothing.
    pub async fn update_tool_call_title(
        &self,
        tool_call_id: &str,
        title: Option<&str>,
        raw_input: Option<&serde_json::Value>,
    ) {
        let mut rows = self.tool_call_rows.lock().await;
        let Some((slot, stored_title)) = rows.get_mut(tool_call_id) else {
            drop(rows);
            log::warn!(
                "Tool call {tool_call_id} updated before it was recorded; recording it from the update"
            );
            let title = title.unwrap_or(UNTITLED_TOOL_CALL);
            self.record_tool_call(tool_call_id, title, raw_input).await;
            return;
        };
        let effective_title = match title {
            Some(t) => {
                let sanitized = sanitize_title(t);
                *stored_title = sanitized.clone();
                sanitized
            }
            None => stored_title.clone(),
        };
        let content = format_tool_call_content(&effective_title, raw_input);
        let slot = slot.clone();
        drop(rows);
        self.queue_row(
            slot,
            MessageRole::ToolCall,
            content,
            Some(tool_call_row_metadata(tool_call_id)),
        );
    }

    /// Record the result/output of a tool call.
    pub async fn record_tool_result(&self, tool_call_id: &str, content: &str) {
        let content = strip_code_fences(content);
        let mut result_rows = self.tool_result_rows.lock().await;
        let slot = result_rows
            .entry(tool_call_id.to_string())
            .or_default()
            .clone();
        drop(result_rows);
        self.queue_row(
            slot,
            MessageRole::ToolResult,
            content,
            Some(tool_call_row_metadata(tool_call_id)),
        );
    }

    // =====================================================================
    // Internal
    // =====================================================================

    /// Queue a write of `content` to `slot`'s row: an insert as `role` if
    /// the row does not exist yet, otherwise an update.
    fn queue_row(
        &self,
        slot: RowSlot,
        role: MessageRole,
        content: String,
        on_insert: Option<AcpMessageMetadata>,
    ) {
        self.store.queue_row_write(RowWrite {
            slot,
            session_id: self.session_id.clone(),
            role,
            content,
            on_insert,
        });
    }

    /// Queue a store write for the writer thread, after this writer's
    /// earlier ones.
    fn queue_write(&self, job: impl FnOnce(&Store, &str) + Send + 'static) {
        let session_id = self.session_id.clone();
        self.store.queue_write(move |store| job(store, &session_id));
    }

    /// Flush accumulated text to the DB unconditionally (insert or update).
    ///
    /// Idempotent — calling it twice without new chunks re-writes the same
    /// content. The buffer is never cleared here; only [`finalize`] resets it.
    async fn flush_text(&self) {
        let text = self.current_text.lock().await;
        if text.is_empty() {
            return;
        }
        let slot = self
            .current_assistant_row
            .lock()
            .await
            .get_or_insert_with(RowSlot::new)
            .clone();
        self.queue_row(slot, MessageRole::Assistant, text.clone(), None);
        *self.last_flush_at.lock().await = Instant::now();
    }

    /// Flush only if enough time has passed since the last flush.
    async fn maybe_flush_text(&self) {
        let elapsed = self.last_flush_at.lock().await.elapsed();
        if elapsed >= FLUSH_INTERVAL {
            self.flush_text().await;
        }
    }
}

// Implement the acp_client MessageWriter trait for our MessageWriter
#[async_trait]
impl acp_client::MessageWriter for MessageWriter {
    async fn append_text(&self, text: &str) {
        self.heard_from_agent();
        self.append_text(text).await
    }

    async fn finalize(&self) {
        self.finalize().await
    }

    async fn wait_until_stored(&self) {
        self.store.flush_writes().await
    }

    async fn record_user_message(
        &self,
        text: &str,
        images: &[PromptImage],
        attachment_ids: &[String],
    ) {
        self.heard_from_agent();
        self.record_user_message(text, images, attachment_ids).await;
    }

    /// Keep the prompt's text for [`session_title_echoes_input`]: an
    /// agent that names a session after its input reads every text block of
    /// the request, run together, so the blocks are kept joined (the join
    /// is whitespace, which the comparison collapses), and the prompt block
    /// by itself for an agent that reads only that one. Not the agent
    /// speaking, so the activity clock is left alone.
    ///
    /// This covers the connection that sent the prompt, not the session.
    /// The next connection to the session, whether a later run or an attach
    /// catching it up, has a new writer that was told none of it and reads
    /// the stored user rows only. A title that runs past a stored row into
    /// the attachment text is therefore ignored on the connection that sent
    /// the message and accepted on any later one. An agent that sends its
    /// title again as the session loads does so before that connection has
    /// sent a prompt, so such a title is stored on the session's next run.
    /// It takes a message short enough that the title is not cut before the
    /// attachment text starts.
    async fn on_prompt_sent(&self, text_blocks: &[&str]) {
        let Some(prompt) = text_blocks.first() else {
            return;
        };
        let mut sent = self.sent_prompts.lock().unwrap();
        sent.push(prompt.to_string());
        if text_blocks.len() > 1 {
            sent.push(text_blocks.join(" "));
        }
    }

    async fn should_finish_after_response(&self) -> bool {
        self.heard_from_agent();
        // Inspect only the currently streamed response, never older transcript
        // rows. The ACP caller invokes this only at a completed response boundary.
        self.flush_text().await;
        let has_note = self
            .store
            .queued({
                let session_id = self.session_id.clone();
                move |store| crate::session_runner::session_has_note_artifact(store, &session_id)
            })
            .await
            .unwrap_or_else(|e| {
                log::warn!(
                    "Session {}: couldn't tell whether the session delivers a note: {e}",
                    self.session_id
                );
                false
            });
        if !has_note {
            return false;
        }
        // The tool call is the deliverable; the `---` text is the fallback.
        if self
            .note_slot
            .as_ref()
            .is_some_and(crate::note_tool::has_submission)
        {
            return true;
        }
        crate::session_runner::extract_note_content(self.current_text.lock().await.as_str())
            .is_some()
    }

    async fn record_tool_call(
        &self,
        tool_call_id: &str,
        title: &str,
        raw_input: Option<&serde_json::Value>,
    ) {
        self.heard_from_agent();
        self.record_tool_call(tool_call_id, title, raw_input).await
    }

    async fn resume_interrupted_text(&self) {
        self.heard_from_agent();
        self.resume_interrupted_text().await
    }

    async fn update_tool_call_title(
        &self,
        tool_call_id: &str,
        title: Option<&str>,
        raw_input: Option<&serde_json::Value>,
    ) {
        self.heard_from_agent();
        self.update_tool_call_title(tool_call_id, title, raw_input)
            .await
    }

    async fn record_tool_result(&self, tool_call_id: &str, content: &str) {
        self.heard_from_agent();
        self.record_tool_result(tool_call_id, content).await
    }

    async fn on_session_info_update(&self, info: &acp_client::SessionInfoUpdate) {
        self.heard_from_agent();
        let metadata = AcpMessageMetadata {
            acp_event_kind: Some("session_info_update".to_string()),
            acp_session_info: serde_json::to_value(info).ok(),
            ..Default::default()
        };
        // The row keeps the update as received; the title is then mirrored
        // onto the session row (see `apply_session_title`). Nothing reads a
        // title from the row.
        let sent = self.sent_prompts.lock().unwrap().clone();
        let title = info.title.clone();
        self.queue_write(move |store, session_id| {
            if let Err(e) = store.add_acp_metadata_message(session_id, &metadata) {
                log::error!("Failed to persist ACP session info update: {e}");
            }
            apply_session_title(store, session_id, &sent, &title);
        });
    }

    async fn on_model_state_update(&self, state: &acp_client::SessionModelState) {
        self.heard_from_agent();
        let metadata = AcpMessageMetadata {
            acp_event_kind: Some("session_mode_state".to_string()),
            acp_session_mode_state: serde_json::to_value(state).ok(),
            ..Default::default()
        };
        self.queue_write(move |store, session_id| {
            if let Err(e) = store.add_acp_metadata_message(session_id, &metadata) {
                log::error!("Failed to persist ACP session mode state: {e}");
            }
        });
    }

    async fn on_config_option_update(&self, options: &[acp_client::SessionConfigOption]) {
        self.heard_from_agent();
        if options.is_empty() {
            return;
        }

        let metadata = AcpMessageMetadata {
            acp_event_kind: Some("config_options_update".to_string()),
            acp_config_options: serde_json::to_value(options).ok(),
            ..Default::default()
        };
        self.queue_write(move |store, session_id| {
            if let Err(e) = store.add_acp_metadata_message(session_id, &metadata) {
                log::error!("Failed to persist ACP config options update: {e}");
            }
        });
    }

    /// Substituting a model out from under a pinned selection is not something
    /// to bury in the log, so it lands as an ordinary assistant row: the
    /// transcript already renders those, and the reader sees why the session is
    /// not running what they picked.
    ///
    /// Tagged [`CONFIG_OPTION_FALLBACK_EVENT`] because the agent never said it.
    /// Replay boundaries are matched against what the agent reproduces on
    /// `session/load`, and a boundary it can never produce would stall the
    /// match cursor — so boundary construction drops rows carrying this kind.
    async fn on_config_option_fallback(&self, notice: &str) {
        self.heard_from_agent();
        self.finalize().await;

        let notice = notice.to_string();
        self.queue_write(move |store, session_id| {
            if let Err(e) = store.add_authored_session_message(
                session_id,
                MessageRole::Assistant,
                &notice,
                CONFIG_OPTION_FALLBACK_EVENT,
            ) {
                log::error!("Failed to persist ACP config fallback notice: {e}");
            }
        });
    }

    async fn on_initialize(&self, metadata: &AcpInitializeMetadata) {
        self.heard_from_agent();
        let metadata = AcpMessageMetadata {
            acp_event_kind: Some("initialize".to_string()),
            acp_protocol_version: Some(metadata.protocol_version.clone()),
            acp_agent_capabilities: metadata.agent_capabilities.clone(),
            acp_auth_methods: metadata.auth_methods.clone(),
            acp_agent_info: metadata.agent_info.clone(),
            ..Default::default()
        };
        self.queue_write(move |store, session_id| {
            if let Err(e) = store.add_acp_metadata_message(session_id, &metadata) {
                log::error!("Failed to persist ACP initialization metadata: {e}");
            }
        });
    }

    async fn record_tool_call_metadata(&self, metadata: AcpToolCallMetadata) {
        self.heard_from_agent();
        let tool_call_id = metadata.tool_call_id.clone();
        let metadata = acp_tool_call_metadata(metadata);
        let row = if let Some(tool_call_id) = tool_call_id {
            let rows = self.tool_call_rows.lock().await;
            rows.get(&tool_call_id).map(|(slot, _)| slot.clone())
        } else {
            None
        };

        self.queue_write(move |store, session_id| {
            // The call's row, if the writes queued before this one stored it.
            let result = match row.as_ref().and_then(RowSlot::id) {
                Some(row_id) => store.update_message_acp_metadata(row_id, &metadata),
                None => store
                    .add_acp_metadata_message(session_id, &metadata)
                    .map(|_| ()),
            };
            if let Err(e) = result {
                log::error!("Failed to persist ACP tool-call metadata: {e}");
            }
        });
    }

    async fn record_acp_event_metadata(&self, metadata: AcpEventMetadata) {
        self.heard_from_agent();
        let provider_metadata = provider_metadata_for_acp_event(&metadata);
        let metadata = acp_event_metadata(metadata);
        let role = acp_event_role(&metadata);
        self.queue_write(move |store, session_id| {
            if let Some(provider_metadata) = provider_metadata {
                if let Err(e) =
                    store.merge_session_provider_metadata(session_id, &provider_metadata)
                {
                    log::warn!("Failed to persist provider metadata: {e}");
                }
            }
            if let Err(e) = store.add_acp_metadata_message_with_role(session_id, role, &metadata) {
                log::error!("Failed to persist ACP event metadata: {e}");
            }
        });
    }

    async fn request_permission(
        &self,
        request: AcpPermissionRequest,
        cancel_token: CancellationToken,
    ) -> AcpPermissionDecision {
        self.heard_from_agent();
        if cancel_token.is_cancelled() {
            AcpPermissionDecision::Cancelled
        } else {
            autoapprove_permission_decision(&request)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use super::{format_tool_call_content, stored_tool_call_title, MessageWriter};
    use crate::store::{MessageRole, Session, Store};
    use acp_client::{
        AcpPermissionDecision, AcpPermissionOption, AcpPermissionOptionKind, AcpPermissionRequest,
    };
    use agent_client_protocol::schema::MaybeUndefined;
    use tokio_util::sync::CancellationToken;

    fn setup_writer() -> (Arc<Store>, String, MessageWriter) {
        let store = Arc::new(Store::in_memory().expect("in-memory store"));
        let session = Session::new_running("test prompt", Path::new("."));
        store.create_session(&session).expect("create session");
        let writer = MessageWriter::new(session.id.clone(), Arc::clone(&store));
        (store, session.id, writer)
    }

    #[tokio::test]
    async fn completed_note_detection_uses_only_current_response_for_both_note_kinds() {
        use crate::store::{Branch, Note, Project, ProjectNote};
        use acp_client::MessageWriter as _;

        for project_note in [false, true] {
            let (store, session_id, writer) = setup_writer();
            let project = Project::new("test/notes");
            store.create_project(&project).unwrap();
            if project_note {
                let note = ProjectNote::new(&project.id, "", "").with_session(&session_id);
                store.create_project_note(&note).unwrap();
            } else {
                let branch = Branch::new(&project.id, "notes", "main");
                store.create_branch(&branch).unwrap();
                let note = Note::new(&branch.id, "", "").with_session(&session_id);
                store.create_note(&note).unwrap();
            }
            store
                .add_session_message(
                    &session_id,
                    MessageRole::Assistant,
                    "---\n# Historical note\nOld body",
                )
                .unwrap();
            assert!(!writer.should_finish_after_response().await);
            writer
                .append_text("Example:\n```\n---\n# Title\nBody\n```")
                .await;
            assert!(!writer.should_finish_after_response().await);
            writer.finalize().await;
            writer.append_text("Preamble\n---\n").await;
            assert!(!writer.should_finish_after_response().await);
            writer.append_text("# Current note\nFull body.\n").await;
            writer.append_text("```suggested-next-steps\n{\"suggestedNextCommitStep\":\"Implement\",\"suggestedNextNoteStep\":null}\n```").await;
            assert!(writer.should_finish_after_response().await);
            store.flush_writes().await;
            let messages = store.get_session_messages(&session_id).unwrap();
            let saved = &messages.last().unwrap().content;
            assert!(saved.contains("Full body."));
            assert!(saved.contains("suggestedNextCommitStep"));
            writer.finalize().await;
            // Closing a response prevents it from satisfying a later boundary.
            assert!(!writer.should_finish_after_response().await);
        }
    }

    /// A message the connection delivers mid-session (steer or interrupt) is
    /// persisted between the assistant text before it and the reply after it,
    /// as three rows in transcript order, and the reply does not extend the
    /// row that was open when the message arrived.
    #[tokio::test]
    async fn record_user_message_lands_between_assistant_rows() {
        let (store, session_id, writer) = setup_writer();

        writer.append_text("Working on the first ").await;
        writer.append_text("instruction.").await;
        writer.finalize().await;
        writer
            .record_user_message("Actually, do the second thing instead.", &[], &[])
            .await;
        writer.append_text("Switching to the second thing.").await;
        writer.finalize().await;
        store.flush_writes().await;

        store.flush_writes().await;
        let messages = store.get_session_messages(&session_id).unwrap();
        let rows: Vec<(MessageRole, &str)> = messages
            .iter()
            .map(|m| (m.role.clone(), m.content.as_str()))
            .collect();
        assert_eq!(
            rows,
            vec![
                (MessageRole::Assistant, "Working on the first instruction."),
                (MessageRole::User, "Actually, do the second thing instead."),
                (MessageRole::Assistant, "Switching to the second thing."),
            ]
        );
        assert!(messages[1].image_ids.is_empty());
        assert!(messages[1].acp.acp_event_kind.is_none());
    }

    /// The trait entry point (what the connection task calls) closes any
    /// still-open assistant text itself, so a caller that skipped the
    /// finalize still gets the row order right; image blocks are accepted
    /// and the text is what gets persisted.
    #[tokio::test]
    async fn record_user_message_closes_open_assistant_text_first() {
        let (store, session_id, writer) = setup_writer();

        writer.append_text("Unflushed partial text").await;
        let image = acp_client::PromptImage::new("AAAA", "image/png");
        acp_client::MessageWriter::record_user_message(&writer, "Look at this.", &[image], &[])
            .await;
        writer.append_text("Reply.").await;
        writer.finalize().await;

        store.flush_writes().await;
        let messages = store.get_session_messages(&session_id).unwrap();
        let rows: Vec<(MessageRole, &str)> = messages
            .iter()
            .map(|m| (m.role.clone(), m.content.as_str()))
            .collect();
        assert_eq!(
            rows,
            vec![
                (MessageRole::Assistant, "Unflushed partial text"),
                (MessageRole::User, "Look at this."),
                (MessageRole::Assistant, "Reply."),
            ]
        );
    }

    /// The attachment ids the connection hands back land on the user row as
    /// its `image_ids`, and the stored images are associated with the session
    /// — what `start_session` does for a prompt's images, so a message with
    /// attachments reads the same whether it was delivered live or on a
    /// resume.
    #[tokio::test]
    async fn record_user_message_persists_attachment_ids_and_associates_the_images() {
        use crate::store::{Image, Project};
        let (store, session_id, writer) = setup_writer();
        let project = Project::new("owner/repo");
        store.create_project(&project).unwrap();
        let image = Image::new(None, &project.id, "shot.png", "image/png", 3, true);
        store.create_image(&image).unwrap();
        assert_ne!(
            store.get_image(&image.id).unwrap().unwrap().session_id,
            Some(session_id.clone())
        );

        writer
            .record_user_message(
                "See attached.",
                &[acp_client::PromptImage::new("AAAA", "image/png")],
                std::slice::from_ref(&image.id),
            )
            .await;
        store.flush_writes().await;

        store.flush_writes().await;
        let messages = store.get_session_messages(&session_id).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[0].image_ids, vec![image.id.clone()]);
        assert_eq!(
            store.get_image(&image.id).unwrap().unwrap().session_id,
            Some(session_id)
        );
    }

    #[tokio::test]
    async fn ordinary_session_does_not_finish_for_note_shaped_output() {
        use acp_client::MessageWriter as _;
        let (_, _, writer) = setup_writer();
        writer.append_text("---\n# A heading\nBody").await;
        assert!(!writer.should_finish_after_response().await);
    }

    fn permission_request(session_id: &str, request_id: &str) -> AcpPermissionRequest {
        AcpPermissionRequest {
            request_id: request_id.to_string(),
            session_id: session_id.to_string(),
            tool_call_id: "tc-permission".to_string(),
            tool_title: Some("Edit src/lib.rs".to_string()),
            tool_kind: Some("edit".to_string()),
            tool_status: Some("pending".to_string()),
            raw_input: Some(serde_json::json!({"path": "src/lib.rs"})),
            raw_output: None,
            content: None,
            locations: None,
            options: vec![
                AcpPermissionOption {
                    option_id: "allow-once".to_string(),
                    name: "Allow once".to_string(),
                    kind: AcpPermissionOptionKind::AllowOnce,
                },
                AcpPermissionOption {
                    option_id: "reject-once".to_string(),
                    name: "Deny".to_string(),
                    kind: AcpPermissionOptionKind::RejectOnce,
                },
            ],
            raw_request: None,
            origin: None,
        }
    }

    #[tokio::test]
    async fn permission_request_autoapproves_without_ui_metadata() {
        let (store, session_id, writer) = setup_writer();
        let request = permission_request(&session_id, "autoapprove-request");
        let cancel_token = CancellationToken::new();

        let decision = <MessageWriter as acp_client::MessageWriter>::request_permission(
            &writer,
            request,
            cancel_token,
        )
        .await;

        assert_eq!(
            decision,
            AcpPermissionDecision::Selected {
                option_id: "allow-once".to_string()
            }
        );
        store.flush_writes().await;
        let metadata = store
            .get_session_acp_metadata_archive(&session_id)
            .expect("metadata");
        assert!(metadata.is_empty());
    }

    #[tokio::test]
    async fn permission_request_prefers_allow_option_over_first_option() {
        let (_store, session_id, writer) = setup_writer();
        let mut request = permission_request(&session_id, "allow-request");
        request.options.reverse();
        let cancel_token = CancellationToken::new();

        let decision = <MessageWriter as acp_client::MessageWriter>::request_permission(
            &writer,
            request,
            cancel_token,
        )
        .await;

        assert_eq!(
            decision,
            AcpPermissionDecision::Selected {
                option_id: "allow-once".to_string()
            }
        );
    }

    #[tokio::test]
    async fn permission_request_returns_cancelled_when_already_cancelled() {
        let (_store, session_id, writer) = setup_writer();
        let request = permission_request(&session_id, "cancel-request");
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();

        let decision = <MessageWriter as acp_client::MessageWriter>::request_permission(
            &writer,
            request,
            cancel_token,
        )
        .await;

        assert_eq!(decision, AcpPermissionDecision::Cancelled);
    }

    /// The writer runs on the session's thread, inline with the agent
    /// reader, so recording a turn must not wait for the store lock: with
    /// another thread holding it, a whole turn (timed text flushes, a tool
    /// call and its streamed result, more text) is recorded at once, and
    /// only waiting for it to be stored waits for the lock. Once it is
    /// free, the rows land in the order the turn made them.
    #[test]
    fn a_turn_is_recorded_while_another_thread_holds_the_store_lock() {
        let (store, session_id, writer) = setup_writer();
        let held = store.hold_connection();
        let (recorded_tx, recorded_rx) = std::sync::mpsc::channel();

        let session = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap();
            runtime.block_on(async {
                writer.append_text("Looking").await;
                tokio::time::sleep(super::FLUSH_INTERVAL).await;
                writer.append_text(" at it.").await;
                tokio::time::sleep(super::FLUSH_INTERVAL).await;
                writer.append_text("..").await;
                writer.record_tool_call("tc-1", "Read", None).await;
                writer.record_tool_result("tc-1", "part").await;
                writer.record_tool_result("tc-1", "all of it").await;
                writer.append_text("Done.").await;
                writer.finalize().await;
                recorded_tx.send(()).unwrap();
                acp_client::MessageWriter::wait_until_stored(&writer).await;
            });
        });

        recorded_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the turn is recorded without waiting for the store lock");
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            !session.is_finished(),
            "waiting for the turn to be stored waits for the lock"
        );
        drop(held);
        session.join().unwrap();

        let rows: Vec<(MessageRole, String)> = store
            .get_session_messages(&session_id)
            .unwrap()
            .into_iter()
            .map(|m| (m.role, m.content))
            .collect();
        assert_eq!(
            rows,
            vec![
                (MessageRole::Assistant, "Looking at it...".to_string()),
                (MessageRole::ToolCall, "Read".to_string()),
                (MessageRole::ToolResult, "all of it".to_string()),
                (MessageRole::Assistant, "Done.".to_string()),
            ]
        );
        let result = &store.get_session_replay_messages(&session_id).unwrap()[2];
        assert_eq!(result.acp.acp_tool_call_id.as_deref(), Some("tc-1"));
    }

    /// Two writers for one session (one connection's, then the next's)
    /// queue onto the same store, so the second's rows land after the
    /// first's even when neither has been stored when the second starts.
    #[tokio::test]
    async fn a_second_writer_queues_behind_the_first() {
        let (store, session_id, first) = setup_writer();
        let held = store.hold_connection();
        first.append_text("First run.").await;
        first.record_tool_call("tc-1", "Bash", None).await;
        first.record_tool_result("tc-1", "ok").await;
        first.finalize().await;

        let second = MessageWriter::new(session_id.clone(), Arc::clone(&store));
        second.append_text("Second run.").await;
        second.finalize().await;
        drop(held);
        acp_client::MessageWriter::wait_until_stored(&second).await;

        let rows: Vec<(MessageRole, String)> = store
            .get_session_messages(&session_id)
            .unwrap()
            .into_iter()
            .map(|m| (m.role, m.content))
            .collect();
        assert_eq!(
            rows,
            vec![
                (MessageRole::Assistant, "First run.".to_string()),
                (MessageRole::ToolCall, "Bash".to_string()),
                (MessageRole::ToolResult, "ok".to_string()),
                (MessageRole::Assistant, "Second run.".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn record_tool_result_updates_existing_row_for_streaming_updates() {
        let (store, session_id, writer) = setup_writer();

        writer
            .record_tool_call("tc-1", "Run echo hello", None)
            .await;
        writer.record_tool_result("tc-1", "first chunk").await;
        writer.record_tool_result("tc-1", "second chunk").await;

        store.flush_writes().await;
        let messages = store
            .get_session_messages(&session_id)
            .expect("query messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::ToolCall);
        assert_eq!(messages[1].role, MessageRole::ToolResult);
        assert_eq!(messages[1].content, "second chunk");
        assert_eq!(messages[1].acp.acp_tool_call_id.as_deref(), Some("tc-1"));
    }

    #[tokio::test]
    async fn record_tool_result_keeps_interleaved_tool_updates_separate() {
        let (store, session_id, writer) = setup_writer();

        writer
            .record_tool_call("tc-1", "Run first command", None)
            .await;
        writer.record_tool_result("tc-1", "first initial").await;
        writer
            .record_tool_call("tc-2", "Run second command", None)
            .await;
        writer.record_tool_result("tc-2", "second initial").await;
        writer.record_tool_result("tc-1", "first final").await;
        writer.record_tool_result("tc-2", "second final").await;

        store.flush_writes().await;
        let messages = store
            .get_session_messages(&session_id)
            .expect("query messages");
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, MessageRole::ToolCall);
        assert_eq!(messages[0].content, "Run first command");
        assert_eq!(messages[1].role, MessageRole::ToolResult);
        assert_eq!(messages[1].content, "first final");
        assert_eq!(messages[1].acp.acp_tool_call_id.as_deref(), Some("tc-1"));
        assert_eq!(messages[2].role, MessageRole::ToolCall);
        assert_eq!(messages[2].content, "Run second command");
        assert_eq!(messages[3].role, MessageRole::ToolResult);
        assert_eq!(messages[3].content, "second final");
        assert_eq!(messages[3].acp.acp_tool_call_id.as_deref(), Some("tc-2"));
    }

    #[tokio::test]
    async fn resumed_text_extends_the_row_a_tool_call_interrupted() {
        let (store, session_id, writer) = setup_writer();

        writer.append_text("The").await;
        writer.record_tool_call("tc-sub", "Run grep", None).await;
        writer.resume_interrupted_text().await;
        writer.append_text(" search is back").await;
        writer.finalize().await;

        store.flush_writes().await;
        let messages = store.get_session_messages(&session_id).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::Assistant);
        assert_eq!(messages[0].content, "The search is back");
        assert_eq!(messages[1].role, MessageRole::ToolCall);
    }

    #[tokio::test]
    async fn text_after_a_tool_call_starts_a_new_row_unless_resumed() {
        let (store, session_id, writer) = setup_writer();

        writer.append_text("Running tests").await;
        writer.record_tool_call("tc-1", "Run tests", None).await;
        writer.append_text("They passed").await;
        writer.finalize().await;

        store.flush_writes().await;
        let messages = store.get_session_messages(&session_id).unwrap();
        let rows: Vec<_> = messages
            .iter()
            .map(|m| (m.role.clone(), m.content.as_str()))
            .collect();
        assert_eq!(
            rows,
            vec![
                (MessageRole::Assistant, "Running tests"),
                (MessageRole::ToolCall, "Run tests"),
                (MessageRole::Assistant, "They passed"),
            ]
        );
    }

    #[tokio::test]
    async fn record_tool_call_same_id_updates_instead_of_inserting() {
        let (store, session_id, writer) = setup_writer();

        writer
            .record_tool_call("tc-dup", "Run first title", None)
            .await;
        writer
            .record_tool_call("tc-dup", "Run updated title", None)
            .await;

        store.flush_writes().await;
        let messages = store
            .get_session_messages(&session_id)
            .expect("query messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, MessageRole::ToolCall);
        assert_eq!(messages[0].content, "Run updated title");
    }

    #[tokio::test]
    async fn record_tool_call_with_raw_input_stores_json() {
        let (store, session_id, writer) = setup_writer();

        let raw_input = serde_json::json!({"path": "foo.rs"});
        writer
            .record_tool_call("tc-json", "Read file", Some(&raw_input))
            .await;

        store.flush_writes().await;
        let messages = store
            .get_session_messages(&session_id)
            .expect("query messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, MessageRole::ToolCall);

        let parsed: serde_json::Value =
            serde_json::from_str(&messages[0].content).expect("content should be valid JSON");
        assert_eq!(parsed["name"], "Read file");
        assert_eq!(parsed["input"]["path"], "foo.rs");
    }

    #[tokio::test]
    async fn update_tool_call_raw_input_without_title_preserves_title() {
        let (store, session_id, writer) = setup_writer();

        writer.record_tool_call("tc-ri", "Read file", None).await;

        // Update with raw_input only (no title).
        let raw_input = serde_json::json!({"path": "bar.rs"});
        writer
            .update_tool_call_title("tc-ri", None, Some(&raw_input))
            .await;

        store.flush_writes().await;
        let messages = store
            .get_session_messages(&session_id)
            .expect("query messages");
        assert_eq!(messages.len(), 1);

        let parsed: serde_json::Value =
            serde_json::from_str(&messages[0].content).expect("content should be valid JSON");
        assert_eq!(parsed["name"], "Read file");
        assert_eq!(parsed["input"]["path"], "bar.rs");
    }

    /// The call's id is written with the row, and the result's with its
    /// row: a read between the writer's writes (the session stream makes
    /// them) never finds a tool row it cannot tie to its call.
    #[tokio::test]
    async fn tool_rows_carry_their_call_id_from_the_insert() {
        let (store, session_id, writer) = setup_writer();

        writer.record_tool_call("tc-1", "Read file", None).await;
        let messages = store.get_session_messages(&session_id).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, MessageRole::ToolCall);
        assert_eq!(messages[0].acp.acp_tool_call_id.as_deref(), Some("tc-1"));
        assert_eq!(messages[0].acp.acp_event_kind, None);

        writer.record_tool_result("tc-1", "contents").await;
        let messages = store.get_session_messages(&session_id).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].role, MessageRole::ToolResult);
        assert_eq!(messages[1].content, "contents");
        assert_eq!(messages[1].acp.acp_tool_call_id.as_deref(), Some("tc-1"));
    }

    #[tokio::test]
    async fn record_tool_call_metadata_updates_existing_tool_call_row() {
        let (store, session_id, writer) = setup_writer();

        writer.record_tool_call("tc-meta", "Read file", None).await;
        <MessageWriter as acp_client::MessageWriter>::record_tool_call_metadata(
            &writer,
            acp_client::AcpToolCallMetadata {
                event_kind: Some("tool_call".to_string()),
                tool_call_id: Some("tc-meta".to_string()),
                tool_kind: Some("read".to_string()),
                tool_status: Some("completed".to_string()),
                raw_input: Some(serde_json::json!({"path": "src/lib.rs"})),
                ..Default::default()
            },
        )
        .await;

        store.flush_writes().await;
        let messages = store
            .get_session_messages(&session_id)
            .expect("query messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, MessageRole::ToolCall);
        assert_eq!(messages[0].acp.acp_event_kind.as_deref(), Some("tool_call"));
        assert_eq!(messages[0].acp.acp_tool_call_id.as_deref(), Some("tc-meta"));
        assert_eq!(messages[0].acp.acp_tool_kind.as_deref(), Some("read"));
        assert_eq!(
            messages[0].acp.acp_raw_input.as_ref().unwrap()["path"],
            "src/lib.rs"
        );
    }

    /// A `tool_call_update` for a call whose `tool_call` this session never
    /// received, applied as the driver applies one (title, then metadata,
    /// then result): the call gets a row from the update, titled, which its
    /// metadata and later updates land on, rather than an untitled card of
    /// bare metadata rows.
    #[tokio::test]
    async fn a_tool_call_update_with_no_call_row_records_the_call() {
        let (store, session_id, writer) = setup_writer();
        let raw_input = serde_json::json!({"command": "cargo test"});
        let update = |status: &str| acp_client::AcpToolCallMetadata {
            event_kind: Some("tool_call_update".to_string()),
            tool_call_id: Some("tc-orphan".to_string()),
            tool_status: Some(status.to_string()),
            raw_input: Some(raw_input.clone()),
            ..Default::default()
        };

        writer
            .update_tool_call_title("tc-orphan", None, Some(&raw_input))
            .await;
        <MessageWriter as acp_client::MessageWriter>::record_tool_call_metadata(
            &writer,
            update("in_progress"),
        )
        .await;
        writer
            .update_tool_call_title("tc-orphan", Some("`cargo test`"), Some(&raw_input))
            .await;
        <MessageWriter as acp_client::MessageWriter>::record_tool_call_metadata(
            &writer,
            update("completed"),
        )
        .await;
        writer.record_tool_result("tc-orphan", "3 passed").await;

        store.flush_writes().await;
        let messages = store
            .get_session_messages(&session_id)
            .expect("query messages");
        let roles: Vec<_> = messages
            .iter()
            .map(|message| message.role.clone())
            .collect();
        assert_eq!(roles, [MessageRole::ToolCall, MessageRole::ToolResult]);
        let call: serde_json::Value =
            serde_json::from_str(&messages[0].content).expect("content should be valid JSON");
        assert_eq!(
            call["name"], "cargo test",
            "a later title replaces the fallback"
        );
        assert_eq!(call["input"]["command"], "cargo test");
        assert_eq!(
            messages[0].acp.acp_tool_call_id.as_deref(),
            Some("tc-orphan")
        );
        assert_eq!(
            messages[0].acp.acp_tool_status.as_deref(),
            Some("completed")
        );
        assert_eq!(messages[1].content, "3 passed");

        // An update with raw input but no title records the call under a
        // stand-in title, between the text around it.
        writer.append_text("Done.").await;
        writer
            .update_tool_call_title("tc-input-only", None, Some(&raw_input))
            .await;
        writer.append_text(" Next.").await;
        writer.finalize().await;
        store.flush_writes().await;
        let messages = store
            .get_session_messages(&session_id)
            .expect("query messages");
        let tail: Vec<_> = messages[2..]
            .iter()
            .map(|message| (message.role.clone(), message.content.as_str()))
            .collect();
        assert_eq!(tail.len(), 3, "{tail:?}");
        assert_eq!(tail[0], (MessageRole::Assistant, "Done."));
        assert_eq!(tail[1].0, MessageRole::ToolCall);
        let call: serde_json::Value =
            serde_json::from_str(tail[1].1).expect("content should be valid JSON");
        assert_eq!(
            call,
            serde_json::json!({"name": "Tool call", "input": {"command": "cargo test"}})
        );
        assert_eq!(tail[2], (MessageRole::Assistant, " Next."));
    }

    /// An attach's writer completes the call the lost connection's writer
    /// left running: its call and result rows are updated in place, with the
    /// real output and the final status, and nothing is added.
    #[tokio::test]
    async fn a_catching_up_writer_updates_the_rows_another_writer_recorded() {
        let (store, session_id, writer) = setup_writer();
        let raw_input = serde_json::json!({"command": "./step1.sh"});
        writer
            .record_tool_call("tc-1", "./step1.sh", Some(&raw_input))
            .await;
        <MessageWriter as acp_client::MessageWriter>::record_tool_call_metadata(
            &writer,
            acp_client::AcpToolCallMetadata {
                event_kind: Some("tool_call".to_string()),
                tool_call_id: Some("tc-1".to_string()),
                tool_status: Some("pending".to_string()),
                ..Default::default()
            },
        )
        .await;
        // The Claude bridge's refinement: the description is the content.
        writer.record_tool_result("tc-1", "Run step 1").await;
        writer.finalize().await;
        // As the lost connection does as it ends: what it recorded is stored
        // before anything reads the transcript back.
        acp_client::MessageWriter::wait_until_stored(&writer).await;
        drop(writer);

        let writer = MessageWriter::catching_up(session_id.clone(), Arc::clone(&store));
        writer
            .update_tool_call_title("tc-1", None, Some(&raw_input))
            .await;
        <MessageWriter as acp_client::MessageWriter>::record_tool_call_metadata(
            &writer,
            acp_client::AcpToolCallMetadata {
                event_kind: Some("tool_call_update".to_string()),
                tool_call_id: Some("tc-1".to_string()),
                tool_status: Some("completed".to_string()),
                ..Default::default()
            },
        )
        .await;
        writer.record_tool_result("tc-1", "step 1 done").await;

        store.flush_writes().await;
        let messages = store.get_session_replay_messages(&session_id).unwrap();
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert_eq!(messages[0].role, MessageRole::ToolCall);
        assert_eq!(
            messages[0].acp.acp_tool_status.as_deref(),
            Some("completed")
        );
        let parsed: serde_json::Value = serde_json::from_str(&messages[0].content).unwrap();
        assert_eq!(parsed["name"], "./step1.sh", "the stored title is kept");
        assert_eq!(messages[1].role, MessageRole::ToolResult);
        assert_eq!(messages[1].content, "step 1 done");

        // A call it has never seen is recorded as usual.
        writer.record_tool_call("tc-2", "ls", None).await;
        writer.record_tool_result("tc-2", "a b").await;
        store.flush_writes().await;
        let messages = store.get_session_messages(&session_id).unwrap();
        assert_eq!(messages.len(), 4);
    }

    #[test]
    fn stored_tool_call_titles_round_trip() {
        let input = serde_json::json!({"path": "a.rs"});
        assert_eq!(
            stored_tool_call_title(&format_tool_call_content("Read a.rs", Some(&input))),
            "Read a.rs"
        );
        assert_eq!(
            stored_tool_call_title(&format_tool_call_content("Run ls", None)),
            "Run ls"
        );
    }

    #[tokio::test]
    async fn initialize_metadata_is_persisted_as_hidden_row() {
        let (store, session_id, writer) = setup_writer();

        <MessageWriter as acp_client::MessageWriter>::on_initialize(
            &writer,
            &acp_client::AcpInitializeMetadata {
                protocol_version: "1".to_string(),
                agent_capabilities: Some(serde_json::json!({
                    "promptCapabilities": {"image": false}
                })),
                auth_methods: Some(serde_json::json!([
                    {"id": "default", "name": "Default"}
                ])),
                agent_info: Some(serde_json::json!({
                    "name": "codex",
                    "version": "0.1.0"
                })),
            },
        )
        .await;

        store.flush_writes().await;
        assert!(
            store.get_session_messages(&session_id).unwrap().is_empty(),
            "initialization metadata should not appear in transcript rows"
        );

        store.flush_writes().await;
        let metadata = store
            .get_session_acp_initialization(&session_id)
            .unwrap()
            .expect("initialization metadata");
        assert_eq!(metadata.acp_protocol_version.as_deref(), Some("1"));
        assert_eq!(
            metadata.acp_agent_capabilities.unwrap()["promptCapabilities"]["image"],
            false
        );
        assert_eq!(metadata.acp_agent_info.unwrap()["name"], "codex");
    }

    #[tokio::test]
    async fn acp_event_metadata_is_persisted_as_hidden_row() {
        let (store, session_id, writer) = setup_writer();

        <MessageWriter as acp_client::MessageWriter>::record_acp_event_metadata(
            &writer,
            acp_client::AcpEventMetadata {
                event_kind: Some("agent_message_chunk".to_string()),
                message_id: Some("assistant-msg-1".to_string()),
                content: Some(serde_json::json!({
                    "content": {"type": "text", "text": "hello"}
                })),
                usage: Some(serde_json::json!({
                    "totalTokens": 12,
                    "inputTokens": 5,
                    "outputTokens": 7
                })),
                origin: None,
            },
        )
        .await;

        store.flush_writes().await;
        assert!(
            store.get_session_messages(&session_id).unwrap().is_empty(),
            "ACP event metadata should not appear in transcript rows"
        );

        store.flush_writes().await;
        let metadata_rows = store.get_session_acp_metadata_archive(&session_id).unwrap();
        assert_eq!(metadata_rows.len(), 1);
        assert_eq!(
            metadata_rows[0].acp.acp_event_kind.as_deref(),
            Some("agent_message_chunk")
        );
        assert_eq!(
            metadata_rows[0].acp.acp_message_id.as_deref(),
            Some("assistant-msg-1")
        );
        assert_eq!(
            metadata_rows[0].acp.acp_usage.as_ref().unwrap()["outputTokens"],
            7
        );
        assert_eq!(
            metadata_rows[0].acp.acp_origin, None,
            "a live turn's chunk carries no background attribution"
        );
    }

    #[tokio::test]
    async fn background_continuation_attribution_is_persisted_with_the_chunk() {
        let (store, session_id, writer) = setup_writer();
        let origin = acp_client::background_continuation_origin(Some("task-notification"));

        <MessageWriter as acp_client::MessageWriter>::record_acp_event_metadata(
            &writer,
            acp_client::AcpEventMetadata {
                event_kind: Some("agent_message_chunk".to_string()),
                message_id: Some("background-continuation-1".to_string()),
                content: Some(serde_json::json!({
                    "content": {"type": "text", "text": "the build passed"}
                })),
                usage: None,
                origin: Some(origin.clone()),
            },
        )
        .await;

        store.flush_writes().await;
        let metadata_rows = store.get_session_acp_metadata_archive(&session_id).unwrap();
        assert_eq!(metadata_rows.len(), 1);
        assert_eq!(metadata_rows[0].acp.acp_origin.as_deref(), Some(&*origin));
        assert_eq!(
            metadata_rows[0].acp.acp_message_id.as_deref(),
            Some("background-continuation-1")
        );
    }

    #[tokio::test]
    async fn session_info_update_title_value_sets_session_acp_title() {
        let (store, session_id, writer) = setup_writer();

        let info = acp_client::SessionInfoUpdate::new().title("  `Fix the login flow`  ");
        <MessageWriter as acp_client::MessageWriter>::on_session_info_update(&writer, &info).await;

        store.flush_writes().await;
        let session = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.acp_title.as_deref(), Some("Fix the login flow"));

        // The metadata-row archive of the raw update is unchanged behavior.
        store.flush_writes().await;
        let metadata_rows = store.get_session_acp_metadata_archive(&session_id).unwrap();
        assert_eq!(metadata_rows.len(), 1);
        assert_eq!(
            metadata_rows[0].acp.acp_event_kind.as_deref(),
            Some("session_info_update")
        );
    }

    #[tokio::test]
    async fn session_info_update_null_title_clears_session_acp_title() {
        let (store, session_id, writer) = setup_writer();
        store
            .update_session_acp_title(&session_id, Some("Earlier title"))
            .unwrap();

        let info = acp_client::SessionInfoUpdate::new().title(MaybeUndefined::Null);
        <MessageWriter as acp_client::MessageWriter>::on_session_info_update(&writer, &info).await;

        store.flush_writes().await;
        let session = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.acp_title, None);
    }

    #[tokio::test]
    async fn session_info_update_undefined_title_preserves_session_acp_title() {
        let (store, session_id, writer) = setup_writer();
        store
            .update_session_acp_title(&session_id, Some("Earlier title"))
            .unwrap();

        let info = acp_client::SessionInfoUpdate::new().updated_at("2026-08-04T00:00:00Z");
        <MessageWriter as acp_client::MessageWriter>::on_session_info_update(&writer, &info).await;

        store.flush_writes().await;
        let session = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.acp_title.as_deref(), Some("Earlier title"));
    }

    #[tokio::test]
    async fn session_info_update_blank_title_value_preserves_session_acp_title() {
        let (store, session_id, writer) = setup_writer();
        store
            .update_session_acp_title(&session_id, Some("Earlier title"))
            .unwrap();

        let info = acp_client::SessionInfoUpdate::new().title("  ``  ");
        <MessageWriter as acp_client::MessageWriter>::on_session_info_update(&writer, &info).await;

        store.flush_writes().await;
        let session = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.acp_title.as_deref(), Some("Earlier title"));
    }

    /// A prompt as Staged builds one: its instructions, then the user's text.
    const STAGED_PROMPT: &str = "<action>\nThe user is requesting you make a commit based on \
        the prompt below. Make the necessary\ncode changes, then create a commit.\n\n\
        Project information:\n  You are working in project \"Login\".\n</action>\n\n\
        Fix the login flow so expired tokens are refreshed";

    /// A writer for a session whose first message is [`STAGED_PROMPT`],
    /// stored the way `start_session` stores it before the agent starts.
    fn setup_writer_after_prompt() -> (Arc<Store>, String, MessageWriter) {
        let (store, session_id, writer) = setup_writer();
        store
            .add_session_message(&session_id, MessageRole::User, STAGED_PROMPT)
            .unwrap();
        (store, session_id, writer)
    }

    async fn push_title(writer: &MessageWriter, title: &str) {
        let info = acp_client::SessionInfoUpdate::new().title(title);
        <MessageWriter as acp_client::MessageWriter>::on_session_info_update(writer, &info).await;
    }

    fn acp_title(store: &Store, session_id: &str) -> Option<String> {
        assert!(store.flush_writes_blocking(std::time::Duration::from_secs(30)));
        store.get_session(session_id).unwrap().unwrap().acp_title
    }

    /// What the connection tells the writer just before it sends a prompt.
    /// Through the trait object, which is all the connection has.
    async fn prompt_sent(writer: &MessageWriter, text_blocks: &[&str]) {
        let writer: &dyn acp_client::MessageWriter = writer;
        writer.on_prompt_sent(text_blocks).await;
    }

    /// A chat follow-up: stored, and sent, exactly as the user typed it.
    const TYPED_FOLLOW_UP: &str = "Add tests for the token refresh and run them";

    /// A writer for the turn that sends [`TYPED_FOLLOW_UP`] to a session
    /// whose first message was [`STAGED_PROMPT`].
    async fn setup_writer_after_typed_follow_up() -> (Arc<Store>, String, MessageWriter) {
        let (store, session_id, writer) = setup_writer_after_prompt();
        store
            .add_session_message(&session_id, MessageRole::Assistant, "Tokens now refresh.")
            .unwrap();
        store
            .add_session_message(&session_id, MessageRole::User, TYPED_FOLLOW_UP)
            .unwrap();
        prompt_sent(&writer, &[TYPED_FOLLOW_UP]).await;
        (store, session_id, writer)
    }

    #[tokio::test]
    async fn session_title_that_starts_the_sent_prompt_is_ignored() {
        let (store, session_id, writer) = setup_writer_after_prompt();

        push_title(
            &writer,
            "<action>\nThe user is requesting you make a commit",
        )
        .await;

        assert_eq!(acp_title(&store, &session_id), None);
        // The update itself is still on record, as every ACP event is.
        store.flush_writes().await;
        let metadata_rows = store.get_session_acp_metadata_archive(&session_id).unwrap();
        assert_eq!(metadata_rows.len(), 1);
        assert_eq!(
            metadata_rows[0].acp.acp_event_kind.as_deref(),
            Some("session_info_update")
        );
    }

    #[tokio::test]
    async fn truncated_and_whitespace_normalised_prompt_echoes_are_ignored() {
        let (store, session_id, writer) = setup_writer_after_prompt();

        for title in [
            // Cut with an ellipsis, mid-word and at a word.
            "<action>\nThe user is requesting you make a comm…",
            "<action>\nThe user is requesting you make a commit...",
            // Newlines and runs of spaces collapsed, ends padded.
            "<action> The user is requesting you make a commit based on the prompt below. \
             Make the necessary code changes",
            "  <action>   The user is\trequesting  ",
            // Both.
            "<action> The user is requesting you make a commit based on the prompt below. \
             Make the necessary code changes, then create a commit. Project information: \
             You are working in project \"Login\". </action> Fix the login flow so exp…",
        ] {
            push_title(&writer, title).await;
            assert_eq!(acp_title(&store, &session_id), None, "{title:?}");
        }
    }

    #[tokio::test]
    async fn written_session_title_is_accepted_after_the_prompt_was_sent() {
        let (store, session_id, writer) = setup_writer_after_prompt();

        push_title(&writer, "Refresh expired login tokens").await;
        assert_eq!(
            acp_title(&store, &session_id).as_deref(),
            Some("Refresh expired login tokens")
        );

        // The user's own words, which are not how the prompt starts, and a
        // title that only shares its first word with it.
        push_title(&writer, "Fix the login flow").await;
        assert_eq!(
            acp_title(&store, &session_id).as_deref(),
            Some("Fix the login flow")
        );
        push_title(&writer, "<action> tags in prompts").await;
        assert_eq!(
            acp_title(&store, &session_id).as_deref(),
            Some("<action> tags in prompts")
        );
    }

    #[tokio::test]
    async fn written_session_title_is_accepted_after_an_ignored_one() {
        let (store, session_id, writer) = setup_writer_after_prompt();

        push_title(&writer, "<action> The user is requesting…").await;
        assert_eq!(acp_title(&store, &session_id), None);

        push_title(&writer, "Refresh expired login tokens").await;
        assert_eq!(
            acp_title(&store, &session_id).as_deref(),
            Some("Refresh expired login tokens")
        );
    }

    #[tokio::test]
    async fn ignored_session_title_leaves_the_existing_title() {
        let (store, session_id, writer) = setup_writer_after_prompt();
        push_title(&writer, "Refresh expired login tokens").await;

        // The echo sent again after the agent's own title, as on a reload.
        push_title(&writer, "<action> The user is requesting…").await;

        assert_eq!(
            acp_title(&store, &session_id).as_deref(),
            Some("Refresh expired login tokens")
        );
    }

    /// A later turn has its own writer, and so does an attach catching the
    /// session up: neither was handed the first prompt, and both know it
    /// from the session's stored messages. The later messages count too.
    #[tokio::test]
    async fn prompt_echo_is_ignored_in_later_turns_and_for_later_messages() {
        let (store, session_id, _first_turn_writer) = setup_writer_after_prompt();
        store
            .add_session_message(&session_id, MessageRole::Assistant, "Tokens now refresh.")
            .unwrap();
        let follow_up = "<action>\nThe user sent a follow-up message.\n</action>\n\nAlso log it";
        store
            .add_session_message(&session_id, MessageRole::User, follow_up)
            .unwrap();

        let later_turn = MessageWriter::new(session_id.clone(), Arc::clone(&store));
        prompt_sent(&later_turn, &[follow_up]).await;
        let catching_up = MessageWriter::catching_up(session_id.clone(), Arc::clone(&store));
        for writer in [&later_turn, &catching_up] {
            push_title(writer, "<action> The user is requesting you make a commit…").await;
            push_title(writer, "<action> The user sent a follow-up message.").await;
            assert_eq!(acp_title(&store, &session_id), None);
        }

        push_title(&later_turn, "Log token refreshes").await;
        assert_eq!(
            acp_title(&store, &session_id).as_deref(),
            Some("Log token refreshes")
        );
    }

    /// A title that is the start of a message sent as typed, with nothing to
    /// say it was cut, is a title the agent wrote: "Add tests" names a
    /// session whose user asked to "Add tests for ...".
    #[tokio::test]
    async fn written_title_that_starts_a_typed_follow_up_is_accepted() {
        let (store, session_id, writer) = setup_writer_after_typed_follow_up().await;

        for title in [
            "Add tests for the token refresh",
            "Add tests",
            "Add tests for the token refresh and run",
        ] {
            push_title(&writer, title).await;
            assert_eq!(acp_title(&store, &session_id).as_deref(), Some(title));
        }

        // The same for the message Staged sends to resume a paused session.
        let resume = crate::rate_limit_scheduler::AUTO_RETRY_RESUME_PROMPT;
        store
            .add_session_message(&session_id, MessageRole::User, resume)
            .unwrap();
        prompt_sent(&writer, &[resume]).await;
        push_title(&writer, "Continue").await;
        assert_eq!(acp_title(&store, &session_id).as_deref(), Some("Continue"));
    }

    /// The whole of a typed message, or its start marked as cut, is still
    /// the agent repeating its input.
    #[tokio::test]
    async fn whole_and_marked_as_cut_echoes_of_a_typed_follow_up_are_ignored() {
        let (store, session_id, writer) = setup_writer_after_typed_follow_up().await;
        let resume = crate::rate_limit_scheduler::AUTO_RETRY_RESUME_PROMPT;
        store
            .add_session_message(&session_id, MessageRole::User, resume)
            .unwrap();
        // One that sent the follow-up, and one that only has the stored rows.
        let catching_up = MessageWriter::catching_up(session_id.clone(), Arc::clone(&store));

        for writer in [&writer, &catching_up] {
            for title in [
                TYPED_FOLLOW_UP,
                "  Add tests for the token refresh\nand run them ",
                "Add tests for the token refr…",
                "Add tests for the token refresh...",
                resume,
            ] {
                push_title(writer, title).await;
                assert_eq!(acp_title(&store, &session_id), None, "{title:?}");
            }
        }
    }

    /// A session titled by a build that still stored echoes keeps getting
    /// the same echo from its agent (on every reload, for one). The echo
    /// arriving again takes the stored one out, so the session falls back
    /// the way a new one does.
    #[tokio::test]
    async fn stored_prompt_echo_is_cleared_when_the_echo_arrives_again() {
        let echo = "<action> The user is requesting you make a commit based on the prompt \
                    below. Make the necessary code chan…";
        for stored in [
            // The echo that arrives.
            echo,
            // Another cut of the same prompt, and one with no marker.
            "<action> The user is requesting you make a comm…",
            "<action> The user is requesting",
        ] {
            let (store, session_id, _) = setup_writer_after_prompt();
            store
                .update_session_acp_title(&session_id, Some(stored))
                .unwrap();
            // A reload's writer: it has sent nothing itself.
            let writer = MessageWriter::new(session_id.clone(), Arc::clone(&store));

            push_title(&writer, echo).await;
            assert_eq!(acp_title(&store, &session_id), None, "{stored:?}");

            // The session takes a written title as any other does.
            push_title(&writer, "Refresh expired login tokens").await;
            assert_eq!(
                acp_title(&store, &session_id).as_deref(),
                Some("Refresh expired login tokens")
            );
        }
    }

    /// A title is stored without its backticks, so a stored echo of a prompt
    /// that has some is recognised with the prompt read the same way. An
    /// echo of a typed message that was stored is cleared like any other.
    #[tokio::test]
    async fn stored_echo_is_recognised_as_it_was_stored() {
        let (store, session_id, writer) = setup_writer();
        let prompt = "<action>\nCall the `write_note` tool with the note's `title`.\n</action>\n\n\
                      Summarise the branch";
        store
            .add_session_message(&session_id, MessageRole::User, prompt)
            .unwrap();
        store
            .add_session_message(&session_id, MessageRole::User, TYPED_FOLLOW_UP)
            .unwrap();

        // As the earlier build stored `<action> Call the `write_note` tool…`.
        store
            .update_session_acp_title(&session_id, Some("<action> Call the write_note tool…"))
            .unwrap();
        push_title(&writer, "<action> Call the `write_note` tool…").await;
        assert_eq!(acp_title(&store, &session_id), None);

        store
            .update_session_acp_title(&session_id, Some("Add tests for the token refr…"))
            .unwrap();
        push_title(&writer, TYPED_FOLLOW_UP).await;
        assert_eq!(acp_title(&store, &session_id), None);
    }

    /// Only a stored echo is cleared. A title the agent wrote stays through
    /// every kind of echo, including one that is the bare start of a typed
    /// message and one that shares its opening with the prompt.
    #[tokio::test]
    async fn stored_written_title_is_never_cleared_by_an_echo() {
        let (store, session_id, writer) = setup_writer_after_typed_follow_up().await;
        let catching_up = MessageWriter::catching_up(session_id.clone(), Arc::clone(&store));

        for written in [
            "Refresh expired login tokens",
            "Add tests for the token refresh",
            "<action> tags in prompts",
        ] {
            store
                .update_session_acp_title(&session_id, Some(written))
                .unwrap();
            for writer in [&writer, &catching_up] {
                for echo in [
                    "<action> The user is requesting you make a commit…",
                    "<action>\nThe user is requesting",
                    TYPED_FOLLOW_UP,
                    "Add tests for the token refr…",
                ] {
                    push_title(writer, echo).await;
                    assert_eq!(
                        acp_title(&store, &session_id).as_deref(),
                        Some(written),
                        "{echo:?}"
                    );
                }
            }
        }
    }

    /// An agent that names a session after its input reads every text block
    /// of the request: the prompt, then what the connection adds for the
    /// attachments. The connection reports those blocks, and a title that
    /// runs on into them is an echo of the message as a whole, to the writer
    /// of the connection that sent it. Every later connection to the
    /// session has a writer that was not told them, and keeps the title.
    #[tokio::test]
    async fn prompt_echo_is_measured_against_every_text_block_sent() {
        let (store, session_id, _) = setup_writer();
        // The stored row is what the user typed.
        store
            .add_session_message(&session_id, MessageRole::User, "also add tests")
            .unwrap();
        let title = "also add tests page 1 of 2 of spec.pdf page 2 of 2 of spec.pdf";

        let writer = MessageWriter::new(session_id.clone(), Arc::clone(&store));
        prompt_sent(
            &writer,
            &[
                "also add tests",
                "page 1 of 2 of spec.pdf",
                "page 2 of 2 of spec.pdf",
            ],
        )
        .await;
        for echo in [title, "also add tests page 1 of 2 of sp…", "also add tests"] {
            push_title(&writer, echo).await;
            assert_eq!(acp_title(&store, &session_id), None, "{echo:?}");
        }
        // Its bare start is a title, as for any typed message.
        push_title(&writer, "also add").await;
        assert_eq!(acp_title(&store, &session_id).as_deref(), Some("also add"));

        // That was the connection that sent the message. A later one has a
        // writer of its own that was told nothing of it: the next run of
        // the session, where an agent that sends its title again as the
        // session loads does so before the run's own prompt goes out, as
        // much as an attach catching the session up. Each has the stored
        // row only, which the title runs past, so the title is kept.
        let later_run = MessageWriter::new(session_id.clone(), Arc::clone(&store));
        let catching_up = MessageWriter::catching_up(session_id.clone(), Arc::clone(&store));
        for (connection, writer) in [("a later run", &later_run), ("catching up", &catching_up)] {
            store.update_session_acp_title(&session_id, None).unwrap();
            push_title(writer, title).await;
            assert_eq!(
                acp_title(&store, &session_id).as_deref(),
                Some(title),
                "{connection}"
            );
        }
        // Sending its own prompt does not teach the later run the earlier
        // one's attachment text.
        store
            .add_session_message(&session_id, MessageRole::User, "and document them")
            .unwrap();
        prompt_sent(&later_run, &["and document them"]).await;
        store.update_session_acp_title(&session_id, None).unwrap();
        push_title(&later_run, title).await;
        assert_eq!(acp_title(&store, &session_id).as_deref(), Some(title));
    }

    /// The prompt block can itself say more than the stored user message
    /// (a note about an attachment that was left out, a notice for the
    /// agent only). It is kept by itself too, for an agent that names the
    /// session after that block alone.
    #[tokio::test]
    async fn prompt_echo_is_measured_against_the_prompt_block_as_sent() {
        let (store, session_id, _) = setup_writer();
        store
            .add_session_message(&session_id, MessageRole::User, "Describe this")
            .unwrap();
        let prompt_block = "Describe this\n\n[Attached image big.png (9000000 bytes) not included \
                            in this prompt: over the 20 MB image budget]";
        let title = "Describe this [Attached image big.png (9000000 bytes) not included in \
                     this prompt: over the 20 MB image budget]";

        let writer = MessageWriter::new(session_id.clone(), Arc::clone(&store));
        prompt_sent(&writer, &[prompt_block, "page 1 of 1 of small.pdf"]).await;
        push_title(&writer, title).await;
        assert_eq!(acp_title(&store, &session_id), None);

        // Nothing is kept for a request with no text.
        let silent = MessageWriter::new(session_id.clone(), Arc::clone(&store));
        prompt_sent(&silent, &[]).await;
        push_title(&silent, title).await;
        assert_eq!(acp_title(&store, &session_id).as_deref(), Some(title));
    }

    fn acp_event(event_kind: &str, content: serde_json::Value) -> acp_client::AcpEventMetadata {
        acp_client::AcpEventMetadata {
            event_kind: Some(event_kind.to_string()),
            content: Some(content),
            ..Default::default()
        }
    }

    #[test]
    fn usage_update_without_rate_limit_fragment_skips_session_row_write() {
        let metadata = acp_event(
            "usage_update",
            serde_json::json!({ "used": 123, "size": 200000 }),
        );
        assert_eq!(super::provider_metadata_for_acp_event(&metadata), None);
    }

    #[test]
    fn usage_update_with_rate_limit_fragment_persists_provider_metadata() {
        let content = serde_json::json!({
            "used": 123,
            "size": 200000,
            "_meta": { "_claude/rateLimit": { "status": "rejected" } }
        });
        let metadata = acp_event("usage_update", content.clone());
        assert_eq!(
            super::provider_metadata_for_acp_event(&metadata),
            Some(serde_json::json!({ "acp": { "usageUpdate": content } }))
        );
    }

    #[test]
    fn prompt_events_persist_provider_metadata_without_a_rate_limit_fragment() {
        let error = acp_event("prompt_error", serde_json::json!({ "code": -32603 }));
        assert!(super::provider_metadata_for_acp_event(&error).is_some());

        let response = acp_event(
            "prompt_response",
            serde_json::json!({ "stopReason": "end" }),
        );
        assert!(super::provider_metadata_for_acp_event(&response).is_some());
    }
}
