//! Staged — AI-powered development workspace.
//!
//! Tauri commands for the new frontend, built incrementally.
//! See `src-archive/lib.rs` for the previous implementation.

pub mod actions;
pub mod agent;
pub mod blox;
pub mod branches;
pub mod diff_commands;
pub mod doctor;
pub mod git;
pub mod github_commands;
pub mod image_commands;
pub mod note_commands;
pub mod paths;
pub mod project_commands;
pub mod project_mcp;
pub mod prs;
pub mod review_commands;
pub mod session_commands;
pub mod session_runner;
pub mod store;
pub mod timeline;
pub mod util_commands;

use serde::Serialize;
use std::path::PathBuf;
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

pub(crate) fn preferences_store_path_buf() -> Option<PathBuf> {
    crate::paths::data_dir().map(|d| d.join("preferences.json"))
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
    pub workstation_id: Option<u64>,
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

/// Result of polling a remote workspace's status.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollWorkspaceResult {
    pub status: String,
    pub workstation_id: Option<u64>,
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
    pub title: Option<String>,
    pub comment_count: usize,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Image with session status resolved.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageTimelineItem {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub session_id: Option<String>,
    pub session_status: Option<String>,
    pub created_at: i64,
}

/// Composite timeline for a branch.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchTimeline {
    pub commits: Vec<CommitTimelineItem>,
    pub notes: Vec<NoteTimelineItem>,
    pub reviews: Vec<ReviewTimelineItem>,
    pub images: Vec<ImageTimelineItem>,
}

// =============================================================================
// Helper — get the store or return a clear error
// =============================================================================

