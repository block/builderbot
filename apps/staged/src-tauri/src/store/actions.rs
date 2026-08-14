//! Repo-context action CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::{ActionContext, ActionType, RepoAction, RepoContextActions, RunDetectionMode};
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn get_or_create_action_context(
        &self,
        github_repo: &str,
        subpath: Option<&str>,
    ) -> Result<ActionContext, StoreError> {
        if let Some(existing) = self.get_action_context_by_repo_and_subpath(github_repo, subpath)? {
            return Ok(existing);
        }

        let context = ActionContext::new(github_repo.to_string(), subpath.map(str::to_string));
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO action_contexts (id, github_repo, subpath, has_detected_actions, detecting_actions, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                context.id,
                context.github_repo,
                context.subpath,
                context.has_detected_actions as i32,
                context.detecting_actions as i32,
                context.created_at,
                context.updated_at,
            ],
        )?;
        Ok(context)
    }

    pub fn list_action_contexts(&self) -> Result<Vec<ActionContext>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, github_repo, subpath, has_detected_actions, detecting_actions, created_at, updated_at
             FROM action_contexts
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], Self::row_to_action_context)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_action_context(&self, id: &str) -> Result<Option<ActionContext>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, github_repo, subpath, has_detected_actions, detecting_actions, created_at, updated_at
             FROM action_contexts WHERE id = ?1",
            params![id],
            Self::row_to_action_context,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn count_action_contexts_for_repo(&self, github_repo: &str) -> Result<i64, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM action_contexts WHERE github_repo = ?1",
            params![github_repo],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    pub fn get_action_context_by_repo_and_subpath(
        &self,
        github_repo: &str,
        subpath: Option<&str>,
    ) -> Result<Option<ActionContext>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, github_repo, subpath, has_detected_actions, detecting_actions, created_at, updated_at
             FROM action_contexts
             WHERE github_repo = ?1 AND subpath IS ?2",
            params![github_repo, subpath],
            Self::row_to_action_context,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Close a detection window without marking the context detected.
    ///
    /// The fallback for a failed [`Store::mark_action_context_detected`]: a
    /// flag left set rejects every later detection for the context. Only the
    /// claim opens a window, so there is no "set detecting" counterpart — the
    /// owner pid has to be recorded with it.
    pub fn clear_action_context_detection(&self, context_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE action_contexts
             SET detecting_actions = 0, detecting_pid = NULL, updated_at = ?1
             WHERE id = ?2",
            params![now_timestamp(), context_id],
        )?;
        Ok(())
    }

    /// Atomically claim the detection window for a context on behalf of `pid`.
    ///
    /// Returns `true` when this caller flipped `detecting_actions` from unset
    /// to set (i.e. it "won" and owns the window), `false` when detection was
    /// already in progress — or when the context no longer exists. Reading the
    /// flag and then setting it in two calls lets two racing callers both pass
    /// the check and start concurrent AI detections; this is the one-statement
    /// version, mirroring [`Store::mark_branch_setup_complete`].
    ///
    /// `pid` is the claiming process, recorded so a window orphaned by a hard
    /// kill can be told apart from one another live Staged instance still owns
    /// — see [`Store::list_detecting_action_contexts`]. Callers pass
    /// `std::process::id()`; tests pass an arbitrary pid to stand in for
    /// another process.
    pub fn claim_action_context_detection(
        &self,
        context_id: &str,
        pid: u32,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE action_contexts
             SET detecting_actions = 1, detecting_pid = ?1, updated_at = ?2
             WHERE id = ?3 AND detecting_actions = 0",
            params![pid, now_timestamp(), context_id],
        )?;
        Ok(rows > 0)
    }

    pub fn mark_action_context_detected(&self, context_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE action_contexts
             SET has_detected_actions = 1, detecting_actions = 0, detecting_pid = NULL, updated_at = ?1
             WHERE id = ?2",
            params![now_timestamp(), context_id],
        )?;
        Ok(())
    }

    /// Claim a detection window the way a build predating `detecting_pid` did:
    /// the flag set with no owner recorded. Only the sweep's tests need one.
    #[cfg(test)]
    pub(crate) fn claim_action_context_detection_without_owner(
        &self,
        context_id: &str,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE action_contexts
             SET detecting_actions = 1, detecting_pid = NULL, updated_at = ?1
             WHERE id = ?2",
            params![now_timestamp(), context_id],
        )?;
        Ok(())
    }

    /// Every context currently holding a detection window, with the pid that
    /// claimed it (`None` for rows written before `detecting_pid` existed).
    ///
    /// Only the startup sweep reads the owner pid, so it stays off
    /// [`ActionContext`] and the six SELECT column lists that build it.
    pub fn list_detecting_action_contexts(&self) -> Result<Vec<(String, Option<u32>)>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, detecting_pid FROM action_contexts
             WHERE detecting_actions = 1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Release a detection window claimed by `expected_pid`, returning whether
    /// a row matched.
    ///
    /// The expectation lives in the WHERE clause so the sweep's read-then-write
    /// is safe: if the claim changed hands in between — the window closed and
    /// another process opened a new one — this matches zero rows instead of
    /// clobbering a live window. `IS` rather than `=` so a NULL `expected_pid`
    /// matches the pre-migration rows it stands for.
    pub fn release_detection_claim(
        &self,
        context_id: &str,
        expected_pid: Option<u32>,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE action_contexts
             SET detecting_actions = 0, detecting_pid = NULL, updated_at = ?1
             WHERE id = ?2 AND detecting_actions = 1 AND detecting_pid IS ?3",
            params![now_timestamp(), context_id, expected_pid],
        )?;
        Ok(rows > 0)
    }

    /// Move a detection window claimed by `expected_pid` to `new_pid` without
    /// ever dropping the flag, returning whether a row matched.
    ///
    /// What a waiter that finds an orphaned window uses instead of
    /// [`Store::release_detection_claim`] plus a fresh
    /// [`Store::claim_action_context_detection`]: those two statements leave a
    /// gap with the flag unset, which is exactly the read-then-write race the
    /// claim was collapsed into one UPDATE to close. In that gap another waiter
    /// reads "no window open" and takes the context's half-written action list
    /// for final, and anyone claiming there sends the taker-over back an
    /// already-in-progress rejection for a window it thought it had won. Here
    /// `detecting_actions` stays 1 throughout and only the owner changes, so
    /// every concurrent reader sees a window that was open the whole time.
    ///
    /// The expectation is in the WHERE clause for the same reason it is on the
    /// release: a claim that changed hands between the read and this call
    /// matches zero rows rather than being stolen from its new owner. `IS`
    /// rather than `=` so a NULL `expected_pid` matches the pre-migration rows
    /// it stands for.
    pub fn take_over_detection_claim(
        &self,
        context_id: &str,
        expected_pid: Option<u32>,
        new_pid: u32,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE action_contexts
             SET detecting_pid = ?1, updated_at = ?2
             WHERE id = ?3 AND detecting_actions = 1 AND detecting_pid IS ?4",
            params![new_pid, now_timestamp(), context_id, expected_pid],
        )?;
        Ok(rows > 0)
    }

    pub fn create_repo_action(&self, action: &RepoAction) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let run_detection_mode_json = action
            .run_detection_mode
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| StoreError(format!("Failed to serialize run_detection_mode: {e}")))?;
        conn.execute(
            "INSERT INTO repo_actions (id, context_id, name, command, action_type, sort_order, auto_commit, run_detection_mode, pinned, icon, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                action.id,
                action.context_id,
                action.name,
                action.command,
                action.action_type.as_str(),
                action.sort_order,
                action.auto_commit as i32,
                run_detection_mode_json,
                action.pinned as i32,
                action.icon,
                action.created_at,
                action.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_repo_action(&self, id: &str) -> Result<Option<RepoAction>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, context_id, name, command, action_type, sort_order, auto_commit, run_detection_mode, pinned, icon, created_at, updated_at
             FROM repo_actions WHERE id = ?1",
            params![id],
            Self::row_to_repo_action,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_repo_actions(&self, context_id: &str) -> Result<Vec<RepoAction>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, context_id, name, command, action_type, sort_order, auto_commit, run_detection_mode, pinned, icon, created_at, updated_at
             FROM repo_actions WHERE context_id = ?1 ORDER BY sort_order ASC",
        )?;
        let rows = stmt.query_map(params![context_id], Self::row_to_repo_action)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Every context's actions in a single query, grouped per context.
    ///
    /// Deliberately read-only: surfaces that render one card per repo use this
    /// instead of a `get_or_create_action_context` lookup per card, so merely
    /// rendering them can't insert context rows. A repo absent from the result
    /// has no context yet, which callers treat as an empty action list.
    pub fn list_all_repo_actions(&self) -> Result<Vec<RepoContextActions>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT a.id, a.context_id, a.name, a.command, a.action_type, a.sort_order, a.auto_commit, a.run_detection_mode, a.pinned, a.icon, a.created_at, a.updated_at, c.github_repo, c.subpath
             FROM repo_actions a
             JOIN action_contexts c ON a.context_id = c.id
             ORDER BY c.id ASC, a.sort_order ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let action = Self::row_to_repo_action(row)?;
            let github_repo: String = row.get(12)?;
            let subpath: Option<String> = row.get(13)?;
            Ok((github_repo, subpath, action))
        })?;

        let mut grouped: Vec<RepoContextActions> = Vec::new();
        let mut current_context: Option<String> = None;
        for row in rows {
            let (github_repo, subpath, action) = row?;
            // Ordered by context id, so one context's actions arrive contiguously.
            if current_context.as_deref() != Some(action.context_id.as_str()) {
                current_context = Some(action.context_id.clone());
                grouped.push(RepoContextActions {
                    github_repo,
                    subpath,
                    actions: Vec::new(),
                });
            }
            grouped
                .last_mut()
                .expect("a context group was just pushed")
                .actions
                .push(action);
        }
        Ok(grouped)
    }

    pub fn update_repo_action(&self, action: &RepoAction) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let run_detection_mode_json = action
            .run_detection_mode
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| StoreError(format!("Failed to serialize run_detection_mode: {e}")))?;
        conn.execute(
            "UPDATE repo_actions SET name = ?1, command = ?2, action_type = ?3, sort_order = ?4, auto_commit = ?5, run_detection_mode = ?6, pinned = ?7, icon = ?8, updated_at = ?9 WHERE id = ?10",
            params![
                action.name,
                action.command,
                action.action_type.as_str(),
                action.sort_order,
                action.auto_commit as i32,
                run_detection_mode_json,
                action.pinned as i32,
                action.icon,
                now_timestamp(),
                action.id,
            ],
        )?;
        Ok(())
    }

    pub fn delete_repo_action(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM repo_actions WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn delete_all_repo_actions(&self, context_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM repo_actions WHERE context_id = ?1",
            params![context_id],
        )?;
        Ok(())
    }

    pub fn delete_action_context(&self, context_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM action_contexts WHERE id = ?1",
            params![context_id],
        )?;
        Ok(())
    }

    pub fn reorder_repo_actions(&self, action_ids: &[String]) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();
        for (i, id) in action_ids.iter().enumerate() {
            conn.execute(
                "UPDATE repo_actions SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
                params![i as i32, now, id],
            )?;
        }
        Ok(())
    }

    fn row_to_action_context(row: &rusqlite::Row) -> rusqlite::Result<ActionContext> {
        let has_detected_actions: i32 = row.get(3)?;
        let detecting_actions: i32 = row.get(4)?;
        Ok(ActionContext {
            id: row.get(0)?,
            github_repo: row.get(1)?,
            subpath: row.get(2)?,
            has_detected_actions: has_detected_actions != 0,
            detecting_actions: detecting_actions != 0,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    }

    fn row_to_repo_action(row: &rusqlite::Row) -> rusqlite::Result<RepoAction> {
        let action_type_str: String = row.get(4)?;
        let auto_commit: i32 = row.get(6)?;
        let run_detection_mode_str: Option<String> = row.get(7)?;
        let run_detection_mode: Option<RunDetectionMode> = run_detection_mode_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        let pinned: i32 = row.get(8)?;
        Ok(RepoAction {
            id: row.get(0)?,
            context_id: row.get(1)?,
            name: row.get(2)?,
            command: row.get(3)?,
            action_type: ActionType::parse(&action_type_str).unwrap_or(ActionType::Run),
            sort_order: row.get(5)?,
            auto_commit: auto_commit != 0,
            run_detection_mode,
            pinned: pinned != 0,
            icon: row.get(9)?,
            created_at: row.get(10)?,
            updated_at: row.get(11)?,
        })
    }
}
