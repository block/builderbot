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

    assert_eq!(version, 29);
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
    assert!(column_exists(&conn, "sessions", "branch_id"));
    assert!(column_exists(&conn, "sessions", "completion_effects_at"));
    assert!(column_exists(&conn, "notes", "parent_project_note_id"));
    assert!(!column_exists(&conn, "reviews", "is_auto"));
    assert!(column_exists(&conn, "repo_actions", "pinned"));
    assert!(column_exists(&conn, "repo_actions", "icon"));
    assert!(column_exists(&conn, "session_messages", "acp_origin"));

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
        -- `status` and `updated_at` predate every migration below; they are
        -- spelled out because the 0023 backfill reads them.
        CREATE TABLE sessions (
            id         TEXT PRIMARY KEY,
            status     TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE session_messages (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  TEXT NOT NULL,
            role        TEXT NOT NULL,
            content     TEXT NOT NULL,
            created_at  INTEGER NOT NULL,
            image_ids   TEXT DEFAULT NULL
        );
        CREATE TABLE notes (id TEXT PRIMARY KEY, session_id TEXT);
        -- Only the table/column the 0026 auto-review cleanup targets.
        CREATE TABLE reviews (
            id       TEXT PRIMARY KEY,
            is_auto  INTEGER NOT NULL DEFAULT 0
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
        -- Only the columns the 0024 and 0028 backfills read; the rest of the
        -- real tables predates every migration below.
        CREATE TABLE action_contexts (
            id                 TEXT PRIMARY KEY,
            detecting_actions  INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            context_id  TEXT NOT NULL,
            action_type TEXT NOT NULL,
            sort_order  INTEGER NOT NULL,
            created_at  INTEGER NOT NULL
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
    assert_eq!(version, 29);
    assert!(column_exists(&conn, "sessions", "pipeline"));
    assert!(column_exists(&conn, "sessions", "acp_config_selection"));
    assert!(column_exists(&conn, "sessions", "acp_title"));
    assert!(column_exists(&conn, "sessions", "branch_id"));
    assert!(column_exists(&conn, "sessions", "completion_effects_at"));
    assert!(column_exists(
        &conn,
        "session_messages",
        "acp_agent_capabilities"
    ));
    assert!(table_exists(&conn, "queued_session_messages"));
    assert!(column_exists(&conn, "session_messages", "acp_origin"));

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
            id         TEXT PRIMARY KEY,
            status     TEXT NOT NULL,
            pipeline   TEXT,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE session_messages (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  TEXT NOT NULL,
            role        TEXT NOT NULL,
            content     TEXT NOT NULL,
            created_at  INTEGER NOT NULL,
            image_ids   TEXT DEFAULT NULL
        );
        CREATE TABLE notes (id TEXT PRIMARY KEY, session_id TEXT);
        -- Only the table/column the 0026 auto-review cleanup targets.
        CREATE TABLE reviews (
            id       TEXT PRIMARY KEY,
            is_auto  INTEGER NOT NULL DEFAULT 0
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
        -- Only the columns the 0024 and 0028 backfills read; the rest of the
        -- real tables predates every migration below.
        CREATE TABLE action_contexts (
            id                 TEXT PRIMARY KEY,
            detecting_actions  INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            context_id  TEXT NOT NULL,
            action_type TEXT NOT NULL,
            sort_order  INTEGER NOT NULL,
            created_at  INTEGER NOT NULL
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
    assert_eq!(version, 29);
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
    assert!(column_exists(&conn, "sessions", "branch_id"));
    assert!(column_exists(&conn, "sessions", "completion_effects_at"));

    cleanup_db(&path);
}

#[test]
fn test_completion_effects_migration_backfills_finished_pipeline_sessions() {
    let path = temp_db_path("completion-effects-backfill");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 22;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        CREATE TABLE sessions (
            id         TEXT PRIMARY KEY,
            status     TEXT NOT NULL,
            pipeline   TEXT,
            updated_at INTEGER NOT NULL
        );
        INSERT INTO sessions (id, status, pipeline, updated_at) VALUES
            ('completed-pipeline', 'completed', '{}', 100),
            ('running-pipeline',   'running',   '{}', 200),
            ('error-pipeline',     'error',     '{}', 300),
            ('completed-ai',       'completed', NULL, 400);
        -- Only the columns the 0024 and 0028 backfills read.
        CREATE TABLE action_contexts (
            id                 TEXT PRIMARY KEY,
            detecting_actions  INTEGER NOT NULL DEFAULT 0
        );
        -- Only the table the 0025/0027 note column adds target.
        CREATE TABLE notes (id TEXT PRIMARY KEY, session_id TEXT);
        -- Only the table/column the 0026 auto-review cleanup targets.
        CREATE TABLE reviews (
            id       TEXT PRIMARY KEY,
            is_auto  INTEGER NOT NULL DEFAULT 0
        );
        -- Only the columns the 0028 backfill reads.
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            context_id  TEXT NOT NULL,
            action_type TEXT NOT NULL,
            sort_order  INTEGER NOT NULL,
            created_at  INTEGER NOT NULL
        );
        -- Only the table the 0029 ALTER targets; the rest of the real table
        -- predates every migration below.
        CREATE TABLE session_messages (id INTEGER PRIMARY KEY AUTOINCREMENT);
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
    assert_eq!(version, 29);
    assert!(column_exists(&conn, "sessions", "completion_effects_at"));

    let marker = |id: &str| -> Option<i64> {
        conn.query_row(
            "SELECT completion_effects_at FROM sessions WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap()
    };
    // Already-finished pipeline sessions are exactly the rows whose next resume
    // would replay their outcome effects, so they start out marked.
    assert_eq!(marker("completed-pipeline"), Some(100));
    // Still-running and failed pipeline sessions have outcomes yet to deliver.
    assert_eq!(marker("running-pipeline"), None);
    assert_eq!(marker("error-pipeline"), None);
    // Plain AI sessions never had completion side effects to begin with.
    assert_eq!(marker("completed-ai"), None);

    cleanup_db(&path);
}

#[test]
fn test_auto_review_removal_migration_deletes_auto_reviews_and_drops_flag() {
    let path = temp_db_path("auto-review-removal");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 25;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        CREATE TABLE reviews (
            id       TEXT PRIMARY KEY,
            is_auto  INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO reviews (id, is_auto) VALUES
            ('user-review', 0),
            ('auto-review', 1);
        -- Only the table the 0027 note column add targets.
        CREATE TABLE notes (id TEXT PRIMARY KEY, session_id TEXT);
        -- Only the columns the 0028 backfill reads.
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            context_id  TEXT NOT NULL,
            action_type TEXT NOT NULL,
            sort_order  INTEGER NOT NULL,
            created_at  INTEGER NOT NULL
        );
        -- Only the table the 0029 ALTER targets; the rest of the real table
        -- predates every migration below.
        CREATE TABLE session_messages (id INTEGER PRIMARY KEY AUTOINCREMENT);
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
    assert_eq!(version, 29);
    assert!(!column_exists(&conn, "reviews", "is_auto"));

    // Reviews the removed auto-review feature created in the background are
    // deleted; user-initiated (and adopted, is_auto = 0) reviews survive.
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM reviews")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(ids, vec!["user-review".to_string()]);

    cleanup_db(&path);
}

#[test]
fn test_detecting_pid_migration_clears_orphaned_detection_flags() {
    let path = temp_db_path("detecting-pid-backfill");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 23;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        CREATE TABLE action_contexts (
            id                 TEXT PRIMARY KEY,
            detecting_actions  INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO action_contexts (id, detecting_actions) VALUES
            ('wedged', 1),
            ('idle',   0);
        -- Only the table the 0025/0027 note column adds target.
        CREATE TABLE notes (id TEXT PRIMARY KEY, session_id TEXT);
        -- Only the table/column the 0026 auto-review cleanup targets.
        CREATE TABLE reviews (
            id       TEXT PRIMARY KEY,
            is_auto  INTEGER NOT NULL DEFAULT 0
        );
        -- Only the columns the 0028 backfill reads.
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            context_id  TEXT NOT NULL,
            action_type TEXT NOT NULL,
            sort_order  INTEGER NOT NULL,
            created_at  INTEGER NOT NULL
        );
        -- Only the table the 0029 ALTER targets; the rest of the real table
        -- predates every migration below.
        CREATE TABLE session_messages (id INTEGER PRIMARY KEY AUTOINCREMENT);
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
    assert_eq!(version, 29);
    assert!(column_exists(&conn, "action_contexts", "detecting_pid"));

    // No shipped build ever cleared the flag from outside the process that set
    // it, so a row arriving here with it set is orphaned by definition — and
    // rejects every detection for its repo until something clears it.
    let detecting: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM action_contexts WHERE detecting_actions = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(detecting, 0);

    cleanup_db(&path);
}

#[test]
fn test_note_subtype_migration_backfills_session_less_notes() {
    let path = temp_db_path("note-subtype-backfill");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 25;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        CREATE TABLE notes (
            id         TEXT PRIMARY KEY,
            session_id TEXT
        );
        INSERT INTO notes (id, session_id) VALUES
            ('dropped', NULL),
            ('agent',   'session-1');
        -- Only the table/column the 0026 auto-review cleanup targets.
        CREATE TABLE reviews (
            id       TEXT PRIMARY KEY,
            is_auto  INTEGER NOT NULL DEFAULT 0
        );
        -- Only the columns the 0028 backfill reads.
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            context_id  TEXT NOT NULL,
            action_type TEXT NOT NULL,
            sort_order  INTEGER NOT NULL,
            created_at  INTEGER NOT NULL
        );
        -- Only the table the 0029 ALTER targets; the rest of the real table
        -- predates every migration below.
        CREATE TABLE session_messages (id INTEGER PRIMARY KEY AUTOINCREMENT);
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
    assert_eq!(version, 29);
    assert!(column_exists(&conn, "notes", "subtype"));

    let subtype = |id: &str| -> Option<String> {
        conn.query_row(
            "SELECT subtype FROM notes WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap()
    };

    // Drag-dropped files and saved action output are already user-authored
    // notes with no owning session, so they become editable alongside newly
    // written ones. Notes an agent produced stay untagged.
    assert_eq!(subtype("dropped").as_deref(), Some("written"));
    assert_eq!(subtype("agent"), None);

    cleanup_db(&path);
}

#[test]
fn test_pinned_actions_migration_pins_each_contexts_first_run_action() {
    let path = temp_db_path("pinned-actions-backfill");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 25;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        -- Only the columns the 0028 backfill reads.
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            context_id  TEXT NOT NULL,
            action_type TEXT NOT NULL,
            sort_order  INTEGER NOT NULL,
            created_at  INTEGER NOT NULL
        );
        -- 'staged' is a context whose header button was the first run action;
        -- the build action ahead of it never had one, and the second run action
        -- lived in the Actions submenu. 'libs' has no run action at all.
        -- Inserted out of sort order, and with a sort_order collision on the
        -- two run actions, so the deterministic tie-break is what decides.
        INSERT INTO repo_actions (id, context_id, action_type, sort_order, created_at) VALUES
            ('staged-run-second', 'staged', 'run',   1, 300),
            ('staged-build',      'staged', 'build', 0, 100),
            ('staged-run-first',  'staged', 'run',   1, 200),
            ('libs-test',         'libs',   'test',  0, 400),
            ('libs-format',       'libs',   'format', 1, 500);
        -- Only the table/column the 0026 auto-review cleanup targets.
        CREATE TABLE reviews (
            id       TEXT PRIMARY KEY,
            is_auto  INTEGER NOT NULL DEFAULT 0
        );
        -- Only the table the 0027 note column add targets.
        CREATE TABLE notes (id TEXT PRIMARY KEY, session_id TEXT);
        -- Only the table the 0029 ALTER targets; the rest of the real table
        -- predates every migration below.
        CREATE TABLE session_messages (id INTEGER PRIMARY KEY AUTOINCREMENT);
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
    assert_eq!(version, 29);
    assert!(column_exists(&conn, "repo_actions", "pinned"));
    assert!(column_exists(&conn, "repo_actions", "icon"));

    let pinned: Vec<String> = conn
        .prepare("SELECT id FROM repo_actions WHERE pinned = 1 ORDER BY id ASC")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    // Exactly the action each context's header already promoted: the earliest
    // run action, and nothing for the context that never had one.
    assert_eq!(pinned, vec!["staged-run-first".to_string()]);

    // Nothing picks an icon on the way in — NULL is what keeps the migrated
    // button rendering as the play icon it has always shown.
    let icons: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM repo_actions WHERE icon IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(icons, 0);

    cleanup_db(&path);
}
