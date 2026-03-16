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

#[test]
fn test_legacy_registry_covers_each_pre_baseline_schema_version() {
    let versions = migrations::legacy_step_versions();
    let expected: Vec<i64> = (1..=migrations::baseline_schema_version()).collect();
    assert_eq!(versions, expected);
}

#[test]
fn test_future_migration_registry_covers_post_baseline_schema_versions() {
    let baseline = migrations::baseline_schema_version();
    let versions = migrations::migration_versions();
    let expected: Vec<i64> = if baseline < super::SCHEMA_VERSION {
        ((baseline + 1)..=super::SCHEMA_VERSION).collect()
    } else {
        Vec::new()
    };
    assert_eq!(versions, expected);
}

fn seed_v1_database(path: &std::path::Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "
        CREATE TABLE schema_version (
            version     INTEGER NOT NULL,
            app_version TEXT
        );
        INSERT INTO schema_version (version, app_version) VALUES (1, '0.2.0');

        CREATE TABLE projects (
            id          TEXT PRIMARY KEY,
            repo_path   TEXT NOT NULL UNIQUE,
            subpath     TEXT,
            created_at  INTEGER NOT NULL,
            updated_at  INTEGER NOT NULL
        );

        CREATE TABLE branches (
            id              TEXT PRIMARY KEY,
            project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            branch_name     TEXT NOT NULL,
            base_branch     TEXT NOT NULL,
            pr_number       INTEGER,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL,
            UNIQUE(project_id, branch_name)
        );

        CREATE TABLE workdirs (
            id              TEXT PRIMARY KEY,
            project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            path            TEXT NOT NULL,
            branch_id       TEXT REFERENCES branches(id) ON DELETE SET NULL,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL,
            UNIQUE(project_id, path)
        );
        CREATE INDEX idx_workdirs_project ON workdirs(project_id);
        CREATE INDEX idx_workdirs_branch ON workdirs(branch_id);

        CREATE TABLE sessions (
            id              TEXT PRIMARY KEY,
            prompt          TEXT NOT NULL,
            status          TEXT NOT NULL,
            agent_id        TEXT,
            error_message   TEXT,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL
        );

        CREATE TABLE commits (
            id              TEXT PRIMARY KEY,
            branch_id       TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
            sha             TEXT,
            session_id      TEXT,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL
        );
        CREATE INDEX idx_commits_branch ON commits(branch_id);
        CREATE UNIQUE INDEX idx_commits_branch_sha
            ON commits(branch_id, sha) WHERE sha IS NOT NULL;

        CREATE TABLE session_messages (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            role            TEXT NOT NULL,
            content         TEXT NOT NULL,
            created_at      INTEGER NOT NULL
        );
        CREATE INDEX idx_session_messages_session
            ON session_messages(session_id);

        CREATE TABLE notes (
            id              TEXT PRIMARY KEY,
            branch_id       TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
            session_id      TEXT,
            title           TEXT NOT NULL,
            content         TEXT NOT NULL,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL
        );
        CREATE INDEX idx_notes_branch ON notes(branch_id);

        CREATE TABLE reviews (
            id              TEXT PRIMARY KEY,
            branch_id       TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
            commit_sha      TEXT NOT NULL,
            scope           TEXT NOT NULL,
            session_id      TEXT,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL,
            UNIQUE(branch_id, commit_sha, scope)
        );
        CREATE INDEX idx_reviews_branch ON reviews(branch_id);

        CREATE TABLE reviewed_files (
            review_id   TEXT NOT NULL REFERENCES reviews(id) ON DELETE CASCADE,
            path        TEXT NOT NULL,
            PRIMARY KEY (review_id, path)
        );

        CREATE TABLE comments (
            id          TEXT PRIMARY KEY,
            review_id   TEXT NOT NULL REFERENCES reviews(id) ON DELETE CASCADE,
            path        TEXT NOT NULL,
            span_start  INTEGER NOT NULL,
            span_end    INTEGER NOT NULL,
            content     TEXT NOT NULL,
            created_at  INTEGER NOT NULL
        );
        CREATE INDEX idx_comments_review ON comments(review_id);

        CREATE TABLE reference_files (
            review_id   TEXT NOT NULL REFERENCES reviews(id) ON DELETE CASCADE,
            path        TEXT NOT NULL,
            PRIMARY KEY (review_id, path)
        );

        CREATE TABLE project_actions (
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
        CREATE INDEX idx_project_actions_project
            ON project_actions(project_id);
        ",
    )
    .unwrap();

    conn.execute(
        "INSERT INTO projects (id, repo_path, subpath, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params!["project-1", "/tmp/example/repo", "packages/app", 100, 100],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO branches (id, project_id, branch_name, base_branch, pr_number, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params!["branch-1", "project-1", "feature/auth", "main", 7, 110, 110],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO workdirs (id, project_id, path, branch_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            "workdir-1",
            "project-1",
            "/tmp/example/worktrees/feature-auth",
            "branch-1",
            115,
            115
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, prompt, status, agent_id, error_message, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            "session-1",
            "fix auth flow",
            "completed",
            "codex",
            None::<String>,
            120,
            120
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO commits (id, branch_id, sha, session_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params!["commit-1", "branch-1", "abc123", "session-1", 130, 130],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_messages (session_id, role, content, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params!["session-1", "user", "please fix auth", 121],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO notes (id, branch_id, session_id, title, content, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            "note-1",
            "branch-1",
            "session-1",
            "Auth note",
            "Remember CSRF validation",
            140,
            140
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO reviews (id, branch_id, commit_sha, scope, session_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            "review-1",
            "branch-1",
            "abc123",
            "commit",
            "session-1",
            150,
            150
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO comments (id, review_id, path, span_start, span_end, content, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            "comment-1",
            "review-1",
            "src/auth.rs",
            1,
            2,
            "Looks good",
            151
        ],
    )
    .unwrap();
}

#[test]
fn test_check_db_compatibility_requires_reset_for_unversioned_beta_db() {
    let path = temp_db_path("compat-pre-versioning");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE sessions (id TEXT PRIMARY KEY);")
        .unwrap();
    drop(conn);

    let compat = check_db_compatibility(&path).unwrap();
    match compat {
        DbCompatibility::NeedsReset { db_app_version } => {
            assert_eq!(db_app_version, "0.1.0");
        }
        other => panic!("expected reset for unversioned database, got {other:?}"),
    }

    cleanup_db(&path);
}

#[test]
fn test_check_db_compatibility_requires_reset_for_partial_version_zero_db() {
    let path = temp_db_path("compat-version-zero");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "
        CREATE TABLE schema_version (
            version     INTEGER NOT NULL,
            app_version TEXT
        );
        INSERT INTO schema_version (version, app_version) VALUES (0, '0.2.5');
        CREATE TABLE sessions (id TEXT PRIMARY KEY);
        ",
    )
    .unwrap();
    drop(conn);

    let compat = check_db_compatibility(&path).unwrap();
    match compat {
        DbCompatibility::NeedsReset { db_app_version } => {
            assert_eq!(db_app_version, "0.2.5");
        }
        other => panic!("expected reset for partial version-zero database, got {other:?}"),
    }

    cleanup_db(&path);
}

#[test]
fn test_migrates_v1_database_without_reset() {
    let path = temp_db_path("migrate-v1");
    seed_v1_database(&path);

    assert!(matches!(
        check_db_compatibility(&path).unwrap(),
        DbCompatibility::Ok
    ));

    let store = Store::new(&path).unwrap();

    let project = store.get_project("project-1").unwrap().unwrap();
    assert_eq!(project.name, "repo (packages/app)");
    assert!(project.github_repo.is_none());
    assert_eq!(project.subpath.as_deref(), Some("packages/app"));
    assert!(store.list_project_repos("project-1").unwrap().is_empty());

    let session = store.get_session("session-1").unwrap().unwrap();
    assert_eq!(session.working_dir, "/tmp/example/worktrees/feature-auth");

    let review = store.get_review("review-1").unwrap().unwrap();
    assert_eq!(review.title, None);
    assert!(!review.is_auto);

    drop(store);

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, super::SCHEMA_VERSION);
    assert!(table_exists(&conn, "project_notes"));
    assert!(table_exists(&conn, "repo_actions"));
    assert!(!table_exists(&conn, "project_actions"));

    cleanup_db(&path);
}

#[test]
fn test_migrates_v10_project_actions_to_repo_actions() {
    let path = temp_db_path("migrate-v10-actions");
    let conn = Connection::open(&path).unwrap();
    migrations::initialize_to_version(&conn, 10).unwrap();

    conn.execute(
        "INSERT INTO projects (id, name, github_repo, location, subpath, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            "project-1",
            "Demo",
            "owner/repo",
            "local",
            "packages/app",
            100,
            100
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO project_repos (id, project_id, github_repo, branch_name, subpath, is_primary, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            "repo-1",
            "project-1",
            "owner/repo",
            "feature/demo",
            "packages/app",
            1,
            101,
            101
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO branches (
            id, project_id, project_repo_id, branch_name, base_branch, pr_number, branch_type,
            workspace_name, workspace_status, agent, pr_state, pr_checks_status, pr_review_decision,
            pr_mergeable, pr_draft, pr_url, pr_updated_at, pr_fetched_at, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'local', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, ?6, ?7)",
        params!["branch-1", "project-1", "repo-1", "feature/demo", "origin/main", 102, 102],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO project_actions (id, project_id, name, command, action_type, sort_order, auto_commit, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            "action-1",
            "project-1",
            "Build",
            "pnpm build",
            "build",
            0,
            0,
            110,
            110
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO project_actions (id, project_id, name, command, action_type, sort_order, auto_commit, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            "action-2",
            "project-1",
            "Test",
            "pnpm test",
            "test",
            1,
            1,
            111,
            111
        ],
    )
    .unwrap();
    drop(conn);

    let store = Store::new(&path).unwrap();
    let context = store
        .get_action_context_by_repo_and_subpath("owner/repo", Some("packages/app"))
        .unwrap()
        .unwrap();
    let actions = store.list_repo_actions(&context.id).unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].id, "action-1");
    assert_eq!(actions[0].sort_order, 0);
    assert_eq!(actions[1].id, "action-2");
    assert_eq!(actions[1].sort_order, 1);
    assert!(actions[1].auto_commit);
    drop(store);

    let conn = Connection::open(&path).unwrap();
    assert!(!table_exists(&conn, "project_actions"));
    assert!(table_exists(&conn, "repo_actions"));

    cleanup_db(&path);
}

#[test]
fn test_migrates_v14_project_notes_through_latest_schema() {
    let path = temp_db_path("migrate-v14-notes");
    let conn = Connection::open(&path).unwrap();
    migrations::initialize_to_version(&conn, 14).unwrap();

    conn.execute(
        "INSERT INTO projects (id, name, github_repo, location, subpath, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            "project-1",
            "Demo",
            "owner/repo",
            "local",
            Option::<String>::None,
            100,
            100
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO project_repos (id, project_id, github_repo, branch_name, subpath, is_primary, reason, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, NULL, 1, ?5, ?6, ?7)",
        params!["repo-1", "project-1", "owner/repo", "feature/demo", "seed reason", 101, 101],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, prompt, status, working_dir, provider, agent_id, error_message, created_at, updated_at, owner_pid)
         VALUES (?1, ?2, ?3, ?4, NULL, NULL, NULL, ?5, ?6, NULL)",
        params!["session-1", "write note", "completed", "/tmp/demo", 102, 102],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO project_notes (id, project_id, session_id, title, content, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            "project-note-1",
            "project-1",
            "session-1",
            "Migration note",
            "Still here",
            103,
            103
        ],
    )
    .unwrap();
    drop(conn);

    let store = Store::new(&path).unwrap();
    let note = store.get_project_note("project-note-1").unwrap().unwrap();
    assert_eq!(note.title, "Migration note");
    assert_eq!(note.content, "Still here");
    let session = store.get_session("session-1").unwrap().unwrap();
    assert_eq!(session.working_dir, "/tmp/demo");
    drop(store);

    let conn = Connection::open(&path).unwrap();
    assert!(table_exists(&conn, "images"));
    let version: i64 = conn
        .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, super::SCHEMA_VERSION);

    cleanup_db(&path);
}
