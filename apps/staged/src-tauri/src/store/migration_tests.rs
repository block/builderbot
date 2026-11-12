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

fn index_exists(conn: &Connection, index: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM sqlite_master
            WHERE type = 'index' AND name = ?1
        )",
        params![index],
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

/// Add whichever of the tables migration 0041 recreates its artifact-delete
/// triggers on a minimal old schema leaves out, so the run reaches the
/// migration a test is about.
fn add_trigger_tables(conn: &Connection) {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS commits (session_id TEXT);
        CREATE TABLE IF NOT EXISTS notes (session_id TEXT);
        CREATE TABLE IF NOT EXISTS reviews (session_id TEXT);
        CREATE TABLE IF NOT EXISTS project_notes (session_id TEXT);
        CREATE TABLE IF NOT EXISTS images (session_id TEXT);
        ",
    )
    .unwrap();
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

    assert_eq!(version, 48);
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
    assert!(column_exists(&conn, "sessions", "parent_session_id"));
    assert!(column_exists(&conn, "sessions", "pre_head_sha"));
    assert!(column_exists(&conn, "sessions", "blox_command_id"));
    assert!(column_exists(&conn, "sessions", "started_by"));
    assert!(table_exists(&conn, "control_grants"));
    assert!(column_exists(&conn, "notes", "parent_project_note_id"));
    assert!(!column_exists(&conn, "reviews", "is_auto"));
    assert!(column_exists(&conn, "repo_actions", "pinned"));
    assert!(column_exists(&conn, "repo_actions", "icon"));
    assert!(!column_exists(&conn, "repo_actions", "auto_commit"));
    assert!(column_exists(&conn, "session_messages", "acp_origin"));
    // Frozen since tags (0045): nothing reads or writes it except the one-shot
    // legacy import, but it stays as a recovery source.
    assert!(column_exists(&conn, "projects", "status_override"));
    assert!(column_exists(&conn, "images", "width"));
    assert!(column_exists(&conn, "images", "height"));
    assert!(column_exists(&conn, "images", "density"));
    assert!(column_exists(&conn, "branches", "pr_title"));
    assert!(table_exists(&conn, "tag_groups"));
    assert!(table_exists(&conn, "tags"));
    assert!(table_exists(&conn, "project_tags"));
    assert!(index_exists(&conn, "idx_tag_groups_name"));
    assert!(index_exists(&conn, "idx_tag_groups_icon"));
    assert!(index_exists(&conn, "idx_tags_scope_name"));
    // 0046: a group's board can be pinned to the sidebar.
    assert!(column_exists(&conn, "tag_groups", "show_in_sidebar"));
    // The legacy status import's marker, until the app runs the import.
    assert!(table_exists(&conn, "legacy_project_status_import"));
    let marker_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM legacy_project_status_import WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(marker_rows, 1);
    assert!(column_exists(&conn, "sessions", "provider_metadata"));
    assert!(column_exists(&conn, "sessions", "rate_limit_resets_at"));
    assert!(column_exists(&conn, "sessions", "rate_limit_kind"));

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
        -- Only the tables 0032 and 0033 add columns to.
        CREATE TABLE projects (id TEXT PRIMARY KEY);
        CREATE TABLE images (id TEXT PRIMARY KEY);
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
        CREATE TABLE notes (id TEXT PRIMARY KEY, branch_id TEXT, session_id TEXT, updated_at INTEGER NOT NULL, completed_at INTEGER);
        -- Only the columns 0038 adds to and the 0039 repair reads.
        CREATE TABLE project_notes (id TEXT PRIMARY KEY, project_id TEXT, completed_at INTEGER);
        -- Only the table/column the 0026 auto-review cleanup targets.
        CREATE TABLE reviews (
            id         TEXT PRIMARY KEY,
            branch_id  TEXT,
            session_id TEXT,
            is_auto    INTEGER NOT NULL DEFAULT 0
        );
        -- Only the table/columns the 0034 backfill joins.
        CREATE TABLE commits (id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, session_id TEXT);
        -- The parent of the `sessions.branch_id` key 0022 adds, which the 0034
        -- backfill writes, with the `project_id` the 0039 repair joins on.
        CREATE TABLE branches (id TEXT PRIMARY KEY, project_id TEXT);
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
        -- Only the columns the 0024 and 0028 backfills read and the 0030 drop
        -- targets; the rest of the real tables predates every migration below.
        CREATE TABLE action_contexts (
            id                 TEXT PRIMARY KEY,
            detecting_actions  INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            context_id  TEXT NOT NULL,
            action_type TEXT NOT NULL,
            sort_order  INTEGER NOT NULL,
            created_at  INTEGER NOT NULL,
            auto_commit INTEGER NOT NULL DEFAULT 0
        );
        ",
    )
    .unwrap();
    add_trigger_tables(&conn);
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);
    assert!(column_exists(&conn, "sessions", "pipeline"));
    assert!(column_exists(&conn, "sessions", "acp_config_selection"));
    assert!(column_exists(&conn, "sessions", "acp_title"));
    assert!(column_exists(&conn, "sessions", "branch_id"));
    assert!(column_exists(&conn, "sessions", "completion_effects_at"));
    assert!(column_exists(&conn, "sessions", "provider_metadata"));
    assert!(column_exists(&conn, "sessions", "rate_limit_resets_at"));
    assert!(column_exists(&conn, "sessions", "rate_limit_kind"));
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
        -- Only the tables 0032 and 0033 add columns to.
        CREATE TABLE projects (id TEXT PRIMARY KEY);
        CREATE TABLE images (id TEXT PRIMARY KEY);
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
        CREATE TABLE notes (id TEXT PRIMARY KEY, branch_id TEXT, session_id TEXT, updated_at INTEGER NOT NULL, completed_at INTEGER);
        -- Only the columns 0038 adds to and the 0039 repair reads.
        CREATE TABLE project_notes (id TEXT PRIMARY KEY, project_id TEXT, completed_at INTEGER);
        -- Only the table/column the 0026 auto-review cleanup targets.
        CREATE TABLE reviews (
            id         TEXT PRIMARY KEY,
            branch_id  TEXT,
            session_id TEXT,
            is_auto    INTEGER NOT NULL DEFAULT 0
        );
        -- Only the table/columns the 0034 backfill joins.
        CREATE TABLE commits (id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, session_id TEXT);
        -- The parent of the `sessions.branch_id` key 0022 adds, which the 0034
        -- backfill writes, with the `project_id` the 0039 repair joins on.
        CREATE TABLE branches (id TEXT PRIMARY KEY, project_id TEXT);
        CREATE TABLE repo_badges (
            github_repo TEXT NOT NULL,
            subpath     TEXT NOT NULL DEFAULT '',
            short_name  TEXT NOT NULL,
            hue         REAL NOT NULL,
            created_at  INTEGER NOT NULL,
            PRIMARY KEY (github_repo, subpath)
        );
        CREATE TABLE comments (id TEXT PRIMARY KEY);
        -- Only the columns the 0024 and 0028 backfills read and the 0030 drop
        -- targets; the rest of the real tables predates every migration below.
        CREATE TABLE action_contexts (
            id                 TEXT PRIMARY KEY,
            detecting_actions  INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            context_id  TEXT NOT NULL,
            action_type TEXT NOT NULL,
            sort_order  INTEGER NOT NULL,
            created_at  INTEGER NOT NULL,
            auto_commit INTEGER NOT NULL DEFAULT 0
        );
        ",
    )
    .unwrap();
    add_trigger_tables(&conn);
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);
    assert!(column_exists(&conn, "sessions", "provider_metadata"));
    assert!(column_exists(&conn, "sessions", "rate_limit_resets_at"));
    assert!(column_exists(&conn, "sessions", "rate_limit_kind"));
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
        -- Only the tables 0032 and 0033 add columns to.
        CREATE TABLE projects (id TEXT PRIMARY KEY);
        CREATE TABLE images (id TEXT PRIMARY KEY);
        -- `branch_id` is the 0022 column the 0034 backfill writes.
        CREATE TABLE sessions (
            id         TEXT PRIMARY KEY,
            status     TEXT NOT NULL,
            pipeline   TEXT,
            branch_id  TEXT,
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
        -- Only the table the 0025/0027 note column adds and the 0031 backfill target.
        CREATE TABLE notes (id TEXT PRIMARY KEY, branch_id TEXT, session_id TEXT, updated_at INTEGER NOT NULL, completed_at INTEGER);
        -- Only the columns 0038 adds to and the 0039 repair reads.
        CREATE TABLE project_notes (id TEXT PRIMARY KEY, project_id TEXT, completed_at INTEGER);
        -- Only the table/column the 0026 auto-review cleanup targets.
        CREATE TABLE reviews (
            id         TEXT PRIMARY KEY,
            branch_id  TEXT,
            session_id TEXT,
            is_auto    INTEGER NOT NULL DEFAULT 0
        );
        -- Only the table/columns the 0034 backfill joins.
        CREATE TABLE commits (id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, session_id TEXT);
        -- The parent of the `sessions.branch_id` key 0022 adds, which the 0034
        -- backfill writes, with the `project_id` the 0039 repair joins on.
        CREATE TABLE branches (id TEXT PRIMARY KEY, project_id TEXT);
        -- Only the columns the 0028 backfill reads and the 0030 drop targets.
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            context_id  TEXT NOT NULL,
            action_type TEXT NOT NULL,
            sort_order  INTEGER NOT NULL,
            created_at  INTEGER NOT NULL,
            auto_commit INTEGER NOT NULL DEFAULT 0
        );
        -- Only the tables the 0029, 0042, and 0043 ALTERs target; the rest of
        -- the real tables predate every migration below.
        CREATE TABLE session_messages (id INTEGER PRIMARY KEY AUTOINCREMENT);
        ",
    )
    .unwrap();
    add_trigger_tables(&conn);
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);
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
        -- Only the tables 0032 and 0033 add columns to.
        CREATE TABLE projects (id TEXT PRIMARY KEY);
        CREATE TABLE images (id TEXT PRIMARY KEY);
        CREATE TABLE reviews (
            id         TEXT PRIMARY KEY,
            branch_id  TEXT,
            session_id TEXT,
            is_auto    INTEGER NOT NULL DEFAULT 0
        );
        -- Only the table/columns the 0034 backfill joins.
        CREATE TABLE commits (id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, session_id TEXT);
        CREATE TABLE sessions (id TEXT PRIMARY KEY, branch_id TEXT);
        INSERT INTO reviews (id, is_auto) VALUES
            ('user-review', 0),
            ('auto-review', 1);
        -- Only the table the 0027 note column add and the 0031 backfill target,
        -- plus the `parent_project_note_id` (0025) the 0039 repair clears.
        CREATE TABLE notes (id TEXT PRIMARY KEY, branch_id TEXT, session_id TEXT, updated_at INTEGER NOT NULL, completed_at INTEGER, parent_project_note_id TEXT);
        -- Only the columns 0038 adds to and the 0039 repair reads.
        CREATE TABLE project_notes (id TEXT PRIMARY KEY, project_id TEXT, completed_at INTEGER);
        -- Only the columns the 0028 backfill reads and the 0030 drop targets.
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            context_id  TEXT NOT NULL,
            action_type TEXT NOT NULL,
            sort_order  INTEGER NOT NULL,
            created_at  INTEGER NOT NULL,
            auto_commit INTEGER NOT NULL DEFAULT 0
        );
        -- Only the tables the 0029, 0042, and 0043 ALTERs target; the rest of
        -- the real tables predate every migration below.
        CREATE TABLE session_messages (id INTEGER PRIMARY KEY AUTOINCREMENT);
        -- Only the table 0037 adds its column to, with the `project_id` the
        -- 0039 repair joins on.
        CREATE TABLE branches (id TEXT PRIMARY KEY, project_id TEXT);
        ",
    )
    .unwrap();
    add_trigger_tables(&conn);
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);
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
        -- Only the tables 0032 and 0033 add columns to.
        CREATE TABLE projects (id TEXT PRIMARY KEY);
        CREATE TABLE images (id TEXT PRIMARY KEY);
        CREATE TABLE action_contexts (
            id                 TEXT PRIMARY KEY,
            detecting_actions  INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO action_contexts (id, detecting_actions) VALUES
            ('wedged', 1),
            ('idle',   0);
        -- Only the table the 0025/0027 note column adds and the 0031 backfill target.
        CREATE TABLE notes (id TEXT PRIMARY KEY, branch_id TEXT, session_id TEXT, updated_at INTEGER NOT NULL, completed_at INTEGER);
        -- Only the columns 0038 adds to and the 0039 repair reads.
        CREATE TABLE project_notes (id TEXT PRIMARY KEY, project_id TEXT, completed_at INTEGER);
        -- Only the table/column the 0026 auto-review cleanup targets.
        CREATE TABLE reviews (
            id         TEXT PRIMARY KEY,
            branch_id  TEXT,
            session_id TEXT,
            is_auto    INTEGER NOT NULL DEFAULT 0
        );
        -- Only the table/columns the 0034 backfill joins.
        CREATE TABLE commits (id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, session_id TEXT);
        CREATE TABLE sessions (id TEXT PRIMARY KEY, branch_id TEXT);
        -- Only the columns the 0028 backfill reads and the 0030 drop targets.
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            context_id  TEXT NOT NULL,
            action_type TEXT NOT NULL,
            sort_order  INTEGER NOT NULL,
            created_at  INTEGER NOT NULL,
            auto_commit INTEGER NOT NULL DEFAULT 0
        );
        -- Only the tables the 0029, 0042, and 0043 ALTERs target; the rest of
        -- the real tables predate every migration below.
        CREATE TABLE session_messages (id INTEGER PRIMARY KEY AUTOINCREMENT);
        -- Only the table 0037 adds its column to, with the `project_id` the
        -- 0039 repair joins on.
        CREATE TABLE branches (id TEXT PRIMARY KEY, project_id TEXT);
        ",
    )
    .unwrap();
    add_trigger_tables(&conn);
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);
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
fn test_note_migrations_backfill_subtype_and_completion_for_session_less_notes() {
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
        -- Only the tables 0032 and 0033 add columns to.
        CREATE TABLE projects (id TEXT PRIMARY KEY);
        CREATE TABLE images (id TEXT PRIMARY KEY);
        CREATE TABLE notes (
            id                     TEXT PRIMARY KEY,
            branch_id              TEXT,
            session_id             TEXT,
            updated_at             INTEGER NOT NULL,
            completed_at           INTEGER,
            parent_project_note_id TEXT
        );
        INSERT INTO notes (id, session_id, updated_at, completed_at) VALUES
            ('dropped',   NULL,        100, NULL),
            ('with-body', NULL,        200,  150),
            ('agent',     'session-1', 300, NULL);
        -- Only the columns 0038 adds to and the 0039 repair reads.
        CREATE TABLE project_notes (id TEXT PRIMARY KEY, project_id TEXT, completed_at INTEGER);
        -- Only the table/column the 0026 auto-review cleanup targets.
        CREATE TABLE reviews (
            id         TEXT PRIMARY KEY,
            branch_id  TEXT,
            session_id TEXT,
            is_auto    INTEGER NOT NULL DEFAULT 0
        );
        -- Only the table/columns the 0034 backfill joins.
        CREATE TABLE commits (id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, session_id TEXT);
        CREATE TABLE sessions (id TEXT PRIMARY KEY, branch_id TEXT);
        -- Only the columns the 0028 backfill reads and the 0030 drop targets.
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            context_id  TEXT NOT NULL,
            action_type TEXT NOT NULL,
            sort_order  INTEGER NOT NULL,
            created_at  INTEGER NOT NULL,
            auto_commit INTEGER NOT NULL DEFAULT 0
        );
        -- Only the tables the 0029, 0042, and 0043 ALTERs target; the rest of
        -- the real tables predate every migration below.
        CREATE TABLE session_messages (id INTEGER PRIMARY KEY AUTOINCREMENT);
        -- Only the table 0037 adds its column to, with the `project_id` the
        -- 0039 repair joins on.
        CREATE TABLE branches (id TEXT PRIMARY KEY, project_id TEXT);
        ",
    )
    .unwrap();
    add_trigger_tables(&conn);
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);
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

    let completed_at = |id: &str| -> Option<i64> {
        conn.query_row(
            "SELECT completed_at FROM notes WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap()
    };

    // 0031: a session-less note is never pending, so an unset completion time
    // is backfilled from updated_at while an existing one is left alone. Only a
    // session-owned stub stays NULL — that one really may still be generating.
    assert_eq!(completed_at("dropped"), Some(100));
    assert_eq!(completed_at("with-body"), Some(150));
    assert_eq!(completed_at("agent"), None);

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
        -- Only the tables 0032 and 0033 add columns to.
        CREATE TABLE projects (id TEXT PRIMARY KEY);
        CREATE TABLE images (id TEXT PRIMARY KEY);
        -- Only the columns the 0028 backfill reads and the 0030 drop targets.
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            context_id  TEXT NOT NULL,
            action_type TEXT NOT NULL,
            sort_order  INTEGER NOT NULL,
            created_at  INTEGER NOT NULL,
            auto_commit INTEGER NOT NULL DEFAULT 0
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
            id         TEXT PRIMARY KEY,
            branch_id  TEXT,
            session_id TEXT,
            is_auto    INTEGER NOT NULL DEFAULT 0
        );
        -- Only the table/columns the 0034 backfill joins.
        CREATE TABLE commits (id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, session_id TEXT);
        CREATE TABLE sessions (id TEXT PRIMARY KEY, branch_id TEXT);
        -- Only the table the 0027 note column add and the 0031 backfill target,
        -- plus the `parent_project_note_id` (0025) the 0039 repair clears.
        CREATE TABLE notes (id TEXT PRIMARY KEY, branch_id TEXT, session_id TEXT, updated_at INTEGER NOT NULL, completed_at INTEGER, parent_project_note_id TEXT);
        -- Only the columns 0038 adds to and the 0039 repair reads.
        CREATE TABLE project_notes (id TEXT PRIMARY KEY, project_id TEXT, completed_at INTEGER);
        -- Only the tables the 0029, 0042, and 0043 ALTERs target; the rest of
        -- the real tables predate every migration below.
        CREATE TABLE session_messages (id INTEGER PRIMARY KEY AUTOINCREMENT);
        -- Only the table 0037 adds its column to, with the `project_id` the
        -- 0039 repair joins on.
        CREATE TABLE branches (id TEXT PRIMARY KEY, project_id TEXT);
        ",
    )
    .unwrap();
    add_trigger_tables(&conn);
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);
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

