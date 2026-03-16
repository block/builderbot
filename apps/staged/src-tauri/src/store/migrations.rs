//! SQLite schema bootstrap and migrations for the Staged store.
//!
//! Fresh installs bootstrap from `store/migrations/baseline.sql`.
//! Legacy versioned databases (before the baseline) are upgraded by the
//! compatibility steps in this file.
//! New schema changes after the baseline should be appended as ordered SQL
//! migrations in `store/migrations/`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use super::{StoreError, APP_VERSION, SCHEMA_VERSION};

const BASELINE_SCHEMA_VERSION: i64 = 22;
const BASELINE_SCHEMA_SQL: &str = include_str!("migrations/baseline.sql");

struct MigrationSpec {
    version: i64,
    name: &'static str,
    body: MigrationBody,
}

enum MigrationBody {
    Sql(&'static str),
    Rust(fn(&Connection) -> Result<(), StoreError>),
}

impl MigrationSpec {
    const fn sql(version: i64, name: &'static str, sql: &'static str) -> Self {
        Self {
            version,
            name,
            body: MigrationBody::Sql(sql),
        }
    }

    const fn rust(
        version: i64,
        name: &'static str,
        migration: fn(&Connection) -> Result<(), StoreError>,
    ) -> Self {
        Self {
            version,
            name,
            body: MigrationBody::Rust(migration),
        }
    }

