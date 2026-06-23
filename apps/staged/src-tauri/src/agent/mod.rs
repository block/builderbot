//! Agent abstraction layer.
//!
//! This module provides adapters between Staged's storage layer and the
//! acp-client crate's generic interfaces.

use std::collections::{HashMap, HashSet};

pub mod permissions;
pub mod writer;

pub use acp_client::{discover_providers, AcpDriver, AcpProviderInfo, AgentDriver};
pub use permissions::{PermissionDecision, PermissionRegistry};

use crate::store::{MessageRole, SessionMessage, Store};

// Implement the acp_client::Store trait for our Store
impl acp_client::Store for Store {
    fn set_agent_session_id(&self, session_id: &str, agent_session_id: &str) -> Result<(), String> {
        self.set_agent_session_id(session_id, agent_session_id)
            .map_err(|e| e.to_string())
    }

    fn get_session_messages(&self, session_id: &str) -> Result<Vec<(String, String)>, String> {
        self.get_session_messages(session_id)
            .map(|msgs| {
                msgs.into_iter()
                    .map(|m| (m.role.as_str().to_string(), m.content))
                    .collect()
            })
            .map_err(|e| e.to_string())
    }

    fn get_session_replay_boundaries(
        &self,
        session_id: &str,
    ) -> Result<Vec<acp_client::ReplayBoundary>, String> {
        self.get_session_replay_messages(session_id)
            .map(replay_boundaries_from_messages)
            .map_err(|e| e.to_string())
    }
}

// Re-export writer for backward compatibility
pub use writer::MessageWriter;

fn replay_boundaries_from_messages(
    messages: Vec<SessionMessage>,
) -> Vec<acp_client::ReplayBoundary> {
    let acp_message_chunk_keys = acp_message_chunk_content_keys(&messages);
    let mut boundaries: Vec<acp_client::ReplayBoundary> = Vec::new();
    let mut message_id_indexes: HashMap<String, usize> = HashMap::new();

    for message in messages {
        if is_acp_message_chunk_with_id(&message) {
            let message_id = message.acp.acp_message_id.clone().unwrap_or_default();
            let content = acp_chunk_text(&message).unwrap_or_default();
            if let Some(index) = message_id_indexes.get(&message_id).copied() {
                boundaries[index].content.push_str(&content);
            } else {
                let index = boundaries.len();
                message_id_indexes.insert(message_id.clone(), index);
                boundaries.push(acp_client::ReplayBoundary {
                    role: message.role.as_str().to_string(),
                    content,
                    acp_message_id: Some(message_id),
                    acp_tool_call_id: None,
                });
            }
            continue;
        }

        let is_visible_assistant_or_user =
            matches!(message.role, MessageRole::Assistant | MessageRole::User)
                && !is_hidden_acp_metadata(&message);
        let is_duplicate_acp_message_projection = is_visible_assistant_or_user
            && acp_message_chunk_keys
                .contains(&(message.role.as_str().to_string(), message.content.clone()));
        if is_duplicate_acp_message_projection {
            continue;
        }

        if is_hidden_acp_metadata(&message) {
            continue;
        }

        boundaries.push(acp_client::ReplayBoundary {
            role: message.role.as_str().to_string(),
            content: message.content,
            acp_message_id: message.acp.acp_message_id,
            acp_tool_call_id: message.acp.acp_tool_call_id,
        });
    }

    boundaries
}

fn acp_message_chunk_content_keys(messages: &[SessionMessage]) -> HashSet<(String, String)> {
    let mut content_by_id: HashMap<String, (String, String)> = HashMap::new();
    for message in messages
        .iter()
        .filter(|message| is_acp_message_chunk_with_id(message))
    {
        let message_id = message.acp.acp_message_id.clone().unwrap_or_default();
        let entry = content_by_id
            .entry(message_id)
            .or_insert_with(|| (message.role.as_str().to_string(), String::new()));
        entry
            .1
            .push_str(&acp_chunk_text(message).unwrap_or_default());
    }

    content_by_id
        .into_values()
        .filter(|(_, content)| !content.is_empty())
        .collect()
}

