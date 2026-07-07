//! Persistent follow-up message queue operations.

use rusqlite::{params, OptionalExtension};

use super::messages::{image_ids_json, parse_image_ids};
use super::models::{QueuedSessionMessage, QueuedSessionMessageStatus, SessionStatus};
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn add_queued_session_message(
        &self,
        session_id: &str,
        content: &str,
        image_ids: &[String],
        branch_id: Option<&str>,
    ) -> Result<QueuedSessionMessage, StoreError> {
        let message = QueuedSessionMessage::new(session_id, branch_id, content, image_ids);
        let image_ids_json = image_ids_json(image_ids);
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let rows = tx.execute(
            "INSERT INTO queued_session_messages (
                id, session_id, branch_id, content, image_ids, status, last_error,
                created_at, updated_at, claimed_at, owner_pid, sent_message_id
             )
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12
             WHERE EXISTS (
                 SELECT 1 FROM sessions
                 WHERE id = ?2 AND status = ?13
             )",
            params![
                message.id,
                message.session_id,
                message.branch_id,
                message.content,
                image_ids_json,
                message.status.as_str(),
                message.last_error,
                message.created_at,
                message.updated_at,
                message.claimed_at,
                message.owner_pid,
                message.sent_message_id,
                SessionStatus::Running.as_str(),
            ],
        )?;
        if rows == 0 {
            let status = tx
                .query_row(
                    "SELECT status FROM sessions WHERE id = ?1",
                    params![session_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            return match status {
                Some(_) => Err(StoreError("Session is not running".to_string())),
                None => Err(StoreError(format!("Session not found: {session_id}"))),
            };
        }
        for image_id in image_ids {
            tx.execute(
                "UPDATE images SET session_id = ?1 WHERE id = ?2",
                params![session_id, image_id],
            )?;
        }
        tx.commit()?;
        Ok(message)
    }

    pub fn list_queued_session_messages(
        &self,
        session_id: &str,
    ) -> Result<Vec<QueuedSessionMessage>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, branch_id, content, image_ids, status, last_error,
                    created_at, updated_at, claimed_at, owner_pid, sent_message_id
             FROM queued_session_messages
             WHERE session_id = ?1 AND status != 'sent'
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![session_id], Self::row_to_queued_session_message)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_sending_queued_session_messages(
        &self,
    ) -> Result<Vec<QueuedSessionMessage>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, branch_id, content, image_ids, status, last_error,
                    created_at, updated_at, claimed_at, owner_pid, sent_message_id
             FROM queued_session_messages
             WHERE status = 'sending' AND sent_message_id IS NULL
             ORDER BY claimed_at ASC, created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], Self::row_to_queued_session_message)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn delete_queued_session_message(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM queued_session_messages
             WHERE id = ?1 AND status = 'queued'",
            params![id],
        )?;
        Ok(rows > 0)
    }

    pub fn claim_queued_session_message(
        &self,
        id: &str,
    ) -> Result<Option<QueuedSessionMessage>, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let now = now_timestamp();
        let rows = tx.execute(
            "UPDATE queued_session_messages
             SET status = 'sending',
                 claimed_at = ?1,
                 owner_pid = ?2,
                 updated_at = ?1,
                 last_error = NULL
             WHERE id = ?3
               AND status = 'queued'
               AND sent_message_id IS NULL
               AND EXISTS (
                   SELECT 1 FROM sessions s
                   WHERE s.id = queued_session_messages.session_id
                     AND s.status != 'running'
               )",
            params![now, std::process::id(), id],
        )?;
        if rows == 0 {
            tx.commit()?;
            return Ok(None);
        }
        let message = select_queued_session_message(&tx, id)?;
        tx.commit()?;
        Ok(message)
    }

    pub fn claim_oldest_queued_session_message(
        &self,
        session_id: &str,
    ) -> Result<Option<QueuedSessionMessage>, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let now = now_timestamp();
        let id = tx
            .query_row(
                "SELECT q.id
                 FROM queued_session_messages q
                 INNER JOIN sessions s ON s.id = q.session_id
                 WHERE q.session_id = ?1
                   AND q.status = 'queued'
                   AND q.sent_message_id IS NULL
                   AND s.status != 'running'
                 ORDER BY q.created_at ASC, q.id ASC
                 LIMIT 1",
                params![session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(id) = id else {
            tx.commit()?;
            return Ok(None);
        };
        let rows = tx.execute(
            "UPDATE queued_session_messages
             SET status = 'sending',
                 claimed_at = ?1,
                 owner_pid = ?2,
                 updated_at = ?1,
                 last_error = NULL
             WHERE id = ?3
               AND status = 'queued'
               AND sent_message_id IS NULL",
            params![now, std::process::id(), id],
        )?;
        if rows == 0 {
            tx.commit()?;
            return Ok(None);
        }
        let message = select_queued_session_message(&tx, &id)?;
        tx.commit()?;
        Ok(message)
    }

    pub fn mark_queued_session_message_sent(
        &self,
        id: &str,
        sent_message_id: i64,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE queued_session_messages
             SET status = 'sent',
                 sent_message_id = ?1,
                 last_error = NULL,
                 updated_at = ?2
             WHERE id = ?3
               AND status = 'sending'
               AND sent_message_id IS NULL",
            params![sent_message_id, now_timestamp(), id],
        )?;
        Ok(rows > 0)
    }

    pub fn release_queued_session_message(
        &self,
        id: &str,
        error: Option<&str>,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE queued_session_messages
             SET status = 'queued',
                 last_error = ?1,
                 claimed_at = NULL,
                 owner_pid = NULL,
                 updated_at = ?2
             WHERE id = ?3
               AND status = 'sending'
               AND sent_message_id IS NULL",
            params![error, now_timestamp(), id],
        )?;
        Ok(rows > 0)
    }

    fn row_to_queued_session_message(
        row: &rusqlite::Row,
    ) -> rusqlite::Result<QueuedSessionMessage> {
        let status: String = row.get(5)?;
        let image_ids: Option<String> = row.get(4)?;
        Ok(QueuedSessionMessage {
            id: row.get(0)?,
            session_id: row.get(1)?,
            branch_id: row.get(2)?,
            content: row.get(3)?,
            image_ids: parse_image_ids(image_ids),
            status: QueuedSessionMessageStatus::parse(&status)
                .unwrap_or(QueuedSessionMessageStatus::Queued),
            last_error: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
            claimed_at: row.get(9)?,
            owner_pid: row.get(10)?,
            sent_message_id: row.get(11)?,
        })
    }
}

fn select_queued_session_message(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
) -> Result<Option<QueuedSessionMessage>, StoreError> {
    tx.query_row(
        "SELECT id, session_id, branch_id, content, image_ids, status, last_error,
                created_at, updated_at, claimed_at, owner_pid, sent_message_id
         FROM queued_session_messages
         WHERE id = ?1",
        params![id],
        Store::row_to_queued_session_message,
    )
    .optional()
    .map_err(Into::into)
}
