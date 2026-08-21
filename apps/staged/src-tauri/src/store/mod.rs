//! SQLite storage for Staged.
//!
//! Schema migrations are tracked with SQLite `user_version` via
//! `rusqlite_migration`. Fresh installs bootstrap from the migration
//! directory and future releases append new migrations in place. Databases
//! with user tables but no `user_version` are intentionally treated as
//! incompatible and trigger a reset-and-recreate dialog. This release is
//! the starting point for the new migration path.
//!
//! Tables: app_metadata, projects, project_repos, branches, workdirs, commits,
//! sessions, session_messages, notes, project_notes, reviews, images,
//! action_contexts, repo_actions, repo_affinities, queued_session_messages.

pub mod models;

mod actions;
mod branch_move;
mod branches;
mod commits;
pub mod images;
mod messages;
mod migrations;
mod notes;
mod project_notes;
mod project_repos;
mod projects;
mod queued_messages;
mod recent_repos;
pub mod repo_affinities;
pub mod repo_badges;
mod reviews;
mod sessions;
mod workdirs;

#[cfg(test)]
mod migration_tests;
#[cfg(test)]
mod tests;

// Re-export all model types for backwards compatibility.
pub use branch_move::{BranchMove, RepoPlacement, WorkdirMove};
pub use models::*;
pub use project_notes::DeletedProjectNoteSessions;
pub use repo_badges::{fallback_short_name, next_hue};

use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// =============================================================================
// Error type
// =============================================================================

#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self(e.to_string())
    }
}

/// The app version of this build, pulled from Cargo.toml at compile time.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Result of checking database compatibility.
#[derive(Debug, Clone)]
pub enum DbCompatibility {
    /// Database is compatible (or doesn't exist yet).
    Ok,
    /// Database uses an unsupported pre-versioning schema — offer to reset.
    NeedsReset {
        /// The app version that last opened this database (e.g. "0.3.0"),
        /// or "0.1.0" for pre-versioning databases.
        db_app_version: String,
    },
    /// Database was written by a *newer* schema — user should update the app.
    TooNew {
        /// The app version that last opened this database.
        db_app_version: String,
    },
}

/// Check whether an existing database file is compatible with this build.
///
/// Returns [`DbCompatibility::Ok`] if the file doesn't exist (fresh DB),
/// is empty, or has a `user_version` schema that this build can migrate
/// from. Returns [`DbCompatibility::NeedsReset`] for populated databases
/// with no `user_version`, including databases created before this
/// migration system landed, or [`DbCompatibility::TooNew`] for newer ones.
///
/// This opens a temporary read-only connection and closes it before
/// returning — it does **not** create a `Store`.
pub fn check_db_compatibility(path: &Path) -> Result<DbCompatibility, String> {
    if !path.exists() {
        return Ok(DbCompatibility::Ok);
    }

    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("Cannot open database: {e}"))?;

    let user_table_count = || {
        conn.query_row(
            "SELECT COUNT(*)
             FROM sqlite_master
             WHERE type = 'table'
               AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get::<_, i64>(0),
        )
    };

    let user_version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| format!("Cannot read schema version: {e}"))?;

    let has_app_metadata = conn
        .query_row(
            "SELECT COUNT(*)
             FROM sqlite_master
             WHERE type = 'table' AND name = 'app_metadata'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false);

    let db_app_version = if has_app_metadata {
        conn.query_row(
            "SELECT app_version FROM app_metadata WHERE id = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Cannot read app metadata: {e}"))?
    } else {
        None
    }
    .unwrap_or_else(|| "0.1.0".to_string());
    let user_table_count = user_table_count().unwrap_or(0);

    if user_version == 0 {
        if user_table_count == 0 {
            return Ok(DbCompatibility::Ok);
        }
        // Cut over to the new migration system here: any populated database
        // that never wrote `user_version` must be reset before continuing.
        return Ok(DbCompatibility::NeedsReset { db_app_version });
    }

    if user_table_count == 0 {
        return Ok(DbCompatibility::NeedsReset { db_app_version });
    }

    let pending = migrations::pending_migrations(&conn)
        .map_err(|e| format!("Cannot evaluate database migrations: {e}"))?;
    if pending < 0 {
        Ok(DbCompatibility::TooNew { db_app_version })
    } else if user_version > 0 {
        Ok(DbCompatibility::Ok)
    } else {
        Ok(DbCompatibility::NeedsReset { db_app_version })
    }
}

/// Remove a database file and its WAL/SHM companions.
pub fn remove_db_files(path: &Path) -> Result<(), StoreError> {
    for suffix in &["", "-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{}", path.display(), suffix));
        if p.exists() {
            std::fs::remove_file(&p)
                .map_err(|e| StoreError(format!("Failed to remove {}: {e}", p.display())))?;
        }
    }
    Ok(())
}

// =============================================================================
// ResolvedSession
// =============================================================================

/// Session metadata resolved from an optional session ID.
///
/// Returned by [`Store::resolve_session_status`] to avoid an unwieldy
/// positional tuple.
#[derive(Debug, Default)]
pub struct ResolvedSession {
    pub session_id: Option<String>,
    pub status: Option<String>,
    pub completion_reason: Option<String>,
    pub provider: Option<String>,
    /// Latest ACP-provided session title, if the agent pushed one.
    pub acp_title: Option<String>,
}

// =============================================================================
// Store
// =============================================================================

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Open (or create) the database at the given path.
    ///
    /// The caller should check compatibility first with
    /// [`check_db_compatibility`] — this method does not validate
    /// an existing schema.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// In-memory database for testing.
    #[cfg(test)]
    pub fn in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Resolve an optional session_id into its associated metadata.
    ///
    /// Returns a default (all-`None`) [`ResolvedSession`] when `session_id` is
    /// `None`, otherwise looks up the session and populates its status,
    /// completion reason, and provider.
    pub fn resolve_session_status(&self, session_id: Option<&str>) -> ResolvedSession {
        match session_id {
            Some(sid) => {
                let session = self.get_session(sid).ok().flatten();
                ResolvedSession {
                    session_id: Some(sid.to_string()),
                    status: session.as_ref().map(|s| s.status.as_str().to_string()),
                    completion_reason: session
                        .as_ref()
                        .and_then(|s| s.completion_reason.as_ref().map(|r| r.as_str().to_string())),
                    provider: session.as_ref().and_then(|s| s.provider.clone()),
                    acp_title: session.as_ref().and_then(|s| s.acp_title.clone()),
                }
            }
            None => ResolvedSession::default(),
        }
    }

    fn init_schema(&self) -> Result<(), StoreError> {
        let mut conn = self.conn.lock().unwrap();
        migrations::initialize(&mut conn)
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Current timestamp in milliseconds.
pub fn now_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
