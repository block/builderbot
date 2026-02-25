//! Mark — AI-powered development workspace.
//!
//! Tauri commands for the new frontend, built incrementally.
//! See `src-archive/lib.rs` for the previous implementation.

pub mod actions;
pub mod agent;
pub mod blox;
pub mod branches;
pub mod doctor;
pub mod git;
pub mod paths;
pub mod project_commands;
pub mod project_mcp;
pub mod prs;
pub mod session_commands;
pub mod session_runner;
pub mod store;

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use store::Store;
use tauri::{Emitter, Manager};

// =============================================================================
// Managed state
// =============================================================================

/// Holds the database path and optional incompatibility info detected
/// at startup. When `needs_reset` is `Some`, the store has not been
/// created yet — the frontend shows a confirmation dialog and then
/// calls `confirm_reset_store` to proceed.
struct DbState {
    db_path: PathBuf,
    needs_reset: Mutex<Option<StoreIncompatibility>>,
}

fn migrate_db_path_prefixes(
    db_path: &Path,
    old_prefix: &Path,
    new_prefix: &Path,
) -> Result<(), String> {
    if !db_path.exists() {
        return Ok(());
    }

    let old_prefix = old_prefix.to_string_lossy().to_string();
    let new_prefix = new_prefix.to_string_lossy().to_string();
    if old_prefix == new_prefix {
        return Ok(());
    }

    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| format!("Cannot open database for path migration: {e}"))?;

    for (table, column) in [("workdirs", "path"), ("sessions", "working_dir")] {
        let exists: i64 = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM sqlite_master
                    WHERE type = 'table' AND name = ?1
                )",
                rusqlite::params![table],
                |row| row.get(0),
            )
            .map_err(|e| format!("Cannot inspect schema for path migration: {e}"))?;
        if exists == 0 {
            continue;
        }

        let sql = format!(
            "UPDATE {table}
             SET {column} = replace({column}, ?1, ?2)
             WHERE {column} = ?1
                OR {column} LIKE (?1 || '/%')
                OR {column} LIKE (?1 || '\\\\%')"
        );
        let updated = conn
            .execute(&sql, rusqlite::params![old_prefix, new_prefix])
            .map_err(|e| format!("Failed migrating paths in {table}.{column}: {e}"))?;
        if updated > 0 {
            log::info!(
                "Migrated {updated} row(s) in {}.{} from '{}' to '{}'",
                table,
                column,
                old_prefix,
                new_prefix
            );
        }
    }

    Ok(())
}

/// Structured info about a database incompatibility, passed to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoreIncompatibility {
    /// The app version that last used the database (e.g. "0.1.0").
    db_app_version: String,
    /// The version of this build (e.g. "0.2.0").
    app_version: String,
    /// Whether the user can reset, or must update the app instead.
    /// "needs_reset" = old DB, offer wipe. "too_new" = newer DB, suggest update.
    kind: String,
}

// =============================================================================
// Frontend-facing types (enriched views of store models)
// =============================================================================

/// Branch enriched with its workdir path (if any).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchWithWorkdir {
    pub id: String,
    pub project_id: String,
    pub project_repo_id: Option<String>,
    pub branch_name: String,
    pub base_branch: String,
    pub pr_number: Option<u64>,
    pub branch_type: store::BranchType,
    pub workspace_name: Option<String>,
    pub workspace_status: Option<store::WorkspaceStatus>,
    pub pr_state: Option<String>,
    pub pr_checks_status: Option<String>,
    pub pr_review_decision: Option<String>,
    pub pr_mergeable: Option<bool>,
    pub pr_draft: Option<bool>,
    pub pr_url: Option<String>,
    pub pr_updated_at: Option<i64>,
    pub pr_fetched_at: Option<i64>,
    pub worktree_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Commit info combining git data with our metadata.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitTimelineItem {
    /// DB id – present for pending commits so they can be deleted by id.
    pub id: Option<String>,
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub author: String,
    pub timestamp: i64,
    pub session_id: Option<String>,
    pub session_status: Option<String>,
}

