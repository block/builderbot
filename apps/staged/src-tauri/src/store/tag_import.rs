//! The one-time import of the old single project status into tags.
//!
//! Before tags, a project's status was `projects.status_override` (an option
//! id) and the option list was the frontend `project-status-options`
//! preference, which lives in `preferences.json` where SQL can't read it.
//! Migration 0045 leaves a `legacy_project_status_import` marker table;
//! [`Store::import_legacy_project_statuses`] seeds a Status icon group from
//! the parsed list, copies the selections into `project_tags` and drops the
//! marker, all in one transaction. `projects.status_override` stays, frozen,
//! as a recovery source.
//!
//! The import can be held across launches (an unreadable `preferences.json`
//! keeps the marker), and meanwhile the user can build a catalog of their
//! own, so it never assumes `tag_groups` is empty: the legacy group takes a
//! free name ("Status 2"), the next position, and drives the icon only when
//! no group does yet. Option ids become tag ids either way, so selections
//! still copy across.
//!
//! The backend never writes `preferences.json` (the Tauri store plugin owns
//! it and would overwrite changes), so the old key is left orphaned.

use std::collections::HashSet;

use rusqlite::{params, TransactionBehavior};
use serde_json::Value;

use super::tags::{is_valid_icon, name_key, numbered_name, TagColor};
use super::{now_timestamp, Store, StoreChange, StoreError};

/// The preferences key the frontend kept its status option list under.
pub const LEGACY_STATUS_OPTIONS_KEY: &str = "project-status-options";

/// The icon the frontend gave an option with a missing or empty one.
const LEGACY_FALLBACK_ICON: &str = "circle-dot";

/// The id and name of the group the import creates.
pub const LEGACY_STATUS_GROUP_ID: &str = "status";
const LEGACY_STATUS_GROUP_NAME: &str = "Status";

/// What a blank legacy label imports as (the frontend's display fallback).
const UNTITLED_STATUS: &str = "Untitled status";

/// The built-in options as `(id, label, icon, colour)`, copied from the
/// frontend's `DEFAULT_PROJECT_STATUS_OPTIONS`. Frozen: a record of what the
/// defaults were when the import landed, not a template to evolve.
pub const LEGACY_DEFAULT_STATUSES: [(&str, &str, &str, TagColor); 7] = [
    ("idea", "Idea", "lightbulb", TagColor::Amber),
    ("in-progress", "In progress", "circle-dot", TagColor::Blue),
    ("polishing", "Polishing", "paintbrush", TagColor::Cyan),
    ("needs-review", "Needs review", "eye", TagColor::Purple),
    ("done", "Done", "circle-check", TagColor::Green),
    ("on-hold", "On hold", "circle-pause", TagColor::Gray),
    ("blocked", "Blocked", "octagon-minus", TagColor::Red),
];

