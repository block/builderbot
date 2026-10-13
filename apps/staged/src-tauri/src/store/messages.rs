//! Session message CRUD operations.

use rusqlite::{params, Row};

use super::models::{AcpMessageMetadata, MessageRole, SessionMessage};
use super::tool_images::project_tool_images;
use super::{now_timestamp, Store, StoreError};

const SESSION_MESSAGE_COLUMNS: &str = "id, session_id, role, content, created_at, image_ids,
    acp_event_kind, acp_protocol_version, acp_agent_capabilities, acp_auth_methods,
    acp_agent_info, acp_message_id, acp_tool_call_id, acp_tool_kind, acp_tool_status,
    acp_raw_input, acp_raw_output, acp_content, acp_locations, acp_usage,
    acp_session_info, acp_config_options, acp_session_mode_state, acp_origin";
const VISIBLE_MESSAGE_FILTER: &str = "NOT (content = '' AND acp_event_kind IS NOT NULL)";
/// ACP rows the transcript needs on top of [`Store::get_session_messages`]:
/// hidden rows of the kinds the frontend reads (`acpTranscript.ts`,
/// `acpConfigMetadata.ts`), plus visible tool rows still in flight — the
/// writer mutates those in place, so the poll re-reads them by id until they
/// settle. Everything else (`agent_message_chunk`, `usage_update`,
/// `session_info_update`, settled visible tool rows, …) stays in the table
/// for replay but is not shipped to the UI; chunk rows alone are ~90% of a
/// long session's ACP rows, and settled tool rows duplicate the visible
/// transcript byte for byte.
const TRANSCRIPT_METADATA_FILTER: &str = "(
    (content = '' AND acp_event_kind IN (
        'tool_call', 'tool_call_update', 'plan_update', 'available_commands_update',
        'config_options_update', 'pikchr_session_started'))
    OR (content != '' AND acp_event_kind IN ('tool_call', 'tool_call_update')
        AND COALESCE(acp_tool_status, '') NOT IN (
            'completed', 'succeeded', 'success', 'failed', 'error', 'cancelled', 'canceled'))
)";

/// Which slice of the visible transcript [`Store::get_session_messages_window`]
/// returns. `limit` keeps only the newest rows (still delivered oldest first);
/// `before_id` restricts the query to rows older than a loaded one, which is
/// how the UI pages backwards from the window it opened with. Both unset is
/// the whole transcript.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TranscriptWindow {
    pub limit: Option<u32>,
    pub before_id: Option<i64>,
}

impl TranscriptWindow {
    pub const ALL: Self = Self {
        limit: None,
        before_id: None,
    };
}

/// Parse a JSON array string into a Vec<String>, returning an empty vec on
/// NULL or invalid JSON.
pub(super) fn parse_image_ids(raw: Option<String>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

fn parse_json_value(raw: Option<String>) -> Option<serde_json::Value> {
    raw.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
}

fn json_column(value: Option<&serde_json::Value>) -> Option<String> {
    value.and_then(|v| serde_json::to_string(v).ok())
}

pub(super) fn image_ids_json(image_ids: &[String]) -> Option<String> {
    if image_ids.is_empty() {
        None
    } else {
        Some(serde_json::to_string(image_ids).unwrap())
    }
}

/// The visible-transcript query for a [`TranscriptWindow`]; positional
/// placeholders in the order [`session_messages_window_params`] binds them.
pub(super) fn session_messages_window_sql(window: &TranscriptWindow) -> String {
    let before_filter = if window.before_id.is_some() {
        " AND id < ?"
    } else {
        ""
    };
    let (order, limit_clause) = if window.limit.is_some() {
        ("DESC", " LIMIT ?")
    } else {
        ("ASC", "")
    };
    format!(
        "SELECT {SESSION_MESSAGE_COLUMNS}
         FROM session_messages
         WHERE session_id = ? AND {VISIBLE_MESSAGE_FILTER}{before_filter}
         ORDER BY id {order}{limit_clause}"
    )
}

pub(super) fn session_messages_window_params<'a>(
    session_id: &'a &str,
    window: &'a TranscriptWindow,
) -> Vec<&'a dyn rusqlite::ToSql> {
    let mut query_params: Vec<&dyn rusqlite::ToSql> = vec![session_id];
    if let Some(before_id) = &window.before_id {
        query_params.push(before_id);
    }
    if let Some(limit) = &window.limit {
        query_params.push(limit);
    }
    query_params
}

