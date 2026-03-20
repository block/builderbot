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
    /// Maps external tool-call IDs → DB row IDs.
    tool_call_rows: Mutex<HashMap<String, i64>>,
    /// DB row id of the currently streaming tool result.
    ///
    /// ACP can send multiple content updates for one tool call; we update
    /// the same row instead of inserting duplicates.
    current_tool_result_msg_id: Mutex<Option<i64>>,
}

/// Strip backticks from agent-provided tool-call titles.
fn sanitize_title(title: &str) -> String {
    title.replace('`', "")
}

/// Strip outer markdown code fences from tool-result content.
/// Agents often wrap results in ``` fences which are redundant in our `<pre>` display.
/// The closing fence may be absent when content was truncated by the preview limit.
fn strip_code_fences(content: &str) -> String {
    let trimmed = content.trim();
    if let Some(after_open) = trimmed.strip_prefix("```") {
        if let Some(nl) = after_open.find('\n') {
            let body = after_open[nl + 1..].trim_end();
            return body
                .strip_suffix("```")
                .unwrap_or(body)
                .trim_end()
                .to_string();
        }
    }
    content.to_string()
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
            current_tool_result_msg_id: Mutex::new(None),
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
        *self.current_assistant_msg_id.lock().await = None;
        *self.current_text.lock().await = String::new();
    }

    // =====================================================================
    // Tool calls
    // =====================================================================

    /// Record a tool call. Finalizes any in-progress assistant text first
    /// to maintain correct message ordering.
    pub async fn record_tool_call(&self, tool_call_id: &str, title: &str) {
        self.finalize().await;
        *self.current_tool_result_msg_id.lock().await = None;

        let title = sanitize_title(title);

        // Some providers may resend ToolCall for the same ID while streaming.
        // Treat those as updates to the existing row.
        if let Some(&row_id) = self.tool_call_rows.lock().await.get(tool_call_id) {
            let _ = self.store.update_message_content(row_id, &title);
            return;
        }

        match self
            .store
            .add_session_message(&self.session_id, MessageRole::ToolCall, &title)
        {
            Ok(id) => {
                log::info!(
                    "[msg-order] Inserted tool_call message id={} session={} tool_call_id={}",
                    id,
                    self.session_id,
                    tool_call_id
                );
                self.tool_call_rows
                    .lock()
                    .await
                    .insert(tool_call_id.to_string(), id);
            }
            Err(e) => log::error!("Failed to insert tool_call message: {e}"),
        }
    }

    /// Update a previously recorded tool call's title.
    pub async fn update_tool_call_title(&self, tool_call_id: &str, title: &str) {
        let title = sanitize_title(title);
        let rows = self.tool_call_rows.lock().await;
        if let Some(&row_id) = rows.get(tool_call_id) {
            let _ = self.store.update_message_content(row_id, &title);
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

        match self
            .store
            .add_session_message(&self.session_id, MessageRole::ToolResult, &content)
        {
            Ok(id) => {
                log::info!(
                    "[msg-order] Inserted tool_result message id={} session={}",
                    id,
                    self.session_id
                );
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
                        log::info!(
                            "[msg-order] Inserted assistant message id={} session={}",
                            id,
                            self.session_id
                        );
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

    async fn record_tool_call(&self, tool_call_id: &str, title: &str) {
        self.record_tool_call(tool_call_id, title).await
    }

    async fn update_tool_call_title(&self, tool_call_id: &str, title: &str) {
        self.update_tool_call_title(tool_call_id, title).await
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
        let writer = MessageWriter::new(session.id.clone(), Arc::clone(&store));
        (store, session.id, writer)
    }

    #[tokio::test]
    async fn record_tool_result_updates_existing_row_for_streaming_updates() {
        let (store, session_id, writer) = setup_writer();

        writer.record_tool_call("tc-1", "Run echo hello").await;
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

        writer.record_tool_call("tc-dup", "Run first title").await;
        writer.record_tool_call("tc-dup", "Run updated title").await;

        let messages = store
            .get_session_messages(&session_id)
            .expect("query messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, MessageRole::ToolCall);
        assert_eq!(messages[0].content, "Run updated title");
    }
}