    fn apply(&self, conn: &Connection) -> Result<(), StoreError> {
        match self.body {
            MigrationBody::Sql(sql) => execute_sql(conn, sql),
            MigrationBody::Rust(migration) => migration(conn),
        }
    }
}

const LEGACY_STEPS: &[MigrationSpec] = &[
    MigrationSpec::rust(1, "initial schema", migration_1_initial_schema),
    MigrationSpec::sql(
        2,
        "add session working_dir",
        LEGACY_SQL_0002_ADD_SESSION_WORKING_DIR,
    ),
    MigrationSpec::sql(
        3,
        "add session provider",
        LEGACY_SQL_0003_ADD_SESSION_PROVIDER,
    ),
    MigrationSpec::sql(
        4,
        "add remote branch fields",
        LEGACY_SQL_0004_ADD_REMOTE_BRANCH_FIELDS,
    ),
    MigrationSpec::sql(
        5,
        "rename projects.repo_path to github_repo",
        LEGACY_SQL_0005_PROJECTS_REPO_PATH_TO_GITHUB_REPO,
    ),
    MigrationSpec::sql(
        6,
        "allow per-subpath project uniqueness",
        LEGACY_SQL_0006_PROJECTS_REPO_SUBPATH_UNIQUENESS,
    ),
    MigrationSpec::rust(
        7,
        "relax review uniqueness and add comment author",
        migration_7_review_relaxation,
    ),
    MigrationSpec::rust(
        8,
        "split projects into project_repos",
        migration_8_project_repo_split,
    ),
    MigrationSpec::rust(
        9,
        "add project location and project_repo branch name",
        migration_9_project_location_and_repo_branch_name,
    ),
    MigrationSpec::sql(
        10,
        "add branch PR metadata",
        LEGACY_SQL_0010_BRANCH_PR_METADATA,
    ),
    MigrationSpec::rust(
        11,
        "move project actions into action contexts",
        migration_11_action_contexts,
    ),
    MigrationSpec::sql(
        12,
        "add sessions.owner_pid and recent_repos",
        LEGACY_SQL_0012_SESSIONS_OWNER_PID_AND_RECENT_REPOS,
    ),
    MigrationSpec::sql(
        13,
        "add comments.comment_type",
        LEGACY_SQL_0013_COMMENTS_COMMENT_TYPE,
    ),
    MigrationSpec::rust(
        14,
        "add project_notes and project_repo reason",
        migration_14_project_notes_and_repo_reason,
    ),
    MigrationSpec::sql(
        15,
        "add repo_actions.run_detection_mode",
        LEGACY_SQL_0015_REPO_ACTIONS_RUN_DETECTION_MODE,
    ),
    MigrationSpec::sql(
        16,
        "reserved workstation_id migration",
        LEGACY_SQL_0016_REMOVED_WORKSTATION_ID_NOOP,
    ),
    MigrationSpec::sql(
        17,
        "reserved workstation_id cleanup migration",
        LEGACY_SQL_0017_REMOVED_WORKSTATION_ID_NOOP,
    ),
    MigrationSpec::sql(18, "add reviews.title", LEGACY_SQL_0018_REVIEWS_TITLE),
    MigrationSpec::sql(
        19,
        "reserved image rollout migration",
        LEGACY_SQL_0019_IMAGES_ROLLOUT_NOOP,
    ),
    MigrationSpec::sql(
        20,
        "reserved image rollout cleanup migration",
        LEGACY_SQL_0020_IMAGES_ROLLOUT_NOOP,
    ),
    MigrationSpec::rust(21, "add images support", migration_21_images),
    MigrationSpec::sql(22, "add reviews.is_auto", LEGACY_SQL_0022_REVIEWS_IS_AUTO),
];

// Append new migrations here once the baseline schema version advances.
const MIGRATIONS: &[MigrationSpec] = &[];

enum DatabaseState {
    Empty,
    Versioned { version: i64, has_user_tables: bool },
}

pub(super) fn initialize(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

    let mut current_version = match detect_database_state(conn)? {
        DatabaseState::Empty => {
            bootstrap_baseline(conn)?;
            BASELINE_SCHEMA_VERSION
        }
        DatabaseState::Versioned {
            version: 0,
            has_user_tables: false,
        } => {
            bootstrap_baseline(conn)?;
            BASELINE_SCHEMA_VERSION
        }
        DatabaseState::Versioned {
            version: 0,
            has_user_tables: true,
        } => {
            return Err(StoreError(
                "Database schema version 0 is only valid for an empty database".to_string(),
            ));
        }
        DatabaseState::Versioned { version, .. } => {
            if version > SCHEMA_VERSION {
                return Err(StoreError(format!(
                    "Database schema version {version} is newer than supported version {SCHEMA_VERSION}"
                )));
            }

            if version < BASELINE_SCHEMA_VERSION {
                apply_legacy_upgrades(conn, version, BASELINE_SCHEMA_VERSION)?;
                BASELINE_SCHEMA_VERSION
            } else {
                version
            }
        }
    };

    if current_version < SCHEMA_VERSION {
        apply_pending_migrations(conn, current_version)?;
        current_version = SCHEMA_VERSION;
    }

    // These triggers are derived from the current schema and cheap to
    // rebuild, so we treat them as self-healing bootstrapped state.
    let (include_project_notes, include_images) = trigger_flags_for_version(current_version);
    recreate_cleanup_triggers(conn, include_project_notes, include_images)?;

    conn.execute("UPDATE schema_version SET app_version = ?1", [APP_VERSION])?;
    Ok(())
}

#[cfg(test)]
pub(super) fn initialize_to_version(
    conn: &Connection,
    target_version: i64,
) -> Result<(), StoreError> {
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

    if target_version > SCHEMA_VERSION {
        return Err(StoreError(format!(
            "Cannot initialize test database past schema version {SCHEMA_VERSION}: {target_version}"
        )));
    }

    let mut current_version = match detect_database_state(conn)? {
        DatabaseState::Empty => {
            ensure_schema_version_row(conn, 0)?;
            0
        }
        DatabaseState::Versioned {
            version: 0,
            has_user_tables: false,
        } => {
            ensure_schema_version_row(conn, 0)?;
            0
        }
        DatabaseState::Versioned {
            version: 0,
            has_user_tables: true,
        } => {
            return Err(StoreError(
                "Cannot initialize a test database from a partially initialized schema".to_string(),
            ));
        }
        DatabaseState::Versioned { version, .. } => version,
    };

    if current_version > target_version {
        return Err(StoreError(format!(
            "Cannot roll test database back from schema version {current_version} to {target_version}"
        )));
    }

    if current_version < target_version && current_version < BASELINE_SCHEMA_VERSION {
        let legacy_target = target_version.min(BASELINE_SCHEMA_VERSION);
        apply_legacy_upgrades(conn, current_version, legacy_target)?;
        current_version = legacy_target;
    }

    if current_version < target_version {
        apply_future_migrations_through(conn, current_version, target_version)?;
    }

    conn.execute("UPDATE schema_version SET app_version = ?1", [APP_VERSION])?;
    Ok(())
}

#[cfg(test)]
pub(super) fn baseline_schema_version() -> i64 {
    BASELINE_SCHEMA_VERSION
}

#[cfg(test)]
pub(super) fn legacy_step_versions() -> Vec<i64> {
    LEGACY_STEPS
        .iter()
        .map(|migration| migration.version)
        .collect()
}

#[cfg(test)]
pub(super) fn migration_versions() -> Vec<i64> {
    MIGRATIONS
        .iter()
        .map(|migration| migration.version)
        .collect()
}

fn detect_database_state(conn: &Connection) -> Result<DatabaseState, StoreError> {
    if table_exists(conn, "schema_version")? {
        let version = conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?;

        let version = match version {
            Some(version) => version,
            None => {
                ensure_schema_version_row(conn, 0)?;
                0
            }
        };

        return Ok(DatabaseState::Versioned {
            version,
            has_user_tables: has_user_tables(conn)?,
        });
    }

    if has_user_tables(conn)? {
        return Err(StoreError(
            "Database uses an unsupported pre-versioning schema; reset required".to_string(),
        ));
    }

    Ok(DatabaseState::Empty)
}

fn bootstrap_baseline(conn: &Connection) -> Result<(), StoreError> {
    if has_user_tables(conn)? {
        return Err(StoreError(
            "Cannot bootstrap the staged store baseline over an existing schema".to_string(),
        ));
    }

    conn.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE;")?;

    let result = (|| {
        ensure_schema_version_table(conn)?;
        drop_cleanup_triggers(conn)?;
        execute_sql(conn, BASELINE_SCHEMA_SQL)?;
        set_schema_version(conn, BASELINE_SCHEMA_VERSION)?;
        let (include_project_notes, include_images) =
            trigger_flags_for_version(BASELINE_SCHEMA_VERSION);
        recreate_cleanup_triggers(conn, include_project_notes, include_images)?;
        assert_integrity(conn)?;
        Ok(())
    })();

    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT; PRAGMA foreign_keys=ON;")?;
            Ok(())
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK; PRAGMA foreign_keys=ON;");
            Err(err)
        }
    }
}