#[test]
fn test_auto_commit_removal_migration_drops_flag_and_keeps_actions() {
    let path = temp_db_path("auto-commit-removal");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 29;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        -- Only the tables 0032 and 0033 add columns to.
        CREATE TABLE projects (id TEXT PRIMARY KEY);
        CREATE TABLE images (id TEXT PRIMARY KEY);
        -- Only the column the 0030 drop targets; the rest of the real table
        -- predates it.
        CREATE TABLE repo_actions (
            id          TEXT PRIMARY KEY,
            auto_commit INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO repo_actions (id, auto_commit) VALUES
            ('format', 1),
            ('test',   0);
        -- Only the table the 0031 backfill targets, plus the
        -- `parent_project_note_id` (0025) the 0039 repair clears.
        CREATE TABLE notes (id TEXT PRIMARY KEY, branch_id TEXT, session_id TEXT, updated_at INTEGER NOT NULL, completed_at INTEGER, parent_project_note_id TEXT);
        -- Only the columns 0038 adds to and the 0039 repair reads.
        CREATE TABLE project_notes (id TEXT PRIMARY KEY, project_id TEXT, completed_at INTEGER);
        -- Only the tables/columns the 0034 backfill joins.
        CREATE TABLE sessions (id TEXT PRIMARY KEY, branch_id TEXT);
        CREATE TABLE commits (id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, session_id TEXT);
        CREATE TABLE reviews (id TEXT PRIMARY KEY, branch_id TEXT, session_id TEXT);
        -- Only the table 0037 adds its column to, with the `project_id` the
        -- 0039 repair joins on.
        CREATE TABLE branches (id TEXT PRIMARY KEY, project_id TEXT);
        ",
    )
    .unwrap();
    add_trigger_tables(&conn);
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);
    assert!(!column_exists(&conn, "repo_actions", "auto_commit"));

    // Only the flag goes: an action that had auto-commit enabled keeps running
    // its command, it just no longer commits afterwards.
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM repo_actions ORDER BY id ASC")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(ids, vec!["format".to_string(), "test".to_string()]);

    cleanup_db(&path);
}

