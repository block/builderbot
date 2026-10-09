//! Successful SDK response boundaries, independent of prompt settlement and idle.
use super::*;

/// Transient wire state only; persisted artifacts remain the writer's concern.
#[derive(Default)]
pub(super) struct ResponseState {
    session_id: Option<String>,
    pub(super) text: String,
    /// Provider ACP message id of the chunk last appended to `text`.
    pub(super) message_id: Option<String>,
    /// A tool call arrived since `text` was last extended (even when it is
    /// empty); the next chunk either resumes that message (same id) or starts
    /// a new one.
    pub(super) interrupted: bool,
    pub(super) completed: bool,
    fallback: String,
    seen_results: HashSet<String>,
}

fn successful_result(message: &serde_json::Value) -> bool {
    message["type"] == "result"
        && message["subtype"] == "success"
        && message["is_error"] == false
        && message["num_turns"].as_u64().is_some_and(|turns| turns > 0)
        && (matches!(
            message.get("stop_reason"),
            None | Some(serde_json::Value::Null)
        ) || message["stop_reason"] == "end_turn")
}

impl ResponseState {
    pub(super) fn start_prompt(&mut self, session_id: String) {
        self.session_id = Some(session_id);
        self.reset_output();
    }

    pub(super) fn reset_output(&mut self) {
        self.close_message();
        self.completed = false;
        self.fallback.clear();
    }

    /// Forget the streamed message: the writer has closed (or will close) it.
    pub(super) fn close_message(&mut self) {
        self.text.clear();
        self.message_id = None;
        self.interrupted = false;
    }

    pub(super) fn suppress_fallback(&mut self, text: &str) -> bool {
        if !self.fallback.is_empty() && self.fallback.starts_with(text) {
            self.fallback.drain(..text.len());
            return true;
        }
        self.fallback.clear();
        false
    }

    pub(super) async fn accept_result(
        &mut self,
        handler: &AcpNotificationHandler,
        params: &serde_json::Value,
    ) -> agent_client_protocol::Result<()> {
        let message = &params["message"];
        if message["type"] != "result" {
            return Ok(());
        }
        // Remember replayed results too, so they cannot later accept live text.
        let Some(id) = message["uuid"].as_str() else {
            return Ok(());
        };
        if !self.seen_results.insert(id.to_string())
            || self.session_id.as_deref() != params["sessionId"].as_str()
            || self.session_id.is_none()
            || !message["parent_tool_use_id"].is_null()
            || !matches!(
                &*handler.phase.lock().await,
                HandlerPhase::Live { .. } | HandlerPhase::BackgroundHolding { .. }
            )
            || self.completed
        {
            return Ok(());
        }
        let successful = successful_result(message);
        let final_text = message["result"].as_str().unwrap_or_default();
        // A result is the final assistant text, not necessarily all text in the
        // turn. Require agreement with streamed output before accepting it.
        let (missing, extends_buffer) = if final_text.is_empty() || self.text.ends_with(final_text)
        {
            ("", false)
        } else if let Some(suffix) = final_text.strip_prefix(&self.text) {
            (suffix, true)
        } else if self.interrupted {
            // A tool call ended the buffered message and nothing came after
            // it; the result is text that was never streamed, as when the
            // buffer is empty.
            (final_text, false)
        } else {
            // This boundary cannot certify the buffered text. Retire it so a
            // later result with no intervening output cannot accept it either.
            self.completed = true;
            return Ok(());
        };
        if successful && !missing.is_empty() {
            let mut chunk = ContentChunk::new(AcpContentBlock::Text(TextContent::new(missing)));
            // A suffix continues the buffered message, so it carries that
            // message's id: if a tool call interrupted it, the chunk resumes
            // the same row instead of opening a new one.
            if extends_buffer {
                chunk.message_id = self.message_id.clone().map(Into::into);
            }
            handler
                .session_notification_inner(
                    SessionNotification::new(
                        self.session_id.clone().expect("active prompt"),
                        SessionUpdate::AgentMessageChunk(chunk),
                    ),
                    self,
                )
                .await?;
            // The bridge forwards raw results before its own fallback chunk.
            self.fallback = missing.to_string();
        }
        handler
            .writer
            .record_acp_event_metadata(AcpEventMetadata {
                event_kind: Some("response_result".into()),
                usage: message.get("usage").cloned(),
                content: Some(message.clone()),
                origin: handler.out_of_turn_origin().await,
                ..Default::default()
            })
            .await;
        self.completed = true;
        if successful && !final_text.is_empty() && !self.text.is_empty() {
            handler.finish_completed_response().await;
        }
        Ok(())
    }
}

impl AcpNotificationHandler {
    pub(super) async fn wait_for_response_finish(&self) {
        // Subscribe before reading the latch: completion between a check and
        // subscription must not strand a still-pending session/prompt request.
        let mut activity = self.subscribe_background_activity();
        loop {
            if activity.borrow_and_update().finish_requested {
                return;
            }
            if activity.changed().await.is_err() {
                return;
            }
        }
    }
}
