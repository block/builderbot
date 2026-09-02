//! Staged — AI-powered development workspace.
//!
//! Tauri commands for the new frontend, built incrementally.
//! See `src-archive/lib.rs` for the previous implementation.

pub(crate) mod acp_config;
pub mod acp_tools;
pub mod acp_tools_reconciler;
pub mod actions;
pub mod agent;
pub mod background_sync;
pub mod blox;
pub mod branches;
pub(crate) mod commit_reassociation;
pub mod diff_cache;
pub mod diff_commands;
pub mod doctor;
pub mod git;
pub mod github_commands;
pub mod image_commands;
pub mod managed_acp_tools;
pub mod managed_node;
pub mod migrations;
pub mod note_commands;
pub mod paths;
pub mod pikchr_mcp;
pub(crate) mod pikchr_subsession;
pub(crate) mod pikchr_validation;
pub mod pr_poll_scheduler;
pub mod project_commands;
pub mod project_mcp;
pub mod prs;
pub mod review_commands;
pub mod session_commands;
pub mod session_completion;
pub mod session_runner;
pub mod shell_env;
pub mod store;
pub mod store_events;
pub(crate) mod terminal_output;
pub mod timeline;
pub mod util_commands;
pub mod web_server;
pub mod window_commands;

#[cfg(test)]
pub mod test_utils;

use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
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

#[derive(Default)]
struct ShutdownState {
    quit_in_progress: AtomicBool,
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
    pub pr_head_sha: Option<String>,
    pub setup_complete: bool,
    pub worktree_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Number of finalized commits (with a git SHA) on this branch.
    /// Only populated by `list_branches_for_project`; `None` elsewhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_count: Option<u64>,
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
    pub author_email: String,
    /// Unix seconds the UI renders. Branch commits carry author time (`%at`),
    /// which a rebase preserves, so a rewritten commit still shows when it was
    /// written; pending rows carry their DB `created_at`, and repo-browse
    /// listings carry committer time.
    pub timestamp: i64,
    /// Unix seconds the timeline *sorts* on — never rendered. Same as
    /// `timestamp`, except that branch commits clamp it so it can't decrease in
    /// branch order (see `timeline::clamp_commit_sort_timestamps`): a
    /// cherry-picked commit keeps a week-old author date that would otherwise
    /// sort it above the commit it follows.
    pub sort_timestamp: i64,
    /// Position in git's topological order (0 = oldest on the branch).
    /// Used as a tiebreaker when multiple commits share the same second-level timestamp.
    pub order: i64,
    pub session_id: Option<String>,
    pub session_status: Option<String>,
    pub completion_reason: Option<String>,
    /// Whether this commit was authored by the current git user.
    pub is_own_commit: bool,
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
    pub completion_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
    pub suggested_next_commit_step: Option<String>,
    pub suggested_next_note_step: Option<String>,
    /// `None` for session-produced notes, `"written"` for user-authored ones —
    /// the frontend routes the latter to the editor instead of the viewer.
    pub subtype: Option<String>,
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
    pub session_provider: Option<String>,
    pub completion_reason: Option<String>,
    pub title: Option<String>,
    pub comment_count: usize,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
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
    pub completion_reason: Option<String>,
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
    pub git_state: Option<git::BranchGitState>,
}

/// A repo badge enriched with clone-state for the home screen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoHomeItem {
    #[serde(flatten)]
    pub badge: store::RepoBadge,
    /// Whether this repo has a local clone on disk.
    pub has_local_clone: bool,
}

/// Timeline of commits on a repo's default branch.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoDefaultBranchTimeline {
    pub commits: Vec<CommitTimelineItem>,
    pub default_branch: String,
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

fn stop_actions_for_app_shutdown(app_handle: &tauri::AppHandle) {
    let executor = app_handle.state::<Arc<actions::ActionExecutor>>();
    let registry = app_handle.state::<Arc<actions::ActionRegistry>>();
    let stopped_execution_ids = actions::commands::stop_all_actions(
        &executor,
        &registry,
        actions::StopOptions {
            force_kill_after: Some(Duration::from_secs(1)),
        },
    );

    if stopped_execution_ids.is_empty() {
        return;
    }

    if !executor.wait_for_executions(&stopped_execution_ids, Duration::from_secs(2)) {
        log::warn!(
            "Timed out waiting for {} action(s) to stop during app shutdown",
            stopped_execution_ids.len()
        );
    }
}

