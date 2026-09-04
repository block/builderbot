//! Protocol-agnostic message writer — streams agent output into the DB.
//!
//! [`MessageWriter`] accumulates streaming text in memory and flushes to
//! the DB at a throttled interval ([`FLUSH_INTERVAL`]). Tool calls and
//! results are written immediately.
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

use crate::store::{AcpMessageMetadata, MessageRole, Store};

use acp_client::{
    autoapprove_permission_decision, strip_code_fences, AcpEventMetadata, AcpInitializeMetadata,
    AcpPermissionDecision, AcpPermissionRequest, AcpToolCallMetadata,
};
use agent_client_protocol::schema::MaybeUndefined;

/// ACP event kind tagging a visible transcript row Staged authored itself
/// rather than received from the agent: the announcement that a pinned config
/// value was unavailable and a substitute was applied. Rows carrying it are
/// excluded from replay boundaries — see
/// [`super::replay_boundaries_from_messages`].
pub(crate) const CONFIG_OPTION_FALLBACK_EVENT: &str = "config_option_fallback";

/// Minimum interval between DB flushes for streaming text. Chunks accumulate
/// in memory and are written at most this often, reducing mutex contention
/// when many sessions stream concurrently. [`MessageWriter::finalize`]
/// always forces an immediate flush regardless of this interval.
const FLUSH_INTERVAL: Duration = Duration::from_millis(150);

