//! The tag catalog: tag groups and tags (migration 0045).
//!
//! A tag is either in one group (single choice: a project holds at most one
//! of the group's tags) or ungrouped (multi-select). Assignments live in
//! `project_tags.rs`; the one-time import of the old project status lives in
//! `tag_import.rs`.
//!
//! Rules SQL can't express, enforced by every write inside one
//! `BEGIN IMMEDIATE` transaction (and serialized by the connection mutex):
//! - **Dense positions.** Tags run 0..n-1 per scope, where a scope is one
//!   group or the ungrouped set; groups run 0..n-1 too. Any write that changes
//!   a scope renumbers it, and callers pass target indexes, never positions.
//! - **Names** are trimmed, 1–64 characters, and unique per scope ignoring
//!   case. The unique indexes catch duplicates; their errors are mapped to a
//!   message naming the scope.
//! - **Icons** are `None` or a kebab-case Lucide name.
//! - **Colours.** A create that omits one gets the least-used colour.
//!
//! Every write publishes [`StoreChange::Tags`] only if a row changed.

use std::collections::HashSet;
use std::str::FromStr;

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::project_tags::{assign_tag, ensure_project};
use super::{now_timestamp, Store, StoreChange, StoreError};

// =============================================================================
// Types
// =============================================================================

/// A tag's colour, from a fixed palette so tags stay theme-aware (the
/// frontend maps each to a `--project-status-*` token).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagColor {
    Green,
    Red,
    Amber,
    Blue,
    Purple,
    Cyan,
    Gray,
}

impl TagColor {
    /// The tie-break order when picking the least-used colour for a tag
    /// created without one.
    pub const AUTO_ORDER: [TagColor; 7] = [
        Self::Blue,
        Self::Green,
        Self::Amber,
        Self::Purple,
        Self::Cyan,
        Self::Red,
        Self::Gray,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Red => "red",
            Self::Amber => "amber",
            Self::Blue => "blue",
            Self::Purple => "purple",
            Self::Cyan => "cyan",
            Self::Gray => "gray",
        }
    }
}

impl FromStr for TagColor {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "green" => Ok(Self::Green),
            "red" => Ok(Self::Red),
            "amber" => Ok(Self::Amber),
            "blue" => Ok(Self::Blue),
            "purple" => Ok(Self::Purple),
            "cyan" => Ok(Self::Cyan),
            "gray" => Ok(Self::Gray),
            other => Err(format!("unknown tag color: {other}")),
        }
    }
}

/// A single-choice group of tags, e.g. Status or Priority.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagGroup {
    pub id: String,
    pub name: String,
    pub position: i64,
    /// Whether this group's tag drives the project icon. At most one group.
    pub is_icon_group: bool,
    /// Whether the group's board is pinned to the projects sidebar (0046).
    pub show_in_sidebar: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: String,
    /// `None` = ungrouped (multi-select).
    pub group_id: Option<String>,
    pub name: String,
    pub color: TagColor,
    /// Kebab-case Lucide icon name; `None` renders as a coloured dot.
    pub icon: Option<String>,
    /// Dense 0..n-1 within its group, or within the ungrouped set.
    pub position: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One project ↔ tag assignment. `project_tags.sort_order` is reserved for
/// manual board order and stays off the wire in v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTag {
    pub project_id: String,
    pub tag_id: String,
}

/// Everything the frontend needs about tags, in one small payload.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagState {
    /// Sorted by `position, created_at`.
    pub groups: Vec<TagGroup>,
    /// Sorted by `position, created_at`; the frontend buckets by `group_id`.
    pub tags: Vec<Tag>,
    /// Sorted by `project_id, tag_id`.
    pub assignments: Vec<ProjectTag>,
}

/// A tag to create. A missing colour picks the least-used one.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTag {
    pub name: String,
    #[serde(default)]
    pub color: Option<TagColor>,
    #[serde(default)]
    pub icon: Option<String>,
}

/// What happens to a deleted group's tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeleteTagGroupMode {
    /// The tags become ungrouped; projects keep them as free tags.
    Ungroup,
    /// The tags and their assignments are deleted with the group.
    DeleteTags,
}

// =============================================================================
// Validation and naming
// =============================================================================

