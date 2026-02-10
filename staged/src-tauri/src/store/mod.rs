//! SQLite storage for Staged.
//!
//! Fresh schema — no migrations. A `schema_version` table tracks
//! compatibility. If the database is missing that table or has an
//! incompatible version, it is deleted and recreated after user
//! confirmation.
//!
//! Tables: schema_version, projects, branches, workdirs, commits,
//! sessions, session_messages, notes, reviews, project_actions.

pub mod models;

mod actions;
mod branches;
mod commits;
mod messages;
mod notes;
mod projects;
mod reviews;
mod sessions;
mod workdirs;

#[cfg(test)]
mod tests;

// Re-export all model types for backwards compatibility.
pub use models::*;

use rusqlite::Connection;
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
/// Bump this whenever the schema changes in an incompatible way.
/// Many app versions may share the same schema version.
pub const SCHEMA_VERSION: i64 = 2;

/// The app version of this build, pulled from Cargo.toml at compile time.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Result of checking database compatibility.
#[derive(Debug, Clone)]
pub enum DbCompatibility {
    /// Database is compatible (or doesn't exist yet).
    Ok,
    /// Database exists with an older schema — offer to reset.
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
/// Returns [`DbCompatibility::Ok`] if the file doesn't exist (fresh DB) or has a
/// matching schema version. Returns [`DbCompatibility::NeedsReset`] or
/// [`DbCompatibility::TooNew`] when the schema version doesn't match.
///
/// This opens a temporary read-only connection and closes it before
/// returning — it does **not** create a `Store`.
pub fn check_db_compatibility(path: &Path) -> Result<DbCompatibility, String> {
    if !path.exists() {
        return Ok(DbCompatibility::Ok);
    }

    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("Cannot open database: {e}"))?;