fn session_message_from_row(row: &Row<'_>) -> rusqlite::Result<SessionMessage> {
    let role_str: String = row.get(2)?;
    let image_ids_raw: Option<String> = row.get(5)?;
    Ok(SessionMessage {
        id: row.get(0)?,
        session_id: row.get(1)?,
        role: MessageRole::parse(&role_str).unwrap_or(MessageRole::User),
        content: row.get(3)?,
        created_at: row.get(4)?,
        image_ids: parse_image_ids(image_ids_raw),
        acp: AcpMessageMetadata {
            acp_event_kind: row.get(6)?,
            acp_protocol_version: row.get(7)?,
            acp_agent_capabilities: parse_json_value(row.get(8)?),
            acp_auth_methods: parse_json_value(row.get(9)?),
            acp_agent_info: parse_json_value(row.get(10)?),
            acp_message_id: row.get(11)?,
            acp_tool_call_id: row.get(12)?,
            acp_tool_kind: row.get(13)?,
            acp_tool_status: row.get(14)?,
            acp_raw_input: parse_json_value(row.get(15)?),
            acp_raw_output: parse_json_value(row.get(16)?),
            acp_content: parse_json_value(row.get(17)?),
            acp_locations: parse_json_value(row.get(18)?),
            acp_usage: parse_json_value(row.get(19)?),
            acp_session_info: parse_json_value(row.get(20)?),
            acp_config_options: parse_json_value(row.get(21)?),
            acp_session_mode_state: parse_json_value(row.get(22)?),
            acp_origin: row.get(23)?,
        },
    })
}

/// [`session_message_from_row`] for the queries that feed the UI: base64
/// image blocks in the ACP output/content become `/api/tool-images` refs
/// (see `tool_images.rs`). Replay, archive and initialization readers keep
/// the raw row.
fn transcript_message_from_row(row: &Row<'_>) -> rusqlite::Result<SessionMessage> {
    let mut message = session_message_from_row(row)?;
    project_tool_images(&mut message);
    Ok(message)
}

impl Store {
    pub fn add_session_message(
        &self,
        session_id: &str,
        role: MessageRole,
        content: &str,
    ) -> Result<i64, StoreError> {
        self.add_session_message_with_images(session_id, role, content, &[])
    }

    /// Insert a session message, optionally recording attached image IDs.
    ///
    /// Image IDs are stored as a JSON array string. An empty slice results
    /// in NULL (no image_ids column value).
    pub fn add_session_message_with_images(
        &self,
        session_id: &str,
        role: MessageRole,
        content: &str,
        image_ids: &[String],
    ) -> Result<i64, StoreError> {
        let conn = self.conn.lock().unwrap();
        let image_ids_json = image_ids_json(image_ids);
        conn.execute(
            "INSERT INTO session_messages (session_id, role, content, created_at, image_ids)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session_id,
                role.as_str(),
                content,
                now_timestamp(),
                image_ids_json
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Insert a visible message that Staged itself authored, tagged with the
    /// ACP event kind that explains where it came from.
    ///
    /// Unlike [`Self::add_acp_metadata_message`] the row keeps its content, so
    /// it renders as ordinary transcript prose. The event kind is what lets
    /// replay-boundary construction tell it apart from text the agent actually
    /// produced — see `agent::replay_boundaries_from_messages`.
    pub fn add_authored_session_message(
        &self,
        session_id: &str,
        role: MessageRole,
        content: &str,
        event_kind: &str,
    ) -> Result<i64, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO session_messages (session_id, role, content, created_at, acp_event_kind)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session_id,
                role.as_str(),
                content,
                now_timestamp(),
                event_kind
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Insert a user message and atomically mark a claimed queued follow-up as sent.
    pub fn add_session_message_with_images_from_queue(
        &self,
        session_id: &str,
        role: MessageRole,
        content: &str,
        image_ids: &[String],
        queued_message_id: &str,
    ) -> Result<i64, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let image_ids_json = image_ids_json(image_ids);
        tx.execute(
            "INSERT INTO session_messages (session_id, role, content, created_at, image_ids)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session_id,
                role.as_str(),
                content,
                now_timestamp(),
                image_ids_json
            ],
        )?;
        let message_id = tx.last_insert_rowid();
        let now = now_timestamp();
        let rows = tx.execute(
            "UPDATE queued_session_messages
             SET status = 'sent',
                 sent_message_id = ?1,
                 last_error = NULL,
                 updated_at = ?2
             WHERE id = ?3
               AND session_id = ?4
               AND status = 'sending'
               AND sent_message_id IS NULL",
            params![message_id, now, queued_message_id, session_id],
        )?;
        if rows != 1 {
            return Err(StoreError(format!(
                "Queued message is no longer claimed: {queued_message_id}"
            )));
        }
        tx.commit()?;
        Ok(message_id)
    }

