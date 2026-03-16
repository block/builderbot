//! SQLite storage for Staged.
//!
//! A `schema_version` table tracks compatibility. Fresh installs bootstrap
//! from the current baseline schema and older versioned databases are
//! upgraded in place in [`Store::init_schema`]. Only unsupported
//! pre-versioning databases trigger a reset-and-recreate dialog.
//!
//! Tables: schema_version, projects, project_repos, branches, workdirs, commits,
//! sessions, session_messages, notes, project_notes, reviews, images, action_contexts, repo_actions.

pub mod models;

mod actions;
mod branches;
mod commits;
pub mod images;
mod messages;
mod migrations;
mod notes;
mod project_notes;
mod project_repos;
mod projects;
mod recent_repos;
mod reviews;
mod sessions;
mod workdirs;

#[cfg(test)]
mod migration_tests;
#[cfg(test)]
mod tests;

// Re-export all model types for backwards compatibility.
pub use models::*;

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

// =============================================================================
// Schema version
// =============================================================================

/// The schema version written by this build.
///
/// Bump this whenever the schema changes.
/// Many app versions may share the same schema version.
pub const SCHEMA_VERSION: i64 = 22;

/// Oldest schema version we can migrate forward from.
///
/// Databases with a version in `MIN_MIGRATABLE_VERSION..=SCHEMA_VERSION` are
/// upgraded in place by [`Store::init_schema`]. Databases older than this are
/// pre-versioning beta databases that require a full wipe.
///
/// Version `0` is reserved for a freshly initialized but not-yet-migrated
/// database, so it is also considered migratable.
pub const MIN_MIGRATABLE_VERSION: i64 = 0;

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
/// has a matching schema version, or has a version within the migratable
/// range ([`MIN_MIGRATABLE_VERSION`]..=`SCHEMA_VERSION`). Returns
/// [`DbCompatibility::NeedsReset`] for unsupported pre-versioning schemas or
/// [`DbCompatibility::TooNew`] for newer ones.
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
               AND name NOT LIKE 'sqlite_%'
               AND name != 'schema_version'",
            [],
            |row| row.get::<_, i64>(0),
        )
    };

    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false);

    if !table_exists {
        let user_table_count = user_table_count().unwrap_or(0);

        if user_table_count == 0 {
            return Ok(DbCompatibility::Ok);
        }

        // Pre-versioning beta database — this shape predates the stable
        // versioned store and still requires a reset.
        return Ok(DbCompatibility::NeedsReset {
            db_app_version: "0.1.0".to_string(),
        });
    }

    let version_row: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT version, app_version FROM schema_version LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("Cannot read schema version: {e}"))?;

    let (version, app_version) = version_row.unwrap_or((0, None));

    let db_app_version = app_version.unwrap_or_else(|| "0.1.0".to_string());
    let user_table_count = user_table_count().unwrap_or(0);

    if version == 0 && user_table_count > 0 {
        return Ok(DbCompatibility::NeedsReset { db_app_version });
    }

    if version == SCHEMA_VERSION {
        return Ok(DbCompatibility::Ok);
    }

    if version > SCHEMA_VERSION {
        Ok(DbCompatibility::TooNew { db_app_version })
    } else if version >= MIN_MIGRATABLE_VERSION {
        // We have incremental migrations for this range — let Store::new()
        // apply them.
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

    /// Resolve an optional session_id into (session_id, session_status).
    ///
    /// Returns `(None, None)` when `session_id` is `None`, otherwise
    /// looks up the session and returns its status string.
    pub fn resolve_session_status(
        &self,
        session_id: Option<&str>,
    ) -> (Option<String>, Option<String>) {
        match session_id {
            Some(sid) => {
                let status = self
                    .get_session(sid)
                    .ok()
                    .flatten()
                    .map(|s| s.status.as_str().to_string());
                (Some(sid.to_string()), status)
            }
            None => (None, None),
        }
    }

    fn init_schema(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        migrations::initialize(&conn)
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