#[test]
fn test_session_branch_id_backfill_links_artifact_backed_sessions() {
    let path = temp_db_path("session-branch-id-backfill");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 33;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        -- Only the columns the 0034 backfill reads and writes; the rest of the
        -- real tables predates it.
        CREATE TABLE sessions (id TEXT PRIMARY KEY, branch_id TEXT);
        INSERT INTO sessions (id, branch_id) VALUES
            ('commit-session',  NULL),
            ('note-session',    NULL),
            ('review-session',  NULL),
            ('push-session',    'branch-a'),
            ('project-session', NULL);
        CREATE TABLE commits (id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, session_id TEXT);
        INSERT INTO commits (id, branch_id, session_id) VALUES ('c1', 'branch-a', 'commit-session');
        CREATE TABLE notes (id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, session_id TEXT, completed_at INTEGER, parent_project_note_id TEXT);
        INSERT INTO notes (id, branch_id, session_id) VALUES
            ('n1', 'branch-b', 'note-session'),
            ('n2', 'branch-b', NULL);
        -- Only the columns 0038 adds to and the 0039 repair reads.
        CREATE TABLE project_notes (id TEXT PRIMARY KEY, project_id TEXT, completed_at INTEGER);
        CREATE TABLE reviews (id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, session_id TEXT);
        INSERT INTO reviews (id, branch_id, session_id) VALUES ('r1', 'branch-a', 'review-session');
        -- Only the table 0037 adds its column to, with the `project_id` the
        -- 0039 repair joins on.
        CREATE TABLE branches (id TEXT PRIMARY KEY, project_id TEXT);
        ",
    )
    .unwrap();
    add_trigger_tables(&conn);
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);

    let branch_of = |id: &str| -> Option<String> {
        conn.query_row(
            "SELECT branch_id FROM sessions WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap()
    };
    // Each artifact-backed session takes the branch of the row pointing at it.
    assert_eq!(branch_of("commit-session").as_deref(), Some("branch-a"));
    assert_eq!(branch_of("note-session").as_deref(), Some("branch-b"));
    assert_eq!(branch_of("review-session").as_deref(), Some("branch-a"));
    // A pipeline session that already recorded its branch is left alone.
    assert_eq!(branch_of("push-session").as_deref(), Some("branch-a"));
    // A session with no artifact is not branch work.
    assert_eq!(branch_of("project-session"), None);

    cleanup_db(&path);
}