    /// The visible transcript, with tool-result images projected to refs.
    pub fn get_session_messages(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionMessage>, StoreError> {
        self.get_session_messages_window(session_id, TranscriptWindow::ALL)
    }

    /// [`Self::get_session_messages`] restricted to a [`TranscriptWindow`].
    /// Rows come back in ascending id order whatever the window; a `limit`
    /// is applied from the newest row backwards.
    ///
    /// The limited query reads `idx_session_messages_session` in descending
    /// rowid order and the result is reversed here rather than in SQL: a
    /// subquery with an outer `ORDER BY id ASC` would sort through a temp
    /// B-tree, while the index already yields the newest-first order for
    /// free (pinned by `test_session_messages_window_query_plan`).
    pub fn get_session_messages_window(
        &self,
        session_id: &str,
        window: TranscriptWindow,
    ) -> Result<Vec<SessionMessage>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&session_messages_window_sql(&window))?;
        let rows = stmt.query_map(
            &session_messages_window_params(&session_id, &window)[..],
            transcript_message_from_row,
        )?;
        let mut messages = rows.collect::<Result<Vec<_>, _>>()?;
        if window.limit.is_some() {
            messages.reverse();
        }
        Ok(messages)
    }

    /// Return visible transcript rows plus hidden ACP rows that can identify
    /// replay boundaries. This keeps resume matching on `session_messages`
    /// without exposing metadata-only rows to the legacy transcript path.
    pub fn get_session_replay_messages(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionMessage>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {SESSION_MESSAGE_COLUMNS}
             FROM session_messages
             WHERE session_id = ?1
               AND ({VISIBLE_MESSAGE_FILTER}
                    OR acp_message_id IS NOT NULL
                    OR acp_tool_call_id IS NOT NULL)
             ORDER BY id ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![session_id], session_message_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Update the content of an existing message (used for streaming updates).
    pub fn update_message_content(&self, id: i64, content: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE session_messages SET content = ?1 WHERE id = ?2",
            params![content, id],
        )?;
        Ok(())
    }

    /// Attach ACP metadata to an existing session message without changing its
    /// legacy transcript projection. `None` fields preserve existing values.
    pub fn update_message_acp_metadata(
        &self,
        id: i64,
        metadata: &AcpMessageMetadata,
    ) -> Result<(), StoreError> {
        let raw_input = json_column(metadata.acp_raw_input.as_ref());
        let raw_output = json_column(metadata.acp_raw_output.as_ref());
        let content = json_column(metadata.acp_content.as_ref());
        let locations = json_column(metadata.acp_locations.as_ref());
        let usage = json_column(metadata.acp_usage.as_ref());
        let agent_capabilities = json_column(metadata.acp_agent_capabilities.as_ref());
        let auth_methods = json_column(metadata.acp_auth_methods.as_ref());
        let agent_info = json_column(metadata.acp_agent_info.as_ref());
        let session_info = json_column(metadata.acp_session_info.as_ref());
        let config_options = json_column(metadata.acp_config_options.as_ref());
        let session_mode_state = json_column(metadata.acp_session_mode_state.as_ref());

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE session_messages
             SET acp_event_kind = COALESCE(?1, acp_event_kind),
                 acp_protocol_version = COALESCE(?2, acp_protocol_version),
                 acp_agent_capabilities = COALESCE(?3, acp_agent_capabilities),
                 acp_auth_methods = COALESCE(?4, acp_auth_methods),
                 acp_agent_info = COALESCE(?5, acp_agent_info),
                 acp_message_id = COALESCE(?6, acp_message_id),
                 acp_tool_call_id = COALESCE(?7, acp_tool_call_id),
                 acp_tool_kind = COALESCE(?8, acp_tool_kind),
                 acp_tool_status = COALESCE(?9, acp_tool_status),
                 acp_raw_input = COALESCE(?10, acp_raw_input),
                 acp_raw_output = COALESCE(?11, acp_raw_output),
                 acp_content = COALESCE(?12, acp_content),
                 acp_locations = COALESCE(?13, acp_locations),
                 acp_usage = COALESCE(?14, acp_usage),
                 acp_session_info = COALESCE(?15, acp_session_info),
                 acp_config_options = COALESCE(?16, acp_config_options),
                 acp_session_mode_state = COALESCE(?17, acp_session_mode_state),
                 acp_origin = COALESCE(?18, acp_origin)
             WHERE id = ?19",
            params![
                metadata.acp_event_kind.as_deref(),
                metadata.acp_protocol_version.as_deref(),
                agent_capabilities,
                auth_methods,
                agent_info,
                metadata.acp_message_id.as_deref(),
                metadata.acp_tool_call_id.as_deref(),
                metadata.acp_tool_kind.as_deref(),
                metadata.acp_tool_status.as_deref(),
                raw_input,
                raw_output,
                content,
                locations,
                usage,
                session_info,
                config_options,
                session_mode_state,
                metadata.acp_origin.as_deref(),
                id
            ],
        )?;
        Ok(())
    }

    /// Insert an ACP metadata-only row. Existing transcript reads hide these
    /// rows while keeping the raw ACP information in `session_messages`.
    pub fn add_acp_metadata_message(
        &self,
        session_id: &str,
        metadata: &AcpMessageMetadata,
    ) -> Result<i64, StoreError> {
        self.add_acp_metadata_message_with_role(session_id, MessageRole::Assistant, metadata)
    }

    /// Insert an ACP metadata-only row with an explicit role. This is used for
    /// hidden ACP events whose source role matters, such as user message chunks.
    pub fn add_acp_metadata_message_with_role(
        &self,
        session_id: &str,
        role: MessageRole,
        metadata: &AcpMessageMetadata,
    ) -> Result<i64, StoreError> {
        let raw_input = json_column(metadata.acp_raw_input.as_ref());
        let raw_output = json_column(metadata.acp_raw_output.as_ref());
        let content = json_column(metadata.acp_content.as_ref());
        let locations = json_column(metadata.acp_locations.as_ref());
        let usage = json_column(metadata.acp_usage.as_ref());
        let agent_capabilities = json_column(metadata.acp_agent_capabilities.as_ref());
        let auth_methods = json_column(metadata.acp_auth_methods.as_ref());
        let agent_info = json_column(metadata.acp_agent_info.as_ref());
        let session_info = json_column(metadata.acp_session_info.as_ref());
        let config_options = json_column(metadata.acp_config_options.as_ref());
        let session_mode_state = json_column(metadata.acp_session_mode_state.as_ref());

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO session_messages (
                 session_id, role, content, created_at, image_ids,
                 acp_event_kind, acp_protocol_version, acp_agent_capabilities,
                 acp_auth_methods, acp_agent_info, acp_message_id,
                 acp_tool_call_id, acp_tool_kind, acp_tool_status,
                 acp_raw_input, acp_raw_output, acp_content, acp_locations,
                 acp_usage, acp_session_info, acp_config_options,
                 acp_session_mode_state, acp_origin
             )
             VALUES (?1, ?2, '', ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
            params![
                session_id,
                role.as_str(),
                now_timestamp(),
                metadata.acp_event_kind.as_deref(),
                metadata.acp_protocol_version.as_deref(),
                agent_capabilities,
                auth_methods,
                agent_info,
                metadata.acp_message_id.as_deref(),
                metadata.acp_tool_call_id.as_deref(),
                metadata.acp_tool_kind.as_deref(),
                metadata.acp_tool_status.as_deref(),
                raw_input,
                raw_output,
                content,
                locations,
                usage,
                session_info,
                config_options,
                session_mode_state,
                metadata.acp_origin.as_deref()
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Return the ACP metadata rows the transcript needs beyond
    /// [`Self::get_session_messages`]; see [`TRANSCRIPT_METADATA_FILTER`].
    pub fn get_session_acp_metadata_messages(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionMessage>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {SESSION_MESSAGE_COLUMNS}
             FROM session_messages
             WHERE session_id = ?1 AND {TRANSCRIPT_METADATA_FILTER}
             ORDER BY id ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![session_id], transcript_message_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Every row carrying ACP metadata, unfiltered — the replay archive the
    /// writer persists. Tests use it to assert on rows the transcript
    /// projection deliberately leaves out.
    #[cfg(test)]
    pub fn get_session_acp_metadata_archive(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionMessage>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {SESSION_MESSAGE_COLUMNS}
             FROM session_messages
             WHERE session_id = ?1 AND acp_event_kind IS NOT NULL
             ORDER BY id ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![session_id], session_message_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Incremental variant of [`Self::get_session_acp_metadata_messages`]:
    /// returns metadata rows with `id > since_id`, plus fresh copies of
    /// `refetch_ids`. Rows for tool calls that have not reached a terminal
    /// status are updated in place by the message writer rather than
    /// re-inserted, so callers pass those ids to observe the mutations.
    /// Re-fetched rows bypass the transcript filter so a caller sees the row
    /// settle (and stops asking for it) instead of it silently vanishing.
    pub fn get_session_acp_metadata_messages_since(
        &self,
        session_id: &str,
        since_id: i64,
        refetch_ids: &[i64],
    ) -> Result<Vec<SessionMessage>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let refetch_placeholders = refetch_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let id_filter = if refetch_ids.is_empty() {
            format!("(id > ? AND {TRANSCRIPT_METADATA_FILTER})")
        } else {
            format!("((id > ? AND {TRANSCRIPT_METADATA_FILTER}) OR id IN ({refetch_placeholders}))")
        };
        let sql = format!(
            "SELECT {SESSION_MESSAGE_COLUMNS}
             FROM session_messages
             WHERE session_id = ? AND acp_event_kind IS NOT NULL AND {id_filter}
             ORDER BY id ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut query_params: Vec<&dyn rusqlite::ToSql> = vec![&session_id, &since_id];
        for id in refetch_ids {
            query_params.push(id);
        }
        let rows = stmt.query_map(&query_params[..], transcript_message_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Return the latest ACP initialization metadata row for a session.
    ///
    /// This exposes negotiated provider/protocol/capability data without
    /// including metadata-only rows in the legacy transcript projection.
    pub fn get_session_acp_initialization(
        &self,
        session_id: &str,
    ) -> Result<Option<AcpMessageMetadata>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {SESSION_MESSAGE_COLUMNS}
             FROM session_messages
             WHERE session_id = ?1 AND acp_event_kind = 'initialize'
             ORDER BY id DESC
             LIMIT 1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params![session_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(session_message_from_row(row)?.acp)),
            None => Ok(None),
        }
    }

    /// Count assistant messages created after a given timestamp.
    pub fn count_assistant_messages_after(
        &self,
        session_id: &str,
        after_timestamp: i64,
    ) -> Result<i64, StoreError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM session_messages
             WHERE session_id = ?1
               AND role = 'assistant'
               AND created_at > ?2
               AND NOT (content = '' AND acp_event_kind IS NOT NULL)",
            params![session_id, after_timestamp],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Get messages with id >= since_id, plus fresh copies of `refetch_ids`.
    ///
    /// The inclusive `>=` re-fetches the last known message so the caller
    /// picks up streaming content updates to the tail. `refetch_ids` covers an
    /// assistant row the message writer is still extending behind later
    /// tool-call rows (a mid-message tool call interrupts but does not close
    /// it), which the tail re-fetch can no longer reach.
    pub fn get_session_messages_since(
        &self,
        session_id: &str,
        since_id: i64,
        refetch_ids: &[i64],
    ) -> Result<Vec<SessionMessage>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let refetch_placeholders = refetch_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let id_filter = if refetch_ids.is_empty() {
            "id >= ?".to_string()
        } else {
            format!("(id >= ? OR id IN ({refetch_placeholders}))")
        };
        let sql = format!(
            "SELECT {SESSION_MESSAGE_COLUMNS}
             FROM session_messages
             WHERE session_id = ?
               AND {id_filter}
               AND {VISIBLE_MESSAGE_FILTER}
             ORDER BY id ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut query_params: Vec<&dyn rusqlite::ToSql> = vec![&session_id, &since_id];
        for id in refetch_ids {
            query_params.push(id);
        }
        let rows = stmt.query_map(&query_params[..], transcript_message_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}
