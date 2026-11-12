//! Project tag assignments (`project_tags`, migration 0045).
//!
//! Single choice: a project holds at most one tag per group. There is no
//! denormalised `group_id` on `project_tags` (it would need reconciling every
//! time a tag moves group), so every write here enforces the rule itself, in
//! the same transaction, through [`assign_tag`].
//!
//! Both setters are idempotent and publish [`StoreChange::Tags`] only when a
//! row changed.
//!
//! [`StoreChange::Tags`]: super::StoreChange::Tags

use rusqlite::{params, Connection};

use super::tags::{find_group, find_tag};
use super::{now_timestamp, Store, StoreError};

pub(super) fn ensure_project(conn: &Connection, project_id: &str) -> Result<(), StoreError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
        [project_id],
        |row| row.get(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(StoreError(format!("Project not found: {project_id}")))
    }
}

/// Give a project a tag. For a grouped tag (`group_id`), first drop the
/// project's other tag in that group. Returns whether a row changed.
pub(super) fn assign_tag(
    conn: &Connection,
    project_id: &str,
    tag_id: &str,
    group_id: Option<&str>,
    now: i64,
) -> Result<bool, StoreError> {
    let mut changed = false;
    if let Some(group_id) = group_id {
        changed |= conn.execute(
            "DELETE FROM project_tags
             WHERE project_id = ?1
               AND tag_id != ?2
               AND tag_id IN (SELECT id FROM tags WHERE group_id = ?3)",
            params![project_id, tag_id, group_id],
        )? > 0;
    }
    changed |= conn.execute(
        "INSERT OR IGNORE INTO project_tags (project_id, tag_id, sort_order, created_at)
         VALUES (?1, ?2, NULL, ?3)",
        params![project_id, tag_id, now],
    )? > 0;
    Ok(changed)
}

impl Store {
    /// Set a project's tag in `group_id`, replacing the one it had; `None`
    /// clears it. `tag_id` must belong to `group_id`.
    pub fn set_project_group_tag(
        &self,
        project_id: &str,
        group_id: &str,
        tag_id: Option<&str>,
    ) -> Result<(), StoreError> {
        self.tag_write(|tx| {
            find_group(tx, group_id)?;
            let changed = match tag_id {
                Some(tag_id) => {
                    let tag = find_tag(tx, tag_id)?;
                    if tag.group_id.as_deref() != Some(group_id) {
                        return Err(StoreError(format!(
                            "Tag {tag_id} is not in tag group {group_id}"
                        )));
                    }
                    ensure_project(tx, project_id)?;
                    assign_tag(tx, project_id, tag_id, Some(group_id), now_timestamp())?
                }
                None => {
                    tx.execute(
                        "DELETE FROM project_tags
                         WHERE project_id = ?1
                           AND tag_id IN (SELECT id FROM tags WHERE group_id = ?2)",
                        params![project_id, group_id],
                    )? > 0
                }
            };
            Ok(((), changed))
        })
    }

    /// Add or remove one tag on a project. Adding a grouped tag removes its
    /// siblings, so this is safe for any tag; the frontend uses it for
    /// ungrouped tags and [`Store::set_project_group_tag`] for grouped ones.
    pub fn set_project_tag_assigned(
        &self,
        project_id: &str,
        tag_id: &str,
        assigned: bool,
    ) -> Result<(), StoreError> {
        self.tag_write(|tx| {
            let tag = find_tag(tx, tag_id)?;
            let changed = if assigned {
                ensure_project(tx, project_id)?;
                assign_tag(
                    tx,
                    project_id,
                    tag_id,
                    tag.group_id.as_deref(),
                    now_timestamp(),
                )?
            } else {
                tx.execute(
                    "DELETE FROM project_tags WHERE project_id = ?1 AND tag_id = ?2",
                    params![project_id, tag_id],
                )? > 0
            };
            Ok(((), changed))
        })
    }
}
