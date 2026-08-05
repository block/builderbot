use rusqlite::{params, Connection};
use uuid::Uuid;

use super::{check_db_compatibility, migrations, remove_db_files, DbCompatibility, Store};

fn temp_db_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("staged-store-{name}-{}.db", Uuid::new_v4()))
}

fn cleanup_db(path: &std::path::Path) {
    remove_db_files(path).unwrap();
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM sqlite_master
            WHERE type = 'table' AND name = ?1
        )",
        params![table],
        |row| row.get::<_, i64>(0),
    )
    .unwrap()
        == 1
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    columns.iter().any(|name| name == column)
}

#[test]
fn test_migration_registry_is_valid() {
    migrations::validate().unwrap();
}

#[test]
fn test_check_db_compatibility_requires_reset_for_unversioned_db_with_tables() {
    let path = temp_db_path("compat-unversioned");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
        ",
    )
    .unwrap();
    drop(conn);

    let compat = check_db_compatibility(&path).unwrap();
    match compat {
        DbCompatibility::NeedsReset { db_app_version } => {
            assert_eq!(db_app_version, "0.2.9");
        }
        other => panic!("expected reset for unversioned database, got {other:?}"),
    }

    cleanup_db(&path);
}

#[test]
fn test_check_db_compatibility_requires_reset_for_pre_migration_db() {
    let path = temp_db_path("compat-pre-migration");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        CREATE TABLE schema_version (
            version     INTEGER NOT NULL,
            app_version TEXT
        );
        INSERT INTO schema_version (version, app_version) VALUES (22, '0.2.9');
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
        ",
    )
    .unwrap();
    drop(conn);

    let compat = check_db_compatibility(&path).unwrap();
    match compat {
        DbCompatibility::NeedsReset { db_app_version } => {
            assert_eq!(db_app_version, "0.1.0");
        }
        other => panic!("expected reset for pre-migration database, got {other:?}"),
    }

    cleanup_db(&path);
}

#[test]
fn test_check_db_compatibility_detects_newer_schema_version() {
    let path = temp_db_path("compat-too-new");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 999;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '9.9.9');
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
        ",
    )
    .unwrap();
    drop(conn);

    let compat = check_db_compatibility(&path).unwrap();
    match compat {
        DbCompatibility::TooNew { db_app_version } => {
            assert_eq!(db_app_version, "9.9.9");
        }
        other => panic!("expected too-new database, got {other:?}"),
    }

    cleanup_db(&path);
}

#[test]
fn test_store_bootstraps_fresh_database_with_baseline_migration() {
    let path = temp_db_path("bootstrap");
    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let app_version: String = conn
        .query_row(
            "SELECT app_version FROM app_metadata WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(version, 21);
    assert_eq!(app_version, super::APP_VERSION);
    assert!(table_exists(&conn, "projects"));
    assert!(table_exists(&conn, "project_notes"));
    assert!(table_exists(&conn, "images"));
    assert!(table_exists(&conn, "queued_session_messages"));
    assert!(column_exists(&conn, "comments", "note_session_id"));
    assert!(column_exists(&conn, "comments", "commit_session_id"));
    assert!(column_exists(
        &conn,
        "session_messages",
        "acp_agent_capabilities"
    ));
    assert!(column_exists(&conn, "sessions", "acp_config_selection"));
    assert!(column_exists(&conn, "sessions", "acp_title"));

    let trigger_count: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM sqlite_master
             WHERE type = 'trigger'
               AND name LIKE 'trg_cleanup_session_after_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trigger_count, 5);

    cleanup_db(&path);
}

#[test]
fn test_store_repairs_github_comment_tracking_user_version() {
    let path = temp_db_path("github-comment-tracking-repair");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 12;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
        CREATE TABLE session_messages (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  TEXT NOT NULL,
            role        TEXT NOT NULL,
            content     TEXT NOT NULL,
            created_at  INTEGER NOT NULL,
            image_ids   TEXT DEFAULT NULL
        );
        CREATE TABLE repo_badges (
            github_repo TEXT NOT NULL,
            subpath     TEXT NOT NULL DEFAULT '',
            short_name  TEXT NOT NULL,
            hue         REAL NOT NULL,
            created_at  INTEGER NOT NULL,
            PRIMARY KEY (github_repo, subpath)
        );
        CREATE TABLE comments (
            id                    TEXT PRIMARY KEY,
            github_comment_id     INTEGER,
            github_comment_type   TEXT,
            github_comment_stale  INTEGER NOT NULL DEFAULT 0
        );
        ",
    )
    .unwrap();
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 21);
    assert!(column_exists(&conn, "sessions", "pipeline"));
    assert!(column_exists(&conn, "sessions", "acp_config_selection"));
    assert!(column_exists(&conn, "sessions", "acp_title"));
    assert!(column_exists(
        &conn,
        "session_messages",
        "acp_agent_capabilities"
    ));
    assert!(table_exists(&conn, "queued_session_messages"));

    cleanup_db(&path);
}

#[test]
fn test_store_repairs_pipeline_user_version() {
    let path = temp_db_path("pipeline-version-repair");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 13;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        CREATE TABLE sessions (
            id       TEXT PRIMARY KEY,
            pipeline TEXT
        );
        CREATE TABLE session_messages (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  TEXT NOT NULL,
            role        TEXT NOT NULL,
            content     TEXT NOT NULL,
            created_at  INTEGER NOT NULL,
            image_ids   TEXT DEFAULT NULL
        );
        CREATE TABLE repo_badges (
            github_repo TEXT NOT NULL,
            subpath     TEXT NOT NULL DEFAULT '',
            short_name  TEXT NOT NULL,
            hue         REAL NOT NULL,
            created_at  INTEGER NOT NULL,
            PRIMARY KEY (github_repo, subpath)
        );
        CREATE TABLE comments (id TEXT PRIMARY KEY);
        ",
    )
    .unwrap();
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 21);
    assert!(column_exists(&conn, "comments", "github_comment_id"));
    assert!(column_exists(&conn, "comments", "github_comment_type"));
    assert!(column_exists(&conn, "comments", "github_comment_stale"));
    assert!(column_exists(
        &conn,
        "session_messages",
        "acp_agent_capabilities"
    ));
    assert!(table_exists(&conn, "queued_session_messages"));
    assert!(column_exists(&conn, "sessions", "acp_config_selection"));
    assert!(column_exists(&conn, "sessions", "acp_title"));

    cleanup_db(&path);
}
