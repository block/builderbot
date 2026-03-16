//! SQLite schema bootstrap and migrations for the Staged store.
//!
//! Migrations are managed with `rusqlite_migration`, which stores the
//! current schema version in SQLite `user_version`. Migration files are
//! discovered from `store/migrations/` using the crate's built-in
//! directory loader.

use std::sync::LazyLock;

use include_dir::{include_dir, Dir};
use rusqlite::{params, Connection};
use rusqlite_migration::Migrations;

use super::{StoreError, APP_VERSION};

static MIGRATION_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/src/store/migrations");
static MIGRATIONS: LazyLock<Migrations<'static>> = LazyLock::new(|| {
    Migrations::from_directory(&MIGRATION_DIR).expect("staged store migrations should be valid")
});

pub(super) fn initialize(conn: &mut Connection) -> Result<(), StoreError> {
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

    let pending = pending_migrations(conn)?;
    if pending < 0 {
        let version = current_user_version(conn)?;
        return Err(StoreError(format!(
            "Database schema version {version} is newer than supported by this build"
        )));
    }

    if pending > 0 {
        log::info!("Applying {pending} staged store migration(s)");
        MIGRATIONS.to_latest(conn).map_err(map_migration_error)?;
    }

    update_app_metadata(conn)?;
    Ok(())
}

pub(super) fn pending_migrations(conn: &Connection) -> Result<i32, StoreError> {
    MIGRATIONS
        .pending_migrations(conn)
        .map_err(map_migration_error)
}

#[cfg(test)]
pub(super) fn validate() -> Result<(), StoreError> {
    MIGRATIONS.validate().map_err(map_migration_error)
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

fn map_migration_error(error: rusqlite_migration::Error) -> StoreError {
    StoreError(error.to_string())
}