/// Streams agent output into the DB, one session at a time.
///
/// All methods are `&self` + async — the struct uses interior mutability
/// via `tokio::sync::Mutex` so it can be shared behind an `Arc`.
pub struct MessageWriter {
    session_id: String,
    store: Arc<Store>,
    /// DB row id of the current assistant message being streamed into.
    current_assistant_msg_id: Mutex<Option<i64>>,
    /// Accumulated text for the current assistant message (complete, not
    /// a delta). Flushed wholesale on each DB write.
    current_text: Mutex<String>,
    /// When we last wrote to the DB — used to throttle flush frequency.
    last_flush_at: Mutex<Instant>,
    /// Maps external tool-call IDs → (DB row ID, last-known title).
    tool_call_rows: Mutex<HashMap<String, (i64, String)>>,
    /// Maps external tool-call IDs -> DB row ID for streaming tool results.
    ///
    /// ACP can send interleaved content updates for multiple tool calls; each
    /// tool call updates its own row instead of sharing one global result row.
    tool_result_rows: Mutex<HashMap<String, i64>>,
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

impl MessageWriter {
    pub fn new(session_id: String, store: Arc<Store>) -> Self {
        Self {
            session_id,
            store,
            current_assistant_msg_id: Mutex::new(None),
            current_text: Mutex::new(String::new()),
            last_flush_at: Mutex::new(Instant::now()),
            tool_call_rows: Mutex::new(HashMap::new()),
            tool_result_rows: Mutex::new(HashMap::new()),
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
        self.current_assistant_msg_id.lock().await.take();
        *self.current_text.lock().await = String::new();
    }

    // =====================================================================
    // Tool calls
    // =====================================================================

    /// Record a tool call. Finalizes any in-progress assistant text first
    /// to maintain correct message ordering.
    pub async fn record_tool_call(
        &self,
        tool_call_id: &str,
        title: &str,
        raw_input: Option<&serde_json::Value>,
    ) {
        self.finalize().await;

        let title = sanitize_title(title);
        let content = format_tool_call_content(&title, raw_input);

        // Some providers may resend ToolCall for the same ID while streaming.
        // Treat those as updates to the existing row.
        let mut rows = self.tool_call_rows.lock().await;
        if let Some((row_id, stored_title)) = rows.get_mut(tool_call_id) {
            *stored_title = title.clone();
            let _ = self.store.update_message_content(*row_id, &content);
            return;
        }

        match self
            .store
            .add_session_message(&self.session_id, MessageRole::ToolCall, &content)
        {
            Ok(id) => {
                rows.insert(tool_call_id.to_string(), (id, title));
            }
            Err(e) => log::error!("Failed to insert tool_call message: {e}"),
        }
    }

    /// Update a previously recorded tool call's title and/or raw input.
    ///
    /// When `title` is `None`, the last-known title stored at recording time
    /// is reused so that a `raw_input`-only update doesn't blank the name.
    pub async fn update_tool_call_title(
        &self,
        tool_call_id: &str,
        title: Option<&str>,
        raw_input: Option<&serde_json::Value>,
    ) {
        let mut rows = self.tool_call_rows.lock().await;
        if let Some((row_id, stored_title)) = rows.get_mut(tool_call_id) {
            let effective_title = match title {
                Some(t) => {
                    let sanitized = sanitize_title(t);
                    *stored_title = sanitized.clone();
                    sanitized
                }
                None => stored_title.clone(),
            };
            let content = format_tool_call_content(&effective_title, raw_input);
            let _ = self.store.update_message_content(*row_id, &content);
        }
    }

    /// Record the result/output of a tool call.
    pub async fn record_tool_result(&self, tool_call_id: &str, content: &str) {
        let content = strip_code_fences(content);
        let mut result_rows = self.tool_result_rows.lock().await;
        if let Some(id) = result_rows.get(tool_call_id).copied() {
            let _ = self.store.update_message_content(id, &content);
            return;
        }

        match self
            .store
            .add_session_message(&self.session_id, MessageRole::ToolResult, &content)
        {
            Ok(id) => {
                result_rows.insert(tool_call_id.to_string(), id);
                let metadata = AcpMessageMetadata {
                    acp_tool_call_id: Some(tool_call_id.to_string()),
                    ..Default::default()
                };
                if let Err(e) = self.store.update_message_acp_metadata(id, &metadata) {
                    log::error!("Failed to persist ACP tool-result metadata: {e}");
                }
            }
            Err(e) => log::error!("Failed to insert tool_result message: {e}"),
        }
    }

    // =====================================================================
    // Internal
    // =====================================================================

    /// Flush accumulated text to the DB unconditionally (insert or update).
    ///
    /// Idempotent — calling it twice without new chunks re-writes the same
    /// content. The buffer is never cleared here; only [`finalize`] resets it.
    async fn flush_text(&self) {
        let text = self.current_text.lock().await;
        if text.is_empty() {
            return;
        }
        let mut msg_id = self.current_assistant_msg_id.lock().await;
        match *msg_id {
            Some(id) => {
                let _ = self.store.update_message_content(id, &text);
            }
            None => {
                match self.store.add_session_message(
                    &self.session_id,
                    MessageRole::Assistant,
                    &text,
                ) {
                    Ok(id) => {
                        *msg_id = Some(id);
                    }
                    Err(e) => log::error!("Failed to insert assistant message: {e}"),
                }
            }
        }
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
        self.append_text(text).await
    }

    async fn finalize(&self) {
        self.finalize().await
    }

    async fn record_tool_call(
        &self,
        tool_call_id: &str,
        title: &str,
        raw_input: Option<&serde_json::Value>,
    ) {
        self.record_tool_call(tool_call_id, title, raw_input).await
    }

    async fn update_tool_call_title(
        &self,
        tool_call_id: &str,
        title: Option<&str>,
        raw_input: Option<&serde_json::Value>,
    ) {
        self.update_tool_call_title(tool_call_id, title, raw_input)
            .await
    }

    async fn record_tool_result(&self, tool_call_id: &str, content: &str) {
        self.record_tool_result(tool_call_id, content).await
    }

    async fn on_session_info_update(&self, info: &acp_client::SessionInfoUpdate) {
        let metadata = AcpMessageMetadata {
            acp_event_kind: Some("session_info_update".to_string()),
            acp_session_info: serde_json::to_value(info).ok(),
            ..Default::default()
        };
        if let Err(e) = self
            .store
            .add_acp_metadata_message(&self.session_id, &metadata)
        {
            log::error!("Failed to persist ACP session info update: {e}");
        }

        // Mirror the agent-provided title onto the session row so it can be
        // shown as an interim session name while the session runs.
        match &info.title {
            MaybeUndefined::Value(title) => {
                let title = sanitize_title(title);
                let title = title.trim();
                if !title.is_empty() {
                    if let Err(e) = self
                        .store
                        .update_session_acp_title(&self.session_id, Some(title))
                    {
                        log::error!("Failed to persist ACP session title: {e}");
                    }
                }
            }
            // The agent explicitly retracted the title.
            MaybeUndefined::Null => {
                if let Err(e) = self.store.update_session_acp_title(&self.session_id, None) {
                    log::error!("Failed to clear ACP session title: {e}");
                }
            }
            // Update was about `updated_at`/meta only — leave the title alone.
            MaybeUndefined::Undefined => {}
        }
    }

    async fn on_model_state_update(&self, state: &acp_client::SessionModelState) {
        let metadata = AcpMessageMetadata {
            acp_event_kind: Some("session_mode_state".to_string()),
            acp_session_mode_state: serde_json::to_value(state).ok(),
            ..Default::default()
        };
        if let Err(e) = self
            .store
            .add_acp_metadata_message(&self.session_id, &metadata)
        {
            log::error!("Failed to persist ACP session mode state: {e}");
        }
    }

    async fn on_config_option_update(&self, options: &[acp_client::SessionConfigOption]) {
        if options.is_empty() {
            return;
        }

        let metadata = AcpMessageMetadata {
            acp_event_kind: Some("config_options_update".to_string()),
            acp_config_options: serde_json::to_value(options).ok(),
            ..Default::default()
        };
        if let Err(e) = self
            .store
            .add_acp_metadata_message(&self.session_id, &metadata)
        {
            log::error!("Failed to persist ACP config options update: {e}");
        }
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
        self.finalize().await;

        if let Err(e) = self.store.add_authored_session_message(
            &self.session_id,
            MessageRole::Assistant,
            notice,
            CONFIG_OPTION_FALLBACK_EVENT,
        ) {
            log::error!("Failed to persist ACP config fallback notice: {e}");
        }
    }

    async fn on_initialize(&self, metadata: &AcpInitializeMetadata) {
        let metadata = AcpMessageMetadata {
            acp_event_kind: Some("initialize".to_string()),
            acp_protocol_version: Some(metadata.protocol_version.clone()),
            acp_agent_capabilities: metadata.agent_capabilities.clone(),
            acp_auth_methods: metadata.auth_methods.clone(),
            acp_agent_info: metadata.agent_info.clone(),
            ..Default::default()
        };
        if let Err(e) = self
            .store
            .add_acp_metadata_message(&self.session_id, &metadata)
        {
            log::error!("Failed to persist ACP initialization metadata: {e}");
        }
    }

    async fn record_tool_call_metadata(&self, metadata: AcpToolCallMetadata) {
        let tool_call_id = metadata.tool_call_id.clone();
        let metadata = acp_tool_call_metadata(metadata);
        let row_id = if let Some(tool_call_id) = tool_call_id {
            let rows = self.tool_call_rows.lock().await;
            rows.get(&tool_call_id).map(|(row_id, _)| *row_id)
        } else {
            None
        };

        let result = match row_id {
            Some(row_id) => self.store.update_message_acp_metadata(row_id, &metadata),
            None => self
                .store
                .add_acp_metadata_message(&self.session_id, &metadata)
                .map(|_| ()),
        };

        if let Err(e) = result {
            log::error!("Failed to persist ACP tool-call metadata: {e}");
        }
    }

    async fn record_acp_event_metadata(&self, metadata: AcpEventMetadata) {
        let metadata = acp_event_metadata(metadata);
        let role = acp_event_role(&metadata);
        if let Err(e) =
            self.store
                .add_acp_metadata_message_with_role(&self.session_id, role, &metadata)
        {
            log::error!("Failed to persist ACP event metadata: {e}");
        }
    }

    async fn request_permission(
        &self,
        request: AcpPermissionRequest,
        cancel_token: CancellationToken,
    ) -> AcpPermissionDecision {
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

    use super::MessageWriter;
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
        let metadata = store
            .get_session_acp_metadata_messages(&session_id)
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

    #[tokio::test]
    async fn record_tool_result_updates_existing_row_for_streaming_updates() {
        let (store, session_id, writer) = setup_writer();

        writer
            .record_tool_call("tc-1", "Run echo hello", None)
            .await;
        writer.record_tool_result("tc-1", "first chunk").await;
        writer.record_tool_result("tc-1", "second chunk").await;

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
    async fn record_tool_call_same_id_updates_instead_of_inserting() {
        let (store, session_id, writer) = setup_writer();

        writer
            .record_tool_call("tc-dup", "Run first title", None)
            .await;
        writer
            .record_tool_call("tc-dup", "Run updated title", None)
            .await;

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

        let messages = store
            .get_session_messages(&session_id)
            .expect("query messages");
        assert_eq!(messages.len(), 1);

        let parsed: serde_json::Value =
            serde_json::from_str(&messages[0].content).expect("content should be valid JSON");
        assert_eq!(parsed["name"], "Read file");
        assert_eq!(parsed["input"]["path"], "bar.rs");
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

        assert!(
            store.get_session_messages(&session_id).unwrap().is_empty(),
            "initialization metadata should not appear in transcript rows"
        );

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

        assert!(
            store.get_session_messages(&session_id).unwrap().is_empty(),
            "ACP event metadata should not appear in transcript rows"
        );

        let metadata_rows = store
            .get_session_acp_metadata_messages(&session_id)
            .unwrap();
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

        let metadata_rows = store
            .get_session_acp_metadata_messages(&session_id)
            .unwrap();
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

        let session = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.acp_title.as_deref(), Some("Fix the login flow"));

        // The metadata-row archive of the raw update is unchanged behavior.
        let metadata_rows = store
            .get_session_acp_metadata_messages(&session_id)
            .unwrap();
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

        let session = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.acp_title.as_deref(), Some("Earlier title"));
    }
}