/// Note with session status resolved.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteTimelineItem {
    pub id: String,
    pub title: String,
    pub content: String,
    pub session_id: Option<String>,
    pub session_status: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Review with session status resolved.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewTimelineItem {
    pub id: String,
    pub commit_sha: String,
    pub scope: String,
    pub session_id: Option<String>,
    pub session_status: Option<String>,
    pub comment_count: usize,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Composite timeline for a branch.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchTimeline {
    pub commits: Vec<CommitTimelineItem>,
    pub notes: Vec<NoteTimelineItem>,
    pub reviews: Vec<ReviewTimelineItem>,
}

// =============================================================================
// Helper — get the store or return a clear error
// =============================================================================

fn get_store(store: &tauri::State<'_, Mutex<Option<Arc<Store>>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Database not initialized — please reset from the startup prompt".into())
}

// =============================================================================
// Store status commands
// =============================================================================

/// Returns null if the store is ready, or version info if a reset is needed.
#[tauri::command]
fn get_store_status(db_state: tauri::State<'_, DbState>) -> Option<StoreIncompatibility> {
    db_state.needs_reset.lock().unwrap().clone()
}

/// Delete the old database and create a fresh store.
///
/// Called after the user confirms the reset dialog.
#[tauri::command]
fn confirm_reset_store(
    db_state: tauri::State<'_, DbState>,
    store_slot: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<(), String> {
    store::remove_db_files(&db_state.db_path).map_err(|e| e.to_string())?;

    let s = Store::new(&db_state.db_path).map_err(|e| e.to_string())?;
    *store_slot.lock().unwrap() = Some(Arc::new(s));
    *db_state.needs_reset.lock().unwrap() = None;
    Ok(())
}

// =============================================================================
// Project commands
// =============================================================================

#[tauri::command]
fn list_projects(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Vec<store::Project>, String> {
    get_store(&store)?
        .list_projects()
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn create_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    name: String,
    github_repo: Option<String>,
    location: Option<String>,
    subpath: Option<String>,
) -> Result<store::Project, String> {
    let store = get_store(&store)?;
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Project name is required".to_string());
    }
    let project_location = match location.as_deref() {
        Some("remote") => store::ProjectLocation::Remote,
        _ => store::ProjectLocation::Local,
    };
    let inferred_branch_name = branches::infer_branch_name(trimmed);
    let mut project = store::Project::named(trimmed);
    project.location = project_location;
    if let Some(repo) = github_repo.clone() {
        project = project.with_primary_repo(&repo);
    }
    if let Some(sub) = subpath.clone() {
        project = project.with_subpath(sub);
    }
    store.create_project(&project).map_err(|e| e.to_string())?;

    // Create the project-scoped worktree root so project sessions always
    // have a real directory to run in, even before any repos are attached.
    if let Ok(project_dir) = git::project_worktree_root_for(&project.id) {
        let _ = std::fs::create_dir_all(&project_dir);
    }

    if let Some(repo) = project.primary_repo() {
        store
            .get_or_create_action_context(repo, project.subpath.as_deref())
            .map_err(|e| e.to_string())?;
    }

    if let Some(repo) = github_repo.clone() {
        // Record this repo as recently used
        store
            .record_recent_repo(&repo, subpath.clone())
            .map_err(|e| e.to_string())?;
    }

    if let Some(repo) = github_repo {
        let project_repo =
            store::ProjectRepo::new(&project.id, &repo, &inferred_branch_name, subpath).primary();
        store
            .create_project_repo(&project_repo)
            .map_err(|e| e.to_string())?;

        // Create the initial branch record for the first repo so each new
        // project starts with exactly one branch tracked for that repository.
        let detected_base =
            git::detect_default_branch_for_repo(&repo).unwrap_or_else(|_| "main".to_string());
        let effective_base = if detected_base.starts_with("origin/") {
            detected_base
        } else {
            format!("origin/{detected_base}")
        };

        let branch_id = match project.location {
            store::ProjectLocation::Local => {
                let branch =
                    store::Branch::new(&project.id, &inferred_branch_name, &effective_base)
                        .with_project_repo(&project_repo.id);
                store.create_branch(&branch).map_err(|e| e.to_string())?;
                Some(branch.id)
            }
            store::ProjectLocation::Remote => {
                let workspace_name = branches::infer_workspace_name(&inferred_branch_name);
                let branch = store::Branch::new_remote(
                    &project.id,
                    &inferred_branch_name,
                    &effective_base,
                    &workspace_name,
                )
                .with_project_repo(&project_repo.id);
                store.create_branch(&branch).map_err(|e| e.to_string())?;
                None // remote branches don't need worktree setup
            }
        };

        // Spawn background worktree + prerun-actions setup for local branches.
        if let Some(branch_id) = branch_id {
            let project_id = project.id.clone();
            let store_bg = Arc::clone(&store);
            tauri::async_runtime::spawn(async move {
                let _ = app_handle.emit("project-setup-progress", project_id.clone());

                let store_clone = Arc::clone(&store_bg);
                let branch_id_clone = branch_id.clone();
                let worktree_result = tauri::async_runtime::spawn_blocking(move || {
                    branches::setup_worktree_sync(&store_clone, &branch_id_clone)
                })
                .await;

                match worktree_result {
                    Ok(Ok(path)) => {
                        log::info!("[create_project] worktree ready at {path}");
                        let _ = app_handle.emit("project-setup-progress", project_id.clone());
                    }
                    Ok(Err(e)) => {
                        log::warn!("[create_project] worktree setup failed: {e}");
                        return;
                    }
                    Err(e) => {
                        log::warn!("[create_project] worktree task panicked: {e}");
                        return;
                    }
                }

                let executor = app_handle.state::<Arc<actions::ActionExecutor>>();
                let act_registry = app_handle.state::<Arc<actions::ActionRegistry>>();
                match branches::run_prerun_actions_for_branch(
                    &store_bg,
                    &app_handle,
                    &branch_id,
                    &executor,
                    &act_registry,
                )
                .await
                {
                    Ok(count) => {
                        log::info!("[create_project] ran {count} prerun actions");
                        let _ = app_handle.emit("project-setup-progress", project_id);
                    }
                    Err(e) => {
                        log::warn!("[create_project] prerun actions failed: {e}");
                    }
                }
            });
        }
    }

    Ok(project)
}

#[tauri::command(rename_all = "camelCase")]
fn list_project_repos(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
) -> Result<Vec<store::ProjectRepo>, String> {
    let store = get_store(&store)?;
    store
        .list_project_repos(&project_id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn list_recent_repos(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    limit: Option<usize>,
) -> Result<Vec<store::RecentRepo>, String> {
    let store = get_store(&store)?;
    let effective_limit = limit.unwrap_or(10);
    store
        .list_recent_repos(effective_limit)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
async fn add_project_repo(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    project_id: String,
    github_repo: String,
    branch_name: Option<String>,
    subpath: Option<String>,
    set_as_primary: Option<bool>,
) -> Result<store::ProjectRepo, String> {
    let store = get_store(&store)?;
    let repo = project_commands::add_project_repo_impl(
        Arc::clone(&store),
        project_id.clone(),
        github_repo,
        branch_name,
        subpath,
        set_as_primary,
        None,
    )
    .await?;

    // Spawn background worktree + prerun-actions setup — fire and forget.
    tauri::async_runtime::spawn({
        let repo_id = repo.id.clone();
        async move {
            let _ = app_handle.emit("project-setup-progress", project_id.clone());

            let branch = match store.list_branches_for_project(&project_id) {
                Ok(branches) => branches
                    .into_iter()
                    .find(|b| b.project_repo_id.as_deref() == Some(repo_id.as_str())),
                Err(e) => {
                    log::warn!("[add_project_repo] failed to list branches: {e}");
                    return;
                }
            };
            let branch = match branch {
                Some(b) => b,
                None => {
                    log::warn!("[add_project_repo] no branch found for repo {repo_id}");
                    return;
                }
            };

            if branch.workspace_name.is_none() {
                let branch_id = branch.id.clone();
                let store_clone = Arc::clone(&store);
                let worktree_result = tauri::async_runtime::spawn_blocking(move || {
                    branches::setup_worktree_sync(&store_clone, &branch_id)
                })
                .await;

                match worktree_result {
                    Ok(Ok(path)) => {
                        log::info!("[add_project_repo] worktree ready at {path}");
                        let _ = app_handle.emit("project-setup-progress", project_id.clone());
                    }
                    Ok(Err(e)) => {
                        log::warn!("[add_project_repo] worktree setup failed: {e}");
                        return;
                    }
                    Err(e) => {
                        log::warn!("[add_project_repo] worktree task panicked: {e}");
                        return;
                    }
                }

                let executor = app_handle.state::<Arc<actions::ActionExecutor>>();
                let act_registry = app_handle.state::<Arc<actions::ActionRegistry>>();
                match branches::run_prerun_actions_for_branch(
                    &store,
                    &app_handle,
                    &branch.id,
                    &executor,
                    &act_registry,
                )
                .await
                {
                    Ok(count) => {
                        log::info!("[add_project_repo] ran {count} prerun actions");
                        let _ = app_handle.emit("project-setup-progress", project_id);
                    }
                    Err(e) => {
                        log::warn!("[add_project_repo] prerun actions failed: {e}");
                    }
                }
            }
        }
    });

    Ok(repo)
}

#[tauri::command(rename_all = "camelCase")]
fn clear_project_repo_reason(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_repo_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    store
        .clear_project_repo_reason(&project_repo_id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
async fn remove_project_repo(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    project_repo_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    let removed = store
        .get_project_repo(&project_repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project repo not found: {project_repo_id}"))?;
    let branches = store
        .list_branches_for_project(&project_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|b| b.project_repo_id.as_deref() == Some(project_repo_id.as_str()))
        .collect::<Vec<_>>();

    // Run heavy cleanup (worktree removal, remote workspace deletion) off the
    // main thread so the UI stays responsive for large repos.
    tauri::async_runtime::spawn_blocking({
        let store = Arc::clone(&store);
        let branches = branches.clone();
        move || -> Result<(), String> {
            for branch in &branches {
                branches::cleanup_branch_resources(&store, branch)?;
            }
            Ok(())
        }
    })
    .await
    .map_err(|e| format!("Failed to clean up branch resources: {e}"))??;

    for branch in &branches {
        store.delete_branch(&branch.id).map_err(|e| e.to_string())?;
    }
    store
        .delete_project_repo(&project_repo_id)
        .map_err(|e| e.to_string())?;

    if removed.is_primary {
        let next_primary = store
            .list_project_repos(&project_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .next();
        let project = store
            .get_project(&project_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Project not found: {project_id}"))?;
        if let Some(next) = next_primary {
            store
                .set_primary_project_repo(&project_id, &next.id)
                .map_err(|e| e.to_string())?;
            store
                .update_project(
                    &project_id,
                    &project.name,
                    Some(&next.github_repo),
                    &project.location,
                    next.subpath.as_deref(),
                )
                .map_err(|e| e.to_string())?;
        } else {
            store
                .update_project(&project_id, &project.name, None, &project.location, None)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn set_primary_project_repo(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    project_repo_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    let repo = store
        .get_project_repo(&project_repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project repo not found: {project_repo_id}"))?;
    store
        .set_primary_project_repo(&project_id, &project_repo_id)
        .map_err(|e| e.to_string())?;
    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;
    store
        .update_project(
            &project_id,
            &project.name,
            Some(&repo.github_repo),
            &project.location,
            repo.subpath.as_deref(),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn update_project_repo_branch_name(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    project_repo_id: String,
    branch_name: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    let trimmed = branch_name.trim();
    if trimmed.is_empty() {
        return Err("Branch name is required".to_string());
    }
    store
        .update_project_repo_branch_name(&project_id, &project_repo_id, trimmed)
        .map_err(|e| e.to_string())
}

/// List the authenticated user's GitHub organization memberships.
#[tauri::command]
async fn list_github_orgs() -> Result<Vec<String>, String> {
    git::list_github_orgs().map_err(|e| e.to_string())
}

/// List GitHub repositories for the authenticated user or a specific owner.
#[tauri::command]
async fn list_github_repos(owner: Option<String>) -> Result<Vec<git::GitHubRepo>, String> {
    git::list_github_repos(owner.as_deref()).map_err(|e| e.to_string())
}

/// List repositories the authenticated user has recently pushed to.
/// Returns repos across all orgs, sorted by most recently pushed.
#[tauri::command]
async fn list_user_repos(limit: Option<u32>) -> Result<Vec<git::GitHubRepo>, String> {
    git::list_user_repos(limit.unwrap_or(30)).map_err(|e| e.to_string())
}

/// Fetch a single GitHub repository by owner/repo.
/// Returns None if the repo doesn't exist or user lacks access.
#[tauri::command]
async fn get_github_repo(owner: String, repo: String) -> Result<Option<git::GitHubRepo>, String> {
    git::fetch_github_repo(&owner, &repo).map_err(|e| e.to_string())
}

/// Search GitHub repositories for the authenticated user or a specific owner.
#[tauri::command]
async fn search_github_repos(
    query: String,
    owner: Option<String>,
) -> Result<Vec<git::GitHubRepo>, String> {
    git::search_github_repos(&query, owner.as_deref()).map_err(|e| e.to_string())
}

/// Check if a repository is likely a monorepo by counting modules in MODULES.yaml.
/// Returns the module count (0 if file doesn't exist).
#[tauri::command]
async fn check_monorepo_modules(github_repo: String) -> Result<u32, String> {
    git::check_monorepo_modules(&github_repo).map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;

    // Clean up branch-backed resources (local worktrees / remote workspaces)
    // before removing DB records via cascade.
    let branches = store
        .list_branches_for_project(&id)
        .map_err(|e| e.to_string())?;

    // Run heavy cleanup (worktree removal, remote workspace deletion, directory
    // cleanup) off the main thread so the UI stays responsive for large repos.
    tauri::async_runtime::spawn_blocking({
        let store = Arc::clone(&store);
        let id = id.clone();
        let branches = branches.clone();
        move || {
            for branch in &branches {
                branches::cleanup_branch_resources_best_effort(&store, branch);
            }

            // Best-effort cleanup for project-scoped local worktree roots.
            // Worktree-level cleanup removes individual directories; this removes any
            // leftover project container folder.
            if let Ok(project_root) = git::project_worktree_root_for(&id) {
                if project_root.exists() {
                    if let Err(e) = std::fs::remove_dir_all(&project_root) {
                        log::warn!(
                            "failed to remove project worktree root '{}': {e}",
                            project_root.display()
                        );
                    }
                }
            }
        }
    })
    .await
    .map_err(|e| format!("Failed to clean up project resources: {e}"))?;

    store.delete_project(&id).map_err(|e| e.to_string())
}

// =============================================================================
// Repo Actions commands
// =============================================================================

fn get_or_create_project_action_context(
    store: &Arc<Store>,
    project_id: &str,
) -> Result<store::models::ActionContext, String> {
    let project = store
        .get_project(project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;
    let repo = project
        .primary_repo()
        .ok_or_else(|| "Project has no repository attached".to_string())?;
    store
        .get_or_create_action_context(repo, project.subpath.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn list_project_actions(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    project_repo_id: Option<String>,
) -> Result<Vec<store::models::RepoAction>, String> {
    let store = get_store(&store)?;
    let context = if let Some(repo_id) = project_repo_id {
        let repo = store
            .get_project_repo(&repo_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Project repo not found: {repo_id}"))?;
        store
            .get_or_create_action_context(&repo.github_repo, repo.subpath.as_deref())
            .map_err(|e| e.to_string())?
    } else {
        get_or_create_project_action_context(&store, &project_id)?
    };
    store
        .list_repo_actions(&context.id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn update_project_action(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    action_id: String,
    name: String,
    command: String,
    action_type: String,
    sort_order: i32,
    auto_commit: bool,
) -> Result<(), String> {
    let store = get_store(&store)?;
    let action = store
        .get_repo_action(&action_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Action not found: {action_id}"))?;

    let updated = store::models::RepoAction {
        id: action.id,
        context_id: action.context_id,
        name,
        command,
        action_type: builderbot_actions::ActionType::parse(&action_type)
            .ok_or_else(|| format!("Invalid action type: {action_type}"))?,
        sort_order,
        auto_commit,
        created_at: action.created_at,
        updated_at: store::now_timestamp(),
    };

    store
        .update_repo_action(&updated)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn delete_project_action(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    action_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    store
        .delete_repo_action(&action_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_action_contexts(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Vec<store::models::ActionContext>, String> {
    let store = get_store(&store)?;
    store.list_action_contexts().map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn list_repo_actions(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    github_repo: String,
    subpath: Option<String>,
) -> Result<Vec<store::models::RepoAction>, String> {
    let store = get_store(&store)?;
    let context = store
        .get_or_create_action_context(&github_repo, subpath.as_deref())
        .map_err(|e| e.to_string())?;
    store
        .list_repo_actions(&context.id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)]
fn create_repo_action(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    github_repo: String,
    subpath: Option<String>,
    name: String,
    command: String,
    action_type: String,
    sort_order: i32,
    auto_commit: bool,
) -> Result<store::models::RepoAction, String> {
    let store = get_store(&store)?;
    let context = store
        .get_or_create_action_context(&github_repo, subpath.as_deref())
        .map_err(|e| e.to_string())?;
    let parsed_type = builderbot_actions::ActionType::parse(&action_type)
        .ok_or_else(|| format!("Invalid action type: {action_type}"))?;
    let action = store::models::RepoAction::new(context.id, name, command, parsed_type, sort_order)
        .with_auto_commit(auto_commit);
    store
        .create_repo_action(&action)
        .map_err(|e| e.to_string())?;
    Ok(action)
}

// =============================================================================
// Timeline commands
// =============================================================================

/// Create a standalone note (no session) for a branch.
#[tauri::command(rename_all = "camelCase")]
fn create_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    title: String,
    content: String,
) -> Result<NoteTimelineItem, String> {
    let store = get_store(&store)?;
    let note = store::models::Note::new(&branch_id, &title, &content);
    store.create_note(&note).map_err(|e| e.to_string())?;
    Ok(NoteTimelineItem {
        id: note.id,
        title: note.title,
        content: note.content,
        session_id: None,
        session_status: None,
        created_at: note.created_at,
        updated_at: note.updated_at,
    })
}

/// Delete a note and optionally its linked session.
#[tauri::command(rename_all = "camelCase")]
fn delete_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    note_id: String,
    delete_session: Option<bool>,
) -> Result<(), String> {
    let store = get_store(&store)?;
    // Look up the note first so we can find its session
    let note = store
        .get_note(&note_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Note not found: {note_id}"))?;

    store.delete_note(&note_id).map_err(|e| e.to_string())?;

    if delete_session.unwrap_or(false) {
        if let Some(sid) = note.session_id {
            let _ = store.delete_session(&sid);
        }
    }
    Ok(())
}

// =============================================================================
// Project note commands
// =============================================================================

#[tauri::command]
fn create_project_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    title: String,
    content: String,
) -> Result<store::ProjectNote, String> {
    let store = get_store(&store)?;
    let note = store::ProjectNote::new(&project_id, &title, &content);
    store
        .create_project_note(&note)
        .map_err(|e| e.to_string())?;
    Ok(note)
}

#[tauri::command]
fn list_project_notes(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
) -> Result<Vec<store::ProjectNote>, String> {
    get_store(&store)?
        .list_project_notes(&project_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_project_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    note_id: String,
) -> Result<(), String> {
    get_store(&store)?
        .delete_project_note(&note_id)
        .map_err(|e| e.to_string())
}

/// Delete a review and all its comments, optionally deleting its linked session.
#[tauri::command(rename_all = "camelCase")]
fn delete_review(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    delete_session: Option<bool>,
) -> Result<(), String> {
    let store = get_store(&store)?;
    let review = store
        .get_review(&review_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Review not found: {review_id}"))?;

    store.delete_review(&review_id).map_err(|e| e.to_string())?;

    if delete_session.unwrap_or(false) {
        if let Some(sid) = review.session_id {
            let _ = store.delete_session(&sid);
        }
    }
    Ok(())
}

/// Delete a pending commit (one with no SHA) by its DB id.
/// This does NOT touch git — it only removes the DB record and optionally its session.
#[tauri::command(rename_all = "camelCase")]
fn delete_pending_commit(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    commit_id: String,
    delete_session: Option<bool>,
) -> Result<(), String> {
    let store = get_store(&store)?;

    let commit = store
        .get_commit(&commit_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Commit not found: {commit_id}"))?;

    // Safety check: only allow deleting commits that have no SHA (pending/failed)
    if commit.sha.is_some() {
        return Err(
            "Cannot use delete_pending_commit for commits with a SHA. Use delete_commit instead."
                .to_string(),
        );
    }

    store.delete_commit(&commit_id).map_err(|e| e.to_string())?;

    if delete_session.unwrap_or(false) {
        if let Some(sid) = commit.session_id {
            let _ = store.delete_session(&sid);
        }
    }

    Ok(())
}

/// Delete a commit: resets the branch HEAD to the parent commit,
/// removing the git commit, then cleans up the DB record and session.
///
/// Only works for the tip commit (HEAD) of the branch's worktree.
/// Returns an error if the commit is not the current HEAD.
#[tauri::command(rename_all = "camelCase")]
fn delete_commit(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    commit_sha: String,
    delete_session: Option<bool>,
) -> Result<(), String> {
    let store = get_store(&store)?;

    // Get the worktree path for this branch
    let workdir = store
        .get_workdir_for_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

    let worktree = Path::new(&workdir.path);

    // Verify the commit is the current HEAD
    let head_sha = git::get_head_sha(worktree).map_err(|e| e.to_string())?;
    if !head_sha.starts_with(&commit_sha)
        && !commit_sha.starts_with(&head_sha)
        && head_sha != commit_sha
    {
        return Err(format!(
            "Can only delete the latest commit. {} is not HEAD ({})",
            &commit_sha[..7.min(commit_sha.len())],
            &head_sha[..7.min(head_sha.len())]
        ));
    }

    // Find the parent commit
    let parent = git::get_parent_commit(worktree, &commit_sha)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Cannot delete the initial commit".to_string())?;

    // Reset to parent — this removes the commit from the branch
    git::reset_to_commit(worktree, &parent).map_err(|e| e.to_string())?;

    // Clean up DB record if one exists
    if let Ok(Some(db_commit)) = store.get_commit_by_sha(&branch_id, &commit_sha) {
        let _ = store.delete_commit(&db_commit.id);

        if delete_session.unwrap_or(false) {
            if let Some(sid) = db_commit.session_id {
                let _ = store.delete_session(&sid);
            }
        }
    }

    Ok(())
}

fn build_branch_timeline(store: &Arc<Store>, branch_id: &str) -> Result<BranchTimeline, String> {
    // Get the branch and its workdir for git operations
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let workdir = store
        .get_workdir_for_branch(branch_id)
        .map_err(|e| e.to_string())?;

    // Get commits from git (the source of truth for commit data)
    let mut commits = Vec::new();
    if let Some(ref ws_name) = branch.workspace_name {
        let repo_subpath = branches::resolve_branch_workspace_subpath(store, &branch)?;
        // Remote branch: fetch commits via ws_exec.
        // Use merge-base to find the fork point so that only the branch's
        // own commits are shown, even after a rebase or when the base ref
        // has moved forward.
        let range = if let Ok(mb_output) = branches::run_workspace_git(
            ws_name,
            repo_subpath.as_deref(),
            &["merge-base", &branch.base_branch, "HEAD"],
        ) {
            let mb = mb_output.trim().to_string();
            format!("{mb}..HEAD")
        } else {
            // Fallback: if merge-base fails (e.g. shallow clone), use
            // the raw base ref.
            format!("{}..HEAD", &branch.base_branch)
        };
        let format_arg = "--format=%H|%h|%s|%an|%ct";
        if let Ok(output) = branches::run_workspace_git(
            ws_name,
            repo_subpath.as_deref(),
            &["log", format_arg, &range],
        ) {
            for line in output.lines() {
                if line.is_empty() {
                    continue;
                }
                let parts: Vec<&str> = line.splitn(5, '|').collect();
                if parts.len() >= 5 {
                    let sha = parts[0].to_string();
                    let our_commit = store.get_commit_by_sha(branch_id, &sha).unwrap_or(None);
                    let (session_id, session_status) = store.resolve_session_status(
                        our_commit.as_ref().and_then(|c| c.session_id.as_deref()),
                    );

                    commits.push(CommitTimelineItem {
                        id: our_commit.as_ref().map(|c| c.id.clone()),
                        sha,
                        short_sha: parts[1].to_string(),
                        subject: parts[2].to_string(),
                        author: parts[3].to_string(),
                        timestamp: parts[4].parse().unwrap_or(0),
                        session_id,
                        session_status,
                    });
                }
            }
        }
    } else if let Some(ref wd) = workdir {
        // Local branch: fetch commits from the local worktree
        let worktree_path = Path::new(&wd.path);
        if worktree_path.exists() {
            let git_commits =
                git::get_commits_since_base(worktree_path, &branch.base_branch).unwrap_or_default();

            // For each git commit, look up our metadata (session linkage)
            for gc in git_commits {
                let our_commit = store.get_commit_by_sha(branch_id, &gc.sha).unwrap_or(None);
                let (session_id, session_status) = store.resolve_session_status(
                    our_commit.as_ref().and_then(|c| c.session_id.as_deref()),
                );

                commits.push(CommitTimelineItem {
                    id: our_commit.as_ref().map(|c| c.id.clone()),
                    sha: gc.sha,
                    short_sha: gc.short_sha,
                    subject: gc.subject,
                    author: gc.author,
                    timestamp: gc.timestamp,
                    session_id,
                    session_status,
                });
            }
        }
    }

    // Also include pending commits (sha = None, i.e. session in progress)
    let db_commits = store
        .list_commits_for_branch(branch_id)
        .map_err(|e| e.to_string())?;
    for dc in db_commits {
        if dc.sha.is_none() {
            let (session_id, session_status) =
                store.resolve_session_status(dc.session_id.as_deref());

            commits.push(CommitTimelineItem {
                id: Some(dc.id.clone()),
                sha: String::new(),
                short_sha: String::new(),
                subject: session_id
                    .as_deref()
                    .and_then(|sid| {
                        store
                            .get_session(sid)
                            .ok()
                            .flatten()
                            .map(|s| s.prompt.clone())
                    })
                    .unwrap_or_else(|| "Pending commit".to_string()),
                author: String::new(),
                timestamp: dc.created_at / 1000, // convert ms to seconds
                session_id,
                session_status,
            });
        }
    }

    // Get notes
    let db_notes = store
        .list_notes_for_branch(branch_id)
        .map_err(|e| e.to_string())?;
    let notes: Vec<NoteTimelineItem> = db_notes
        .into_iter()
        .map(|n| {
            let (session_id, session_status) =
                store.resolve_session_status(n.session_id.as_deref());
            NoteTimelineItem {
                id: n.id,
                title: n.title,
                content: n.content,
                session_id,
                session_status,
                created_at: n.created_at,
                updated_at: n.updated_at,
            }
        })
        .collect();

    // Get reviews
    let db_reviews = store
        .list_reviews_for_branch(branch_id)
        .map_err(|e| e.to_string())?;
    let reviews: Vec<ReviewTimelineItem> = db_reviews
        .into_iter()
        .map(|r| {
            let (session_id, session_status) =
                store.resolve_session_status(r.session_id.as_deref());
            let comment_count = r.comments.len();
            ReviewTimelineItem {
                id: r.id,
                commit_sha: r.commit_sha,
                scope: r.scope.as_str().to_string(),
                session_id,
                session_status,
                comment_count,
                created_at: r.created_at,
                updated_at: r.updated_at,
            }
        })
        .collect();

    Ok(BranchTimeline {
        commits,
        notes,
        reviews,
    })
}

#[tauri::command]
async fn get_branch_timeline(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<BranchTimeline, String> {
    let store = get_store(&store)?;

    tauri::async_runtime::spawn_blocking(move || build_branch_timeline(&store, &branch_id))
        .await
        .map_err(|e| format!("Timeline task failed: {e}"))?
}

// =============================================================================
// Diff commands
// =============================================================================

/// Context needed to compute diffs for a branch.
struct BranchDiffContext {
    base_branch: String,
    worktree_path: Option<String>,
    workspace_name: Option<String>,
    repo_subpath: Option<String>,
}

/// Resolve the worktree path and base branch for a given branch.
fn resolve_branch_context(
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<BranchDiffContext, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    if branch.branch_type == store::BranchType::Remote {
        let workspace_name = branch
            .workspace_name
            .clone()
            .ok_or_else(|| format!("Branch has no workspace name: {branch_id}"))?;
        let repo_subpath = branches::resolve_branch_workspace_subpath(store, &branch)?;
        return Ok(BranchDiffContext {
            base_branch: branch.base_branch,
            worktree_path: None,
            workspace_name: Some(workspace_name),
            repo_subpath,
        });
    }

    let workdir = store
        .get_workdir_for_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

    Ok(BranchDiffContext {
        base_branch: branch.base_branch,
        worktree_path: Some(workdir.path),
        workspace_name: None,
        repo_subpath: None,
    })
}

fn run_remote_git(ctx: &BranchDiffContext, args: &[&str]) -> Result<String, String> {
    let workspace = ctx
        .workspace_name
        .as_deref()
        .ok_or("Missing remote workspace context")?;
    branches::run_workspace_git(workspace, ctx.repo_subpath.as_deref(), args)
        .map_err(|e| e.to_string())
}

/// Build a DiffSpec for a branch diff.
///
/// - Branch scope with no commit_sha: merge-base(base, tip)..tip
/// - Branch scope with commit_sha: merge-base(base, sha)..sha
/// - Commit scope: sha~1..sha
fn build_diff_spec(
    worktree: &Path,
    base_branch: &str,
    commit_sha: Option<&str>,
    scope: &str,
) -> Result<(git::DiffSpec, String), String> {
    match scope {
        "commit" => {
            let sha = commit_sha.ok_or("commit_sha required for commit scope")?;
            let parent = git::get_parent_commit(worktree, sha)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("No parent commit for {sha}"))?;
            let spec = git::DiffSpec {
                base: git::GitRef::Rev(parent),
                head: git::GitRef::Rev(sha.to_string()),
            };
            Ok((spec, sha.to_string()))
        }
        _ => {
            let resolved_sha = match commit_sha {
                Some(sha) => sha.to_string(),
                None => git::get_head_sha(worktree).map_err(|e| e.to_string())?,
            };
            let spec = git::DiffSpec {
                base: git::GitRef::MergeBaseOf([base_branch.to_string(), resolved_sha.clone()]),
                head: git::GitRef::Rev(resolved_sha.clone()),
            };
            Ok((spec, resolved_sha))
        }
    }
}

/// Build explicit base/head refs for a remote branch diff.
///
/// Returns `(base_sha, head_sha, resolved_sha)`.
fn build_remote_diff_refs(
    ctx: &BranchDiffContext,
    commit_sha: Option<&str>,
    scope: &str,
) -> Result<(String, String, String), String> {
    match scope {
        "commit" => {
            let head = commit_sha
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or("commit_sha required for commit scope")?
                .to_string();
            let parent = run_remote_git(ctx, &["rev-parse", &format!("{head}^")])
                .map(|s| s.trim().to_string())
                .map_err(|_| format!("No parent commit for {head}"))?;
            Ok((parent, head.clone(), head))
        }
        _ => {
            let head = match commit_sha.map(str::trim).filter(|s| !s.is_empty()) {
                Some(sha) => sha.to_string(),
                None => run_remote_git(ctx, &["rev-parse", "HEAD"])?
                    .trim()
                    .to_string(),
            };
            let base = run_remote_git(ctx, &["merge-base", &ctx.base_branch, &head])?
                .trim()
                .to_string();
            Ok((base, head.clone(), head))
        }
    }
}

/// Parse `git diff --name-status -z` output.
fn parse_name_status_z(output: &str) -> Vec<git::FileDiffSummary> {
    let mut results = Vec::new();
    let mut parts = output.split('\0').peekable();

    while let Some(status) = parts.next() {
        if status.is_empty() {
            continue;
        }

        let status_char = status.chars().next().unwrap_or(' ');

        match status_char {
            'A' => {
                if let Some(path) = parts.next() {
                    results.push(git::FileDiffSummary {
                        before: None,
                        after: Some(path.into()),
                    });
                }
            }
            'D' => {
                if let Some(path) = parts.next() {
                    results.push(git::FileDiffSummary {
                        before: Some(path.into()),
                        after: None,
                    });
                }
            }
            'M' | 'T' => {
                if let Some(path) = parts.next() {
                    results.push(git::FileDiffSummary {
                        before: Some(path.into()),
                        after: Some(path.into()),
                    });
                }
            }
            'R' | 'C' => {
                if let (Some(old), Some(new)) = (parts.next(), parts.next()) {
                    results.push(git::FileDiffSummary {
                        before: Some(old.into()),
                        after: Some(new.into()),
                    });
                }
            }
            _ => {
                parts.next();
            }
        }
    }

    results
}

#[derive(Debug, Clone, Copy)]
struct RemoteHunk {
    old_start: u32,
    old_lines: u32,
    new_start: u32,
    new_lines: u32,
}

fn parse_hunk_range(raw: &str) -> Option<(u32, u32)> {
    let (start_raw, lines_raw) = match raw.split_once(',') {
        Some((start, lines)) => (start, lines),
        None => (raw, "1"),
    };
    let start = start_raw.trim().parse::<u32>().ok()?;
    let lines = lines_raw.trim().parse::<u32>().ok()?;
    let start_zero = if start == 0 { 0 } else { start - 1 };
    Some((start_zero, lines))
}

fn parse_unified_hunks(diff_text: &str) -> Vec<RemoteHunk> {
    let mut hunks = Vec::new();

    for line in diff_text.lines() {
        if !line.starts_with("@@ -") {
            continue;
        }
        let Some(after_minus) = line.strip_prefix("@@ -") else {
            continue;
        };
        let Some((old_part, rest)) = after_minus.split_once(" +") else {
            continue;
        };
        let Some((new_part, _)) = rest.split_once(" @@") else {
            continue;
        };

        let Some((old_start, old_lines)) = parse_hunk_range(old_part) else {
            continue;
        };
        let Some((new_start, new_lines)) = parse_hunk_range(new_part) else {
            continue;
        };

        hunks.push(RemoteHunk {
            old_start,
            old_lines,
            new_start,
            new_lines,
        });
    }

    hunks
}

fn file_content_from_text(text: &str) -> git::FileContent {
    if text.as_bytes()[..text.len().min(8192)].contains(&0) {
        return git::FileContent::Binary;
    }
    git::FileContent::Text {
        lines: text.lines().map(|line| line.to_string()).collect(),
    }
}

fn is_missing_object_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("not a valid object name")
        || lower.contains("pathspec")
        || lower.contains("does not exist")
        || lower.contains("exists on disk, but not in")
        || lower.contains("path '")
}

fn is_utf8_parse_error(msg: &str) -> bool {
    msg.to_lowercase()
        .contains("invalid utf-8 in sq blox output")
}

fn load_remote_file_at_ref(
    ctx: &BranchDiffContext,
    ref_name: &str,
    path: &str,
) -> Result<Option<git::File>, String> {
    let spec = format!("{ref_name}:{path}");

    match run_remote_git(ctx, &["cat-file", "-e", &spec]) {
        Ok(_) => {}
        Err(e) if is_missing_object_error(&e) => return Ok(None),
        Err(e) => return Err(e),
    }

    match run_remote_git(ctx, &["show", &spec]) {
        Ok(content) => Ok(Some(git::File {
            path: path.to_string(),
            content: file_content_from_text(&content),
        })),
        Err(e) if is_utf8_parse_error(&e) => Ok(Some(git::File {
            path: path.to_string(),
            content: git::FileContent::Binary,
        })),
        Err(e) => Err(e),
    }
}

fn remote_file_len(file: &Option<git::File>) -> u32 {
    match file {
        Some(git::File {
            content: git::FileContent::Text { lines },
            ..
        }) => lines.len() as u32,
        _ => 0,
    }
}

fn compute_remote_alignments(
    hunks: &[RemoteHunk],
    before: &Option<git::File>,
    after: &Option<git::File>,
) -> Vec<git::Alignment> {
    let before_len = remote_file_len(before);
    let after_len = remote_file_len(after);

    if before_len == 0 && after_len == 0 {
        return vec![];
    }

    if hunks.is_empty() {
        if before_len == 0 {
            return vec![git::Alignment {
                before: git::Span::new(0, 0),
                after: git::Span::new(0, after_len),
                changed: true,
            }];
        }
        if after_len == 0 {
            return vec![git::Alignment {
                before: git::Span::new(0, before_len),
                after: git::Span::new(0, 0),
                changed: true,
            }];
        }
        return vec![git::Alignment {
            before: git::Span::new(0, before_len),
            after: git::Span::new(0, after_len),
            changed: false,
        }];
    }

    let mut alignments = Vec::new();
    let mut before_pos = 0u32;
    let mut after_pos = 0u32;

    for hunk in hunks {
        if before_pos < hunk.old_start || after_pos < hunk.new_start {
            alignments.push(git::Alignment {
                before: git::Span::new(before_pos, hunk.old_start),
                after: git::Span::new(after_pos, hunk.new_start),
                changed: false,
            });
        }

        let before_end = hunk.old_start + hunk.old_lines;
        let after_end = hunk.new_start + hunk.new_lines;

        alignments.push(git::Alignment {
            before: git::Span::new(hunk.old_start, before_end),
            after: git::Span::new(hunk.new_start, after_end),
            changed: true,
        });

        before_pos = before_end;
        after_pos = after_end;
    }

    if before_pos < before_len || after_pos < after_len {
        alignments.push(git::Alignment {
            before: git::Span::new(before_pos, before_len),
            after: git::Span::new(after_pos, after_len),
            changed: false,
        });
    }

    alignments
}

/// Response from get_diff_files including the resolved commit SHA.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiffFilesResponse {
    /// The resolved commit SHA (tip for branch scope, or the passed-in SHA).
    commit_sha: String,
    /// Changed files in the diff.
    files: Vec<git::FileDiffSummary>,
}

/// List files changed in a branch or commit diff.
///
/// For branch scope: merge-base(base, tip)..tip
/// For commit scope: parent..sha
///
/// `commit_sha` is optional for branch scope (resolves to current tip).
#[tauri::command(rename_all = "camelCase")]
async fn get_diff_files(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    commit_sha: Option<String>,
    scope: String,
) -> Result<DiffFilesResponse, String> {
    let store = get_store(&store)?;
    let ctx = resolve_branch_context(&store, &branch_id)?;
    let (files, resolved_sha) = if let Some(worktree_path) = ctx.worktree_path.as_deref() {
        let worktree = Path::new(worktree_path);
        let (spec, resolved_sha) =
            build_diff_spec(worktree, &ctx.base_branch, commit_sha.as_deref(), &scope)?;
        let files = git::list_diff_files(worktree, &spec).map_err(|e| e.to_string())?;
        (files, resolved_sha)
    } else {
        let (base, head, resolved_sha) =
            build_remote_diff_refs(&ctx, commit_sha.as_deref(), &scope)?;
        let output = run_remote_git(&ctx, &["diff", "--name-status", "-z", &base, &head])?;
        (parse_name_status_z(&output), resolved_sha)
    };

    Ok(DiffFilesResponse {
        commit_sha: resolved_sha,
        files,
    })
}

/// Get the full diff content for a single file.
#[tauri::command(rename_all = "camelCase")]
async fn get_file_diff(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    commit_sha: String,
    scope: String,
    path: String,
) -> Result<git::FileDiff, String> {
    let store = get_store(&store)?;
    let ctx = resolve_branch_context(&store, &branch_id)?;
    if let Some(worktree_path) = ctx.worktree_path.as_deref() {
        let worktree = Path::new(worktree_path);
        let (spec, _) = build_diff_spec(worktree, &ctx.base_branch, Some(&commit_sha), &scope)?;
        let file_path = Path::new(&path);
        return git::get_file_diff(worktree, &spec, file_path).map_err(|e| e.to_string());
    }

    let (base, head, _) = build_remote_diff_refs(&ctx, Some(&commit_sha), &scope)?;
    let before = load_remote_file_at_ref(&ctx, &base, &path)?;
    let after = load_remote_file_at_ref(&ctx, &head, &path)?;
    let patch = run_remote_git(
        &ctx,
        &[
            "-c",
            "color.ui=never",
            "diff",
            "--unified=0",
            &base,
            &head,
            "--",
            &path,
        ],
    )?;
    let hunks = parse_unified_hunks(&patch);
    let alignments = compute_remote_alignments(&hunks, &before, &after);

    Ok(git::FileDiff {
        before,
        after,
        alignments,
    })
}

/// Get file content at a specific ref (for reference files).
#[tauri::command(rename_all = "camelCase")]
async fn get_file_at_ref(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    ref_name: String,
    path: String,
) -> Result<git::File, String> {
    let store = get_store(&store)?;
    let ctx = resolve_branch_context(&store, &branch_id)?;
    if let Some(worktree_path) = ctx.worktree_path.as_deref() {
        let worktree = Path::new(worktree_path);
        return git::get_file_at_ref(worktree, &ref_name, &path).map_err(|e| e.to_string());
    }

    let effective_ref = if ref_name == git::WORKDIR {
        "HEAD"
    } else {
        ref_name.as_str()
    };
    load_remote_file_at_ref(&ctx, effective_ref, &path)?
        .ok_or_else(|| format!("File not found: {path}"))
}

// =============================================================================
// Review commands
// =============================================================================

/// Get or create a review for a branch + commit + scope.
///
/// This is the "lazy create" entry point — called when the user does
/// their first persistent action (comment, mark reviewed, etc.).
/// If a review already exists for this triple, returns it.
#[tauri::command(rename_all = "camelCase")]
async fn ensure_review(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    commit_sha: String,
    scope: String,
) -> Result<store::Review, String> {
    let store = get_store(&store)?;
    let review_scope =
        store::ReviewScope::parse(&scope).ok_or_else(|| format!("Invalid scope: {scope}"))?;

    store
        .ensure_review(&branch_id, &commit_sha, review_scope)
        .map_err(|e| e.to_string())
}

/// Find an existing review by (branch, commit, scope) without creating one.
#[tauri::command(rename_all = "camelCase")]
async fn find_review(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    commit_sha: String,
    scope: String,
) -> Result<Option<store::Review>, String> {
    let store = get_store(&store)?;
    let review_scope =
        store::ReviewScope::parse(&scope).ok_or_else(|| format!("Invalid scope: {scope}"))?;

    store
        .find_review(&branch_id, &commit_sha, review_scope)
        .map_err(|e| e.to_string())
}

/// Get a review by ID with all child data.
#[tauri::command(rename_all = "camelCase")]
async fn get_review(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
) -> Result<Option<store::Review>, String> {
    get_store(&store)?
        .get_review(&review_id)
        .map_err(|e| e.to_string())
}

/// Mark a file as reviewed.
#[tauri::command(rename_all = "camelCase")]
async fn mark_reviewed(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
) -> Result<(), String> {
    get_store(&store)?
        .mark_reviewed(&review_id, &path)
        .map_err(|e| e.to_string())
}

/// Unmark a file as reviewed.
#[tauri::command(rename_all = "camelCase")]
async fn unmark_reviewed(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
) -> Result<(), String> {
    get_store(&store)?
        .unmark_reviewed(&review_id, &path)
        .map_err(|e| e.to_string())
}

/// Add a comment to a review.
#[tauri::command(rename_all = "camelCase")]
async fn add_comment(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
    span_start: u32,
    span_end: u32,
    content: String,
) -> Result<store::Comment, String> {
    let store = get_store(&store)?;
    let comment = store::Comment::new(&path, git::Span::new(span_start, span_end), &content);
    store
        .add_comment(&review_id, &comment)
        .map_err(|e| e.to_string())?;
    Ok(comment)
}

/// Update a comment's content.
#[tauri::command(rename_all = "camelCase")]
async fn update_comment(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    comment_id: String,
    content: String,
) -> Result<(), String> {
    get_store(&store)?
        .update_comment(&comment_id, &content)
        .map_err(|e| e.to_string())
}

/// Delete a comment.
#[tauri::command(rename_all = "camelCase")]
async fn delete_comment(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    comment_id: String,
) -> Result<(), String> {
    get_store(&store)?
        .delete_comment(&comment_id)
        .map_err(|e| e.to_string())
}

/// Add a reference file to a review.
#[tauri::command(rename_all = "camelCase")]
async fn add_reference_file(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
) -> Result<(), String> {
    get_store(&store)?
        .add_reference_file(&review_id, &path)
        .map_err(|e| e.to_string())
}

/// Remove a reference file from a review.
#[tauri::command(rename_all = "camelCase")]
async fn remove_reference_file(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
) -> Result<(), String> {
    get_store(&store)?
        .remove_reference_file(&review_id, &path)
        .map_err(|e| e.to_string())
}

// =============================================================================
// Git helper commands
// =============================================================================

/// List branches for a repo via GitHub API (no local clone needed).
#[tauri::command(rename_all = "camelCase")]
async fn list_git_branches(github_repo: String) -> Result<Vec<git::BranchRef>, String> {
    git::list_branches_for_repo(&github_repo).map_err(|e| e.to_string())
}

/// Detect default branch via GitHub API (no local clone needed).
#[tauri::command(rename_all = "camelCase")]
async fn detect_default_branch_cmd(github_repo: String) -> Result<String, String> {
    git::detect_default_branch_for_repo(&github_repo).map_err(|e| e.to_string())
}

/// Prune stale remote-tracking refs. With GitHub-repo-based projects,
/// branch listing uses the API directly, so this is a no-op.
#[tauri::command(rename_all = "camelCase")]
async fn prune_remote_refs(github_repo: String) -> Result<(), String> {
    git::prune_remote_for_repo(&github_repo).map_err(|e| e.to_string())
}

/// Check if a local branch already exists in the project's local clone.
///
/// Used for "new branch" modal copy so users can intentionally attach to
/// existing local branches.
#[tauri::command(rename_all = "camelCase")]
async fn check_existing_local_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    branch_name: String,
) -> Result<bool, String> {
    let store = get_store(&store)?;
    let branch_name = branch_name.trim();
    if branch_name.is_empty() {
        return Ok(false);
    }

    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;

    let Some(repo_path) = project.clone_path() else {
        return Ok(false);
    };

    if !repo_path.exists() {
        return Ok(false);
    }

    match git::branch_exists(&repo_path, branch_name) {
        Ok(exists) => Ok(exists),
        Err(e) => {
            log::debug!(
                "check_existing_local_branch failed for '{}': {e}",
                branches::project_primary_repo(&project).unwrap_or("<no-primary-repo>")
            );
            Ok(false)
        }
    }
}

/// List open pull requests for a repository (via `-R owner/repo`).
#[tauri::command(rename_all = "camelCase")]
fn list_pull_requests(github_repo: String) -> Result<Vec<git::github::PullRequest>, String> {
    git::list_pull_requests_for_repo(&github_repo).map_err(|e| e.to_string())
}

/// List open issues for a repository (via `-R owner/repo`).
#[tauri::command(rename_all = "camelCase")]
fn list_issues(github_repo: String) -> Result<Vec<git::github::Issue>, String> {
    git::list_issues_for_repo(&github_repo).map_err(|e| e.to_string())
}

// =============================================================================
// Utilities
// =============================================================================

/// Open a URL in the user's default browser.
#[tauri::command]
fn open_url(app_handle: tauri::AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app_handle
        .opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| format!("Failed to open URL: {e}"))
}

/// Check whether the `sq` CLI is available on this system.
///
/// The frontend uses this to decide whether to show the Remote branch option
/// in the new-branch modal.
#[tauri::command]
fn is_sq_available() -> bool {
    blox::is_sq_available()
}

/// Read a text file from an absolute path.
///
/// Used by the frontend to read file contents from paths provided by
/// Tauri's native drag-and-drop events (which give file paths, not
/// File objects like browser drag events).
#[tauri::command(rename_all = "camelCase")]
fn read_text_file(file_path: String) -> Result<String, String> {
    let path = Path::new(&file_path);
    if !path.exists() {
        return Err(format!("File does not exist: {file_path}"));
    }
    if !path.is_file() {
        return Err(format!("Not a file: {file_path}"));
    }
    std::fs::read_to_string(path).map_err(|e| format!("Failed to read file: {e}"))
}

/// Check whether the user is authenticated with Blox.
///
/// Returns Ok(()) if authenticated, or an error string if not.
/// The frontend can call this before starting a workspace to give
/// an immediate, actionable error instead of a mysterious hang.
#[tauri::command]
async fn check_blox_auth() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(blox::check_auth)
        .await
        .map_err(|e| format!("Failed to run blox auth check: {e}"))?
        .map_err(|e| e.to_string())
}

// =============================================================================
// Open In commands
// =============================================================================

/// An application that can open directories.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenerApp {
    id: String,
    name: String,
}

/// Known applications with their bundle IDs (macOS).
#[cfg(target_os = "macos")]
const KNOWN_OPENERS: &[(&str, &str)] = &[
    // Terminals
    ("terminal", "com.apple.Terminal"),
    ("warp", "dev.warp.Warp-Stable"),
    ("iterm", "com.googlecode.iterm2"),
    ("hyper", "co.zeit.hyper"),
    ("kitty", "net.kovidgoyal.kitty"),
    ("alacritty", "org.alacritty"),
    // Editors
    ("vscode", "com.microsoft.VSCode"),
    ("vscode-insiders", "com.microsoft.VSCodeInsiders"),
    ("cursor", "com.todesktop.230313mzl4w4u92"),
    ("sublime", "com.sublimetext.4"),
    ("atom", "com.github.atom"),
    ("textmate", "com.macromates.TextMate"),
    ("nova", "com.panic.Nova"),
    ("bbedit", "com.barebones.bbedit"),
    ("intellij", "com.jetbrains.intellij"),
    ("webstorm", "com.jetbrains.WebStorm"),
    ("pycharm", "com.jetbrains.pycharm"),
    ("rubymine", "com.jetbrains.rubymine"),
    ("goland", "com.jetbrains.goland"),
    ("fleet", "fleet.app"),
    ("zed", "dev.zed.Zed"),
    // File browsers
    ("finder", "com.apple.finder"),
];

/// Get available opener applications.
///
/// On macOS, uses mdfind to detect which apps are installed.
/// On other platforms, returns an empty list.
#[tauri::command]
async fn get_available_openers() -> Result<Vec<OpenerApp>, String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        let mut available = Vec::new();

        for (id, bundle_id) in KNOWN_OPENERS {
            let output = Command::new("mdfind")
                .arg(format!("kMDItemCFBundleIdentifier == '{bundle_id}'"))
                .output()
                .map_err(|e| format!("Failed to run mdfind: {e}"))?;

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if !stdout.trim().is_empty() {
                    available.push(OpenerApp {
                        id: id.to_string(),
                        name: prettify_app_name(id),
                    });
                }
            }
        }

        Ok(available)
    }

    #[cfg(not(target_os = "macos"))]
    {
        // On non-macOS platforms, return empty list
        Ok(Vec::new())
    }
}

/// Convert app ID to a human-readable name.
#[cfg(target_os = "macos")]
fn prettify_app_name(id: &str) -> String {
    match id {
        "vscode" => "VS Code",
        "vscode-insiders" => "VS Code Insiders",
        "cursor" => "Cursor",
        "sublime" => "Sublime Text",
        "atom" => "Atom",
        "textmate" => "TextMate",
        "nova" => "Nova",
        "bbedit" => "BBEdit",
        "intellij" => "IntelliJ IDEA",
        "webstorm" => "WebStorm",
        "pycharm" => "PyCharm",
        "rubymine" => "RubyMine",
        "goland" => "GoLand",
        "fleet" => "Fleet",
        "zed" => "Zed",
        "terminal" => "Terminal",
        "warp" => "Warp",
        "iterm" => "iTerm",
        "hyper" => "Hyper",
        "kitty" => "Kitty",
        "alacritty" => "Alacritty",
        "finder" => "Finder",
        _ => id,
    }
    .to_string()
}

/// Open a directory in a specific application.
///
/// On macOS, uses the `open -b` command with the app's bundle ID.
/// On other platforms, returns an error.
#[tauri::command]
#[allow(unused_variables)]
async fn open_in_app(path: String, app_id: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        // Find the bundle ID for this app
        let bundle_id = KNOWN_OPENERS
            .iter()
            .find(|(id, _)| *id == app_id)
            .map(|(_, bundle)| *bundle)
            .ok_or_else(|| format!("Unknown app ID: {app_id}"))?;

        let status = Command::new("open")
            .arg("-b")
            .arg(bundle_id)
            .arg(&path)
            .status()
            .map_err(|e| format!("Failed to run open command: {e}"))?;

        if status.success() {
            Ok(())
        } else {
            Err(format!("Failed to open {path} in {app_id}"))
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err("Open in app is only supported on macOS".to_string())
    }
}

// =============================================================================
// Tauri App Setup
// =============================================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(
                    // Restore everything except visibility — the frontend
                    // calls window.show() after the theme is applied to
                    // avoid a flash of wrong-colored background.
                    tauri_plugin_window_state::StateFlags::all()
                        & !tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            // Build a custom macOS application menu so that the app submenu,
            // "About" item, and "Quit" item use the capitalised product name
            // "Mark" instead of the lowercase Cargo package name "mark".
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};

                let handle = app.handle();
                let pkg_info = handle.package_info();
                let config = handle.config();
                let about_metadata = AboutMetadata {
                    name: Some("Mark".into()),
                    version: Some(pkg_info.version.to_string()),
                    copyright: config.bundle.copyright.clone(),
                    authors: config.bundle.publisher.clone().map(|p| vec![p]),
                    ..Default::default()
                };

                let settings_item = MenuItem::with_id(
                    handle,
                    "settings",
                    "Preferences…",
                    true,
                    Some("CmdOrCtrl+,"),
                )?;

                let app_menu = Submenu::with_items(
                    handle,
                    "Mark",
                    true,
                    &[
                        &PredefinedMenuItem::about(
                            handle,
                            Some("About Mark"),
                            Some(about_metadata),
                        )?,
                        &PredefinedMenuItem::separator(handle)?,
                        &settings_item,
                        &PredefinedMenuItem::separator(handle)?,
                        &PredefinedMenuItem::services(handle, None)?,
                        &PredefinedMenuItem::separator(handle)?,
                        &PredefinedMenuItem::hide(handle, None)?,
                        &PredefinedMenuItem::hide_others(handle, None)?,
                        &PredefinedMenuItem::separator(handle)?,
                        &PredefinedMenuItem::quit(handle, Some("Quit Mark"))?,
                    ],
                )?;

                let file_menu = Submenu::with_items(
                    handle,
                    "File",
                    true,
                    &[&PredefinedMenuItem::close_window(handle, None)?],
                )?;

                let edit_menu = Submenu::with_items(
                    handle,
                    "Edit",
                    true,
                    &[
                        &PredefinedMenuItem::undo(handle, None)?,
                        &PredefinedMenuItem::redo(handle, None)?,
                        &PredefinedMenuItem::separator(handle)?,
                        &PredefinedMenuItem::cut(handle, None)?,
                        &PredefinedMenuItem::copy(handle, None)?,
                        &PredefinedMenuItem::paste(handle, None)?,
                        &PredefinedMenuItem::select_all(handle, None)?,
                    ],
                )?;

                let view_menu = Submenu::with_items(
                    handle,
                    "View",
                    true,
                    &[&PredefinedMenuItem::fullscreen(handle, None)?],
                )?;

                let window_menu = Submenu::with_id_and_items(
                    handle,
                    tauri::menu::WINDOW_SUBMENU_ID,
                    "Window",
                    true,
                    &[
                        &PredefinedMenuItem::minimize(handle, None)?,
                        &PredefinedMenuItem::maximize(handle, None)?,
                        &PredefinedMenuItem::separator(handle)?,
                        &PredefinedMenuItem::close_window(handle, None)?,
                    ],
                )?;

                let menu = Menu::with_items(
                    handle,
                    &[&app_menu, &file_menu, &edit_menu, &view_menu, &window_menu],
                )?;

                app.set_menu(menu)?;
            }

            let data_dir = crate::paths::data_dir()
                .ok_or_else(|| "Cannot determine data directory".to_string())?;
            std::fs::create_dir_all(&data_dir)
                .map_err(|e| format!("Cannot create data dir: {e}"))?;

            let db_path = data_dir.join("data.db");

            // Migrate from older data directories if the new location is empty.
            // Priority: legacy ~/Library/Application Support/staged/ first,
            // then the current Tauri app_data_dir (com.mark.app).
            if !db_path.exists() {
                let legacy_candidates: Vec<PathBuf> = [
                    crate::paths::legacy_data_dir(),
                    app.path().app_data_dir().ok(),
                ]
                .into_iter()
                .flatten()
                .filter(|d| d != &data_dir)
                .collect();

                for old_dir in legacy_candidates {
                    let old_db = old_dir.join("data.db");
                    if old_db.exists() {
                        log::info!(
                            "Migrating data from {} to {}",
                            old_dir.display(),
                            data_dir.display()
                        );
                        // Move the entire directory contents (db, repos, worktree/workspace data)
                        crate::paths::migrate_directory_contents(&old_dir, &data_dir);
                        break;
                    }
                }
            }

            // Move local worktrees from the legacy top-level `worktrees/` folder
            // into the workspace-scoped `workspaces/local/` folder.
            if let (Some(old_worktrees), Some(new_worktrees)) = (
                crate::paths::legacy_worktrees_dir(),
                crate::paths::worktrees_dir(),
            ) {
                if old_worktrees.exists() && old_worktrees != new_worktrees {
                    migrate_db_path_prefixes(&db_path, &old_worktrees, &new_worktrees)?;
                    crate::paths::migrate_legacy_worktrees_layout();
                }
            }

            // Check compatibility *before* creating the store.
            let compat = store::check_db_compatibility(&db_path)
                .map_err(|e| format!("Cannot check database: {e}"))?;

            let (store_slot, reset_info) = match compat {
                store::DbCompatibility::Ok => {
                    let s =
                        Store::new(&db_path).map_err(|e| format!("Failed to open store: {e}"))?;
                    let store_arc = Arc::new(s);
                    // Cancel sessions whose owner process is dead; leave sessions
                    // owned by other live Mark instances untouched.
                    session_runner::cancel_dead_sessions(
                        Arc::clone(&store_arc),
                        app.handle().clone(),
                    );
                    (Mutex::new(Some(store_arc)), None)
                }
                store::DbCompatibility::NeedsReset { db_app_version } => {
                    let info = StoreIncompatibility {
                        db_app_version: db_app_version.clone(),
                        app_version: store::APP_VERSION.to_string(),
                        kind: "needs_reset".to_string(),
                    };
                    log::warn!(
                        "Database from v{} incompatible with v{}, will prompt user to reset",
                        db_app_version,
                        store::APP_VERSION,
                    );
                    (Mutex::new(None), Some(info))
                }
                store::DbCompatibility::TooNew { db_app_version } => {
                    let info = StoreIncompatibility {
                        db_app_version: db_app_version.clone(),
                        app_version: store::APP_VERSION.to_string(),
                        kind: "too_new".to_string(),
                    };
                    log::warn!(
                        "Database from v{} is newer than this build (v{}), user should update",
                        db_app_version,
                        store::APP_VERSION,
                    );
                    (Mutex::new(None), Some(info))
                }
            };

            app.manage(store_slot);
            app.manage(Arc::new(session_runner::SessionRegistry::new()));
            app.manage(Arc::new(actions::ActionExecutor::new()));
            app.manage(Arc::new(actions::ActionRegistry::new()));
            app.manage(DbState {
                db_path,
                needs_reset: Mutex::new(reset_info),
            });

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id() == "settings" {
                // Emit an event to the frontend to open the settings page.
                if let Err(e) = app.emit("menu:settings", ()) {
                    log::warn!("Failed to emit menu:settings event: {e}");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_store_status,
            confirm_reset_store,
            list_projects,
            create_project,
            list_project_repos,
            list_recent_repos,
            add_project_repo,
            update_project_repo_branch_name,
            clear_project_repo_reason,
            remove_project_repo,
            set_primary_project_repo,
            delete_project,
            list_github_orgs,
            list_github_repos,
            list_user_repos,
            get_github_repo,
            search_github_repos,
            check_monorepo_modules,
            branches::list_branches_for_project,
            branches::create_branch,
            branches::setup_worktree,
            branches::setup_worktree_from_pr,
            branches::create_remote_branch,
            branches::start_workspace,
            branches::delete_branch,
            branches::rename_branch,
            branches::get_workspace_info,
            branches::poll_workspace_status,
            list_project_actions,
            update_project_action,
            delete_project_action,
            list_action_contexts,
            list_repo_actions,
            create_repo_action,
            get_branch_timeline,
            create_note,
            delete_note,
            create_project_note,
            list_project_notes,
            delete_project_note,
            delete_review,
            delete_commit,
            delete_pending_commit,
            list_git_branches,
            detect_default_branch_cmd,
            prune_remote_refs,
            check_existing_local_branch,
            list_pull_requests,
            list_issues,
            prs::create_pr,
            prs::get_pr_url,
            prs::update_branch_pr,
            prs::refresh_pr_status,
            prs::refresh_all_pr_statuses,
            prs::has_unpushed_commits,
            prs::push_branch,
            open_url,
            is_sq_available,
            read_text_file,
            check_blox_auth,
            get_available_openers,
            open_in_app,
            session_commands::discover_acp_providers,
            session_commands::get_session,
            session_commands::get_session_messages,
            session_commands::get_session_messages_since,
            session_commands::start_session,
            session_commands::resume_session,
            session_commands::cancel_session,
            session_commands::delete_session,
            session_commands::start_branch_session,
            session_commands::start_project_session,
            // Actions
            actions::commands::detect_repo_actions,
            actions::commands::run_branch_action,
            actions::commands::stop_branch_action,
            actions::commands::get_running_branch_actions,
            actions::commands::get_action_output_buffer,
            actions::commands::clear_action_execution,
            actions::commands::run_prerun_actions,
            // Diff
            get_diff_files,
            get_file_diff,
            get_file_at_ref,
            // Review
            ensure_review,
            find_review,
            get_review,
            mark_reviewed,
            unmark_reviewed,
            add_comment,
            update_comment,
            delete_comment,
            add_reference_file,
            remove_reference_file,
            // Doctor
            doctor::run_doctor,
            doctor::run_doctor_fix,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