    // Does the schema_version table exist at all?
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false);

    if !table_exists {
        // Pre-versioning beta database — treat as schema 0, app v0.1.0.
        return Ok(DbCompatibility::NeedsReset {
            db_app_version: "0.1.0".to_string(),
        });
    }

    let (version, app_version): (i64, Option<String>) = conn
        .query_row(
            "SELECT version, app_version FROM schema_version LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("Cannot read schema version: {e}"))?;

    let db_app_version = app_version.unwrap_or_else(|| "0.1.0".to_string());

    if version == SCHEMA_VERSION {
        return Ok(DbCompatibility::Ok);
    }

    if version > SCHEMA_VERSION {
        Ok(DbCompatibility::TooNew { db_app_version })
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
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        // Schema version tracking.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version     INTEGER NOT NULL,
                app_version TEXT
            );",
        )?;
        // Insert on first creation, then always update app_version to
        // record the last build that touched this database.
        conn.execute(
            "INSERT INTO schema_version (version, app_version)
                SELECT ?1, ?2 WHERE NOT EXISTS (SELECT 1 FROM schema_version)",
            rusqlite::params![SCHEMA_VERSION, APP_VERSION],
        )?;
        conn.execute("UPDATE schema_version SET app_version = ?1", [APP_VERSION])?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS projects (
                id          TEXT PRIMARY KEY,
                repo_path   TEXT NOT NULL UNIQUE,
                subpath     TEXT,
                created_at  INTEGER NOT NULL,
                updated_at  INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS branches (
                id              TEXT PRIMARY KEY,
                project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                branch_name     TEXT NOT NULL,
                base_branch     TEXT NOT NULL,
                pr_number       INTEGER,
                created_at      INTEGER NOT NULL,
                updated_at      INTEGER NOT NULL,
                UNIQUE(project_id, branch_name)
            );

            CREATE TABLE IF NOT EXISTS workdirs (
                id              TEXT PRIMARY KEY,
                project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                path            TEXT NOT NULL,
                branch_id       TEXT REFERENCES branches(id) ON DELETE SET NULL,
                created_at      INTEGER NOT NULL,
                updated_at      INTEGER NOT NULL,
                UNIQUE(project_id, path)
            );
            CREATE INDEX IF NOT EXISTS idx_workdirs_project ON workdirs(project_id);
            CREATE INDEX IF NOT EXISTS idx_workdirs_branch ON workdirs(branch_id);

            CREATE TABLE IF NOT EXISTS sessions (
                id              TEXT PRIMARY KEY,
                prompt          TEXT NOT NULL,
                status          TEXT NOT NULL,
                working_dir     TEXT NOT NULL,
                agent_id        TEXT,
                error_message   TEXT,
                created_at      INTEGER NOT NULL,
                updated_at      INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS commits (
                id              TEXT PRIMARY KEY,
                branch_id       TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
                sha             TEXT,
                session_id      TEXT,
                created_at      INTEGER NOT NULL,
                updated_at      INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_commits_branch ON commits(branch_id);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_commits_branch_sha
                ON commits(branch_id, sha) WHERE sha IS NOT NULL;

            CREATE TABLE IF NOT EXISTS session_messages (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                role            TEXT NOT NULL,
                content         TEXT NOT NULL,
                created_at      INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_session_messages_session
                ON session_messages(session_id);

            CREATE TABLE IF NOT EXISTS notes (
                id              TEXT PRIMARY KEY,
                branch_id       TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
                session_id      TEXT,
                title           TEXT NOT NULL,
                content         TEXT NOT NULL,
                created_at      INTEGER NOT NULL,
                updated_at      INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_notes_branch ON notes(branch_id);

            CREATE TABLE IF NOT EXISTS reviews (
                id              TEXT PRIMARY KEY,
                branch_id       TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
                commit_sha      TEXT NOT NULL,
                scope           TEXT NOT NULL,
                session_id      TEXT,
                created_at      INTEGER NOT NULL,
                updated_at      INTEGER NOT NULL,
                UNIQUE(branch_id, commit_sha, scope)
            );
            CREATE INDEX IF NOT EXISTS idx_reviews_branch ON reviews(branch_id);

            CREATE TABLE IF NOT EXISTS reviewed_files (
                review_id   TEXT NOT NULL REFERENCES reviews(id) ON DELETE CASCADE,
                path        TEXT NOT NULL,
                PRIMARY KEY (review_id, path)
            );

            CREATE TABLE IF NOT EXISTS comments (
                id          TEXT PRIMARY KEY,
                review_id   TEXT NOT NULL REFERENCES reviews(id) ON DELETE CASCADE,
                path        TEXT NOT NULL,
                span_start  INTEGER NOT NULL,
                span_end    INTEGER NOT NULL,
                content     TEXT NOT NULL,
                created_at  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_comments_review ON comments(review_id);

            CREATE TABLE IF NOT EXISTS reference_files (
                review_id   TEXT NOT NULL REFERENCES reviews(id) ON DELETE CASCADE,
                path        TEXT NOT NULL,
                PRIMARY KEY (review_id, path)
            );

            CREATE TABLE IF NOT EXISTS project_actions (
                id              TEXT PRIMARY KEY,
                project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                name            TEXT NOT NULL,
                command         TEXT NOT NULL,
                action_type     TEXT NOT NULL,
                sort_order      INTEGER NOT NULL,
                auto_commit     INTEGER NOT NULL DEFAULT 0,
                created_at      INTEGER NOT NULL,
                updated_at      INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_project_actions_project
                ON project_actions(project_id);

            -- Session cleanup triggers: when a commit, note, or review is
            -- deleted (directly or via cascade from branch/project deletion),
            -- delete the referenced session if no other row still points at
            -- it. Only non-running sessions are cleaned up — a running
            -- session may legitimately have no artifacts yet.
            CREATE TRIGGER IF NOT EXISTS trg_cleanup_session_after_commit_delete
            AFTER DELETE ON commits
            WHEN OLD.session_id IS NOT NULL
            BEGIN
                DELETE FROM sessions
                WHERE id = OLD.session_id
                  AND status != 'running'
                  AND NOT EXISTS (SELECT 1 FROM commits  WHERE session_id = OLD.session_id)
                  AND NOT EXISTS (SELECT 1 FROM notes    WHERE session_id = OLD.session_id)
                  AND NOT EXISTS (SELECT 1 FROM reviews  WHERE session_id = OLD.session_id);
            END;

            CREATE TRIGGER IF NOT EXISTS trg_cleanup_session_after_note_delete
            AFTER DELETE ON notes
            WHEN OLD.session_id IS NOT NULL
            BEGIN
                DELETE FROM sessions
                WHERE id = OLD.session_id
                  AND status != 'running'
                  AND NOT EXISTS (SELECT 1 FROM commits  WHERE session_id = OLD.session_id)
                  AND NOT EXISTS (SELECT 1 FROM notes    WHERE session_id = OLD.session_id)
                  AND NOT EXISTS (SELECT 1 FROM reviews  WHERE session_id = OLD.session_id);
            END;

            CREATE TRIGGER IF NOT EXISTS trg_cleanup_session_after_review_delete
            AFTER DELETE ON reviews
            WHEN OLD.session_id IS NOT NULL
            BEGIN
                DELETE FROM sessions
                WHERE id = OLD.session_id
                  AND status != 'running'
                  AND NOT EXISTS (SELECT 1 FROM commits  WHERE session_id = OLD.session_id)
                  AND NOT EXISTS (SELECT 1 FROM notes    WHERE session_id = OLD.session_id)
                  AND NOT EXISTS (SELECT 1 FROM reviews  WHERE session_id = OLD.session_id);
            END;
            ",
        )?;
        Ok(())
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