/// Longest tag or group name, in characters.
pub(super) const MAX_NAME_CHARS: usize = 64;

/// The trimmed name, or an error naming `what` ("Tag" / "Group").
fn validated_name(name: &str, what: &str) -> Result<String, StoreError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(StoreError(format!("{what} name can't be empty")));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(StoreError(format!(
            "{what} name must be {MAX_NAME_CHARS} characters or fewer"
        )));
    }
    Ok(name.to_string())
}

/// Whether `icon` is a kebab-case Lucide name: `^[a-z0-9]+(-[a-z0-9]+)*$`.
pub(super) fn is_valid_icon(icon: &str) -> bool {
    icon.split('-').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

fn validated_icon(icon: Option<&str>) -> Result<Option<String>, StoreError> {
    match icon {
        None => Ok(None),
        Some(icon) if is_valid_icon(icon) => Ok(Some(icon.to_string())),
        Some(icon) => Err(StoreError(format!("Invalid tag icon “{icon}”"))),
    }
}

/// The key SQLite's `NOCASE` collation compares names by: ASCII-only case
/// folding, so a set of these flags exactly the names the unique indexes
/// would reject.
pub(super) fn name_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// `base` cut so that `base + suffix` fits [`MAX_NAME_CHARS`].
pub(super) fn fit_name(base: &str, suffix: &str) -> String {
    let room = MAX_NAME_CHARS.saturating_sub(suffix.chars().count());
    let cut: String = base.chars().take(room).collect();
    format!("{}{suffix}", cut.trim_end())
}

/// `base`, else `base 2`, `base 3`, … — the first `taken` doesn't hold, each
/// cut to fit the name limit.
pub(super) fn numbered_name(base: &str, taken: &HashSet<String>) -> String {
    std::iter::once(fit_name(base, ""))
        .chain((2..).map(|n| fit_name(base, &format!(" {n}"))))
        .find(|candidate| !taken.contains(&name_key(candidate)))
        .expect("an unbounded candidate list always has a free name")
}

/// Whether a write failed on a unique index (as opposed to a primary key).
fn is_unique_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(e, _)
            if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
    )
}

/// Map a tag insert/update error: `idx_tags_scope_name` is the only unique
/// index on `tags`, so a unique violation is a name taken in the scope.
fn tag_write_error(
    conn: &Connection,
    error: rusqlite::Error,
    name: &str,
    group_id: Option<&str>,
) -> StoreError {
    if !is_unique_violation(&error) {
        return error.into();
    }
    match group_id {
        Some(group_id) => {
            let group_name =
                Store::lookup_id(conn, "SELECT name FROM tag_groups WHERE id = ?1", group_id)
                    .unwrap_or_else(|| group_id.to_string());
            StoreError(format!(
                "A tag named “{name}” already exists in {group_name}"
            ))
        }
        None => StoreError(format!(
            "A tag named “{name}” already exists among ungrouped tags"
        )),
    }
}

/// Map a group name insert/update error. Only `idx_tag_groups_name` can fire:
/// the one write that sets `is_icon_group` (`create_tag_group`) does so only
/// after seeing no icon group in the same immediate transaction.
fn group_name_error(error: rusqlite::Error, name: &str) -> StoreError {
    if is_unique_violation(&error) {
        StoreError(format!("A group named “{name}” already exists"))
    } else {
        error.into()
    }
}

// =============================================================================
// Row access
// =============================================================================

const GROUP_COLUMNS: &str =
    "id, name, position, is_icon_group, show_in_sidebar, created_at, updated_at";
const TAG_COLUMNS: &str = "id, group_id, name, color, icon, position, created_at, updated_at";