/// The user's status option list, as the frontend would have resolved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyStatusOptions {
    /// No usable stored list: the built-ins, [`LEGACY_DEFAULT_STATUSES`].
    Defaults,
    /// A stored list. Empty when the user deleted every status.
    Custom(Vec<LegacyStatusOption>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyStatusOption {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub color: TagColor,
}

impl LegacyStatusOptions {
    /// The options this list stands for, in order.
    pub fn options(&self) -> Vec<LegacyStatusOption> {
        match self {
            Self::Defaults => LEGACY_DEFAULT_STATUSES
                .iter()
                .map(|(id, label, icon, color)| LegacyStatusOption {
                    id: id.to_string(),
                    label: label.to_string(),
                    icon: icon.to_string(),
                    color: *color,
                })
                .collect(),
            Self::Custom(options) => options.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyImportOutcome {
    /// The marker was already gone: an earlier launch (or a concurrent
    /// instance) imported.
    AlreadyImported,
    Imported {
        /// Tags created in the Status group.
        tags: usize,
        /// Projects whose selection became a Status tag.
        assignments: usize,
        /// Projects whose selection named no surviving option. They showed
        /// Default already, and now have no Status tag.
        dropped_dangling: usize,
    },
}

/// Parse the stored status option list out of the whole `preferences.json`
/// value, mirroring the frontend's `normalizeProjectStatusOptions`:
/// - a missing key or a non-array value is [`LegacyStatusOptions::Defaults`];
/// - `[]` is an empty custom list (the user deleted every status);
/// - an entry needs a non-empty string `id` and a string `label`; duplicate
///   ids keep their first occurrence;
/// - a missing or empty icon becomes `circle-dot`, as does one that isn't a
///   kebab-case name (tags only accept those);
/// - an unknown colour becomes gray;
/// - if no entry survives, the result is the defaults.
pub fn parse_legacy_status_options(prefs: Option<&Value>) -> LegacyStatusOptions {
    let Some(Value::Array(entries)) = prefs.and_then(|prefs| prefs.get(LEGACY_STATUS_OPTIONS_KEY))
    else {
        return LegacyStatusOptions::Defaults;
    };
    if entries.is_empty() {
        return LegacyStatusOptions::Custom(Vec::new());
    }

    let mut seen = HashSet::new();
    let mut options = Vec::new();
    for entry in entries {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let Some(id) = entry
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if seen.contains(id) {
            continue;
        }
        let Some(label) = entry.get("label").and_then(Value::as_str) else {
            continue;
        };
        seen.insert(id.to_string());
        let icon = entry
            .get("icon")
            .and_then(Value::as_str)
            .filter(|icon| is_valid_icon(icon))
            .unwrap_or(LEGACY_FALLBACK_ICON);
        let color = entry
            .get("color")
            .and_then(Value::as_str)
            .and_then(|color| color.parse().ok())
            .unwrap_or(TagColor::Gray);
        options.push(LegacyStatusOption {
            id: id.to_string(),
            label: label.to_string(),
            icon: icon.to_string(),
            color,
        });
    }

    if options.is_empty() {
        LegacyStatusOptions::Defaults
    } else {
        LegacyStatusOptions::Custom(options)
    }
}

impl Store {
    /// Seed the Status group from `options` and copy every project's
    /// `status_override` into it, once.
    ///
    /// One `BEGIN IMMEDIATE` transaction: the marker check happens inside it,
    /// so two instances launching at once can't both import. Option ids are
    /// preserved as tag ids and order becomes position. A blank label becomes
    /// "Untitled status", and a repeated one (ignoring case) gets " 2", " 3", …
    /// A selection naming no imported option drops out. Not run by
    /// [`Store::new`]: partial fixture databases have no `projects` table.
    ///
    /// Groups the user made while the import was held stay as they are: the
    /// legacy group is appended after them under a free name, and becomes
    /// the icon group only when none of them is.
    pub fn import_legacy_project_statuses(
        &self,
        options: &LegacyStatusOptions,
    ) -> Result<LegacyImportOutcome, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let pending: bool = tx.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'legacy_project_status_import'
            )",
            [],
            |row| row.get(0),
        )?;
        if !pending {
            return Ok(LegacyImportOutcome::AlreadyImported);
        }

        let now = now_timestamp();
        let taken_group_names: HashSet<String> = tx
            .prepare("SELECT name FROM tag_groups")?
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|name| name.map(|name| name_key(&name)))
            .collect::<Result<_, _>>()?;
        let group_name = numbered_name(LEGACY_STATUS_GROUP_NAME, &taken_group_names);
        let group_position: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM tag_groups",
            [],
            |row| row.get(0),
        )?;
        let has_icon_group: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM tag_groups WHERE is_icon_group = 1)",
            [],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO tag_groups (id, name, position, is_icon_group, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![
                LEGACY_STATUS_GROUP_ID,
                group_name,
                group_position,
                !has_icon_group,
                now
            ],
        )?;

        let options = options.options();
        let mut taken = HashSet::new();
        for (position, option) in options.iter().enumerate() {
            let label = option.label.trim();
            let base = if label.is_empty() {
                UNTITLED_STATUS
            } else {
                label
            };
            let name = numbered_name(base, &taken);
            taken.insert(name_key(&name));
            tx.execute(
                "INSERT INTO tags
                     (id, group_id, name, color, icon, position, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![
                    option.id,
                    LEGACY_STATUS_GROUP_ID,
                    name,
                    option.color.as_str(),
                    option.icon,
                    position as i64,
                    now,
                ],
            )?;
        }

        let assignments = tx.execute(
            "INSERT INTO project_tags (project_id, tag_id, sort_order, created_at)
             SELECT p.id, p.status_override, NULL, ?1
             FROM projects p JOIN tags t ON t.id = p.status_override AND t.group_id = ?2",
            params![now, LEGACY_STATUS_GROUP_ID],
        )?;
        let selections: i64 = tx.query_row(
            "SELECT COUNT(*) FROM projects WHERE status_override IS NOT NULL",
            [],
            |row| row.get(0),
        )?;

        tx.execute_batch("DROP TABLE legacy_project_status_import")?;
        tx.commit()?;
        drop(conn);
        self.publish(StoreChange::Tags);

        let outcome = LegacyImportOutcome::Imported {
            tags: options.len(),
            assignments,
            dropped_dangling: (selections as usize).saturating_sub(assignments),
        };
        log::info!("Imported legacy project statuses into tag group {group_name:?}: {outcome:?}");
        Ok(outcome)
    }
}
