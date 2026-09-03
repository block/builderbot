//! Agent abstraction layer.
//!
//! This module provides adapters between Staged's storage layer and the
//! acp-client crate's generic interfaces.

use std::collections::{HashMap, HashSet};

pub mod writer;

pub use acp_client::{discover_providers, AcpDriver, AcpProviderInfo, AgentDriver};

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
    let duplicate_projection_message_ids = acp_message_projection_duplicate_ids(&messages);
    let mut boundaries: Vec<acp_client::ReplayBoundary> = Vec::new();
    let mut message_id_indexes: HashMap<String, usize> = HashMap::new();

    for message in messages {
        if duplicate_projection_message_ids.contains(&message.id) {
            continue;
        }

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

#[derive(Debug)]
struct AcpMessageChunkProjection {
    acp_message_id: String,
    role: String,
    content: String,
    first_chunk_row_id: i64,
    last_chunk_row_id: i64,
}

fn acp_message_projection_duplicate_ids(messages: &[SessionMessage]) -> HashSet<i64> {
    let projections = acp_message_chunk_projections(messages);
    let mut duplicate_ids = HashSet::new();

    for projection in projections {
        if let Some(id) =
            find_visible_projection_duplicate_id(messages, &projection, &duplicate_ids)
        {
            duplicate_ids.insert(id);
        }
    }

    duplicate_ids
}

fn acp_message_chunk_projections(messages: &[SessionMessage]) -> Vec<AcpMessageChunkProjection> {
    let mut projections: Vec<AcpMessageChunkProjection> = Vec::new();
    let mut projection_indexes: HashMap<String, usize> = HashMap::new();

    for message in messages
        .iter()
        .filter(|message| is_acp_message_chunk_with_id(message))
    {
        let message_id = message.acp.acp_message_id.clone().unwrap_or_default();
        let content = acp_chunk_text(message).unwrap_or_default();
        if let Some(index) = projection_indexes.get(&message_id).copied() {
            projections[index].content.push_str(&content);
            projections[index].last_chunk_row_id = message.id;
        } else {
            projection_indexes.insert(message_id.clone(), projections.len());
            projections.push(AcpMessageChunkProjection {
                acp_message_id: message_id,
                role: message.role.as_str().to_string(),
                content,
                first_chunk_row_id: message.id,
                last_chunk_row_id: message.id,
            });
        }
    }

    projections
}

fn find_visible_projection_duplicate_id(
    messages: &[SessionMessage],
    projection: &AcpMessageChunkProjection,
    used_message_ids: &HashSet<i64>,
) -> Option<i64> {
    if let Some(message) = messages.iter().find(|message| {
        is_visible_assistant_or_user(message)
            && !used_message_ids.contains(&message.id)
            && message.role.as_str() == projection.role.as_str()
            && message.acp.acp_message_id.as_deref() == Some(projection.acp_message_id.as_str())
    }) {
        return Some(message.id);
    }

    if projection.content.is_empty() {
        return None;
    }

    find_visible_projection_duplicate_by_content(messages, projection, used_message_ids)
}

fn find_visible_projection_duplicate_by_content(
    messages: &[SessionMessage],
    projection: &AcpMessageChunkProjection,
    used_message_ids: &HashSet<i64>,
) -> Option<i64> {
    let candidates = messages.iter().filter(|message| {
        is_visible_assistant_or_user(message)
            && !used_message_ids.contains(&message.id)
            && message.role.as_str() == projection.role.as_str()
            && message.content == projection.content
    });

    let mut inside_span: Option<&SessionMessage> = None;
    let mut preceding: Option<&SessionMessage> = None;
    let mut following: Option<&SessionMessage> = None;

    for message in candidates {
        if message.id < projection.first_chunk_row_id {
            if match preceding {
                Some(current) => message.id > current.id,
                None => true,
            } {
                preceding = Some(message);
            }
        } else if message.id <= projection.last_chunk_row_id {
            if match inside_span {
                Some(current) => message.id < current.id,
                None => true,
            } {
                inside_span = Some(message);
            }
        } else if match following {
            Some(current) => message.id < current.id,
            None => true,
        } {
            following = Some(message);
        }
    }

    inside_span
        .or(preceding)
        .or(following)
        .map(|message| message.id)
}

fn is_visible_assistant_or_user(message: &SessionMessage) -> bool {
    matches!(message.role, MessageRole::Assistant | MessageRole::User)
        && !is_hidden_acp_metadata(message)
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

    fn acp_text_chunk(event_kind: &str, message_id: &str, text: &str) -> AcpMessageMetadata {
        AcpMessageMetadata {
            acp_event_kind: Some(event_kind.to_string()),
            acp_message_id: Some(message_id.to_string()),
            acp_content: Some(serde_json::json!({
                "content": {"type": "text", "text": text}
            })),
            ..Default::default()
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
                acp_text_chunk("agent_message_chunk", "msg-1", "hello "),
            ),
            message(
                3,
                MessageRole::Assistant,
                "",
                acp_text_chunk("agent_message_chunk", "msg-1", "world"),
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
    fn replay_boundaries_keep_later_visible_rows_with_repeated_chunk_content() {
        let boundaries = replay_boundaries_from_messages(vec![
            message(1, MessageRole::Assistant, "repeat", Default::default()),
            message(
                2,
                MessageRole::Assistant,
                "",
                acp_text_chunk("agent_message_chunk", "msg-1", "repeat"),
            ),
            message(3, MessageRole::Assistant, "repeat", Default::default()),
        ]);

        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].content, "repeat");
        assert_eq!(boundaries[0].acp_message_id.as_deref(), Some("msg-1"));
        assert_eq!(boundaries[1].content, "repeat");
        assert_eq!(boundaries[1].acp_message_id, None);
    }

    #[test]
    fn replay_boundaries_keep_earlier_visible_rows_with_repeated_chunk_content() {
        let boundaries = replay_boundaries_from_messages(vec![
            message(1, MessageRole::Assistant, "repeat", Default::default()),
            message(2, MessageRole::Assistant, "repeat", Default::default()),
            message(
                3,
                MessageRole::Assistant,
                "",
                acp_text_chunk("agent_message_chunk", "msg-1", "repeat"),
            ),
        ]);

        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].content, "repeat");
        assert_eq!(boundaries[0].acp_message_id, None);
        assert_eq!(boundaries[1].content, "repeat");
        assert_eq!(boundaries[1].acp_message_id.as_deref(), Some("msg-1"));
    }

    #[test]
    fn replay_boundaries_prefer_acp_message_id_for_visible_projection() {
        let boundaries = replay_boundaries_from_messages(vec![
            message(
                1,
                MessageRole::Assistant,
                "repeat",
                AcpMessageMetadata {
                    acp_message_id: Some("msg-1".to_string()),
                    ..Default::default()
                },
            ),
            message(2, MessageRole::Assistant, "repeat", Default::default()),
            message(
                3,
                MessageRole::Assistant,
                "",
                acp_text_chunk("agent_message_chunk", "msg-1", "repeat"),
            ),
        ]);

        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].content, "repeat");
        assert_eq!(boundaries[0].acp_message_id, None);
        assert_eq!(boundaries[1].content, "repeat");
        assert_eq!(boundaries[1].acp_message_id.as_deref(), Some("msg-1"));
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
                acp_text_chunk("agent_message_chunk", "msg-2", "new assistant text"),
            ),
        ]);

        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].content, "legacy assistant text");
        assert_eq!(boundaries[0].acp_message_id, None);
        assert_eq!(boundaries[1].content, "new assistant text");
        assert_eq!(boundaries[1].acp_message_id.as_deref(), Some("msg-2"));
    }

    #[test]
    fn replay_boundaries_keep_a_background_continuation_out_of_the_turn_it_followed() {
        // Text streamed while the session was held open for background work
        // lands on its own ACP message id, so resuming the session replays two
        // messages instead of one run-on assistant turn.
        let continuation_id = "background-continuation-1";
        let boundaries = replay_boundaries_from_messages(vec![
            message(
                1,
                MessageRole::Assistant,
                "build running",
                Default::default(),
            ),
            message(
                2,
                MessageRole::Assistant,
                "",
                acp_text_chunk("agent_message_chunk", "msg-1", "build running"),
            ),
            message(
                3,
                MessageRole::Assistant,
                "build finished",
                Default::default(),
            ),
            message(
                4,
                MessageRole::Assistant,
                "",
                AcpMessageMetadata {
                    acp_origin: Some("background-continuation:task-notification".to_string()),
                    ..acp_text_chunk("agent_message_chunk", continuation_id, "build finished")
                },
            ),
        ]);

        assert_eq!(
            boundaries.len(),
            2,
            "the continuation must not coalesce into the turn: {boundaries:?}"
        );
        assert_eq!(boundaries[0].content, "build running");
        assert_eq!(boundaries[0].acp_message_id.as_deref(), Some("msg-1"));
        assert_eq!(boundaries[1].content, "build finished");
        assert_eq!(
            boundaries[1].acp_message_id.as_deref(),
            Some(continuation_id)
        );
    }
}