#[test]
fn test_session_chat_read_at_migration_marks_existing_chats_read() {
    let path = temp_db_path("session-chat-read-at");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 34;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        -- Only the table 0035 adds its column to.
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
        INSERT INTO sessions (id) VALUES ('old-a'), ('old-b');
        -- Only the table 0037 adds its column to, with the `project_id` the
        -- 0039 repair joins on.
        CREATE TABLE branches (id TEXT PRIMARY KEY, project_id TEXT);
        -- Only the tables/columns 0038 and the 0039 repair touch.
        CREATE TABLE notes (id TEXT PRIMARY KEY, branch_id TEXT, completed_at INTEGER, parent_project_note_id TEXT);
        CREATE TABLE project_notes (id TEXT PRIMARY KEY, project_id TEXT, completed_at INTEGER);
        ",
    )
    .unwrap();
    add_trigger_tables(&conn);
    drop(conn);

    let migrated_from = super::now_timestamp() / 1000 * 1000;
    let store = Store::new(&path).unwrap();
    drop(store);
    let migrated_by = super::now_timestamp();

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);
    assert!(column_exists(&conn, "sessions", "chat_read_at"));

    let read_at = |id: &str| -> Option<i64> {
        conn.query_row(
            "SELECT chat_read_at FROM sessions WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap()
    };
    // Chats from before the upgrade read as read, in ms, so the upgrade lights
    // no unread dots.
    for id in ["old-a", "old-b"] {
        let at = read_at(id).unwrap();
        assert!((migrated_from..=migrated_by).contains(&at), "{id}: {at}");
    }
    // A session created afterwards has not been read.
    conn.execute("INSERT INTO sessions (id) VALUES ('new')", [])
        .unwrap();
    assert_eq!(read_at("new"), None);

    cleanup_db(&path);
}