fn group_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TagGroup> {
    Ok(TagGroup {
        id: row.get(0)?,
        name: row.get(1)?,
        position: row.get(2)?,
        is_icon_group: row.get(3)?,
        show_in_sidebar: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn tag_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Tag> {
    let color: String = row.get(3)?;
    Ok(Tag {
        id: row.get(0)?,
        group_id: row.get(1)?,
        name: row.get(2)?,
        // The column's CHECK admits only palette colours.
        color: color.parse().unwrap_or(TagColor::Gray),
        icon: row.get(4)?,
        position: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

pub(super) fn find_tag(conn: &Connection, tag_id: &str) -> Result<Tag, StoreError> {
    conn.query_row(
        &format!("SELECT {TAG_COLUMNS} FROM tags WHERE id = ?1"),
        [tag_id],
        tag_from_row,
    )
    .optional()?
    .ok_or_else(|| StoreError(format!("Tag not found: {tag_id}")))
}

pub(super) fn find_group(conn: &Connection, group_id: &str) -> Result<TagGroup, StoreError> {
    conn.query_row(
        &format!("SELECT {GROUP_COLUMNS} FROM tag_groups WHERE id = ?1"),
        [group_id],
        group_from_row,
    )
    .optional()?
    .ok_or_else(|| StoreError(format!("Tag group not found: {group_id}")))
}

/// The first column of every row `sql` selects, as strings.
fn collect_strings(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare(sql)?;
    let values = stmt
        .query_map(params, |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(values)
}

/// A scope's tag ids (one group, or ungrouped for `None`) in display order.
fn scope_tag_ids(conn: &Connection, group_id: Option<&str>) -> Result<Vec<String>, StoreError> {
    collect_strings(
        conn,
        "SELECT id FROM tags WHERE group_id IS ?1 ORDER BY position, created_at, id",
        params![group_id],
    )
}

/// The [`name_key`]s of a scope's tag names.
fn scope_name_keys(
    conn: &Connection,
    group_id: Option<&str>,
) -> Result<HashSet<String>, StoreError> {
    let names = collect_strings(
        conn,
        "SELECT name FROM tags WHERE group_id IS ?1",
        params![group_id],
    )?;
    Ok(names.iter().map(|name| name_key(name)).collect())
}

fn ordered_group_ids(conn: &Connection) -> Result<Vec<String>, StoreError> {
    collect_strings(
        conn,
        "SELECT id FROM tag_groups ORDER BY position, created_at, id",
        [],
    )
}

/// The position after the last tag in a scope.
fn next_tag_position(conn: &Connection, group_id: Option<&str>) -> Result<i64, StoreError> {
    conn.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM tags WHERE group_id IS ?1",
        params![group_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

/// Write `ids` back as positions 0..n-1 of `table`, touching only rows whose
/// position moved. Returns whether any did.
fn write_order(
    conn: &Connection,
    table: &str,
    ids: &[String],
    now: i64,
) -> Result<bool, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "UPDATE {table} SET position = ?1, updated_at = ?2 WHERE id = ?3 AND position != ?1"
    ))?;
    let mut changed = false;
    for (position, id) in ids.iter().enumerate() {
        changed |= stmt.execute(params![position as i64, now, id])? > 0;
    }
    Ok(changed)
}

/// Close the gaps in a tag scope after a tag left it.
fn renumber_scope(conn: &Connection, group_id: Option<&str>, now: i64) -> Result<(), StoreError> {
    let ids = scope_tag_ids(conn, group_id)?;
    write_order(conn, "tags", &ids, now)?;
    Ok(())
}

/// The least-used colour across all tags, ties going by
/// [`TagColor::AUTO_ORDER`].
fn least_used_color(conn: &Connection) -> Result<TagColor, StoreError> {
    let mut counts = [0i64; TagColor::AUTO_ORDER.len()];
    let mut stmt = conn.prepare("SELECT color, COUNT(*) FROM tags GROUP BY color")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (color, count) = row?;
        if let Some(i) = TagColor::AUTO_ORDER
            .iter()
            .position(|c| c.as_str() == color)
        {
            counts[i] = count;
        }
    }
    // `min_by_key` keeps the first of equal minimums: the tie-break order.
    let (i, _) = counts
        .iter()
        .enumerate()
        .min_by_key(|(_, count)| **count)
        .expect("the palette is not empty");
    Ok(TagColor::AUTO_ORDER[i])
}

/// Insert a tag appended last in its scope.
fn insert_tag(
    conn: &Connection,
    new: &NewTag,
    group_id: Option<&str>,
    now: i64,
) -> Result<Tag, StoreError> {
    let name = validated_name(&new.name, "Tag")?;
    let icon = validated_icon(new.icon.as_deref())?;
    let color = match new.color {
        Some(color) => color,
        None => least_used_color(conn)?,
    };
    let tag = Tag {
        id: Uuid::new_v4().to_string(),
        group_id: group_id.map(str::to_string),
        name,
        color,
        icon,
        position: next_tag_position(conn, group_id)?,
        created_at: now,
        updated_at: now,
    };
    conn.execute(
        &format!("INSERT INTO tags ({TAG_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
        params![
            tag.id,
            tag.group_id,
            tag.name,
            tag.color.as_str(),
            tag.icon,
            tag.position,
            tag.created_at,
            tag.updated_at,
        ],
    )
    .map_err(|e| tag_write_error(conn, e, &tag.name, group_id))?;
    Ok(tag)
}

// =============================================================================
// Store API
// =============================================================================

impl Store {
    /// Run a tag write in one `BEGIN IMMEDIATE` transaction. `write` returns
    /// its value and whether a row changed; [`StoreChange::Tags`] is published
    /// after the commit only in that case. An error rolls everything back.
    pub(super) fn tag_write<T>(
        &self,
        write: impl FnOnce(&Transaction<'_>) -> Result<(T, bool), StoreError>,
    ) -> Result<T, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (value, changed) = write(&tx)?;
        tx.commit()?;
        if changed {
            self.publish(StoreChange::Tags);
        }
        Ok(value)
    }

    /// The whole tag catalog and every assignment, read in one snapshot.
    pub fn get_tag_state(&self) -> Result<TagState, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let groups = {
            let mut stmt = tx.prepare(&format!(
                "SELECT {GROUP_COLUMNS} FROM tag_groups ORDER BY position, created_at"
            ))?;
            let rows = stmt.query_map([], group_from_row)?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let tags = {
            let mut stmt = tx.prepare(&format!(
                "SELECT {TAG_COLUMNS} FROM tags ORDER BY position, created_at"
            ))?;
            let rows = stmt.query_map([], tag_from_row)?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let assignments = {
            let mut stmt = tx.prepare(
                "SELECT project_id, tag_id FROM project_tags ORDER BY project_id, tag_id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(ProjectTag {
                    project_id: row.get(0)?,
                    tag_id: row.get(1)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        tx.commit()?;
        Ok(TagState {
            groups,
            tags,
            assignments,
        })
    }

    /// Create a group, appended last, with `tags` created in the order given.
    ///
    /// When no group drives the project icon, the new one does: a catalog
    /// started from the board's empty state (or rebuilt after the last group
    /// was deleted) gives projects an icon without a second step.
    pub fn create_tag_group(&self, name: &str, tags: &[NewTag]) -> Result<TagGroup, StoreError> {
        self.tag_write(|tx| {
            let name = validated_name(name, "Group")?;
            let now = now_timestamp();
            let position: i64 = tx.query_row(
                "SELECT COALESCE(MAX(position) + 1, 0) FROM tag_groups",
                [],
                |row| row.get(0),
            )?;
            let has_icon_group: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM tag_groups WHERE is_icon_group = 1)",
                [],
                |row| row.get(0),
            )?;
            let group = TagGroup {
                id: Uuid::new_v4().to_string(),
                name,
                position,
                is_icon_group: !has_icon_group,
                show_in_sidebar: false,
                created_at: now,
                updated_at: now,
            };
            tx.execute(
                &format!(
                    "INSERT INTO tag_groups ({GROUP_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
                ),
                params![
                    group.id,
                    group.name,
                    group.position,
                    group.is_icon_group,
                    group.show_in_sidebar,
                    group.created_at,
                    group.updated_at,
                ],
            )
            .map_err(|e| group_name_error(e, &group.name))?;
            for new in tags {
                insert_tag(tx, new, Some(&group.id), now)?;
            }
            Ok((group, true))
        })
    }

    pub fn rename_tag_group(&self, group_id: &str, name: &str) -> Result<(), StoreError> {
        self.tag_write(|tx| {
            let group = find_group(tx, group_id)?;
            let name = validated_name(name, "Group")?;
            if group.name == name {
                return Ok(((), false));
            }
            tx.execute(
                "UPDATE tag_groups SET name = ?1, updated_at = ?2 WHERE id = ?3",
                params![name, now_timestamp(), group_id],
            )
            .map_err(|e| group_name_error(e, &name))?;
            Ok(((), true))
        })
    }

    /// Make `group_id` the one group that drives the project icon, or turn
    /// the icon group off with `None`.
    pub fn set_icon_tag_group(&self, group_id: Option<&str>) -> Result<(), StoreError> {
        self.tag_write(|tx| {
            if let Some(group_id) = group_id {
                find_group(tx, group_id)?;
            }
            let now = now_timestamp();
            // Clear first: the partial unique index admits one flagged row.
            let cleared = tx.execute(
                "UPDATE tag_groups SET is_icon_group = 0, updated_at = ?1
                 WHERE is_icon_group = 1 AND id IS NOT ?2",
                params![now, group_id],
            )?;
            let set = match group_id {
                Some(group_id) => tx.execute(
                    "UPDATE tag_groups SET is_icon_group = 1, updated_at = ?1
                     WHERE id = ?2 AND is_icon_group = 0",
                    params![now, group_id],
                )?,
                None => 0,
            };
            Ok(((), cleared + set > 0))
        })
    }

    /// Pin the group's board to the projects sidebar, or unpin it. Any number
    /// of groups may be pinned; the sidebar lists them in group order.
    pub fn set_tag_group_sidebar(&self, group_id: &str, shown: bool) -> Result<(), StoreError> {
        self.tag_write(|tx| {
            let group = find_group(tx, group_id)?;
            if group.show_in_sidebar == shown {
                return Ok(((), false));
            }
            tx.execute(
                "UPDATE tag_groups SET show_in_sidebar = ?1, updated_at = ?2 WHERE id = ?3",
                params![shown, now_timestamp(), group_id],
            )?;
            Ok(((), true))
        })
    }

    /// Reorder the groups. `group_ids` must list every group exactly once.
    pub fn reorder_tag_groups(&self, group_ids: &[String]) -> Result<(), StoreError> {
        self.tag_write(|tx| {
            let mut current = ordered_group_ids(tx)?;
            let mut requested = group_ids.to_vec();
            current.sort();
            requested.sort();
            if current != requested {
                return Err(StoreError(
                    "Tag group order must list every tag group exactly once".to_string(),
                ));
            }
            let changed = write_order(tx, "tag_groups", group_ids, now_timestamp())?;
            Ok(((), changed))
        })
    }

    /// Delete a group. [`DeleteTagGroupMode::Ungroup`] appends its tags, in
    /// order, after the existing ungrouped tags, renaming a colliding one to
    /// "Name (Group)" (then "Name (Group) 2", …) and keeping its assignments.
    /// [`DeleteTagGroupMode::DeleteTags`] deletes the tags, cascading theirs.
    pub fn delete_tag_group(
        &self,
        group_id: &str,
        mode: DeleteTagGroupMode,
    ) -> Result<(), StoreError> {
        self.tag_write(|tx| {
            let group = find_group(tx, group_id)?;
            let now = now_timestamp();
            match mode {
                DeleteTagGroupMode::Ungroup => {
                    let mut taken = scope_name_keys(tx, None)?;
                    let first = next_tag_position(tx, None)?;
                    let moving = scope_tag_ids(tx, Some(group_id))?;
                    for (position, id) in (first..).zip(moving) {
                        let tag = find_tag(tx, &id)?;
                        let name = if taken.contains(&name_key(&tag.name)) {
                            numbered_name(&format!("{} ({})", tag.name, group.name), &taken)
                        } else {
                            tag.name
                        };
                        taken.insert(name_key(&name));
                        tx.execute(
                            "UPDATE tags SET group_id = NULL, name = ?1, position = ?2,
                                             updated_at = ?3
                             WHERE id = ?4",
                            params![name, position, now, id],
                        )?;
                    }
                    renumber_scope(tx, None, now)?;
                }
                DeleteTagGroupMode::DeleteTags => {
                    tx.execute("DELETE FROM tags WHERE group_id = ?1", [group_id])?;
                }
            }
            tx.execute("DELETE FROM tag_groups WHERE id = ?1", [group_id])?;
            let remaining = ordered_group_ids(tx)?;
            write_order(tx, "tag_groups", &remaining, now)?;
            Ok(((), true))
        })
    }

    /// Create a tag, appended last in its scope. With `assign_project_id` it
    /// is assigned in the same transaction, replacing the project's sibling
    /// in the group for a grouped tag (inline create from a picker).
    pub fn create_tag(
        &self,
        new: &NewTag,
        group_id: Option<&str>,
        assign_project_id: Option<&str>,
    ) -> Result<Tag, StoreError> {
        self.tag_write(|tx| {
            if let Some(group_id) = group_id {
                find_group(tx, group_id)?;
            }
            if let Some(project_id) = assign_project_id {
                ensure_project(tx, project_id)?;
            }
            let now = now_timestamp();
            let tag = insert_tag(tx, new, group_id, now)?;
            if let Some(project_id) = assign_project_id {
                assign_tag(tx, project_id, &tag.id, group_id, now)?;
            }
            Ok((tag, true))
        })
    }

    /// Replace a tag's name, colour and icon (the whole value: no patch).
    pub fn update_tag(
        &self,
        tag_id: &str,
        name: &str,
        color: TagColor,
        icon: Option<&str>,
    ) -> Result<(), StoreError> {
        self.tag_write(|tx| {
            let tag = find_tag(tx, tag_id)?;
            let name = validated_name(name, "Tag")?;
            let icon = validated_icon(icon)?;
            if tag.name == name && tag.color == color && tag.icon == icon {
                return Ok(((), false));
            }
            tx.execute(
                "UPDATE tags SET name = ?1, color = ?2, icon = ?3, updated_at = ?4 WHERE id = ?5",
                params![name, color.as_str(), icon, now_timestamp(), tag_id],
            )
            .map_err(|e| tag_write_error(tx, e, &name, tag.group_id.as_deref()))?;
            Ok(((), true))
        })
    }

    /// Move a tag to `index` (clamped) in the scope `group_id`: a reorder
    /// within its own scope, or a move to another one.
    ///
    /// Moving into a group first removes the tag from projects that already
    /// have a tag in that group — the group's existing value wins. A name
    /// taken in the target scope is an error, so the user renames first.
    pub fn move_tag(
        &self,
        tag_id: &str,
        group_id: Option<&str>,
        index: usize,
    ) -> Result<(), StoreError> {
        self.tag_write(|tx| {
            let tag = find_tag(tx, tag_id)?;
            if let Some(group_id) = group_id {
                find_group(tx, group_id)?;
            }
            let now = now_timestamp();
            let mut order = scope_tag_ids(tx, group_id)?;
            order.retain(|id| id != tag_id);
            order.insert(index.min(order.len()), tag_id.to_string());

            if tag.group_id.as_deref() == group_id {
                let changed = write_order(tx, "tags", &order, now)?;
                return Ok(((), changed));
            }

            tx.execute(
                "UPDATE tags SET group_id = ?1, updated_at = ?2 WHERE id = ?3",
                params![group_id, now, tag_id],
            )
            .map_err(|e| tag_write_error(tx, e, &tag.name, group_id))?;
            if let Some(group_id) = group_id {
                tx.execute(
                    "DELETE FROM project_tags
                     WHERE tag_id = ?1
                       AND project_id IN (
                           SELECT pt.project_id
                           FROM project_tags pt JOIN tags t ON t.id = pt.tag_id
                           WHERE t.group_id = ?2 AND t.id != ?1
                       )",
                    params![tag_id, group_id],
                )?;
            }
            write_order(tx, "tags", &order, now)?;
            renumber_scope(tx, tag.group_id.as_deref(), now)?;
            Ok(((), true))
        })
    }

    /// Delete a tag, cascading its assignments, and renumber its scope.
    pub fn delete_tag(&self, tag_id: &str) -> Result<(), StoreError> {
        self.tag_write(|tx| {
            let tag = find_tag(tx, tag_id)?;
            tx.execute("DELETE FROM tags WHERE id = ?1", [tag_id])?;
            renumber_scope(tx, tag.group_id.as_deref(), now_timestamp())?;
            Ok(((), true))
        })
    }
}
