//! Branch CRUD operations.

use rusqlite::{params, Connection, OptionalExtension};

use super::models::{Branch, BranchType, WorkspaceStatus};
use super::{now_timestamp, Store, StoreChange, StoreError};

/// The PR fields that carry domain state — everything
/// [`Store::update_branch_pr_status`] writes except the `pr_fetched_at` /
/// `updated_at` timestamps, which advance on every poll.
#[derive(PartialEq)]
struct PrStatusFields {
    state: Option<String>,
    checks_status: Option<String>,
    review_decision: Option<String>,
    mergeable: Option<bool>,
    draft: Option<bool>,
    url: Option<String>,
    updated_at: Option<i64>,
    head_sha: Option<String>,
}

impl Store {
    pub fn create_branch(&self, branch: &Branch) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        if let Err(e) = conn.execute(
            "INSERT INTO branches (id, project_id, project_repo_id, branch_name, base_branch, pr_number,
                branch_type, workspace_name, workspace_status,
                pr_state, pr_checks_status, pr_review_decision, pr_mergeable, pr_draft,
                pr_url, pr_updated_at, pr_fetched_at, pr_head_sha, setup_complete, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
            params![
                branch.id,
                branch.project_id,
                branch.project_repo_id,
                branch.branch_name,
                branch.base_branch,
                branch.pr_number.map(|n| n as i64),
                branch.branch_type.as_str(),
                branch.workspace_name,
                branch.workspace_status.as_ref().map(|s| s.as_str()),
                branch.pr_state,
                branch.pr_checks_status,
                branch.pr_review_decision,
                branch.pr_mergeable.map(|b| if b { 1 } else { 0 }),
                branch.pr_draft.map(|b| if b { 1 } else { 0 }),
                branch.pr_url,
                branch.pr_updated_at,
                branch.pr_fetched_at,
                branch.pr_head_sha,
                if branch.setup_complete { 1 } else { 0 },
                branch.created_at,
                branch.updated_at,
            ],
        ) {
            if is_duplicate_branch_error(&e) {
                let scope = if branch.project_repo_id.is_some() {
                    "for this repository in the project"
                } else {
                    "for this project"
                };
                return Err(StoreError(format!(
                    "Branch '{}' is already tracked {}.",
                    branch.branch_name, scope
                )));
            }
            return Err(e.into());
        }
        self.publish(StoreChange::Branch {
            branch_id: branch.id.clone(),
            project_id: Some(branch.project_id.clone()),
        });
        Ok(())
    }

    pub fn get_branch(&self, id: &str) -> Result<Option<Branch>, StoreError> {
        let conn = self.conn.lock().unwrap();
        Self::row_to_branch_query(
            &conn,
            "SELECT id, project_id, project_repo_id, branch_name, base_branch, pr_number,
                    branch_type, workspace_name, workspace_status,
                    pr_state, pr_checks_status, pr_review_decision, pr_mergeable, pr_draft,
                    pr_url, pr_updated_at, pr_fetched_at, pr_head_sha, setup_complete,
                    created_at, updated_at
             FROM branches WHERE id = ?1",
            params![id],
        )
    }

    pub fn list_branches_for_project(&self, project_id: &str) -> Result<Vec<Branch>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, project_repo_id, branch_name, base_branch, pr_number,
                    branch_type, workspace_name, workspace_status,
                    pr_state, pr_checks_status, pr_review_decision, pr_mergeable, pr_draft,
                    pr_url, pr_updated_at, pr_fetched_at, pr_head_sha, setup_complete,
                    created_at, updated_at
             FROM branches WHERE project_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![project_id], Self::row_to_branch)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_branch_ids(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM branches")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_branch_base(&self, id: &str, base_branch: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE branches SET base_branch = ?1, updated_at = ?2 WHERE id = ?3",
            params![base_branch, now_timestamp(), id],
        )?;
        self.publish_with(|| StoreChange::Branch {
            branch_id: id.to_string(),
            project_id: Self::branch_project_id(&conn, id),
        });
        Ok(())
    }

    pub fn update_branch_name(&self, id: &str, branch_name: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE branches SET branch_name = ?1, updated_at = ?2 WHERE id = ?3",
            params![branch_name, now_timestamp(), id],
        )?;
        self.publish_with(|| StoreChange::Branch {
            branch_id: id.to_string(),
            project_id: Self::branch_project_id(&conn, id),
        });
        Ok(())
    }

    /// Update the workspace status for a remote branch.
    ///
    /// The Blox poller calls this for every active workspace on every cycle, so
    /// the common case is rewriting the value already stored. The `IS NOT` guard
    /// makes the statement its own change detector: an unchanged status matches
    /// no row, so nothing is written and nothing is published. `rows > 0`
    /// therefore means "the status actually moved" — the same shape as
    /// [`Store::mark_branch_setup_complete`], and the same contract as the
    /// compare-before-publish in [`Store::update_branch_pr_status`]. (Unlike
    /// that method the write itself is skippable: the only column a no-op would
    /// touch is `updated_at`, which nothing reads.)
    pub fn update_branch_workspace_status(
        &self,
        id: &str,
        status: &WorkspaceStatus,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE branches SET workspace_status = ?1, updated_at = ?2
             WHERE id = ?3 AND workspace_status IS NOT ?1",
            params![status.as_str(), now_timestamp(), id],
        )?;
        if rows > 0 {
            self.publish_with(|| StoreChange::Branch {
                branch_id: id.to_string(),
                project_id: Self::branch_project_id(&conn, id),
            });
        }
        Ok(())
    }

    /// Update workspace status for all branches sharing a given workspace name.
    /// Returns the IDs of all branches on the workspace, moved or not — both
    /// callers read an empty vec as "no such workspace" and surface an error, so
    /// resuming a workspace whose branches already hold `status` must not come
    /// back empty. Only the branches whose status actually moved publish, for
    /// the reason spelled out on [`Store::update_branch_workspace_status`].
    pub fn update_workspace_status_by_workspace_name(
        &self,
        workspace_name: &str,
        status: &WorkspaceStatus,
    ) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();
        // Snapshotted before the write; the set is identical either side of it
        // (the connection lock is held throughout and the UPDATE touches
        // neither `workspace_name` nor row existence).
        let mut stmt =
            conn.prepare("SELECT id, workspace_status FROM branches WHERE workspace_name = ?1")?;
        let previous: Vec<(String, Option<String>)> = stmt
            .query_map(params![workspace_name], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        conn.execute(
            "UPDATE branches SET workspace_status = ?1, updated_at = ?2
             WHERE workspace_name = ?3 AND workspace_status IS NOT ?1",
            params![status.as_str(), now, workspace_name],
        )?;
        for (id, prev) in &previous {
            if prev.as_deref() != Some(status.as_str()) {
                self.publish_with(|| StoreChange::Branch {
                    branch_id: id.clone(),
                    project_id: Self::branch_project_id(&conn, id),
                });
            }
        }
        Ok(previous.into_iter().map(|(id, _)| id).collect())
    }

    /// Update the PR number for a branch.
    pub fn update_branch_pr_number(
        &self,
        id: &str,
        pr_number: Option<u64>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE branches SET pr_number = ?1, updated_at = ?2 WHERE id = ?3",
            params![pr_number.map(|n| n as i64), now_timestamp(), id],
        )?;
        self.publish_with(|| StoreChange::Branch {
            branch_id: id.to_string(),
            project_id: Self::branch_project_id(&conn, id),
        });
        Ok(())
    }

    /// Update PR status fields for a branch.
    ///
    /// The PR poll scheduler calls this after *every* `gh` fetch, so the write
    /// itself is unconditional — `pr_fetched_at` and `updated_at` always
    /// advance — but the change feed only speaks up when the eight domain
    /// fields actually move. A timestamp-only refresh is deliberately silent:
    /// publishing it would drop every window's timeline and diff caches for
    /// every branch with a PR, once per poll cycle. Freshness reaches the UI
    /// through the cheap in-place `pr-status-changed` event instead.
    #[allow(clippy::too_many_arguments)]
    pub fn update_branch_pr_status(
        &self,
        id: &str,
        pr_state: Option<String>,
        pr_checks_status: Option<String>,
        pr_review_decision: Option<String>,
        pr_mergeable: Option<bool>,
        pr_draft: Option<bool>,
        pr_url: Option<String>,
        pr_updated_at: Option<i64>,
        pr_head_sha: Option<String>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();
        let incoming = PrStatusFields {
            state: pr_state,
            checks_status: pr_checks_status,
            review_decision: pr_review_decision,
            mergeable: pr_mergeable,
            draft: pr_draft,
            url: pr_url,
            updated_at: pr_updated_at,
            head_sha: pr_head_sha,
        };
        // Snapshot the domain fields before the write. `None` means the row is
        // gone or unreadable, which compares as "changed" — the `rows > 0`
        // guard below is what suppresses the publish for a missing branch, so
        // a failed read never swallows a real change.
        let previous = Self::read_pr_status_fields(&conn, id);
        let rows = conn.execute(
            "UPDATE branches SET
                pr_state = ?1,
                pr_checks_status = ?2,
                pr_review_decision = ?3,
                pr_mergeable = ?4,
                pr_draft = ?5,
                pr_url = ?6,
                pr_updated_at = ?7,
                pr_fetched_at = ?8,
                pr_head_sha = ?9,
                updated_at = ?10
             WHERE id = ?11",
            params![
                incoming.state,
                incoming.checks_status,
                incoming.review_decision,
                incoming.mergeable.map(|b| if b { 1 } else { 0 }),
                incoming.draft.map(|b| if b { 1 } else { 0 }),
                incoming.url,
                incoming.updated_at,
                now,
                incoming.head_sha,
                now,
                id
            ],
        )?;
        if rows > 0 && previous.as_ref() != Some(&incoming) {
            self.publish_with(|| StoreChange::Branch {
                branch_id: id.to_string(),
                project_id: Self::branch_project_id(&conn, id),
            });
        }
        Ok(())
    }

    /// Read the PR fields that carry domain state, for the
    /// compare-before-publish in [`Store::update_branch_pr_status`].
    /// `None` on a missing row or a read error.
    fn read_pr_status_fields(conn: &Connection, id: &str) -> Option<PrStatusFields> {
        conn.query_row(
            "SELECT pr_state, pr_checks_status, pr_review_decision, pr_mergeable,
                    pr_draft, pr_url, pr_updated_at, pr_head_sha
             FROM branches WHERE id = ?1",
            params![id],
            |row| {
                Ok(PrStatusFields {
                    state: row.get(0)?,
                    checks_status: row.get(1)?,
                    review_decision: row.get(2)?,
                    mergeable: row.get::<_, Option<i64>>(3)?.map(|b| b != 0),
                    draft: row.get::<_, Option<i64>>(4)?.map(|b| b != 0),
                    url: row.get(5)?,
                    updated_at: row.get(6)?,
                    head_sha: row.get(7)?,
                })
            },
        )
        .ok()
    }

    /// Atomically mark a branch as having completed its initial setup (worktree
    /// created and prerun actions have had the opportunity to run).
    ///
    /// Returns `true` if the flag was actually flipped (i.e. this caller
    /// "won" the race), `false` if the branch was already marked complete.
    /// Callers should only run prerun actions when this returns `true`.
    pub fn mark_branch_setup_complete(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE branches SET setup_complete = 1, updated_at = ?1 WHERE id = ?2 AND setup_complete = 0",
            params![now_timestamp(), id],
        )?;
        if rows > 0 {
            self.publish_with(|| StoreChange::Branch {
                branch_id: id.to_string(),
                project_id: Self::branch_project_id(&conn, id),
            });
        }
        Ok(rows > 0)
    }

    pub fn delete_branch(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        // Resolved before the row disappears, published only if the delete lands.
        // A no-op delete (two windows racing to remove the same branch) would
        // otherwise publish `project_id: None`, which the frontend reads as its
        // widest tier: drop every cached branch list and refetch in every window.
        let project_id = Self::branch_project_id(&conn, id);
        let rows = conn.execute("DELETE FROM branches WHERE id = ?1", params![id])?;
        if rows > 0 {
            self.publish(StoreChange::Branch {
                branch_id: id.to_string(),
                project_id,
            });
        }
        Ok(())
    }

    fn row_to_branch(row: &rusqlite::Row) -> rusqlite::Result<Branch> {
        let pr_number: Option<i64> = row.get(5)?;
        let branch_type_str: String = row.get(6)?;
        let workspace_status_str: Option<String> = row.get(8)?;
        let pr_mergeable: Option<i64> = row.get(12)?;
        let pr_draft: Option<i64> = row.get(13)?;
        let setup_complete: i64 = row.get(18)?;
        Ok(Branch {
            id: row.get(0)?,
            project_id: row.get(1)?,
            project_repo_id: row.get(2)?,
            branch_name: row.get(3)?,
            base_branch: row.get(4)?,
            pr_number: pr_number.map(|n| n as u64),
            branch_type: branch_type_str.parse().unwrap_or(BranchType::Local),
            workspace_name: row.get(7)?,
            workspace_status: workspace_status_str.and_then(|s| s.parse().ok()),
            pr_state: row.get(9)?,
            pr_checks_status: row.get(10)?,
            pr_review_decision: row.get(11)?,
            pr_mergeable: pr_mergeable.map(|b| b != 0),
            pr_draft: pr_draft.map(|b| b != 0),
            pr_url: row.get(14)?,
            pr_updated_at: row.get(15)?,
            pr_fetched_at: row.get(16)?,
            pr_head_sha: row.get(17)?,
            setup_complete: setup_complete != 0,
            created_at: row.get(19)?,
            updated_at: row.get(20)?,
        })
    }

    fn row_to_branch_query(
        conn: &Connection,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Option<Branch>, StoreError> {
        conn.query_row(sql, params, Self::row_to_branch)
            .optional()
            .map_err(Into::into)
    }
}

fn is_duplicate_branch_error(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(_, Some(msg)) => {
            msg.contains("UNIQUE constraint failed")
                && msg.contains("branches.project_id")
                && msg.contains("branches.project_repo_id")
                && msg.contains("branches.branch_name")
        }
        _ => false,
    }
}