#[test]
fn test_pr_title_migration_adds_an_empty_title_to_existing_branches() {
    let path = temp_db_path("pr-title");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 36;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        -- Only the table 0037 adds its column to, with the `project_id` the
        -- 0039 repair joins on.
        CREATE TABLE branches (id TEXT PRIMARY KEY, project_id TEXT, pr_number INTEGER);
        INSERT INTO branches (id, pr_number) VALUES ('with-pr', 123), ('no-pr', NULL);
        -- Only the tables/columns 0038 and the 0039 repair touch.
        CREATE TABLE notes (id TEXT PRIMARY KEY, branch_id TEXT, completed_at INTEGER, parent_project_note_id TEXT);
        CREATE TABLE project_notes (id TEXT PRIMARY KEY, project_id TEXT, completed_at INTEGER);
        -- The table 0040 adds its columns to.
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
        ",
    )
    .unwrap();
    add_trigger_tables(&conn);
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);
    assert!(column_exists(&conn, "branches", "pr_title"));

    // No backfill: a linked PR's title arrives with the next status poll.
    let untitled: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE pr_title IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(untitled, 2);

    cleanup_db(&path);
}

#[test]
fn test_note_read_at_migration_marks_landed_notes_read() {
    let path = temp_db_path("note-read-at");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 37;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        -- Only the tables 0038 adds its column to, with the columns the 0039
        -- repair reads and writes.
        CREATE TABLE branches (id TEXT PRIMARY KEY, project_id TEXT);
        CREATE TABLE notes (id TEXT PRIMARY KEY, branch_id TEXT, completed_at INTEGER, parent_project_note_id TEXT);
        CREATE TABLE project_notes (id TEXT PRIMARY KEY, project_id TEXT, completed_at INTEGER);
        INSERT INTO notes (id, completed_at) VALUES ('landed', 1000), ('writing', NULL);
        INSERT INTO project_notes (id, completed_at) VALUES ('landed', 2000), ('writing', NULL);
        -- The table 0040 adds its columns to.
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
        ",
    )
    .unwrap();
    add_trigger_tables(&conn);
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);

    for (table, landed_at) in [("notes", 1000), ("project_notes", 2000)] {
        assert!(column_exists(&conn, table, "read_at"));
        let read_at = |id: &str| -> Option<i64> {
            conn.query_row(
                &format!("SELECT read_at FROM {table} WHERE id = ?1"),
                params![id],
                |row| row.get(0),
            )
            .unwrap()
        };
        // Notes from before the upgrade read as read, so it lights no dots.
        assert_eq!(read_at("landed"), Some(landed_at), "{table}");
        // One still being written reads as unread when it lands.
        assert_eq!(read_at("writing"), None, "{table}");
    }

    cleanup_db(&path);
}

