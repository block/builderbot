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

use crate::store::{MessageRole, Store};

use acp_client::strip_code_fences;

/// Minimum interval between DB flushes for streaming text. Chunks accumulate
/// in memory and are written at most this often, reducing mutex contention
/// when many sessions stream concurrently. [`MessageWriter::finalize`]
/// always forces an immediate flush regardless of this interval.
const FLUSH_INTERVAL: Duration = Duration::from_millis(150);

/// Replay state: roles of previously persisted messages and a cursor tracking
/// how far through replay we are.
struct ReplayState {
    roles: Vec<MessageRole>,
    cursor: usize,
}

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
    /// DB row id of the currently streaming tool result.
    ///
    /// ACP can send multiple content updates for one tool call; we update
    /// the same row instead of inserting duplicates.
    current_tool_result_msg_id: Mutex<Option<i64>>,
    /// Replay dedup state, loaded from DB on resume.
    replay: Mutex<ReplayState>,
    /// Set to `true` while we are skipping a replayed assistant block.
    /// Prevents double-advancing the cursor when `flush_text` is called
    /// multiple times for the same block (throttled flush + finalize).
    skipping_assistant: Mutex<bool>,
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

impl MessageWriter {
    pub fn new(session_id: String, store: Arc<Store>, resuming: bool) -> Self {
        let replay_roles = if resuming {
            store
                .get_session_messages(&session_id)
                .unwrap_or_default()
                .into_iter()
                .filter(|m| m.role != MessageRole::User)
                .map(|m| m.role)
                .collect()
        } else {
            Vec::new()
        };
        Self {
            session_id,
            store,
            current_assistant_msg_id: Mutex::new(None),
            current_text: Mutex::new(String::new()),
            last_flush_at: Mutex::new(Instant::now()),
            tool_call_rows: Mutex::new(HashMap::new()),
            current_tool_result_msg_id: Mutex::new(None),
            replay: Mutex::new(ReplayState {
                roles: replay_roles,
                cursor: 0,
            }),
            skipping_assistant: Mutex::new(false),
        }
    }

    /// Check if the current message matches the next expected replay message.
    /// If so, advance the cursor and return `true` (skip the write).
    async fn try_skip_replay(&self, role: MessageRole) -> bool {
        let mut replay = self.replay.lock().await;
        if replay.cursor < replay.roles.len() && replay.roles[replay.cursor] == role {
            replay.cursor += 1;
            true
        } else {
            false
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
        *self.skipping_assistant.lock().await = false;
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
        *self.current_tool_result_msg_id.lock().await = None;

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

        if self.try_skip_replay(MessageRole::ToolCall).await {
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
    pub async fn record_tool_result(&self, content: &str) {
        let content = strip_code_fences(content);
        let mut current_result_id = self.current_tool_result_msg_id.lock().await;
        if let Some(id) = *current_result_id {
            let _ = self.store.update_message_content(id, &content);
            return;
        }

        if self.try_skip_replay(MessageRole::ToolResult).await {
            return;
        }

        match self
            .store
            .add_session_message(&self.session_id, MessageRole::ToolResult, &content)
        {
            Ok(id) => {
                *current_result_id = Some(id);
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
        // If we already decided to skip this assistant block during replay,
        // don't re-enter try_skip_replay (which would advance the cursor
        // past a subsequent message).
        if *self.skipping_assistant.lock().await {
            return;
        }
        let mut msg_id = self.current_assistant_msg_id.lock().await;
        match *msg_id {
            Some(id) => {
                let _ = self.store.update_message_content(id, &text);
            }
            None => {
                if self.try_skip_replay(MessageRole::Assistant).await {
                    *self.skipping_assistant.lock().await = true;
                    return;
                }
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

    async fn record_tool_result(&self, content: &str) {
        self.record_tool_result(content).await
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use super::MessageWriter;
    use crate::store::{MessageRole, Session, Store};

    fn setup_writer() -> (Arc<Store>, String, MessageWriter) {
        let store = Arc::new(Store::in_memory().expect("in-memory store"));
        let session = Session::new_running("test prompt", Path::new("."));
        store.create_session(&session).expect("create session");
        let writer = MessageWriter::new(session.id.clone(), Arc::clone(&store), false);
        (store, session.id, writer)
    }

    #[tokio::test]
    async fn record_tool_result_updates_existing_row_for_streaming_updates() {
        let (store, session_id, writer) = setup_writer();

        writer
            .record_tool_call("tc-1", "Run echo hello", None)
            .await;
        writer.record_tool_result("first chunk").await;
        writer.record_tool_result("second chunk").await;

        let messages = store
            .get_session_messages(&session_id)
            .expect("query messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::ToolCall);
        assert_eq!(messages[1].role, MessageRole::ToolResult);
        assert_eq!(messages[1].content, "second chunk");
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
    async fn resume_skips_replayed_messages_without_duplicates() {
        let store = Arc::new(Store::in_memory().expect("in-memory store"));
        let session = Session::new_running("test prompt", Path::new("."));
        store.create_session(&session).expect("create session");

        // Simulate a first run: user prompt + assistant + tool call + tool result.
        store
            .add_session_message(&session.id, MessageRole::User, "test prompt")
            .expect("add user msg");
        store
            .add_session_message(&session.id, MessageRole::Assistant, "thinking...")
            .expect("add assistant msg");
        store
            .add_session_message(&session.id, MessageRole::ToolCall, "Run ls")
            .expect("add tool_call msg");
        store
            .add_session_message(&session.id, MessageRole::ToolResult, "file.txt")
            .expect("add tool_result msg");

        // Create a resuming writer — it should load the 3 non-User roles.
        let writer = MessageWriter::new(session.id.clone(), Arc::clone(&store), true);

        // Replay the same sequence the server would send (no User messages).
        writer.append_text("thinking...").await;
        writer.finalize().await;
        writer.record_tool_call("tc-1", "Run ls", None).await;
        writer.record_tool_result("file.txt").await;

        // Now send a new message that goes beyond replay.
        writer.append_text("new response").await;
        writer.finalize().await;

        let messages = store
            .get_session_messages(&session.id)
            .expect("query messages");
        // Original 4 + 1 new assistant = 5
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[4].role, MessageRole::Assistant);
        assert_eq!(messages[4].content, "new response");
    }
}