fn ensure_schema_version_table(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version     INTEGER NOT NULL,
            app_version TEXT
        );",
    )?;
    Ok(())
}

fn ensure_schema_version_row(conn: &Connection, default_version: i64) -> Result<(), StoreError> {
    ensure_schema_version_table(conn)?;
    let version = conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
            row.get::<_, i64>(0)
        })
        .optional()?;
    if version.is_none() {
        set_schema_version(conn, default_version)?;
    }
    Ok(())
}

fn set_schema_version(conn: &Connection, version: i64) -> Result<(), StoreError> {
    ensure_schema_version_table(conn)?;
    conn.execute("DELETE FROM schema_version", [])?;
    conn.execute(
        "INSERT INTO schema_version (version, app_version) VALUES (?1, ?2)",
        params![version, APP_VERSION],
    )?;
    Ok(())
}

fn has_user_tables(conn: &Connection) -> Result<bool, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*)
         FROM sqlite_master
         WHERE type = 'table'
           AND name NOT LIKE 'sqlite_%'
           AND name != 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn apply_pending_migrations(conn: &Connection, current_version: i64) -> Result<(), StoreError> {
    apply_future_migrations_through(conn, current_version, SCHEMA_VERSION)
}

fn apply_future_migrations_through(
    conn: &Connection,
    current_version: i64,
    target_version: i64,
) -> Result<(), StoreError> {
    if current_version >= target_version {
        return Ok(());
    }

    validate_future_registry()?;
    apply_registry_range(conn, current_version, target_version, MIGRATIONS)
}

fn apply_legacy_upgrades(
    conn: &Connection,
    current_version: i64,
    target_version: i64,
) -> Result<(), StoreError> {
    if current_version >= target_version {
        return Ok(());
    }

    validate_legacy_registry()?;
    apply_registry_range(conn, current_version, target_version, LEGACY_STEPS)
}

fn apply_registry_range(
    conn: &Connection,
    current_version: i64,
    target_version: i64,
    registry: &[MigrationSpec],
) -> Result<(), StoreError> {
    ensure_schema_version_row(conn, current_version)?;

    conn.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE;")?;

    let result = (|| {
        drop_cleanup_triggers(conn)?;

        for version in (current_version + 1)..=target_version {
            apply_migration(conn, registry, version)?;
            set_schema_version(conn, version)?;
        }

        if target_version >= 1 {
            let (include_project_notes, include_images) = trigger_flags_for_version(target_version);
            recreate_cleanup_triggers(conn, include_project_notes, include_images)?;
        }

        assert_integrity(conn)?;
        Ok(())
    })();

    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT; PRAGMA foreign_keys=ON;")?;
            Ok(())
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK; PRAGMA foreign_keys=ON;");
            Err(err)
        }
    }
}

fn validate_legacy_registry() -> Result<(), StoreError> {
    if LEGACY_STEPS.len() != BASELINE_SCHEMA_VERSION as usize {
        return Err(StoreError(format!(
            "Legacy step registry length {} does not match baseline schema version {}",
            LEGACY_STEPS.len(),
            BASELINE_SCHEMA_VERSION
        )));
    }

    for (index, migration) in LEGACY_STEPS.iter().enumerate() {
        let expected_version = index as i64 + 1;
        if migration.version != expected_version {
            return Err(StoreError(format!(
                "Legacy step registry is missing version {expected_version}; found version {} at index {}",
                migration.version, index
            )));
        }
    }

    Ok(())
}

fn validate_future_registry() -> Result<(), StoreError> {
    if BASELINE_SCHEMA_VERSION > SCHEMA_VERSION {
        return Err(StoreError(format!(
            "Baseline schema version {BASELINE_SCHEMA_VERSION} cannot exceed schema version {SCHEMA_VERSION}"
        )));
    }

    let expected_len = (SCHEMA_VERSION - BASELINE_SCHEMA_VERSION) as usize;
    if MIGRATIONS.len() != expected_len {
        return Err(StoreError(format!(
            "Migration registry length {} does not match expected count {} for schema version {}",
            MIGRATIONS.len(),
            expected_len,
            SCHEMA_VERSION
        )));
    }

    for (index, migration) in MIGRATIONS.iter().enumerate() {
        let expected_version = BASELINE_SCHEMA_VERSION + index as i64 + 1;
        if migration.version != expected_version {
            return Err(StoreError(format!(
                "Migration registry is missing version {expected_version}; found version {} at index {}",
                migration.version, index
            )));
        }
    }

    Ok(())
}