#[test]
fn test_child_note_repair_detaches_only_cross_project_parents() {
    let path = temp_db_path("child-note-cross-project");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 38;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        -- Only the columns the 0039 repair reads and writes.
        CREATE TABLE branches (id TEXT PRIMARY KEY, project_id TEXT NOT NULL);
        INSERT INTO branches (id, project_id) VALUES
            ('moved',  'target'),
            ('stayed', 'source');
        CREATE TABLE project_notes (id TEXT PRIMARY KEY, project_id TEXT NOT NULL);
        INSERT INTO project_notes (id, project_id) VALUES
            ('source-plan', 'source'),
            ('target-plan', 'target');
        CREATE TABLE notes (id TEXT PRIMARY KEY, branch_id TEXT NOT NULL, parent_project_note_id TEXT);
        INSERT INTO notes (id, branch_id, parent_project_note_id) VALUES
            ('moved-child',    'moved',       'source-plan'),
            ('stayed-child',   'stayed',      'source-plan'),
            ('target-child',   'moved',       'target-plan'),
            ('dangling-child', 'moved',       'deleted-plan'),
            ('orphan-child',   'gone-branch', 'source-plan'),
            ('plain',          'moved',       NULL);
        -- The table 0040 adds its columns to.
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
        ",
    )
    .unwrap();
    add_trigger_tables(&conn);
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);

    let parent_of = |id: &str| -> Option<String> {
        conn.query_row(
            "SELECT parent_project_note_id FROM notes WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap()
    };
    // A branch moved before moves cleared the link: its child is cut loose
    // from the project note it left behind.
    assert_eq!(parent_of("moved-child"), None);
    // Same-project links are what child notes are meant to have.
    assert_eq!(parent_of("stayed-child").as_deref(), Some("source-plan"));
    assert_eq!(parent_of("target-child").as_deref(), Some("target-plan"));
    // With its parent or its branch gone there is no project to compare, so
    // the link stays put.
    assert_eq!(parent_of("dangling-child").as_deref(), Some("deleted-plan"));
    assert_eq!(parent_of("orphan-child").as_deref(), Some("source-plan"));
    assert_eq!(parent_of("plain"), None);

    cleanup_db(&path);
}

#[test]
fn test_session_pre_head_and_blox_command_migration_leaves_existing_rows_null() {
    let path = temp_db_path("session-pre-head-blox-command");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 39;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        -- Only the table 0040 adds its columns to, plus the one 0042 alters
        -- so the run reaches the current version.
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
        INSERT INTO sessions (id) VALUES ('old');
        CREATE TABLE branches (id TEXT PRIMARY KEY);
        ",
    )
    .unwrap();
    add_trigger_tables(&conn);
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);
    let (pre_head, command): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT pre_head_sha, blox_command_id FROM sessions WHERE id = 'old'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(pre_head, None);
    assert_eq!(command, None);

    cleanup_db(&path);
}

#[test]
fn test_session_started_by_migration_leaves_existing_rows_null() {
    let path = temp_db_path("session-started-by");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 41;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        -- Only the table 0042 adds its column to, plus the table 0044 alters.
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
        INSERT INTO sessions (id) VALUES ('old');
        CREATE TABLE branches (id TEXT PRIMARY KEY);
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
    assert_eq!(version, 48);
    let started_by: Option<String> = conn
        .query_row(
            "SELECT started_by FROM sessions WHERE id = 'old'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        started_by, None,
        "a UI-started row predating 0042 reads NULL"
    );

    cleanup_db(&path);
}

#[test]
fn test_artifact_delete_cleanup_migration_spares_disconnected_sessions() {
    let path = temp_db_path("artifact-delete-spares-disconnected");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 40;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        CREATE TABLE sessions (id TEXT PRIMARY KEY, status TEXT NOT NULL);
        -- The table 0042 alters, so the run reaches the current version.
        CREATE TABLE branches (id TEXT PRIMARY KEY);
        CREATE TABLE commits (id TEXT PRIMARY KEY, session_id TEXT);
        CREATE TABLE notes (id TEXT PRIMARY KEY, session_id TEXT);
        CREATE TABLE reviews (id TEXT PRIMARY KEY, session_id TEXT);
        CREATE TABLE project_notes (id TEXT PRIMARY KEY, session_id TEXT);
        CREATE TABLE images (id TEXT PRIMARY KEY, session_id TEXT);
        -- The 0005 trigger, which spared only running sessions.
        CREATE TRIGGER trg_cleanup_session_after_commit_delete
        AFTER DELETE ON commits
        WHEN OLD.session_id IS NOT NULL
        BEGIN
            DELETE FROM sessions
            WHERE id = OLD.session_id AND status != 'running';
        END;
        INSERT INTO sessions (id, status) VALUES
            ('running', 'running'),
            ('disconnected', 'disconnected'),
            ('completed', 'completed');
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
    assert_eq!(version, 48);
    for (table, prefix) in [
        ("commits", "c"),
        ("notes", "n"),
        ("reviews", "r"),
        ("project_notes", "p"),
        ("images", "i"),
    ] {
        for session in ["running", "disconnected", "completed"] {
            conn.execute(
                &format!("INSERT INTO {table} (id, session_id) VALUES (?1, ?2)"),
                params![format!("{prefix}-{session}"), session],
            )
            .unwrap();
        }
    }
    for table in ["commits", "notes", "reviews", "project_notes", "images"] {
        conn.execute(&format!("DELETE FROM {table}"), []).unwrap();
    }
    let mut stmt = conn.prepare("SELECT id FROM sessions ORDER BY id").unwrap();
    let left = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    // A session whose turn may still be running remotely keeps its row once
    // its last artifact goes, like a running one; a finished one is cleaned up.
    assert_eq!(left, vec!["disconnected", "running"]);
    drop(stmt);

    cleanup_db(&path);
}