fn is_hidden_acp_metadata(message: &SessionMessage) -> bool {
    message.content.is_empty() && message.acp.acp_event_kind.is_some()
}

fn is_acp_message_chunk_with_id(message: &SessionMessage) -> bool {
    matches!(
        message.acp.acp_event_kind.as_deref(),
        Some("agent_message_chunk" | "user_message_chunk")
    ) && message.acp.acp_message_id.is_some()
}

fn acp_chunk_text(message: &SessionMessage) -> Option<String> {
    message
        .acp
        .acp_content
        .as_ref()?
        .get("content")?
        .get("text")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::AcpMessageMetadata;

    fn message(
        id: i64,
        role: MessageRole,
        content: &str,
        acp: AcpMessageMetadata,
    ) -> SessionMessage {
        SessionMessage {
            id,
            session_id: "session-1".to_string(),
            role,
            content: content.to_string(),
            created_at: id,
            image_ids: vec![],
            acp,
        }
    }

    #[test]
    fn replay_boundaries_coalesce_acp_message_chunks_by_message_id() {
        let boundaries = replay_boundaries_from_messages(vec![
            message(1, MessageRole::Assistant, "hello world", Default::default()),
            message(
                2,
                MessageRole::Assistant,
                "",
                AcpMessageMetadata {
                    acp_event_kind: Some("agent_message_chunk".to_string()),
                    acp_message_id: Some("msg-1".to_string()),
                    acp_content: Some(serde_json::json!({
                        "content": {"type": "text", "text": "hello "}
                    })),
                    ..Default::default()
                },
            ),
            message(
                3,
                MessageRole::Assistant,
                "",
                AcpMessageMetadata {
                    acp_event_kind: Some("agent_message_chunk".to_string()),
                    acp_message_id: Some("msg-1".to_string()),
                    acp_content: Some(serde_json::json!({
                        "content": {"type": "text", "text": "world"}
                    })),
                    ..Default::default()
                },
            ),
            message(
                4,
                MessageRole::ToolCall,
                "Read file",
                AcpMessageMetadata {
                    acp_tool_call_id: Some("tool-1".to_string()),
                    ..Default::default()
                },
            ),
        ]);

        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].role, "assistant");
        assert_eq!(boundaries[0].content, "hello world");
        assert_eq!(boundaries[0].acp_message_id.as_deref(), Some("msg-1"));
        assert_eq!(boundaries[1].role, "tool_call");
        assert_eq!(boundaries[1].acp_tool_call_id.as_deref(), Some("tool-1"));
    }

    #[test]
    fn replay_boundaries_keep_visible_rows_without_acp_message_ids() {
        let boundaries = replay_boundaries_from_messages(vec![message(
            1,
            MessageRole::Assistant,
            "legacy assistant text",
            Default::default(),
        )]);

        assert_eq!(boundaries.len(), 1);
        assert_eq!(boundaries[0].role, "assistant");
        assert_eq!(boundaries[0].content, "legacy assistant text");
        assert_eq!(boundaries[0].acp_message_id, None);
    }

    #[test]
    fn replay_boundaries_keep_legacy_rows_in_mixed_sessions() {
        let boundaries = replay_boundaries_from_messages(vec![
            message(
                1,
                MessageRole::Assistant,
                "legacy assistant text",
                Default::default(),
            ),
            message(
                2,
                MessageRole::Assistant,
                "",
                AcpMessageMetadata {
                    acp_event_kind: Some("agent_message_chunk".to_string()),
                    acp_message_id: Some("msg-2".to_string()),
                    acp_content: Some(serde_json::json!({
                        "content": {"type": "text", "text": "new assistant text"}
                    })),
                    ..Default::default()
                },
            ),
        ]);

        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].content, "legacy assistant text");
        assert_eq!(boundaries[0].acp_message_id, None);
        assert_eq!(boundaries[1].content, "new assistant text");
        assert_eq!(boundaries[1].acp_message_id.as_deref(), Some("msg-2"));
    }
}