fn assert_integrity(conn: &Connection) -> Result<(), StoreError> {
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(StoreError(format!(
            "SQLite integrity_check failed after migrations: {integrity}"
        )));
    }

    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let mut rows = stmt.query([])?;
    if let Some(row) = rows.next()? {
        let table: String = row.get(0)?;
        let row_id: i64 = row.get(1)?;
        let parent: String = row.get(2)?;
        return Err(StoreError(format!(
            "SQLite foreign_key_check failed after migrations: table={table} rowid={row_id} parent={parent}"
        )));
    }

    Ok(())
}

fn apply_migration(
    conn: &Connection,
    registry: &[MigrationSpec],
    version: i64,
) -> Result<(), StoreError> {
    let migration = registry
        .iter()
        .find(|migration| migration.version == version)
        .ok_or_else(|| StoreError(format!("Unknown migration version: {version}")))?;

    log::info!(
        "Applying staged store migration {:04}: {}",
        migration.version,
        migration.name
    );

    migration.apply(conn)
}

fn execute_sql(conn: &Connection, sql: &str) -> Result<(), StoreError> {
    conn.execute_batch(sql)?;
    Ok(())
}

fn migration_1_initial_schema(conn: &Connection) -> Result<(), StoreError> {
    execute_sql(conn, LEGACY_SQL_0001_INITIAL_SCHEMA)
}

fn migration_7_review_relaxation(conn: &Connection) -> Result<(), StoreError> {
    execute_sql(
        conn,
        LEGACY_SQL_0007_REVIEWS_RELAX_UNIQUE_AND_COMMENTS_AUTHOR,
    )
}

