//! The summary read behind the session stream.
//!
//! A row on the project detail or the branch timeline shows a running
//! session's title and a one-line subtitle. The subtitle is derived in the
//! frontend (`deriveHint` in `liveSessionHints.ts`) from a few session fields
//! and the newest visible rows of the transcript, and of those rows it reads
//! only the id, the role, the attachment ids and, for some roles, the text.
//! [`Store::read_session_summary`] reads exactly that, under one lock and one
//! read transaction, so the session fields and the rows are one snapshot.
//!
//! The read does a column projection and nothing else: no text is processed
//! here, so the display rules stay in the frontend alone.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::messages::{parse_image_ids, VISIBLE_MESSAGE_FILTER};
use super::models::{MessageRole, PipelineExecution, SessionStatus};
use super::{Store, StoreError};

/// One visible transcript row, as far as the subtitle reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryRow {
    pub id: i64,
    pub role: MessageRole,
    /// Left out for `tool_result` rows and for the opening prompt: the
    /// subtitle never reads either text, and an opening prompt is the largest
    /// row of most transcripts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub image_ids: Vec<String>,
}

/// What a row showing a session needs of it: the fields behind its title and
/// status, and for the subtitle the working directory, the pipeline, the id
/// of the opening prompt and the transcript rows asked for.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub status: SessionStatus,
    /// `Session::acp_title`.
    pub acp_title: Option<String>,
    /// The session stream leaves this out at its title level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<PipelineExecution>,
    /// The first visible `user` row: the prompt that opened the session,
    /// which is not a reply. `None` until the session has one, and whenever
    /// the transcript was not read.
    pub opening_prompt_id: Option<i64>,
    /// In id order.
    pub rows: Vec<SummaryRow>,
}

/// Which transcript rows [`Store::read_session_summary`] reads: the newest
/// `newest` visible rows, plus the visible rows among `ids`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SummaryRowsQuery {
    pub newest: u32,
    pub ids: Vec<i64>,
    /// The opening prompt's id when the caller already has it. It is the
    /// first user row, so it does not change once it exists, and passing it
    /// saves the lookup.
    pub opening_prompt_id: Option<i64>,
}

impl Store {
    /// The summary of one session, or `None` when its row does not exist.
    ///
    /// `rows` is `None` to read the session fields alone, which leaves
    /// `opening_prompt_id` unset and `rows` empty.
    pub fn read_session_summary(
        &self,
        session_id: &str,
        rows: Option<&SummaryRowsQuery>,
    ) -> Result<Option<SessionSummary>, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        // As in `poll_session`: the lock excludes this process's writers, and
        // the read transaction extends the snapshot to any other process.
        let tx = conn.transaction()?;
        let fields: Option<(String, Option<String>, String, Option<String>)> = tx
            .query_row(
                "SELECT status, acp_title, working_dir, pipeline FROM sessions WHERE id = ?1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((status, acp_title, working_dir, pipeline_json)) = fields else {
            return Ok(None);
        };
        let mut summary = SessionSummary {
            // As `row_to_session` reads them: an unknown status ends the row,
            // and a pipeline that does not parse is no pipeline.
            status: SessionStatus::parse(&status).unwrap_or(SessionStatus::Error),
            acp_title,
            working_dir: Some(working_dir),
            pipeline: pipeline_json.as_deref().and_then(|json| {
                serde_json::from_str(json)
                    .map_err(|e| log::warn!("Failed to deserialize pipeline JSON: {e}"))
                    .ok()
            }),
            opening_prompt_id: None,
            rows: Vec::new(),
        };
        if let Some(query) = rows {
            summary.opening_prompt_id = match query.opening_prompt_id {
                Some(id) => Some(id),
                None => opening_prompt_id(&tx, session_id)?,
            };
            summary.rows = summary_rows(&tx, session_id, query, summary.opening_prompt_id)?;
        }
        tx.commit()?;
        Ok(Some(summary))
    }
}

/// The id of the first visible `user` row, which is what the subtitle poller
/// took for the opening prompt from the transcript it loaded.
fn opening_prompt_id(conn: &Connection, session_id: &str) -> Result<Option<i64>, StoreError> {
    conn.query_row(
        &format!(
            "SELECT id FROM session_messages
             WHERE session_id = ?1 AND role = 'user' AND {VISIBLE_MESSAGE_FILTER}
             ORDER BY id ASC LIMIT 1"
        ),
        params![session_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn summary_rows(
    conn: &Connection,
    session_id: &str,
    query: &SummaryRowsQuery,
    opening_prompt_id: Option<i64>,
) -> Result<Vec<SummaryRow>, StoreError> {
    // The two contents the subtitle never reads are dropped in the query, so
    // they are not read from the database either.
    let columns = "id, role,
        CASE WHEN role = 'tool_result' OR id IS ?1 THEN NULL ELSE content END,
        image_ids";
    let mut rows = Vec::new();
    if query.newest > 0 {
        let mut statement = conn.prepare(&format!(
            "SELECT {columns} FROM session_messages
             WHERE session_id = ?2 AND {VISIBLE_MESSAGE_FILTER}
             ORDER BY id DESC LIMIT ?3"
        ))?;
        let newest = statement.query_map(
            params![opening_prompt_id, session_id, query.newest],
            summary_row,
        )?;
        rows = newest.collect::<Result<Vec<_>, _>>()?;
    }
    // Only the ids the newest rows did not already cover. They are integers,
    // so they are written into the statement.
    let mut ids: Vec<i64> = query
        .ids
        .iter()
        .copied()
        .filter(|id| !rows.iter().any(|row| row.id == *id))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    if !ids.is_empty() {
        let id_list = ids
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let mut statement = conn.prepare(&format!(
            "SELECT {columns} FROM session_messages
             WHERE session_id = ?2 AND id IN ({id_list}) AND {VISIBLE_MESSAGE_FILTER}"
        ))?;
        let by_id = statement.query_map(params![opening_prompt_id, session_id], summary_row)?;
        for row in by_id {
            rows.push(row?);
        }
    }
    rows.sort_unstable_by_key(|row| row.id);
    Ok(rows)
}

fn summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SummaryRow> {
    let role: String = row.get(1)?;
    Ok(SummaryRow {
        id: row.get(0)?,
        // As `session_message_from_row` reads it.
        role: MessageRole::parse(&role).unwrap_or(MessageRole::User),
        content: row.get(2)?,
        image_ids: parse_image_ids(row.get(3)?),
    })
}