#[test]
fn test_control_grants_migration_creates_the_table_and_cascades_on_project() {
    let path = temp_db_path("control-grants");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 42;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        -- Only the table 0043 references, plus the tables 0044, 0045 and 0046
        -- alter.
        CREATE TABLE projects (id TEXT PRIMARY KEY);
        INSERT INTO projects (id) VALUES ('p1');
        CREATE TABLE branches (id TEXT PRIMARY KEY);
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
        ",
    )
    .unwrap();
    drop(conn);

    let store = Store::new(&path).unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);
    assert!(table_exists(&conn, "control_grants"));
    for column in [
        "id",
        "client",
        "scope_project_id",
        "access",
        "reconnect_secret_hash",
        "created_at",
        "expires_at",
        "last_used_at",
        "revoked_at",
    ] {
        assert!(
            column_exists(&conn, "control_grants", column),
            "missing column {column}"
        );
    }
    conn.execute(
        "INSERT INTO control_grants (id, client, scope_project_id, access,          reconnect_secret_hash, created_at, expires_at)          VALUES ('g1', 'agent', 'p1', 'write', 'h1', 1, 2)",
        [],
    )
    .unwrap();
    assert!(
        conn.execute(
            "INSERT INTO control_grants (id, client, scope_project_id, access,              reconnect_secret_hash, created_at, expires_at)              VALUES ('g2', 'agent', NULL, 'admin', 'h2', 1, 2)",
            [],
        )
        .is_err(),
        "access is CHECKed to read/write"
    );
    conn.execute("DELETE FROM projects WHERE id = 'p1'", [])
        .unwrap();
    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM control_grants", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining, 0, "a project-scoped grant goes with its project");

    cleanup_db(&path);
}

/// 0044 adds the deferred-push columns to `branches`; existing rows read as
/// not armed, and a fresh store round-trips the flag.
#[test]
fn test_branch_deferred_push_migration_leaves_existing_branches_unarmed() {
    let path = temp_db_path("branch-deferred-push");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 43;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        -- Only the table 0044 adds its columns to, plus the table 0045 and
        -- 0046 alter.
        CREATE TABLE branches (id TEXT PRIMARY KEY, project_id TEXT);
        INSERT INTO branches (id, project_id) VALUES ('a', 'p'), ('b', 'p');
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
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
    assert_eq!(version, 48);
    assert!(column_exists(&conn, "branches", "deferred_push"));
    assert!(column_exists(
        &conn,
        "branches",
        "deferred_push_requested_at"
    ));

    let unarmed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches
             WHERE deferred_push IS NULL AND deferred_push_requested_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unarmed, 2);
    drop(conn);

    cleanup_db(&path);
}

#[test]
fn test_fresh_store_has_the_deferred_push_columns() {
    let path = temp_db_path("fresh-deferred-push");
    let store = Store::new(&path).unwrap();
    let project = super::Project::new("owner/repo");
    store.create_project(&project).unwrap();
    let branch = super::Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let armed = super::DeferredPush {
        force: true,
        no_verify: false,
        requested_at: 42,
    };
    store
        .set_branch_deferred_push(&branch.id, Some(armed))
        .unwrap();
    assert_eq!(
        store.get_branch(&branch.id).unwrap().unwrap().deferred_push,
        Some(armed)
    );
    drop(store);

    cleanup_db(&path);
}