pub(crate) fn get_store(
    store: &tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Arc<Store>, String> {
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
    // Validate that the subpath exists as a directory in the repo before
    // creating anything. This prevents projects being created with invalid
    // subpaths that would fail later during worktree setup.
    if let (Some(repo), Some(sub)) = (&github_repo, &subpath) {
        git::validate_subpath_in_repo(repo, sub).map_err(|e| e.to_string())?;
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
                log::info!(
                    "[create_project] created remote branch={} workspace={} status=starting project={}",
                    branch.id,
                    workspace_name,
                    project.id
                );
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
    } else if project.location == store::ProjectLocation::Remote {
        log::info!(
            "[create_project] remote project '{}' created without a repo; workspace start will be deferred until a repo is added",
            project.id
        );
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
                // Local branch: set up git worktree and run prerun actions.
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
            } else {
                // Remote branch: clone the repo into the running workspace,
                // fetch the base branch, and create the feature branch.
                match branches::setup_remote_repo_clone(&store, &branch.id).await {
                    Ok(()) => {
                        log::info!(
                            "[add_project_repo] remote repo cloned for branch '{}'",
                            branch.branch_name
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "[add_project_repo] remote repo clone failed for branch '{}': {e}",
                            branch.branch_name
                        );
                    }
                }
                let _ = app_handle.emit("project-setup-progress", project_id);
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
    executor: tauri::State<'_, Arc<actions::ActionExecutor>>,
    registry: tauri::State<'_, Arc<actions::ActionRegistry>>,
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

    // Stop running actions for branches being removed.
    let branch_ids: Vec<&str> = branches.iter().map(|b| b.id.as_str()).collect();
    actions::commands::stop_actions_for_branches(&executor, &registry, &branch_ids);

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

#[tauri::command]
async fn delete_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    executor: tauri::State<'_, Arc<actions::ActionExecutor>>,
    registry: tauri::State<'_, Arc<actions::ActionRegistry>>,
    id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;

    // Clean up branch-backed resources (local worktrees / remote workspaces)
    // before removing DB records via cascade.
    let branches = store
        .list_branches_for_project(&id)
        .map_err(|e| e.to_string())?;

    // Stop all running actions for branches in this project before cleanup.
    let branch_ids: Vec<&str> = branches.iter().map(|b| b.id.as_str()).collect();
    actions::commands::stop_actions_for_branches(&executor, &registry, &branch_ids);

    // Run heavy cleanup (worktree removal, remote workspace deletion, directory
    // cleanup) off the main thread so the UI stays responsive for large repos.
    tauri::async_runtime::spawn_blocking({
        let store = Arc::clone(&store);
        let id = id.clone();
        let branches = branches.clone();
        move || {
            cleanup_project_branches_best_effort(
                &branches,
                |branch| branches::cleanup_branch_resources_best_effort(&store, branch),
                |branch| store.delete_branch(&branch.id).map_err(|e| e.to_string()),
            );

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

fn cleanup_project_branches_best_effort<F, G>(
    branches: &[store::Branch],
    mut cleanup_branch_resources: F,
    mut delete_branch_row: G,
) where
    F: FnMut(&store::Branch),
    G: FnMut(&store::Branch) -> Result<(), String>,
{
    let mut failed_branch_deletes: Vec<&store::Branch> = Vec::new();

    for branch in branches {
        cleanup_branch_resources(branch);
        // Delete branch rows as we go so shared-workspace cleanup can
        // converge on the final owner and remove the workspace once.
        if let Err(e) = delete_branch_row(branch) {
            log::warn!(
                "failed to delete branch '{}' during project cleanup: {e}",
                branch.id
            );
            failed_branch_deletes.push(branch);
        }
    }

    if failed_branch_deletes.is_empty() {
        return;
    }

    // One retry can unblock convergence when an early branch-row delete failed.
    for branch in failed_branch_deletes {
        if let Err(e) = delete_branch_row(branch) {
            log::warn!(
                "failed retry delete for branch '{}' during project cleanup: {e}",
                branch.id
            );
        }
    }

    // Re-run remote cleanup once after retries so shared workspace deletion
    // can converge when the first pass observed stale peer rows.
    for branch in branches {
        if branch.branch_type == store::BranchType::Remote {
            cleanup_branch_resources(branch);
        }
    }
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
        run_detection_mode: action.run_detection_mode,
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

#[tauri::command(rename_all = "camelCase")]
fn delete_all_repo_actions(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    context_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    store
        .delete_all_repo_actions(&context_id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn delete_action_context(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    context_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    store
        .delete_action_context(&context_id)
        .map_err(|e| e.to_string())
}

// =============================================================================
// Tauri App Setup
// =============================================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
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
            let updater_pubkey_present = app
                .config()
                .plugins
                .0
                .get("updater")
                .and_then(|value| value.as_object())
                .and_then(|updater| updater.get("pubkey"))
                .and_then(|pubkey| pubkey.as_str())
                .is_some_and(|pubkey| !pubkey.trim().is_empty());

            if updater_pubkey_present {
                app.handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())?;
            }

            // Build a custom macOS application menu so that the app submenu,
            // "About" item, and "Quit" item use the capitalised product name
            // "Staged" instead of the lowercase Cargo package name "staged".
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};

                let handle = app.handle();
                let pkg_info = handle.package_info();
                let config = handle.config();
                let about_metadata = AboutMetadata {
                    name: Some("Staged".into()),
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
                let find_item =
                    MenuItem::with_id(handle, "find", "Find…", true, Some("CmdOrCtrl+F"))?;
                let find_next_item =
                    MenuItem::with_id(handle, "find_next", "Find Next", true, Some("CmdOrCtrl+G"))?;
                let find_previous_item = MenuItem::with_id(
                    handle,
                    "find_previous",
                    "Find Previous",
                    true,
                    Some("CmdOrCtrl+Shift+G"),
                )?;
                let zoom_in_item =
                    MenuItem::with_id(handle, "zoom_in", "Zoom In", true, Some("CmdOrCtrl+="))?;
                let zoom_out_item =
                    MenuItem::with_id(handle, "zoom_out", "Zoom Out", true, Some("CmdOrCtrl+-"))?;
                let zoom_reset_item = MenuItem::with_id(
                    handle,
                    "zoom_reset",
                    "Actual Size",
                    true,
                    Some("CmdOrCtrl+0"),
                )?;

                let app_menu = Submenu::with_items(
                    handle,
                    "Staged",
                    true,
                    &[
                        &PredefinedMenuItem::about(
                            handle,
                            Some("About Staged"),
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
                        &PredefinedMenuItem::quit(handle, Some("Quit Staged"))?,
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
                        &PredefinedMenuItem::separator(handle)?,
                        &find_item,
                        &find_next_item,
                        &find_previous_item,
                    ],
                )?;

                let view_menu = Submenu::with_items(
                    handle,
                    "View",
                    true,
                    &[
                        &zoom_in_item,
                        &zoom_out_item,
                        &zoom_reset_item,
                        &PredefinedMenuItem::separator(handle)?,
                        &PredefinedMenuItem::fullscreen(handle, None)?,
                    ],
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

            // Move local worktrees from the legacy top-level `worktrees/`
            // folder into the workspace-scoped `workspaces/local/` folder.
            if let Some(old_worktrees) = crate::paths::legacy_worktrees_dir() {
                if let Some(new_worktrees) = crate::paths::worktrees_dir() {
                    if old_worktrees.exists() && old_worktrees != new_worktrees {
                        crate::paths::migrate_legacy_worktrees_layout();
                    }
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
                    // owned by other live Staged instances untouched.
                    session_runner::cancel_dead_sessions(
                        Arc::clone(&store_arc),
                        app.handle().clone(),
                    );
                    // Clean up images left in "pending" state from compose
                    // dialogs that were abandoned (e.g. user quit mid-dialog).
                    match store_arc.cleanup_pending_images() {
                        Ok(0) => {}
                        Ok(n) => log::info!("Cleaned up {n} pending image(s) from previous run"),
                        Err(e) => log::warn!("Failed to clean up pending images: {e}"),
                    }
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
            let maybe_event_name = match event.id().as_ref() {
                "settings" => Some("menu:settings"),
                "find" => Some("menu:find"),
                "find_next" => Some("menu:find-next"),
                "find_previous" => Some("menu:find-previous"),
                "zoom_in" => Some("menu:zoom-in"),
                "zoom_out" => Some("menu:zoom-out"),
                "zoom_reset" => Some("menu:zoom-reset"),
                _ => None,
            };

            if let Some(event_name) = maybe_event_name {
                if let Err(e) = app.emit(event_name, ()) {
                    log::warn!("Failed to emit {event_name} event: {e}");
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
            // GitHub
            github_commands::list_github_orgs,
            github_commands::list_github_repos,
            github_commands::list_user_repos,
            github_commands::get_github_repo,
            github_commands::search_github_repos,
            github_commands::check_monorepo_modules,
            github_commands::validate_subpath,
            github_commands::list_repo_directories,
            // Branches
            branches::list_branches_for_project,
            branches::create_branch,
            branches::setup_worktree,
            branches::setup_worktree_from_pr,
            branches::create_remote_branch,
            branches::start_workspace,
            branches::delete_branch,
            branches::rename_branch,
            branches::get_blox_env,
            branches::get_workspace_info,
            branches::poll_workspace_status,
            // Actions
            list_project_actions,
            update_project_action,
            delete_project_action,
            list_action_contexts,
            list_repo_actions,
            create_repo_action,
            delete_all_repo_actions,
            delete_action_context,
            // Timeline
            timeline::get_branch_timeline,
            // Notes
            note_commands::create_note,
            note_commands::delete_note,
            note_commands::create_project_note,
            note_commands::list_project_notes,
            note_commands::delete_project_note,
            // Images
            image_commands::create_image,
            image_commands::get_image_path,
            image_commands::get_image_data,
            image_commands::delete_image,
            image_commands::list_branch_images,
            image_commands::create_image_from_data,
            // Timeline delete commands
            timeline::delete_review,
            timeline::delete_commit,
            timeline::delete_pending_commit,
            // Git helpers
            github_commands::list_git_branches,
            github_commands::detect_default_branch_cmd,
            github_commands::prune_remote_refs,
            github_commands::check_existing_local_branch,
            github_commands::list_pull_requests,
            github_commands::list_issues,
            // PRs
            prs::create_pr,
            prs::get_pr_url,
            prs::update_branch_pr,
            prs::refresh_pr_status,
            prs::refresh_all_pr_statuses,
            prs::has_unpushed_commits,
            prs::push_branch,
            prs::clear_branch_pr_status,
            // Utilities
            util_commands::open_url,
            util_commands::is_sq_available,
            util_commands::read_text_file,
            util_commands::preferences_store_path,
            util_commands::check_blox_auth,
            util_commands::get_available_openers,
            util_commands::open_in_app,
            // Sessions
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
            session_commands::find_fresh_auto_review,
            session_commands::set_review_auto,
            // Actions
            actions::commands::detect_repo_actions,
            actions::commands::run_branch_action,
            actions::commands::stop_branch_action,
            actions::commands::get_running_branch_actions,
            actions::commands::get_action_output_buffer,
            actions::commands::clear_action_execution,
            actions::commands::run_prerun_actions,
            actions::commands::get_run_phase,
            actions::commands::update_run_detection_mode,
            // Diff
            diff_commands::get_diff_files,
            diff_commands::get_file_diff,
            diff_commands::get_file_at_ref,
            // Review
            review_commands::ensure_review,
            review_commands::find_review,
            review_commands::get_review,
            review_commands::mark_reviewed,
            review_commands::unmark_reviewed,
            review_commands::add_comment,
            review_commands::update_comment,
            review_commands::delete_comment,
            review_commands::add_reference_file,
            review_commands::remove_reference_file,
            // Doctor
            doctor::run_doctor,
            doctor::run_doctor_fix,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                // Stop all running actions on quit (fire-and-forget).
                let executor = app_handle.state::<Arc<actions::ActionExecutor>>();
                let registry = app_handle.state::<Arc<actions::ActionRegistry>>();
                actions::commands::stop_all_actions(&executor, &registry);
                // Brief grace period for processes to receive SIGTERM.
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::cleanup_project_branches_best_effort;
    use crate::store::{Branch, BranchType};
    use std::collections::HashMap;

    fn remote_branch(
        project_id: &str,
        id: &str,
        branch_name: &str,
        workspace_name: &str,
    ) -> Branch {
        let mut branch = Branch::new_remote(project_id, branch_name, "main", workspace_name);
        branch.id = id.to_string();
        branch
    }

    #[test]
    fn delete_project_cleanup_retries_branch_row_deletes_and_re_sweeps_remote_workspaces() {
        let branches = vec![
            remote_branch("project-1", "a", "feature-a", "ws-shared"),
            remote_branch("project-1", "b", "feature-b", "ws-shared"),
        ];

        let mut cleanup_calls: Vec<String> = Vec::new();
        let mut delete_attempts: HashMap<String, usize> = HashMap::new();

        cleanup_project_branches_best_effort(
            &branches,
            |branch| cleanup_calls.push(branch.id.clone()),
            |branch| {
                let attempts = delete_attempts.entry(branch.id.clone()).or_insert(0);
                *attempts += 1;
                if branch.id == "a" && *attempts == 1 {
                    Err("simulated transient delete failure".to_string())
                } else {
                    Ok(())
                }
            },
        );

        assert_eq!(delete_attempts.get("a"), Some(&2));
        assert_eq!(delete_attempts.get("b"), Some(&1));
        assert_eq!(
            cleanup_calls,
            vec![
                "a".to_string(),
                "b".to_string(),
                "a".to_string(),
                "b".to_string()
            ]
        );
    }

    #[test]
    fn delete_project_cleanup_final_sweep_only_reprocesses_remote_branches() {
        let mut local_branch = Branch::new("project-1", "feature-local", "main");
        local_branch.id = "local".to_string();
        let branches = vec![
            remote_branch("project-1", "remote", "feature-remote", "ws-shared"),
            local_branch,
        ];

        let mut cleanup_calls: Vec<String> = Vec::new();
        let mut delete_attempts: HashMap<String, usize> = HashMap::new();

        cleanup_project_branches_best_effort(
            &branches,
            |branch| cleanup_calls.push(branch.id.clone()),
            |branch| {
                let attempts = delete_attempts.entry(branch.id.clone()).or_insert(0);
                *attempts += 1;
                if branch.id == "remote" && *attempts == 1 {
                    Err("simulated transient delete failure".to_string())
                } else {
                    Ok(())
                }
            },
        );

        assert_eq!(delete_attempts.get("remote"), Some(&2));
        assert_eq!(delete_attempts.get("local"), Some(&1));
        assert_eq!(
            cleanup_calls,
            vec![
                "remote".to_string(),
                "local".to_string(),
                "remote".to_string()
            ]
        );
        assert_eq!(branches[0].branch_type, BranchType::Remote);
        assert_eq!(branches[1].branch_type, BranchType::Local);
    }
}