fn migration_8_project_repo_split(conn: &Connection) -> Result<(), StoreError> {
    execute_sql(conn, LEGACY_SQL_0008_PROJECT_REPO_SPLIT_SETUP)?;

    let legacy_projects = {
        let mut stmt = conn.prepare(
            "SELECT id, github_repo, subpath, created_at, updated_at
             FROM projects
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    let mut used_names = HashSet::new();
    let mut project_repo_ids = HashMap::new();

    for (project_id, github_repo, subpath, created_at, updated_at) in legacy_projects {
        let normalized_repo = normalize_github_repo(&github_repo);
        let project_name = make_unique_project_name(
            &mut used_names,
            normalized_repo.as_deref().unwrap_or(&github_repo),
            subpath.as_deref(),
        );

        conn.execute(
            "INSERT INTO projects_next (id, name, github_repo, subpath, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                project_id,
                project_name,
                normalized_repo,
                subpath,
                created_at,
                updated_at,
            ],
        )?;

        if let Some(repo_slug) = normalize_github_repo(&github_repo) {
            let repo_id = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO project_repos (id, project_id, github_repo, subpath, is_primary, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
                params![repo_id, project_id, repo_slug, subpath, created_at, updated_at],
            )?;
            project_repo_ids.insert(project_id, repo_id);
        }
    }

    let legacy_branches = {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, branch_name, base_branch, pr_number, branch_type, workspace_name, workspace_status, agent, created_at, updated_at
             FROM branches
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, i64>(10)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    for (
        branch_id,
        project_id,
        branch_name,
        base_branch,
        pr_number,
        branch_type,
        workspace_name,
        workspace_status,
        agent,
        created_at,
        updated_at,
    ) in legacy_branches
    {
        conn.execute(
            "INSERT INTO branches_next (
                id, project_id, project_repo_id, branch_name, base_branch, pr_number,
                branch_type, workspace_name, workspace_status, agent, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                branch_id,
                project_id,
                project_repo_ids.get(&project_id),
                branch_name,
                base_branch,
                pr_number,
                branch_type,
                workspace_name,
                workspace_status,
                agent,
                created_at,
                updated_at,
            ],
        )?;
    }

    execute_sql(conn, LEGACY_SQL_0008_PROJECT_REPO_SPLIT_FINALIZE)
}

fn migration_9_project_location_and_repo_branch_name(conn: &Connection) -> Result<(), StoreError> {
    execute_sql(conn, LEGACY_SQL_0009_PROJECT_LOCATION_AND_REPO_BRANCH_NAME)?;

    let project_repos = {
        let mut stmt = conn.prepare(
            "SELECT id, project_id
             FROM project_repos
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    for (repo_id, project_id) in project_repos {
        let branch_name = conn
            .query_row(
                "SELECT branch_name
                 FROM branches
                 WHERE project_repo_id = ?1
                 ORDER BY created_at ASC
                 LIMIT 1",
                params![repo_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .or_else(|| {
                conn.query_row(
                    "SELECT branch_name
                     FROM branches
                     WHERE project_id = ?1
                     ORDER BY created_at ASC
                     LIMIT 1",
                    params![project_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .ok()
                .flatten()
            })
            .unwrap_or_else(|| "main".to_string());

        conn.execute(
            "UPDATE project_repos SET branch_name = ?1 WHERE id = ?2",
            params![branch_name, repo_id],
        )?;
    }

    Ok(())
}

fn migration_11_action_contexts(conn: &Connection) -> Result<(), StoreError> {
    execute_sql(conn, LEGACY_SQL_0011_ACTION_CONTEXTS_AND_REPO_ACTIONS)?;

    if !table_exists(conn, "project_actions")? {
        return Ok(());
    }

    let mut project_contexts: HashMap<String, (String, Option<String>)> = HashMap::new();

    {
        let mut stmt = conn.prepare(
            "SELECT project_id, github_repo, subpath
             FROM project_repos
             WHERE is_primary = 1
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (project_id, github_repo, subpath) = row?;
            if let Some(repo_slug) = normalize_github_repo(&github_repo) {
                project_contexts.insert(project_id, (repo_slug, normalize_subpath(subpath)));
            }
        }
    }

    {
        let mut stmt = conn.prepare(
            "SELECT project_id, github_repo, subpath
             FROM project_repos
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (project_id, github_repo, subpath) = row?;
            if project_contexts.contains_key(&project_id) {
                continue;
            }
            if let Some(repo_slug) = normalize_github_repo(&github_repo) {
                project_contexts.insert(project_id, (repo_slug, normalize_subpath(subpath)));
            }
        }
    }

    {
        let mut stmt = conn.prepare(
            "SELECT id, github_repo, subpath
             FROM projects
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (project_id, github_repo, subpath) = row?;
            if project_contexts.contains_key(&project_id) {
                continue;
            }
            if let Some(repo_slug) = github_repo.and_then(|repo| normalize_github_repo(&repo)) {
                project_contexts.insert(project_id, (repo_slug, normalize_subpath(subpath)));
            }
        }
    }

    let project_actions = {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, name, command, action_type, sort_order, auto_commit, created_at, updated_at
             FROM project_actions
             ORDER BY project_id ASC, sort_order ASC, created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    let mut context_ids: HashMap<(String, Option<String>), String> = HashMap::new();
    let mut next_sort_order: HashMap<(String, Option<String>), i64> = HashMap::new();
    let mut migrated_ids = Vec::new();
    let mut skipped_count = 0usize;

    for (
        action_id,
        project_id,
        name,
        command,
        action_type,
        sort_order,
        auto_commit,
        created_at,
        updated_at,
    ) in project_actions
    {
        let Some((github_repo, subpath)) = project_contexts.get(&project_id).cloned() else {
            skipped_count += 1;
            continue;
        };

        let key = (github_repo.clone(), subpath.clone());
        let context_id = if let Some(existing) = context_ids.get(&key) {
            existing.clone()
        } else {
            let context_id = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO action_contexts (
                    id, github_repo, subpath, has_detected_actions, detecting_actions, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, 0, 0, ?4, ?5)",
                params![context_id, github_repo, subpath, created_at, updated_at],
            )?;
            context_ids.insert(key.clone(), context_id.clone());
            context_id
        };

        let entry = next_sort_order.entry(key).or_insert(0);
        let assigned_sort_order = (*entry).max(sort_order);
        *entry = assigned_sort_order + 1;

        conn.execute(
            "INSERT INTO repo_actions (
                id, context_id, name, command, action_type, sort_order, auto_commit, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                action_id,
                context_id,
                name,
                command,
                action_type,
                assigned_sort_order,
                auto_commit,
                created_at,
                updated_at,
            ],
        )?;
        migrated_ids.push(action_id);
    }

    for action_id in migrated_ids {
        conn.execute(
            "DELETE FROM project_actions WHERE id = ?1",
            params![action_id],
        )?;
    }

    let remaining_actions: i64 =
        conn.query_row("SELECT COUNT(*) FROM project_actions", [], |row| row.get(0))?;
    if remaining_actions == 0 {
        conn.execute_batch("DROP TABLE project_actions;")?;
    } else {
        conn.execute_batch("ALTER TABLE project_actions RENAME TO project_actions_legacy;")?;
        if skipped_count > 0 {
            log::warn!(
                "Skipped migrating {skipped_count} legacy project action(s) without a usable repo context"
            );
        }
    }

    Ok(())
}

fn migration_14_project_notes_and_repo_reason(conn: &Connection) -> Result<(), StoreError> {
    execute_sql(conn, LEGACY_SQL_0014_PROJECT_NOTES_AND_PROJECT_REPO_REASON)
}

fn migration_21_images(conn: &Connection) -> Result<(), StoreError> {
    execute_sql(conn, LEGACY_SQL_0021_IMAGES_AND_MESSAGE_IMAGE_IDS)
}

fn recreate_cleanup_triggers(
    conn: &Connection,
    include_project_notes: bool,
    include_images: bool,
) -> Result<(), StoreError> {
    drop_cleanup_triggers(conn)?;

    let conditions = cleanup_session_conditions(include_project_notes, include_images);

    conn.execute_batch(&format!(
        "
        CREATE TRIGGER trg_cleanup_session_after_commit_delete
        AFTER DELETE ON commits
        WHEN OLD.session_id IS NOT NULL
        BEGIN
            DELETE FROM sessions
            WHERE id = OLD.session_id
              AND status != 'running'
              {conditions};
        END;

        CREATE TRIGGER trg_cleanup_session_after_note_delete
        AFTER DELETE ON notes
        WHEN OLD.session_id IS NOT NULL
        BEGIN
            DELETE FROM sessions
            WHERE id = OLD.session_id
              AND status != 'running'
              {conditions};
        END;

        CREATE TRIGGER trg_cleanup_session_after_review_delete
        AFTER DELETE ON reviews
        WHEN OLD.session_id IS NOT NULL
        BEGIN
            DELETE FROM sessions
            WHERE id = OLD.session_id
              AND status != 'running'
              {conditions};
        END;
        "
    ))?;

    if include_project_notes {
        conn.execute_batch(&format!(
            "
            CREATE TRIGGER trg_cleanup_session_after_project_note_delete
            AFTER DELETE ON project_notes
            WHEN OLD.session_id IS NOT NULL
            BEGIN
                DELETE FROM sessions
                WHERE id = OLD.session_id
                  AND status != 'running'
                  {conditions};
            END;
            "
        ))?;
    }

    if include_images {
        conn.execute_batch(&format!(
            "
            CREATE TRIGGER trg_cleanup_session_after_image_delete
            AFTER DELETE ON images
            WHEN OLD.session_id IS NOT NULL
            BEGIN
                DELETE FROM sessions
                WHERE id = OLD.session_id
                  AND status != 'running'
                  {conditions};
            END;
            "
        ))?;
    }

    Ok(())
}

fn drop_cleanup_triggers(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "
        DROP TRIGGER IF EXISTS trg_cleanup_session_after_commit_delete;
        DROP TRIGGER IF EXISTS trg_cleanup_session_after_note_delete;
        DROP TRIGGER IF EXISTS trg_cleanup_session_after_review_delete;
        DROP TRIGGER IF EXISTS trg_cleanup_session_after_project_note_delete;
        DROP TRIGGER IF EXISTS trg_cleanup_session_after_image_delete;
        ",
    )?;
    Ok(())
}

fn trigger_flags_for_version(version: i64) -> (bool, bool) {
    (version >= 14, version >= 21)
}

fn cleanup_session_conditions(include_project_notes: bool, include_images: bool) -> String {
    let mut conditions = vec![
        "AND NOT EXISTS (SELECT 1 FROM commits WHERE session_id = OLD.session_id)".to_string(),
        "AND NOT EXISTS (SELECT 1 FROM notes WHERE session_id = OLD.session_id)".to_string(),
        "AND NOT EXISTS (SELECT 1 FROM reviews WHERE session_id = OLD.session_id)".to_string(),
    ];

    if include_project_notes {
        conditions.push(
            "AND NOT EXISTS (SELECT 1 FROM project_notes WHERE session_id = OLD.session_id)"
                .to_string(),
        );
    }

    if include_images {
        conditions.push(
            "AND NOT EXISTS (SELECT 1 FROM images WHERE session_id = OLD.session_id)".to_string(),
        );
    }

    conditions.join("\n              ")
}

fn table_exists(conn: &Connection, table_name: &str) -> Result<bool, StoreError> {
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM sqlite_master
            WHERE type = 'table' AND name = ?1
        )",
        params![table_name],
        |row| row.get(0),
    )?;
    Ok(exists == 1)
}

fn normalize_github_repo(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if !looks_like_github_repo_slug(trimmed) {
        return None;
    }
    Some(trimmed.to_string())
}

fn normalize_subpath(value: Option<String>) -> Option<String> {
    value.and_then(|subpath| {
        let trimmed = subpath.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn looks_like_github_repo_slug(value: &str) -> bool {
    if value.is_empty()
        || value.contains(' ')
        || value.starts_with('/')
        || value.starts_with('.')
        || value.contains('\\')
    {
        return false;
    }

    let mut parts = value.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    !owner.is_empty() && !repo.is_empty() && parts.next().is_none()
}

fn make_unique_project_name(
    used_names: &mut HashSet<String>,
    repo_identifier: &str,
    subpath: Option<&str>,
) -> String {
    let base = Path::new(repo_identifier)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("project");

    let mut candidate = match subpath.filter(|subpath| !subpath.trim().is_empty()) {
        Some(subpath) => format!("{base} ({subpath})"),
        None => base.to_string(),
    };

    let mut suffix = 2;
    while !used_names.insert(candidate.to_ascii_lowercase()) {
        candidate = format!("{base} {suffix}");
        suffix += 1;
    }

    candidate
}

const LEGACY_SQL_0001_INITIAL_SCHEMA: &str = r#"
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
"#;

const LEGACY_SQL_0002_ADD_SESSION_WORKING_DIR: &str = r#"
ALTER TABLE sessions ADD COLUMN working_dir TEXT NOT NULL DEFAULT '';

UPDATE sessions
SET working_dir = COALESCE(
    (
        SELECT w.path
        FROM commits c
        JOIN workdirs w ON w.branch_id = c.branch_id
        WHERE c.session_id = sessions.id
        ORDER BY c.created_at DESC
        LIMIT 1
    ),
    working_dir
)
WHERE working_dir = '';
"#;

const LEGACY_SQL_0003_ADD_SESSION_PROVIDER: &str = r#"
ALTER TABLE sessions ADD COLUMN provider TEXT;
"#;

const LEGACY_SQL_0004_ADD_REMOTE_BRANCH_FIELDS: &str = r#"
ALTER TABLE branches ADD COLUMN branch_type TEXT NOT NULL DEFAULT 'local';
ALTER TABLE branches ADD COLUMN workspace_name TEXT;
ALTER TABLE branches ADD COLUMN workspace_status TEXT;
ALTER TABLE branches ADD COLUMN agent TEXT;
"#;

const LEGACY_SQL_0005_PROJECTS_REPO_PATH_TO_GITHUB_REPO: &str = r#"
ALTER TABLE projects RENAME COLUMN repo_path TO github_repo;
"#;

const LEGACY_SQL_0006_PROJECTS_REPO_SUBPATH_UNIQUENESS: &str = r#"
CREATE TABLE projects_next (
    id          TEXT PRIMARY KEY,
    github_repo TEXT NOT NULL,
    subpath     TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

INSERT INTO projects_next (id, github_repo, subpath, created_at, updated_at)
SELECT id, github_repo, subpath, created_at, updated_at
FROM projects;

DROP TABLE projects;
ALTER TABLE projects_next RENAME TO projects;

CREATE UNIQUE INDEX idx_projects_repo_subpath
    ON projects(github_repo, COALESCE(subpath, ''));
"#;

const LEGACY_SQL_0007_REVIEWS_RELAX_UNIQUE_AND_COMMENTS_AUTHOR: &str = r#"
ALTER TABLE comments ADD COLUMN author TEXT NOT NULL DEFAULT 'user';

CREATE TABLE reviews_next (
    id              TEXT PRIMARY KEY,
    branch_id       TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
    commit_sha      TEXT NOT NULL,
    scope           TEXT NOT NULL,
    session_id      TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

INSERT INTO reviews_next (id, branch_id, commit_sha, scope, session_id, created_at, updated_at)
SELECT id, branch_id, commit_sha, scope, session_id, created_at, updated_at
FROM reviews;

DROP TABLE reviews;
ALTER TABLE reviews_next RENAME TO reviews;
CREATE INDEX idx_reviews_branch ON reviews(branch_id);
"#;

const LEGACY_SQL_0008_PROJECT_REPO_SPLIT_SETUP: &str = r#"
CREATE TABLE projects_next (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    github_repo TEXT,
    subpath     TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE project_repos (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    github_repo TEXT NOT NULL,
    subpath     TEXT,
    is_primary  INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_project_repos_unique
    ON project_repos(project_id, github_repo, COALESCE(subpath, ''));
CREATE INDEX idx_project_repos_project
    ON project_repos(project_id);

CREATE TABLE branches_next (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    project_repo_id     TEXT REFERENCES project_repos(id) ON DELETE SET NULL,
    branch_name         TEXT NOT NULL,
    base_branch         TEXT NOT NULL,
    pr_number           INTEGER,
    branch_type         TEXT NOT NULL DEFAULT 'local',
    workspace_name      TEXT,
    workspace_status    TEXT,
    agent               TEXT,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    UNIQUE(project_id, project_repo_id, branch_name)
);
"#;

const LEGACY_SQL_0008_PROJECT_REPO_SPLIT_FINALIZE: &str = r#"
DROP TABLE branches;
ALTER TABLE branches_next RENAME TO branches;

DROP TABLE projects;
ALTER TABLE projects_next RENAME TO projects;
CREATE UNIQUE INDEX idx_projects_name ON projects(name);
"#;

const LEGACY_SQL_0009_PROJECT_LOCATION_AND_REPO_BRANCH_NAME: &str = r#"
ALTER TABLE projects ADD COLUMN location TEXT NOT NULL DEFAULT 'local';

UPDATE projects
SET location = CASE
    WHEN EXISTS (
        SELECT 1
        FROM branches
        WHERE branches.project_id = projects.id
          AND branches.branch_type = 'remote'
    ) THEN 'remote'
    ELSE 'local'
END;

ALTER TABLE project_repos ADD COLUMN branch_name TEXT NOT NULL DEFAULT 'main';
"#;

const LEGACY_SQL_0010_BRANCH_PR_METADATA: &str = r#"
ALTER TABLE branches ADD COLUMN pr_state TEXT;
ALTER TABLE branches ADD COLUMN pr_checks_status TEXT;
ALTER TABLE branches ADD COLUMN pr_review_decision TEXT;
ALTER TABLE branches ADD COLUMN pr_mergeable INTEGER;
ALTER TABLE branches ADD COLUMN pr_draft INTEGER;
ALTER TABLE branches ADD COLUMN pr_url TEXT;
ALTER TABLE branches ADD COLUMN pr_updated_at INTEGER;
ALTER TABLE branches ADD COLUMN pr_fetched_at INTEGER;
"#;

const LEGACY_SQL_0011_ACTION_CONTEXTS_AND_REPO_ACTIONS: &str = r#"
CREATE TABLE action_contexts (
    id                      TEXT PRIMARY KEY,
    github_repo             TEXT NOT NULL,
    subpath                 TEXT,
    has_detected_actions    INTEGER NOT NULL DEFAULT 0,
    detecting_actions       INTEGER NOT NULL DEFAULT 0,
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_action_contexts_repo_subpath
    ON action_contexts(github_repo, COALESCE(subpath, ''));

CREATE TABLE repo_actions (
    id              TEXT PRIMARY KEY,
    context_id      TEXT NOT NULL REFERENCES action_contexts(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    command         TEXT NOT NULL,
    action_type     TEXT NOT NULL,
    sort_order      INTEGER NOT NULL,
    auto_commit     INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX idx_repo_actions_context
    ON repo_actions(context_id);
"#;

const LEGACY_SQL_0012_SESSIONS_OWNER_PID_AND_RECENT_REPOS: &str = r#"
ALTER TABLE sessions ADD COLUMN owner_pid INTEGER;

CREATE TABLE recent_repos (
    id              TEXT PRIMARY KEY,
    github_repo     TEXT NOT NULL,
    subpath         TEXT,
    last_used_at    INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_recent_repos_unique
    ON recent_repos(github_repo, COALESCE(subpath, ''));
CREATE INDEX idx_recent_repos_last_used
    ON recent_repos(last_used_at DESC);
"#;

const LEGACY_SQL_0013_COMMENTS_COMMENT_TYPE: &str = r#"
ALTER TABLE comments ADD COLUMN comment_type TEXT;
"#;

const LEGACY_SQL_0014_PROJECT_NOTES_AND_PROJECT_REPO_REASON: &str = r#"
ALTER TABLE project_repos ADD COLUMN reason TEXT;

CREATE TABLE project_notes (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    session_id      TEXT,
    title           TEXT NOT NULL,
    content         TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX idx_project_notes_project ON project_notes(project_id);
"#;

const LEGACY_SQL_0015_REPO_ACTIONS_RUN_DETECTION_MODE: &str = r#"
ALTER TABLE repo_actions ADD COLUMN run_detection_mode TEXT DEFAULT NULL;
"#;

const LEGACY_SQL_0016_REMOVED_WORKSTATION_ID_NOOP: &str = r#"
-- Reserved schema version. The historical workstation_id migration was removed.
"#;

const LEGACY_SQL_0017_REMOVED_WORKSTATION_ID_NOOP: &str = r#"
-- Reserved schema version. The historical workstation_id migration was removed.
"#;

const LEGACY_SQL_0018_REVIEWS_TITLE: &str = r#"
ALTER TABLE reviews ADD COLUMN title TEXT DEFAULT NULL;
"#;

const LEGACY_SQL_0019_IMAGES_ROLLOUT_NOOP: &str = r#"
-- Reserved schema version during the image-support rollout.
"#;

const LEGACY_SQL_0020_IMAGES_ROLLOUT_NOOP: &str = r#"
-- Reserved schema version during the image-support rollout.
"#;

const LEGACY_SQL_0021_IMAGES_AND_MESSAGE_IMAGE_IDS: &str = r#"
ALTER TABLE session_messages ADD COLUMN image_ids TEXT DEFAULT NULL;

CREATE TABLE images (
    id          TEXT PRIMARY KEY,
    branch_id   TEXT REFERENCES branches(id) ON DELETE CASCADE,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    session_id  TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    filename    TEXT NOT NULL,
    mime_type   TEXT NOT NULL,
    size_bytes  INTEGER NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX idx_images_branch ON images(branch_id);
"#;

const LEGACY_SQL_0022_REVIEWS_IS_AUTO: &str = r#"
ALTER TABLE reviews ADD COLUMN is_auto INTEGER NOT NULL DEFAULT 0;
"#;