#[test]
fn test_project_tags_migration_then_import_moves_status_overrides_into_tags() {
    let path = temp_db_path("project-tags-import");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        PRAGMA user_version = 44;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        -- 0047 and 0048 only alter `sessions`.
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
        -- The projects table as of 0032 (baseline + status_override). 0044 only
        -- alters `branches`, so start past it.
        CREATE TABLE projects (
            id              TEXT PRIMARY KEY,
            name            TEXT NOT NULL,
            github_repo     TEXT,
            location        TEXT NOT NULL DEFAULT 'local',
            subpath         TEXT,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL,
            status_override TEXT
        );
        CREATE UNIQUE INDEX idx_projects_name ON projects(name);
        INSERT INTO projects (id, name, created_at, updated_at, status_override) VALUES
            ('p-custom', 'custom', 1, 1, 'c0ffee00-0000-4000-8000-000000000001'),
            ('p-builtin', 'builtin', 2, 2, 'done'),
            ('p-dangling', 'dangling', 3, 3, 'deleted-option'),
            ('p-default', 'default', 4, 4, NULL);
        ",
    )
    .unwrap();
    drop(conn);

    let store = Store::new(&path).unwrap();
    let prefs = serde_json::json!({
        "theme": "dark",
        "project-status-options": [
            { "id": "done", "label": "Done", "icon": "circle-check", "color": "green" },
            {
                "id": "c0ffee00-0000-4000-8000-000000000001",
                "label": "Waiting on design",
                "icon": "palette",
                "color": "purple"
            }
        ]
    });
    let options = super::parse_legacy_status_options(Some(&prefs));
    assert_eq!(
        store.import_legacy_project_statuses(&options).unwrap(),
        super::LegacyImportOutcome::Imported {
            tags: 2,
            assignments: 2,
            dropped_dangling: 1,
        }
    );
    // A later launch finds the marker gone.
    assert_eq!(
        store.import_legacy_project_statuses(&options).unwrap(),
        super::LegacyImportOutcome::AlreadyImported
    );
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);
    assert!(!table_exists(&conn, "legacy_project_status_import"));

    let group: (String, String, i64, i64) = conn
        .query_row(
            "SELECT id, name, position, is_icon_group FROM tag_groups",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(group, ("status".into(), "Status".into(), 0, 1));

    let mut stmt = conn
        .prepare("SELECT id, name, color, icon, position FROM tags ORDER BY position")
        .unwrap();
    let tags = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        tags,
        vec![
            (
                "done".to_string(),
                "Done".to_string(),
                "green".to_string(),
                "circle-check".to_string(),
                0
            ),
            (
                "c0ffee00-0000-4000-8000-000000000001".to_string(),
                "Waiting on design".to_string(),
                "purple".to_string(),
                "palette".to_string(),
                1
            ),
        ]
    );
    drop(stmt);

    let mut stmt = conn
        .prepare("SELECT project_id, tag_id, sort_order FROM project_tags ORDER BY project_id")
        .unwrap();
    let assignments = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        assignments,
        vec![
            ("p-builtin".to_string(), "done".to_string(), None),
            (
                "p-custom".to_string(),
                "c0ffee00-0000-4000-8000-000000000001".to_string(),
                None
            ),
        ]
    );
    drop(stmt);

    // The old column is frozen, not cleared: it stays a recovery source.
    let mut stmt = conn
        .prepare("SELECT id, status_override FROM projects ORDER BY id")
        .unwrap();
    let overrides = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        overrides,
        vec![
            ("p-builtin".to_string(), Some("done".to_string())),
            (
                "p-custom".to_string(),
                Some("c0ffee00-0000-4000-8000-000000000001".to_string())
            ),
            ("p-dangling".to_string(), Some("deleted-option".to_string())),
            ("p-default".to_string(), None),
        ]
    );
    drop(stmt);

    cleanup_db(&path);
}

#[test]
fn test_tag_group_sidebar_migration_adds_the_column_off_for_existing_groups() {
    let path = temp_db_path("tag-group-sidebar");
    let conn = Connection::open(&path).unwrap();
    // The tag tables as of 0045, with a group already in them.
    conn.execute_batch(
        "
        PRAGMA user_version = 45;
        CREATE TABLE app_metadata (
            id          INTEGER PRIMARY KEY CHECK (id = 1),
            app_version TEXT NOT NULL
        );
        INSERT INTO app_metadata (id, app_version) VALUES (1, '0.2.9');
        -- 0047 and 0048 only alter `sessions`.
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
        CREATE TABLE projects (
            id              TEXT PRIMARY KEY,
            name            TEXT NOT NULL,
            github_repo     TEXT,
            location        TEXT NOT NULL DEFAULT 'local',
            subpath         TEXT,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL,
            status_override TEXT
        );
        CREATE TABLE tag_groups (
            id            TEXT PRIMARY KEY,
            name          TEXT NOT NULL,
            position      INTEGER NOT NULL,
            is_icon_group INTEGER NOT NULL DEFAULT 0 CHECK (is_icon_group IN (0, 1)),
            created_at    INTEGER NOT NULL,
            updated_at    INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX idx_tag_groups_name ON tag_groups (name COLLATE NOCASE);
        CREATE UNIQUE INDEX idx_tag_groups_icon ON tag_groups (is_icon_group) WHERE is_icon_group = 1;
        CREATE TABLE tags (
            id         TEXT PRIMARY KEY,
            group_id   TEXT REFERENCES tag_groups (id),
            name       TEXT NOT NULL,
            color      TEXT NOT NULL CHECK (color IN ('green','red','amber','blue','purple','cyan','gray')),
            icon       TEXT,
            position   INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX idx_tags_scope_name ON tags (ifnull(group_id, ''), name COLLATE NOCASE);
        CREATE INDEX idx_tags_group ON tags (group_id, position);
        CREATE TABLE project_tags (
            project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
            tag_id     TEXT NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
            sort_order INTEGER,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (project_id, tag_id)
        );
        CREATE INDEX idx_project_tags_tag ON project_tags (tag_id);
        INSERT INTO tag_groups (id, name, position, is_icon_group, created_at, updated_at)
            VALUES ('status', 'Status', 0, 1, 1, 1);
        ",
    )
    .unwrap();
    drop(conn);

    let store = Store::new(&path).unwrap();
    let state = store.get_tag_state().unwrap();
    assert_eq!(state.groups.len(), 1);
    // An existing group's board starts unpinned, and the icon flag is kept.
    assert!(!state.groups[0].show_in_sidebar);
    assert!(state.groups[0].is_icon_group);
    store.set_tag_group_sidebar("status", true).unwrap();
    assert!(store.get_tag_state().unwrap().groups[0].show_in_sidebar);
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 48);
    assert!(column_exists(&conn, "tag_groups", "show_in_sidebar"));
    let shown: i64 = conn
        .query_row(
            "SELECT show_in_sidebar FROM tag_groups WHERE id = 'status'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(shown, 1);
    // The CHECK admits only 0 and 1.
    assert!(conn
        .execute("UPDATE tag_groups SET show_in_sidebar = 2", [])
        .is_err());

    cleanup_db(&path);
}