fn start_store_services(
    store: Arc<Store>,
    pr_scheduler: Arc<pr_poll_scheduler::PrPollScheduler>,
    app_handle: tauri::AppHandle,
) {
    background_sync::spawn(Arc::clone(&store), app_handle.clone());
    pr_poll_scheduler::spawn(pr_scheduler, store, app_handle);
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
fn reset_store(
    db_state: &DbState,
    store_slot: &Mutex<Option<Arc<Store>>>,
    store_change_tx: &tokio::sync::broadcast::Sender<store::StoreChange>,
) -> Result<Option<Arc<Store>>, String> {
    // This guard is the reset claim. Holding it through file deletion, store
    // creation, and slot replacement makes concurrent confirmations serialize;
    // the loser observes `None` and must not delete the newly created store.
    let mut needs_reset = db_state.needs_reset.lock().unwrap();
    match needs_reset.as_ref() {
        None => return Ok(None),
        Some(info) if info.kind == "needs_reset" => {}
        Some(_) => {
            return Err(
                "Database was created by a newer Staged version and cannot be reset".to_string(),
            );
        }
    }

    store::remove_db_files(&db_state.db_path).map_err(|e| e.to_string())?;

    let store = Arc::new(
        Store::new(&db_state.db_path)
            .map_err(|e| e.to_string())?
            .with_change_sender(store_change_tx.clone()),
    );
    *store_slot.lock().unwrap() = Some(Arc::clone(&store));
    *needs_reset = None;
    Ok(Some(store))
}

#[tauri::command]
fn confirm_reset_store(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    store_slot: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    store_change_tx: tauri::State<'_, tokio::sync::broadcast::Sender<store::StoreChange>>,
    pr_scheduler: tauri::State<'_, Arc<pr_poll_scheduler::PrPollScheduler>>,
) -> Result<(), String> {
    if let Some(store) = reset_store(&db_state, &store_slot, &store_change_tx)? {
        start_store_services(store, Arc::clone(pr_scheduler.inner()), app_handle.clone());
        // Every window owns its prompt state. Tell peers to dismiss it after
        // the shared backend store has been replaced successfully.
        if let Err(error) = app_handle.emit("store-reset-completed", ()) {
            log::warn!("Failed to broadcast store reset completion: {error}");
        }
    }
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

#[allow(clippy::too_many_arguments)]
#[tauri::command(rename_all = "camelCase")]
fn create_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    name: String,
    github_repo: Option<String>,
    location: Option<String>,
    subpath: Option<String>,
    branch_name: Option<String>,
    pr_number: Option<u64>,
    default_branch: Option<String>,
    head_repo: Option<String>,
) -> Result<store::Project, String> {
    let store = get_store(&store)?;
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Project name is required".to_string());
    }
    // Subpath validation is handled by the frontend before submission
    // (SubpathInput.waitForValidation). Skipping the redundant backend
    // re-validation here removes a ~500ms-2s GitHub API round-trip.

    let project_location = match location.as_deref() {
        Some("remote") if blox::is_sq_available() => store::ProjectLocation::Remote,
        Some("remote") => {
            log::warn!(
                "[create_project] remote project requested but sq CLI is unavailable; creating a local project instead"
            );
            store::ProjectLocation::Local
        }
        _ => store::ProjectLocation::Local,
    };
    let inferred_branch_name = branch_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| branches::infer_prefixed_branch_name(trimmed));
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
        let mut project_repo =
            store::ProjectRepo::new(&project.id, &repo, &inferred_branch_name, subpath).primary();
        project_repo.head_repo = head_repo;
        store
            .create_project_repo(&project_repo)
            .map_err(|e| e.to_string())?;

        // Create the initial branch record for the first repo so each new
        // project starts with exactly one branch tracked for that repository.
        // Use the frontend-prefetched default branch when available to avoid
        // a ~500ms-2s GitHub API round-trip during project creation.
        let effective_base = git::resolve_default_branch(default_branch, &repo);

        let (branch_id, is_local) = match project.location {
            store::ProjectLocation::Local => {
                let mut branch =
                    store::Branch::new(&project.id, &inferred_branch_name, &effective_base)
                        .with_project_repo(&project_repo.id);
                if let Some(pr) = pr_number {
                    branch = branch.with_pr(pr);
                }
                store.create_branch(&branch).map_err(|e| e.to_string())?;
                (branch.id, true)
            }
            store::ProjectLocation::Remote => {
                let workspace_name = branches::infer_workspace_name(&inferred_branch_name);
                let mut branch = store::Branch::new_remote(
                    &project.id,
                    &inferred_branch_name,
                    &effective_base,
                    &workspace_name,
                )
                .with_project_repo(&project_repo.id);
                if let Some(pr) = pr_number {
                    branch = branch.with_pr(pr);
                }
                store.create_branch(&branch).map_err(|e| e.to_string())?;
                log::info!(
                    "[create_project] created remote branch={} workspace={} status=starting project={}",
                    branch.id,
                    workspace_name,
                    project.id
                );
                (branch.id, false)
            }
        };

        if is_local {
            let project_id = project.id.clone();
            let store_bg = Arc::clone(&store);
            tauri::async_runtime::spawn(async move {
                web_server::emit_to_all(&app_handle, "project-setup-progress", project_id.clone());

                let store_clone = Arc::clone(&store_bg);
                let branch_id_clone = branch_id.clone();
                let app_handle_clone = app_handle.clone();
                let worktree_result = tauri::async_runtime::spawn_blocking(move || {
                    branches::setup_worktree_sync(
                        &store_clone,
                        &branch_id_clone,
                        Some(&app_handle_clone),
                    )
                })
                .await;

                match worktree_result {
                    Ok(Ok(path)) => {
                        log::info!("[create_project] worktree ready at {path}");
                        web_server::emit_to_all(
                            &app_handle,
                            "project-setup-progress",
                            project_id.clone(),
                        );
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

                if let branches::PrerunOutcome::Ran(_) = branches::claim_and_run_prerun_actions(
                    &store_bg,
                    &app_handle,
                    &branch_id,
                    &executor,
                    &act_registry,
                    None,
                    "create_project",
                )
                .await
                {
                    web_server::emit_to_all(&app_handle, "project-setup-progress", project_id);
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
fn get_suggested_repos(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    limit: Option<usize>,
) -> Result<Vec<store::SuggestedRepo>, String> {
    let store = get_store(&store)?;
    store
        .get_suggested_repos_for_project(&project_id, limit.unwrap_or(5))
        .map_err(|e| e.to_string())
}

#[allow(clippy::too_many_arguments)]
#[tauri::command(rename_all = "camelCase")]
async fn add_project_repo(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    project_id: String,
    github_repo: String,
    branch_name: Option<String>,
    subpath: Option<String>,
    set_as_primary: Option<bool>,
    pr_number: Option<u64>,
    default_branch: Option<String>,
    head_repo: Option<String>,
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
        pr_number,
        default_branch,
        head_repo,
    )
    .await?;

    // Spawn background worktree + prerun-actions setup — fire and forget.
    log::info!(
        "[add_project_repo] spawning background setup for repo {} in project {}",
        repo.id,
        project_id
    );
    tauri::async_runtime::spawn({
        let repo_id = repo.id.clone();
        async move {
            web_server::emit_to_all(&app_handle, "project-setup-progress", project_id.clone());

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
                let app_handle_clone = app_handle.clone();
                let worktree_result = tauri::async_runtime::spawn_blocking(move || {
                    branches::setup_worktree_sync(&store_clone, &branch_id, Some(&app_handle_clone))
                })
                .await;

                match worktree_result {
                    Ok(Ok(path)) => {
                        log::info!("[add_project_repo] worktree ready at {path}");
                        web_server::emit_to_all(
                            &app_handle,
                            "project-setup-progress",
                            project_id.clone(),
                        );
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
                if let branches::PrerunOutcome::Ran(_) = branches::claim_and_run_prerun_actions(
                    &store,
                    &app_handle,
                    &branch.id,
                    &executor,
                    &act_registry,
                    None,
                    "add_project_repo",
                )
                .await
                {
                    web_server::emit_to_all(
                        &app_handle,
                        "project-setup-progress",
                        project_id.clone(),
                    );
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
                        web_server::emit_to_all(&app_handle, "project-setup-progress", project_id);
                        return;
                    }
                }
                web_server::emit_to_all(&app_handle, "project-setup-progress", project_id);
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

// =============================================================================
// Repo badge commands
// =============================================================================

#[tauri::command]
fn get_all_repo_badges(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Vec<store::RepoBadge>, String> {
    get_store(&store)?
        .list_repo_badges()
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn delete_repo_badge(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    github_repo: String,
    subpath: String,
) -> Result<(), String> {
    get_store(&store)?
        .delete_repo_badge(&github_repo, &subpath)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn update_repo_badge(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    github_repo: String,
    subpath: String,
    short_name: String,
    hue: f64,
) -> Result<store::RepoBadge, String> {
    let short_name = short_name.trim().to_string();
    if short_name.is_empty() || short_name.len() > 6 {
        return Err("Short name must be 1-6 characters".into());
    }
    if !short_name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.')
    {
        return Err("Short name must be lowercase alphanumeric (dots allowed)".into());
    }
    if short_name.chars().filter(|&c| c == '.').count() > 1
        || short_name.starts_with('.')
        || short_name.ends_with('.')
    {
        return Err("Short name may contain at most one dot, not at start or end".into());
    }
    if !(0.0..=360.0).contains(&hue) {
        return Err("Hue must be between 0 and 360".into());
    }
    let store = get_store(&store)?;

    // Check if another badge already uses this short name.
    if let Some(existing) = store
        .get_repo_badge_by_short_name(&short_name)
        .map_err(|e| e.to_string())?
    {
        let is_same_badge = existing.github_repo == github_repo && existing.subpath == subpath;
        if !is_same_badge {
            let owner = if existing.subpath.is_empty() {
                existing.github_repo.clone()
            } else {
                format!("{} ({})", existing.github_repo, existing.subpath)
            };
            return Err(format!(
                "Short name '{}' is already used by {}",
                short_name, owner
            ));
        }
    }

    store
        .update_repo_badge(&github_repo, &subpath, &short_name, hue)
        .map_err(|e| e.to_string())?;
    store
        .get_repo_badge(&github_repo, &subpath)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            format!(
                "Badge not found after update for {}/{}",
                github_repo, subpath
            )
        })
}

#[tauri::command(rename_all = "camelCase")]
fn pin_repo(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    github_repo: String,
    subpath: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    store
        .pin_repo(&github_repo, &subpath)
        .map_err(|e| e.to_string())?;

    // Backfill default_branch if not yet detected
    if let Ok(Some(badge)) = store.get_repo_badge(&github_repo, &subpath) {
        if badge.default_branch.is_none() {
            if let Err(e) = detect_and_store_default_branch(&store, &github_repo, &subpath) {
                log::warn!("[pin_repo] failed to backfill default_branch: {e}");
            }
        }
    }

    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
fn unpin_repo(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    github_repo: String,
    subpath: String,
) -> Result<(), String> {
    get_store(&store)?
        .unpin_repo(&github_repo, &subpath)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn reorder_pinned_repos(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    ordered_keys: Vec<(String, String)>,
) -> Result<(), String> {
    get_store(&store)?
        .reorder_pinned_repos(&ordered_keys)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
async fn list_repos_for_home(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Vec<RepoHomeItem>, String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        let badges = store.list_repos_for_home().map_err(|e| e.to_string())?;
        let items: Vec<RepoHomeItem> = badges
            .into_iter()
            .map(|badge| {
                let has_local_clone = crate::paths::clone_path_for(&badge.github_repo)
                    .map(|p| p.join(".git").exists())
                    .unwrap_or(false);
                RepoHomeItem {
                    badge,
                    has_local_clone,
                }
            })
            .collect();
        Ok(items)
    })
    .await
    .map_err(|e| format!("list_repos_for_home task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
fn set_repo_default_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    github_repo: String,
    subpath: String,
    default_branch: String,
) -> Result<(), String> {
    get_store(&store)?
        .set_default_branch(&github_repo, &subpath, &default_branch)
        .map_err(|e| e.to_string())
}

/// Detect the default branch for a repo, cache it in repo_badges, and return it.
///
/// Fallback chain:
/// 1. Stored value in `repo_badges.default_branch`
/// 2. `git remote show origin | grep "HEAD branch"` on the local clone
/// 3. Local ref check for `origin/main` then `origin/master`
/// 4. `"main"` as the ultimate fallback
pub(crate) fn detect_and_store_default_branch(
    store: &Arc<Store>,
    github_repo: &str,
    subpath: &str,
) -> Result<String, String> {
    // 1. Check stored value first
    if let Ok(Some(badge)) = store.get_repo_badge(github_repo, subpath) {
        if let Some(ref branch) = badge.default_branch {
            return Ok(branch.clone());
        }
    }

    // 2. Try detecting from the local clone
    let branch = if let Some(clone_path) = crate::paths::clone_path_for(github_repo) {
        if clone_path.exists() {
            git::detect_default_branch_from_remote(&clone_path)
                .unwrap_or_else(|_| "main".to_string())
        } else {
            // No local clone — fall back to GitHub API
            git::detect_default_branch_for_repo(github_repo).unwrap_or_else(|_| "main".to_string())
        }
    } else {
        git::detect_default_branch_for_repo(github_repo).unwrap_or_else(|_| "main".to_string())
    };

    // 3. Cache the result in repo_badges (best-effort — badge may not exist yet)
    if let Err(e) = store.set_default_branch(github_repo, subpath, &branch) {
        log::warn!("[detect_default_branch] failed to cache default_branch for {github_repo}: {e}");
    }

    Ok(branch)
}

#[tauri::command(rename_all = "camelCase")]
fn detect_default_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    github_repo: String,
    subpath: String,
) -> Result<String, String> {
    let store = get_store(&store)?;
    detect_and_store_default_branch(&store, &github_repo, &subpath)
}

/// Get the commit timeline for a repo's default branch from its local clone.
#[tauri::command(rename_all = "camelCase")]
async fn get_repo_default_branch_timeline(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    github_repo: String,
    subpath: String,
    limit: Option<usize>,
) -> Result<RepoDefaultBranchTimeline, String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        let default_branch = detect_and_store_default_branch(&store, &github_repo, &subpath)?;
        let clone_path = crate::paths::clone_path_for(&github_repo)
            .ok_or_else(|| "Cannot determine clone path".to_string())?;
        if !clone_path.join(".git").exists() {
            return Err(format!(
                "No local clone found for {github_repo}. Clone the repo first."
            ));
        }

        let max_count = limit.unwrap_or(50);
        let origin_ref = format!("origin/{default_branch}");
        let limit_arg = format!("-{max_count}");
        let output = git::cli_run(
            &clone_path,
            &[
                "log",
                &limit_arg,
                git::BRANCH_COMMIT_LOG_FORMAT,
                &origin_ref,
            ],
        )
        .map_err(|e| format!("Failed to get commits for {github_repo}: {e}"))?;

        let mut commits: Vec<CommitTimelineItem> = output
            .lines()
            .enumerate()
            .filter_map(|(i, line)| {
                let fields = git::parse_branch_commit_line(line)?;
                Some(CommitTimelineItem {
                    id: None,
                    sha: fields.sha.to_string(),
                    short_sha: fields.short_sha.to_string(),
                    subject: fields.subject.to_string(),
                    author: fields.author.to_string(),
                    author_email: fields.author_email.to_string(),
                    // Committer time: this listing never interleaves with notes,
                    // so it has no reason to prefer the rebase-stable clock.
                    timestamp: fields.committer_timestamp,
                    sort_timestamp: fields.committer_timestamp,
                    order: (max_count - 1 - i) as i64,
                    session_id: None,
                    session_status: None,
                    completion_reason: None,
                    is_own_commit: false,
                })
            })
            .collect();

        // Mark commits by current user
        let identity_name = git::cli_run(&clone_path, &["config", "user.name"])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let identity_email = git::cli_run(&clone_path, &["config", "user.email"])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        if identity_name.is_some() || identity_email.is_some() {
            for commit in &mut commits {
                // Simple name/email match
                if let Some(ref name) = identity_name {
                    if commit.author.eq_ignore_ascii_case(name) {
                        commit.is_own_commit = true;
                        continue;
                    }
                }
                if let Some(ref email) = identity_email {
                    if commit.author_email.eq_ignore_ascii_case(email) {
                        commit.is_own_commit = true;
                    }
                }
            }
        }

        Ok(RepoDefaultBranchTimeline {
            commits,
            default_branch,
        })
    })
    .await
    .map_err(|e| format!("get_repo_default_branch_timeline task failed: {e}"))?
}

/// Clone a repo locally that has only been used remotely, then detect its default branch.
#[tauri::command(rename_all = "camelCase")]
async fn clone_repo_locally(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    github_repo: String,
) -> Result<String, String> {
    let store = get_store(&store)?;
    let github_repo_clone = github_repo.clone();
    let store_clone = Arc::clone(&store);

    let clone_path = tauri::async_runtime::spawn_blocking(move || {
        git::ensure_local_clone(&github_repo_clone).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("Clone task failed: {e}"))??;

    // Detect and cache the default branch
    let default_branch = {
        let store_ref = Arc::clone(&store_clone);
        let github_repo_ref = github_repo.clone();
        let clone_path_ref = clone_path.clone();
        tauri::async_runtime::spawn_blocking(move || {
            // `git remote show origin` on a fresh local clone is the canonical
            // source of truth for the default branch, so a successful detection
            // here should overwrite any pre-clone guess (which may be the
            // `"main"` fallback from `detect_and_store_default_branch`).
            match git::detect_default_branch_from_remote(&clone_path_ref) {
                Ok(branch) => {
                    let badges = store_ref.list_repo_badges().unwrap_or_default();
                    for badge in &badges {
                        if badge.github_repo == github_repo_ref {
                            if let Err(e) = store_ref.set_default_branch(
                                &github_repo_ref,
                                &badge.subpath,
                                &branch,
                            ) {
                                log::warn!(
                                    "[clone_repo_locally] failed to set default_branch for {} ({}): {e}",
                                    github_repo_ref,
                                    badge.subpath
                                );
                            }
                        }
                    }
                    branch
                }
                Err(_) => "main".to_string(),
            }
        })
        .await
        .map_err(|e| format!("Default branch detection failed: {e}"))?
    };

    // Emit event so frontend can refresh
    web_server::emit_to_all(&app_handle, "repo-cloned", github_repo);

    Ok(default_branch)
}

/// Return the canonical absolute path where a repo's local clone lives.
///
/// This is the same path used by `ensure_local_clone` and background sync,
/// regardless of whether the clone currently exists on disk. The frontend
/// uses this for "Open in…" and "Copy Path" actions on pinned repo cards.
#[tauri::command(rename_all = "camelCase")]
fn get_repo_clone_path(github_repo: String) -> Result<String, String> {
    let path = crate::paths::clone_path_for(&github_repo)
        .ok_or_else(|| "Cannot determine clone path (no home directory)".to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// Build the prompt for AI short name generation.
fn build_badge_prompt(
    existing_badges: &[store::RepoBadge],
    new_repos: &[(String, String)],
) -> String {
    let mut prompt = String::from(
        "Generate short badge names for GitHub repositories. Each name must be:\n\
         - Max 6 characters\n\
         - Lowercase alphanumeric, may contain a single dot when combining components to make separation clear\n\
         - Unique across all existing and new names\n\
         - A recognizable abbreviation of the most distinguishing part\n\n\
         Examples of good shortenings:\n\
         block/goose → goose\n\
         block/builderbot → bbot\n\
         square/square → sq\n\
         block/mark → mark\n\
         cashapp/redwood → rdwd\n\
         block/bitkey → btky\n\
         block/wallet (apps/server) → wlt.sv\n\
         block/wallet (apps/mobile) → wlt.mb\n\
         block/goose (ui) → gse.ui\n\
         block/builderbot (apps/staged) → staged\n",
    );

    if !existing_badges.is_empty() {
        prompt.push_str("\nAlready assigned (do NOT reuse these names):\n");
        for b in existing_badges {
            if b.subpath.is_empty() {
                prompt.push_str(&format!("{} → {}\n", b.github_repo, b.short_name));
            } else {
                prompt.push_str(&format!(
                    "{} ({}) → {}\n",
                    b.github_repo, b.subpath, b.short_name
                ));
            }
        }
    }

    prompt.push_str("\nGenerate names for:\n");
    for (repo, subpath) in new_repos {
        if subpath.is_empty() {
            prompt.push_str(&format!("{}\n", repo));
        } else {
            prompt.push_str(&format!("{} ({})\n", repo, subpath));
        }
    }

    prompt.push_str(
        "\nRespond with ONLY a JSON object mapping each input to its short name. \
         Use the exact input strings as keys. Example:\n\
         {\"block/builderbot\": \"bbot\", \"block/wallet (apps/server)\": \"wlt.sv\"}\n",
    );

    prompt
}

fn find_badge_agent(provider: Option<&str>) -> Option<acp_client::AcpAgent> {
    if let Some(provider) = provider
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
    {
        let agent = acp_client::find_acp_agent_by_id(provider);
        if agent.is_none() {
            log::warn!("[repo_badges] selected badge-name provider `{provider}` is unavailable");
        }
        return agent;
    }

    let provider = session_commands::discover_preferred_provider_id(None)?;
    acp_client::find_acp_agent_by_id(&provider)
}

/// Try to generate short names via ACP. Returns a map from "repo" or "repo (subpath)" to short name.
async fn ai_generate_short_names(
    existing_badges: &[store::RepoBadge],
    new_repos: &[(String, String)],
    provider: Option<&str>,
) -> Option<std::collections::HashMap<String, String>> {
    let agent = find_badge_agent(provider)?;
    let prompt = build_badge_prompt(existing_badges, new_repos);
    let working_dir = std::env::temp_dir();
    let interpreter_env_snapshot = crate::shell_env::home_env_vars_with_extended_path(
        crate::session_runner::shell_env_cache().as_ref(),
    )
    .await;

    let response = acp_client::run_acp_prompt_with_interpreter_env_snapshot(
        &agent,
        &working_dir,
        &prompt,
        interpreter_env_snapshot,
    )
    .await
    .ok()?;

    // Extract JSON from response (may be wrapped in markdown code fences)
    let json_str = response
        .trim()
        .strip_prefix("```json")
        .or_else(|| response.trim().strip_prefix("```"))
        .and_then(|s| s.strip_suffix("```"))
        .unwrap_or(response.trim());

    let parsed: std::collections::HashMap<String, String> = serde_json::from_str(json_str).ok()?;

    // Validate all values: max 6 chars, lowercase alphanumeric + at most one dot
    let valid = parsed
        .into_iter()
        .filter(|(_, name)| {
            !name.is_empty()
                && name.len() <= 6
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.')
                && name.chars().filter(|&c| c == '.').count() <= 1
                && !name.starts_with('.')
                && !name.ends_with('.')
        })
        .collect::<std::collections::HashMap<_, _>>();

    if valid.is_empty() {
        None
    } else {
        Some(valid)
    }
}

#[tauri::command(rename_all = "camelCase")]
async fn ensure_repo_badges(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    repos: Vec<(String, String)>,
    provider: Option<String>,
) -> Result<Vec<store::RepoBadge>, String> {
    let store = get_store(&store)?;
    let mut result = Vec::new();
    let all_badges = store.list_repo_badges().map_err(|e| e.to_string())?;
    let taken: Vec<String> = all_badges.iter().map(|b| b.short_name.clone()).collect();
    let existing_hues = store.list_badge_hues().map_err(|e| e.to_string())?;
    let mut new_hues = existing_hues.clone();
    let mut new_taken = taken.clone();

    // Separate existing badges from repos that need generation
    let mut existing = Vec::new();
    let mut missing = Vec::new();
    for (github_repo, subpath) in &repos {
        let subpath_str = subpath.as_str();
        if let Some(badge) = store
            .get_repo_badge(github_repo, subpath_str)
            .map_err(|e| e.to_string())?
        {
            existing.push(badge);
        } else {
            missing.push((github_repo.clone(), subpath.clone()));
        }
    }
    result.extend(existing);

    if missing.is_empty() {
        return Ok(result);
    }

    // Try AI generation for all missing repos at once
    let ai_names = ai_generate_short_names(&all_badges, &missing, provider.as_deref()).await;

    for (github_repo, subpath) in &missing {
        let subpath_str = subpath.as_str();

        // Look up AI-generated name by the key format used in the prompt
        let ai_key = if subpath.is_empty() {
            github_repo.clone()
        } else {
            format!("{} ({})", github_repo, subpath)
        };

        let short_name = ai_names
            .as_ref()
            .and_then(|names| names.get(&ai_key))
            .filter(|name| !new_taken.contains(name))
            .cloned()
            .unwrap_or_else(|| store::fallback_short_name(github_repo, subpath_str, &new_taken));

        let hue = store::next_hue(&new_hues);
        let badge = store::RepoBadge {
            github_repo: github_repo.clone(),
            subpath: subpath.to_string(),
            short_name: short_name.clone(),
            hue,
            created_at: store::now_timestamp(),
            pinned: false,
            pin_sort_order: None,
            default_branch: None,
        };
        match store.create_repo_badge(&badge) {
            Ok(()) => {
                new_taken.push(short_name);
                new_hues.push(hue);
                result.push(badge);
            }
            Err(_) => {
                // Race: another call created the badge concurrently.
                // Re-fetch the authoritative row from the database.
                let existing = store
                    .get_repo_badge(github_repo, subpath_str)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| {
                        format!(
                            "Failed to create or find badge for {}/{}",
                            github_repo, subpath
                        )
                    })?;
                result.push(existing);
            }
        }
    }
    Ok(result)
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
#[allow(clippy::too_many_arguments)]
fn update_project_action(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    action_id: String,
    name: String,
    command: String,
    action_type: String,
    sort_order: i32,
    auto_commit: bool,
    pinned: bool,
    icon: Option<String>,
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
        pinned,
        icon,
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

/// Every action context's actions in one read-only call — the bulk hydration
/// path for surfaces that render one card per repo.
#[tauri::command]
fn list_all_repo_actions(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Vec<store::models::RepoContextActions>, String> {
    let store = get_store(&store)?;
    store.list_all_repo_actions().map_err(|e| e.to_string())
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
    pinned: bool,
    icon: Option<String>,
) -> Result<store::models::RepoAction, String> {
    let store = get_store(&store)?;
    let context = store
        .get_or_create_action_context(&github_repo, subpath.as_deref())
        .map_err(|e| e.to_string())?;
    let parsed_type = builderbot_actions::ActionType::parse(&action_type)
        .ok_or_else(|| format!("Invalid action type: {action_type}"))?;
    let action = store::models::RepoAction::new(context.id, name, command, parsed_type, sort_order)
        .with_auto_commit(auto_commit)
        .with_pinned(pinned)
        .with_icon(icon);
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

    // Look up the context before deleting so we can clean up the clone directory.
    let context = store
        .get_action_context(&context_id)
        .map_err(|e| e.to_string())?;

    store
        .delete_action_context(&context_id)
        .map_err(|e| e.to_string())?;

    // Clean up the git clone directory if no other action contexts reference the same repo.
    // This runs on a background thread so the UI isn't blocked by large repo deletions.
    if let Some(ctx) = context {
        let remaining = store
            .count_action_contexts_for_repo(&ctx.github_repo)
            .unwrap_or(1);
        if remaining == 0 {
            if let Some(clone_path) = crate::paths::clone_path_for(&ctx.github_repo) {
                std::thread::spawn(move || {
                    if clone_path.exists() {
                        // Rename to a temporary path first so that concurrent filesystem
                        // activity (e.g. Spotlight indexing) doesn't cause "Directory not
                        // empty" errors during removal.
                        let trash_path = clone_path.with_extension("deleting");
                        let target = if std::fs::rename(&clone_path, &trash_path).is_ok() {
                            trash_path
                        } else {
                            clone_path.clone()
                        };
                        if let Err(e) = std::fs::remove_dir_all(&target) {
                            log::warn!(
                                "Failed to remove clone directory {}: {e}",
                                target.display()
                            );
                        }
                    }
                    // Try to remove the parent owner directory if it's now empty.
                    if let Some(parent) = clone_path.parent() {
                        let _ = std::fs::remove_dir(parent);
                    }
                });
            }
        }
    }

    Ok(())
}

// =============================================================================
// Tauri App Setup
// =============================================================================

/// What the app menu handler should do with a menu event.
#[derive(Debug, PartialEq, Eq)]
enum MenuDispatch {
    /// Forward as this frontend event, addressed to the focused window.
    EmitToFocused(&'static str),
    /// Create a window here in the backend, with no project seed.
    OpenWindowUnseeded,
    /// Nothing to do — unknown item, or a window-scoped item with no target.
    Drop,
}

/// Route a menu item to its handler. Menu actions apply to the focused window
/// only — a broadcast would e.g. open settings in every window, or fire Delete
/// Project in each window against its own selected project.
///
/// With no window focused (every window minimized — reachable on macOS, where
/// the app menu stays live) window-scoped items drop, like a disabled menu item:
/// routing them to an arbitrary minimized window would open settings invisibly,
/// or delete whichever project that window happened to have selected. New Window
/// is the exception. It's exactly what a user reaches for when nothing is
/// visible, and it only round-trips through the frontend to inherit the opener's
/// selected project — with no opener there is nothing to inherit, so the backend
/// can just create it. That also un-strands the other items: the new window is
/// focused, so Settings/Find/zoom route normally again.
fn dispatch_menu_event(id: &str, has_focused_window: bool) -> MenuDispatch {
    let event_name = match id {
        "new_window" => "menu:new-window",
        "settings" => "menu:settings",
        "find" => "menu:find",
        "find_next" => "menu:find-next",
        "find_previous" => "menu:find-previous",
        "delete_project" => "menu:delete-project",
        "zoom_in" => "menu:zoom-in",
        "zoom_out" => "menu:zoom-out",
        "zoom_reset" => "menu:zoom-reset",
        _ => return MenuDispatch::Drop,
    };

    if has_focused_window {
        MenuDispatch::EmitToFocused(event_name)
    } else if id == "new_window" {
        MenuDispatch::OpenWindowUnseeded
    } else {
        MenuDispatch::Drop
    }
}

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
                // Only track the main window. Secondary `win-*` windows get
                // fresh labels each launch, so persisting their geometry would
                // accumulate stale entries in the state file that are never
                // restored — they are placed by cascade instead (see
                // `window_commands::new_window`).
                .with_filter(|label| label == "main")
                .build(),
        )
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            // Register the primary ACP bridge tools dir before any command
            // runs so binary resolution (session spawn, provider discovery)
            // prefers the managed bridge shims (or the STAGED_ACP_TOOLS_DIR
            // dev override) over user-installed copies. The shim dir path is
            // stable even before the first reconcile writes shims —
            // find_command probes per call and falls through to PATH until
            // then.
            if let Some(dir) = acp_tools::primary_tools_dir() {
                acp_client::set_bundled_tools_dir(dir);
            }

            // Install/upgrade the managed ACP bridges in the background at
            // launch and once a day thereafter; the renderer refreshes doctor +
            // provider discovery on each completion event.
            acp_tools_reconciler::spawn_reconcile_loop(app.handle());

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
                // ⇧⌘N, not plain ⌘N: the native accelerator consumes the
                // keydown before the webview sees it, and ⌘N belongs to the
                // frontend's New Project shortcut (`app-new-project`).
                let new_window_item = MenuItem::with_id(
                    handle,
                    "new_window",
                    "New Window",
                    true,
                    Some("CmdOrCtrl+Shift+N"),
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
                let delete_project_item = MenuItem::with_id(
                    handle,
                    "delete_project",
                    "Delete Project",
                    true,
                    Some("CmdOrCtrl+Backspace"),
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
                    &[
                        &new_window_item,
                        &PredefinedMenuItem::separator(handle)?,
                        &PredefinedMenuItem::close_window(handle, None)?,
                    ],
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
                        &delete_project_item,
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

            // `legacy-worktrees-layout` must run synchronously before
            // `Store::new` — it's a filesystem rename that `Store` and
            // various `paths::*` lookups assume has already happened.
            crate::migrations::run_pending(&[crate::migrations::Migration {
                id: "legacy-worktrees-layout",
                run: crate::paths::migrate_legacy_worktrees_layout,
            }]);

            // Check compatibility *before* creating the store.
            let compat = store::check_db_compatibility(&db_path)
                .map_err(|e| format!("Cannot check database: {e}"))?;
            let session_registry = Arc::new(session_runner::SessionRegistry::new());
            // Store change feed: every mutating store method publishes a
            // StoreChange here; the coalescer forwards them to all windows
            // and web clients as domain events. Created unconditionally
            // (like the scheduler) so `confirm_reset_store` can wire the
            // same feed into a replacement store.
            let (store_change_tx, store_change_rx) =
                tokio::sync::broadcast::channel::<store::StoreChange>(1024);
            store_events::spawn(app.handle().clone(), store_change_rx);
            app.manage(store_change_tx.clone());
            // Backend-owned PR-poll scheduler. Managed unconditionally so the
            // interest/hint commands resolve even before the store exists (e.g.
            // during the needs-reset prompt); the tick loop is spawned once the
            // store is ready, either below or after a confirmed reset.
            let pr_scheduler = Arc::new(pr_poll_scheduler::PrPollScheduler::new());

            let (store_slot, reset_info) = match compat {
                store::DbCompatibility::Ok => {
                    let s = Store::new(&db_path)
                        .map_err(|e| format!("Failed to open store: {e}"))?
                        .with_change_sender(store_change_tx.clone());
                    let store_arc = Arc::new(s);
                    // Recover sessions whose owner process is dead; leave sessions
                    // owned by other live Staged instances untouched.
                    session_runner::recover_dead_sessions(
                        Arc::clone(&store_arc),
                        Arc::clone(&session_registry),
                        app.handle().clone(),
                    );
                    session_runner::recover_stale_queued_session_messages(&store_arc);
                    // Release action-detection windows whose owner process is
                    // dead — a hard kill mid-detection otherwise leaves the
                    // flag set forever, rejecting all later detection for that
                    // repo. Windows owned by other live instances stay put.
                    match actions::commands::recover_orphaned_detection_claims(
                        &store_arc,
                        session_runner::is_process_alive,
                    ) {
                        0 => {}
                        n => log::info!(
                            "Released {n} orphaned action-detection claim(s) from previous run(s)"
                        ),
                    }
                    // Clean up images left in "pending" state from compose
                    // dialogs that were abandoned (e.g. user quit mid-dialog).
                    match store_arc.cleanup_pending_images() {
                        Ok(0) => {}
                        Ok(n) => log::info!("Cleaned up {n} pending image(s) from previous run"),
                        Err(e) => log::warn!("Failed to clean up pending images: {e}"),
                    }
                    // Start the store-backed services only once the store is
                    // ready. The reset path calls the same helper after it
                    // creates a compatible replacement.
                    start_store_services(
                        Arc::clone(&store_arc),
                        Arc::clone(&pr_scheduler),
                        app.handle().clone(),
                    );
                    // `fsmonitor-v1` only flips `.git/config` flags on stale
                    // clones the user may not visit this session — per-project
                    // `ensure_local_clone` already re-applies the same config
                    // idempotently, so we can move the sweep off the startup
                    // critical path. A bare `std::thread` keeps it off the
                    // tokio worker pool (the sweep is fully sync) and dodges
                    // any uncertainty about runtime readiness during setup.
                    std::thread::spawn(|| {
                        crate::migrations::run_pending(&[crate::migrations::Migration {
                            id: "fsmonitor-v1",
                            run: crate::git::config_apply::migrate_existing_clones,
                        }]);
                    });
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
            app.manage(session_registry);
            app.manage(pr_scheduler);
            app.manage(window_commands::NewWindowState::new());
            app.manage(window_commands::UpdaterWindowState::default());
            app.manage(Arc::new(actions::ActionExecutor::new()));
            app.manage(Arc::new(actions::ActionRegistry::new()));
            app.manage(ShutdownState::default());
            app.manage(DbState {
                db_path,
                needs_reset: Mutex::new(reset_info),
            });

            // Create the broadcast channel for web event streaming and manage it
            // so the web server and event emitters can access it.
            let (event_tx, _) = tokio::sync::broadcast::channel::<web_server::WebEvent>(256);
            app.manage(event_tx.clone());

            // Web server startup is stubbed out in this build.
            // TODO(web): restore web server startup from the `mobile-web` branch.
            let web_server_enabled = std::env::var("STAGED_WEB_SERVER")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);

            if web_server_enabled {
                let auth_token = web_server::generate_token();
                web_server::start(web_server::WebAppState {
                    app_handle: app.handle().clone(),
                    event_tx,
                    auth_token,
                    sessions: std::sync::Arc::new(std::sync::Mutex::new(
                        std::collections::HashSet::new(),
                    )),
                });
            }

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
            // Thin interpreter over `dispatch_menu_event`, which owns the
            // routing rules (and their tests).
            let focused = window_commands::focused_window(app);
            match dispatch_menu_event(event.id().as_ref(), focused.is_some()) {
                MenuDispatch::EmitToFocused(event_name) => {
                    // Some by construction: EmitToFocused is only returned when
                    // `focused.is_some()`.
                    if let Some(window) = focused {
                        if let Err(e) = app.emit_to(window.label(), event_name, ()) {
                            log::warn!("Failed to emit {event_name} event: {e}");
                        }
                    }
                }
                MenuDispatch::OpenWindowUnseeded => {
                    // Menus exist only on macOS, where menu events are delivered
                    // on the main thread — the same thread `setup` builds the
                    // first window on.
                    if let Err(e) = window_commands::open_new_window(app, None) {
                        log::warn!("Failed to open window from menu: {e}");
                    }
                }
                MenuDispatch::Drop => {}
            }
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                // Native windows have no WS heartbeat and their PR-poll client
                // ids are exempt from TTL eviction, so a closed window must
                // explicitly drop its interest or the scheduler keeps polling
                // at that window's cadence forever.
                let app = window.app_handle();
                app.state::<Arc<pr_poll_scheduler::PrPollScheduler>>()
                    .disconnect_client(format!(
                        "{}{}",
                        pr_poll_scheduler::TAURI_CLIENT_PREFIX,
                        window.label()
                    ));
                // Drop any unconsumed navigation seed (window closed pre-init).
                app.state::<window_commands::NewWindowState>()
                    .discard_seed(window.label());
                // The updater UI is window-owned but process-wide. A native
                // destruction hook is the reliable handoff point even when the
                // webview's frontend teardown never runs.
                if app
                    .state::<window_commands::UpdaterWindowState>()
                    .window_destroyed(window.label())
                {
                    if let Err(error) = app.emit("updater-owner-available", ()) {
                        log::warn!("Failed to announce updater ownership release: {error}");
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_store_status,
            confirm_reset_store,
            // Windows
            window_commands::new_window,
            window_commands::take_window_seed,
            window_commands::claim_updater_ownership,
            list_projects,
            create_project,
            list_project_repos,
            list_recent_repos,
            get_suggested_repos,
            add_project_repo,
            update_project_repo_branch_name,
            clear_project_repo_reason,
            remove_project_repo,
            set_primary_project_repo,
            delete_project,
            // Repo badges
            get_all_repo_badges,
            ensure_repo_badges,
            update_repo_badge,
            delete_repo_badge,
            // Pinned repos
            pin_repo,
            unpin_repo,
            reorder_pinned_repos,
            list_repos_for_home,
            set_repo_default_branch,
            detect_default_branch,
            get_repo_default_branch_timeline,
            clone_repo_locally,
            get_repo_clone_path,
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
            branches::get_branch,
            branches::list_branches_for_project,
            branches::create_branch,
            branches::setup_worktree,
            branches::setup_worktree_and_run_prerun,
            branches::setup_worktree_from_pr,
            branches::create_remote_branch,
            branches::start_workspace,
            branches::resume_workspace,
            branches::delete_branch,
            branches::rename_branch,
            branches::move_branch,
            branches::get_blox_env,
            branches::get_workspace_info,
            branches::poll_workspace_status,
            branches::poll_all_workspace_statuses,
            // Actions
            list_project_actions,
            update_project_action,
            delete_project_action,
            list_action_contexts,
            list_repo_actions,
            list_all_repo_actions,
            create_repo_action,
            delete_all_repo_actions,
            delete_action_context,
            // Timeline
            timeline::get_branch_timeline,
            timeline::refresh_branch_git_state,
            timeline::list_parent_branch_commits,
            timeline::reset_branch_to_remote,
            // Notes
            note_commands::create_note,
            note_commands::update_note,
            note_commands::delete_note,
            note_commands::get_note,
            note_commands::list_child_notes,
            note_commands::create_project_note,
            note_commands::list_project_notes,
            note_commands::get_project_note,
            note_commands::get_project_note_by_session,
            note_commands::get_branch_note_by_session,
            note_commands::delete_project_note,
            // Images
            image_commands::create_image,
            image_commands::get_image_path,
            image_commands::get_image_data,
            image_commands::delete_image,
            image_commands::list_branch_images,
            image_commands::create_image_from_data,
            // Timeline delete commands
            timeline::get_worktree_changes_preview,
            timeline::discard_worktree_changes,
            timeline::delete_review,
            timeline::delete_commit,
            timeline::delete_pending_commit,
            // Git helpers
            github_commands::list_git_branches,
            github_commands::detect_default_branch_cmd,
            github_commands::prune_remote_refs,
            github_commands::check_existing_local_branch,
            github_commands::get_pr_for_repo,
            github_commands::get_pr_for_branch,
            github_commands::list_pull_requests,
            github_commands::get_parent_repo,
            github_commands::list_issues,
            github_commands::post_comment_to_github,
            // PRs
            prs::create_pr,
            prs::get_pr_url,
            prs::update_branch_pr,
            prs::refresh_pr_status,
            prs::refresh_all_pr_statuses,
            prs::has_unpushed_commits,
            prs::push_branch,
            prs::pull_or_queue_branch,
            prs::rebase_branch,
            prs::squash_commits,
            prs::clear_branch_pr_status,
            prs::recover_branch_pr,
            // PR poll scheduler (frontend interest/hint layer)
            pr_poll_scheduler::set_foreground_project,
            pr_poll_scheduler::set_focus,
            pr_poll_scheduler::set_branch_pending,
            pr_poll_scheduler::refresh_now,
            pr_poll_scheduler::disconnect_client,
            // Utilities
            util_commands::open_url,
            util_commands::is_sq_available,
            util_commands::read_text_file,
            util_commands::resolve_path_aliases,
            util_commands::preferences_store_path,
            util_commands::check_blox_auth,
            util_commands::get_available_openers,
            util_commands::open_in_app,
            // Sessions
            session_commands::discover_acp_providers,
            session_commands::discover_acp_config,
            session_commands::get_active_sessions,
            session_commands::get_session,
            session_commands::get_session_messages,
            session_commands::get_session_messages_since,
            session_commands::get_session_acp_metadata_messages,
            session_commands::get_session_acp_metadata_messages_since,
            session_commands::get_session_acp_initialization,
            session_commands::count_assistant_messages_after,
            session_commands::start_session,
            session_commands::resume_session,
            session_commands::queue_session_message,
            session_commands::list_queued_session_messages,
            session_commands::delete_queued_session_message,
            session_commands::send_queued_session_message,
            session_commands::build_note_followup_message,
            session_commands::cancel_session,
            session_commands::stop_session_async_task,
            session_commands::get_session_background_hold,
            session_commands::delete_session,
            session_commands::start_branch_session,
            session_commands::start_or_queue_branch_session,
            session_commands::queue_branch_session,
            session_commands::drain_queued_sessions,
            session_commands::start_project_session,
            // Actions
            actions::commands::detect_repo_actions,
            actions::commands::run_branch_action,
            actions::commands::run_repo_action,
            actions::commands::stop_branch_action,
            actions::commands::get_running_branch_actions,
            actions::commands::get_all_running_actions,
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
            review_commands::link_comment_session,
            review_commands::get_branch_commit_by_session,
            review_commands::delete_comment,
            review_commands::delete_all_comments,
            review_commands::restore_comment,
            review_commands::get_deleted_comments,
            review_commands::add_reference_file,
            review_commands::remove_reference_file,
            // Doctor
            doctor::run_doctor,
            doctor::run_doctor_freshness,
            doctor::run_doctor_fix,
            doctor::run_doctor_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                let shutdown = app_handle.state::<ShutdownState>();
                if shutdown.quit_in_progress.swap(true, Ordering::SeqCst) {
                    return;
                }

                api.prevent_exit();
                stop_actions_for_app_shutdown(app_handle);
                app_handle.exit(0);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_project_branches_best_effort, dispatch_menu_event, reset_store, DbState,
        MenuDispatch, StoreIncompatibility,
    };
    use crate::store::{Branch, BranchType};
    use std::collections::HashMap;
    use std::sync::{Arc, Barrier, Mutex};

    fn reset_info(kind: &str) -> StoreIncompatibility {
        StoreIncompatibility {
            db_app_version: "0.1.0".to_string(),
            app_version: "0.2.0".to_string(),
            kind: kind.to_string(),
        }
    }

    #[test]
    fn concurrent_store_resets_create_the_replacement_once() {
        let dir = tempfile::tempdir().unwrap();
        let db_state = Arc::new(DbState {
            db_path: dir.path().join("data.db"),
            needs_reset: Mutex::new(Some(reset_info("needs_reset"))),
        });
        let store_slot = Arc::new(Mutex::new(None));
        let (store_change_tx, _) = tokio::sync::broadcast::channel(4);
        let barrier = Arc::new(Barrier::new(3));

        let handles: Vec<_> = (0..2)
            .map(|_| {
                let db_state = Arc::clone(&db_state);
                let store_slot = Arc::clone(&store_slot);
                let store_change_tx = store_change_tx.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    reset_store(&db_state, &store_slot, &store_change_tx)
                        .unwrap()
                        .is_some()
                })
            })
            .collect();

        barrier.wait();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();

        assert_eq!(results.iter().filter(|performed| **performed).count(), 1);
        assert!(store_slot.lock().unwrap().is_some());
        assert!(db_state.needs_reset.lock().unwrap().is_none());
        assert!(db_state.db_path.exists());
    }

    #[test]
    fn a_too_new_store_cannot_be_reset_by_invoking_the_command_directly() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("data.db");
        std::fs::write(&db_path, b"newer database").unwrap();
        let db_state = DbState {
            db_path: db_path.clone(),
            needs_reset: Mutex::new(Some(reset_info("too_new"))),
        };
        let store_slot = Mutex::new(None);
        let (store_change_tx, _) = tokio::sync::broadcast::channel(4);

        let error = match reset_store(&db_state, &store_slot, &store_change_tx) {
            Err(error) => error,
            Ok(_) => panic!("too-new store should not be reset"),
        };

        assert!(error.contains("newer Staged version"));
        assert_eq!(std::fs::read(db_path).unwrap(), b"newer database");
        assert!(store_slot.lock().unwrap().is_none());
        assert_eq!(
            db_state
                .needs_reset
                .lock()
                .unwrap()
                .as_ref()
                .map(|info| info.kind.as_str()),
            Some("too_new")
        );
    }

    /// Every menu item this app defines, with the frontend event it routes to.
    const MENU_ITEMS: &[(&str, &str)] = &[
        ("new_window", "menu:new-window"),
        ("settings", "menu:settings"),
        ("find", "menu:find"),
        ("find_next", "menu:find-next"),
        ("find_previous", "menu:find-previous"),
        ("delete_project", "menu:delete-project"),
        ("zoom_in", "menu:zoom-in"),
        ("zoom_out", "menu:zoom-out"),
        ("zoom_reset", "menu:zoom-reset"),
    ];

    #[test]
    fn menu_events_go_to_the_focused_window() {
        for (id, event_name) in MENU_ITEMS {
            assert_eq!(
                dispatch_menu_event(id, true),
                MenuDispatch::EmitToFocused(event_name),
                "menu item {id} should emit {event_name} to the focused window"
            );
        }
    }

    #[test]
    fn new_window_falls_back_to_native_creation_with_no_focused_window() {
        assert_eq!(
            dispatch_menu_event("new_window", false),
            MenuDispatch::OpenWindowUnseeded
        );
    }

    #[test]
    fn other_menu_events_drop_with_no_focused_window() {
        for (id, _) in MENU_ITEMS.iter().filter(|(id, _)| *id != "new_window") {
            assert_eq!(
                dispatch_menu_event(id, false),
                MenuDispatch::Drop,
                "window-scoped menu item {id} has no target and should drop"
            );
        }
    }

    #[test]
    fn unknown_menu_events_drop_regardless_of_focus() {
        for id in ["", "quit", "menu:new-window", "New Window"] {
            assert_eq!(dispatch_menu_event(id, true), MenuDispatch::Drop);
            assert_eq!(dispatch_menu_event(id, false), MenuDispatch::Drop);
        }
    }

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
