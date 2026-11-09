//! Persistent follow-up message queue operations.

use rusqlite::{params, Connection, OptionalExtension};

use super::messages::{image_ids_json, parse_image_ids};
use super::models::{QueuedSessionMessage, QueuedSessionMessageStatus, SessionStatus};
use super::{now_timestamp, Store, StoreError};

impl Store {
    /// Queue a follow-up for a `running` session, to go once its turn ends.
    pub fn add_queued_session_message(
        &self,
        session_id: &str,
        content: &str,
        image_ids: &[String],
        branch_id: Option<&str>,
    ) -> Result<QueuedSessionMessage, StoreError> {
        self.insert_queued_session_message(session_id, content, image_ids, branch_id, true)
    }

    /// Queue a follow-up for a session whatever its status: one on a branch
    /// whose project is being transferred, which can't be resumed until the
    /// transfer is over and is then drained like any other queued follow-up
    /// (see `session_queue::follow_ups`).
    pub fn add_queued_session_message_held_for_transfer(
        &self,
        session_id: &str,
        content: &str,
        image_ids: &[String],
        branch_id: Option<&str>,
    ) -> Result<QueuedSessionMessage, StoreError> {
        self.insert_queued_session_message(session_id, content, image_ids, branch_id, false)
    }

    fn insert_queued_session_message(
        &self,
        session_id: &str,
        content: &str,
        image_ids: &[String],
        branch_id: Option<&str>,
        require_running: bool,
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
                 WHERE id = ?2 AND (status = ?13 OR NOT ?14)
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
                require_running,
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
        Self::list_queued_session_messages_with_conn(&conn, session_id)
    }

    /// [`Self::list_queued_session_messages`] on a connection the caller
    /// already holds (see `poll_session`).
    pub(super) fn list_queued_session_messages_with_conn(
        conn: &Connection,
        session_id: &str,
    ) -> Result<Vec<QueuedSessionMessage>, StoreError> {
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

    /// The sessions on `branch_id` that are not running (nor `disconnected`)
    /// and have a follow-up queued: what a drain can send now. Oldest
    /// follow-up first.
    pub fn session_ids_with_queued_messages_for_branch(
        &self,
        branch_id: &str,
    ) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT q.session_id
             FROM queued_session_messages q
             INNER JOIN sessions s ON s.id = q.session_id
             WHERE s.branch_id = ?1
               AND q.status = 'queued'
               AND q.sent_message_id IS NULL
               AND s.status NOT IN ('running', 'disconnected')
             GROUP BY q.session_id
             ORDER BY MIN(q.created_at) ASC, q.session_id ASC",
        )?;
        let ids = stmt
            .query_map(params![branch_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    /// How many queued follow-ups the sessions on `branch_id` hold.
    pub fn count_queued_session_messages_for_branch(
        &self,
        branch_id: &str,
    ) -> Result<usize, StoreError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM queued_session_messages q
             INNER JOIN sessions s ON s.id = q.session_id
             WHERE s.branch_id = ?1 AND q.status = 'queued' AND q.sent_message_id IS NULL",
            params![branch_id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
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
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let Some(message) = select_queued_session_message(&tx, id)? else {
            tx.commit()?;
            return Ok(false);
        };
        if message.status != QueuedSessionMessageStatus::Queued {
            tx.commit()?;
            return Ok(false);
        }

        let rows = tx.execute(
            "DELETE FROM queued_session_messages
             WHERE id = ?1 AND status = 'queued'",
            params![id],
        )?;
        if rows == 0 {
            tx.commit()?;
            return Ok(false);
        }

        for image_id in &message.image_ids {
            tx.execute(
                "UPDATE images
                 SET session_id = NULL
                 WHERE id = ?1 AND session_id = ?2",
                params![image_id, message.session_id],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// Claim a queued follow-up for sending. Only a session that is not
    /// running can take one — and a `disconnected` session counts as running,
    /// since its remote turn may still be going and a resume would race it.
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
                     AND s.status NOT IN ('running', 'disconnected')
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
                   AND s.status NOT IN ('running', 'disconnected')
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
