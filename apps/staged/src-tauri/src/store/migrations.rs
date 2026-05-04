//! SQLite schema bootstrap and migrations for the Staged store.
//!
//! Migrations are managed with `rusqlite_migration`, which stores the
//! current schema version in SQLite `user_version`. Migration files are
//! discovered from `store/migrations/` using the crate's built-in
//! directory loader.

use std::sync::OnceLock;

use include_dir::{include_dir, Dir};
use rusqlite::{params, Connection};
use rusqlite_migration::Migrations;

use super::{StoreError, APP_VERSION};

static MIGRATION_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/src/store/migrations");
static MIGRATIONS: OnceLock<Migrations<'static>> = OnceLock::new();

pub(super) fn initialize(conn: &mut Connection) -> Result<(), StoreError> {
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

    repair_github_comment_tracking_user_version(conn)?;
    repair_pipeline_user_version(conn)?;

    let migrations = migrations();
    let pending = migrations
        .pending_migrations(conn)
        .map_err(map_migration_error)?;
    if pending < 0 {
        let version = current_user_version(conn)?;
        return Err(StoreError(format!(
            "Database schema version {version} is newer than supported by this build"
        )));
    }

    if pending > 0 {
        log::info!("Applying {pending} staged store migration(s)");
        migrations.to_latest(conn).map_err(map_migration_error)?;
    }

    update_app_metadata(conn)?;
    Ok(())
}

pub(super) fn pending_migrations(conn: &Connection) -> Result<i32, StoreError> {
    migrations()
        .pending_migrations(conn)
        .map_err(map_migration_error)
}

#[cfg(test)]
pub(super) fn validate() -> Result<(), StoreError> {
    migrations().validate().map_err(map_migration_error)
}

fn migrations() -> &'static Migrations<'static> {
    MIGRATIONS.get_or_init(|| {
        Migrations::from_directory(&MIGRATION_DIR).expect("staged store migrations should be valid")
    })
}

fn update_app_metadata(conn: &Connection) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO app_metadata (id, app_version) VALUES (1, ?1)
         ON CONFLICT(id) DO UPDATE SET app_version = excluded.app_version",
        params![APP_VERSION],
    )?;
    Ok(())
}

fn current_user_version(conn: &Connection) -> Result<i64, StoreError> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(StoreError::from)
}

fn repair_github_comment_tracking_user_version(conn: &Connection) -> Result<(), StoreError> {
    if current_user_version(conn)? != 12 || !comments_table_exists(conn)? {
        return Ok(());
    }

    let columns = comment_column_names(conn)?;
    let github_columns = [
        "github_comment_id",
        "github_comment_type",
        "github_comment_stale",
    ];

    if !github_columns
        .iter()
        .any(|name| columns.iter().any(|c| c == name))
    {
        return Ok(());
    }

    ensure_github_comment_tracking_columns(conn, &columns)?;

    conn.execute_batch("PRAGMA user_version = 13;")?;
    Ok(())
}

fn repair_pipeline_user_version(conn: &Connection) -> Result<(), StoreError> {
    if current_user_version(conn)? != 13 || !sessions_table_exists(conn)? {
        return Ok(());
    }

    let columns = session_column_names(conn)?;
    if !columns.iter().any(|name| name == "pipeline") {
        return Ok(());
    }

    if comments_table_exists(conn)? {
        let comment_columns = comment_column_names(conn)?;
        ensure_github_comment_tracking_columns(conn, &comment_columns)?;
    }

    conn.execute_batch("PRAGMA user_version = 14;")?;
    Ok(())
}

fn ensure_github_comment_tracking_columns(
    conn: &Connection,
    columns: &[String],
) -> Result<(), StoreError> {
    if !columns.iter().any(|name| name == "github_comment_id") {
        conn.execute_batch("ALTER TABLE comments ADD COLUMN github_comment_id INTEGER;")?;
    }
    if !columns.iter().any(|name| name == "github_comment_type") {
        conn.execute_batch("ALTER TABLE comments ADD COLUMN github_comment_type TEXT;")?;
    }
    if !columns.iter().any(|name| name == "github_comment_stale") {
        conn.execute_batch(
            "ALTER TABLE comments ADD COLUMN github_comment_stale INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    Ok(())
}

fn comments_table_exists(conn: &Connection) -> Result<bool, StoreError> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM sqlite_master
            WHERE type = 'table' AND name = 'comments'
        )",
        [],
        |row| row.get::<_, bool>(0),
    )
    .map_err(StoreError::from)
}

fn sessions_table_exists(conn: &Connection) -> Result<bool, StoreError> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM sqlite_master
            WHERE type = 'table' AND name = 'sessions'
        )",
        [],
        |row| row.get::<_, bool>(0),
    )
    .map_err(StoreError::from)
}

fn comment_column_names(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare("PRAGMA table_info(comments)")?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(names)
}

fn session_column_names(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare("PRAGMA table_info(sessions)")?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(names)
}

fn map_migration_error(error: rusqlite_migration::Error) -> StoreError {
    StoreError(error.to_string())
}
