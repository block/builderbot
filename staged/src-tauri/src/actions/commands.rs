//! Tauri commands for action execution and detection

use anyhow::Result;
use builderbot_actions::{
    ActionDetector, ActionExecutor, ActionMetadata, FileExplorationMode, SuggestedAction,
};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

use crate::store::Store;

use super::ai_provider::AcpAiProvider;
use super::events::TauriExecutionListener;

/// Helper to get store from Mutex<Option<Arc<Store>>>
fn get_store(store: &State<'_, Mutex<Option<Arc<Store>>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .as_ref()
        .ok_or_else(|| "Store not initialized".to_string())
        .cloned()
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DetectingActionsEvent {
    github_repo: String,
    subpath: Option<String>,
    detecting: bool,
}

async fn detect_actions_for_repo_context(
    github_repo: &str,
    subpath: Option<&str>,
) -> Result<Vec<SuggestedAction>, String> {
    // Check whether a local clone already exists on disk.
    let local_clone = crate::paths::repos_dir()
        .map(|d| d.join(github_repo))
        .filter(|p| p.exists());

    // Pick the right AI provider working directory. When we have a local
    // clone we point the provider at it; otherwise we use a temp dir (the
    // provider only needs a cwd for spawning processes, not for file access).
    let provider_dir = match &local_clone {
        Some(clone_path) => match subpath {
            Some(subpath) => clone_path.join(subpath),
            None => clone_path.clone(),
        },
        None => std::env::temp_dir(),
    };

    let provider = AcpAiProvider::new(provider_dir.clone())
        .map_err(|e| format!("Failed to create AI provider: {e}"))?;

    let detector = ActionDetector::new(Box::new(provider));

    let mode = match local_clone {
        Some(_) => FileExplorationMode::Local {
            working_dir: provider_dir,
        },
        None => FileExplorationMode::GitHub {
            repo: github_repo.to_string(),
            subpath: subpath.map(str::to_string),
        },
    };

    detector
        .detect_actions_with_mode(mode)
        .await
        .map_err(|e| format!("Action detection failed: {e}"))
}

fn resolve_branch_repo_context(
    store: &Store,
    branch: &crate::store::Branch,
    project: &crate::store::Project,
) -> Result<(String, Option<String>), String> {
    if let Some(project_repo_id) = &branch.project_repo_id {
        let project_repo = store
            .get_project_repo(project_repo_id)
            .map_err(|e| format!("Failed to get project repo: {e}"))?
            .ok_or_else(|| format!("Project repo not found: {project_repo_id}"))?;
        return Ok((project_repo.github_repo, project_repo.subpath));
    }

    let repo = project
        .primary_repo()
        .ok_or_else(|| "Project has no repository attached".to_string())?;
    Ok((repo.to_string(), project.subpath.clone()))
}

/// Detect available actions from a project's build files using AI.
///
/// If a local clone already exists on disk we read files from the filesystem.
/// Otherwise we use the GitHub API (via `gh`) to inspect the repository
/// remotely, avoiding an expensive clone just for action detection.
#[tauri::command]
pub async fn detect_project_actions(
    project_id: String,
    app: AppHandle,
    store: State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Vec<SuggestedAction>, String> {
    let store = get_store(&store)?;

    // Get the project
    let project = store
        .get_project(&project_id)
        .map_err(|e| format!("Failed to get project: {e}"))?
        .ok_or_else(|| "Project not found".to_string())?;
    let github_repo = project
        .primary_repo()
        .ok_or_else(|| "Project has no repository attached".to_string())?;

    let context = store
        .get_or_create_action_context(github_repo, project.subpath.as_deref())
        .map_err(|e| format!("Failed to get action context: {e}"))?;
    store
        .set_action_context_detecting(&context.id, true)
        .map_err(|e| format!("Failed to set detection status: {e}"))?;
    let _ = app.emit(
        "repo-actions-detection",
        DetectingActionsEvent {
            github_repo: github_repo.to_string(),
            subpath: project.subpath.clone(),
            detecting: true,
        },
    );

    let result = detect_actions_for_repo_context(github_repo, project.subpath.as_deref()).await;

    store
        .mark_action_context_detected(&context.id)
        .map_err(|e| format!("Failed to update detection status: {e}"))?;
    let _ = app.emit(
        "repo-actions-detection",
        DetectingActionsEvent {
            github_repo: github_repo.to_string(),
            subpath: project.subpath.clone(),
            detecting: false,
        },
    );

    result
}

/// Detect available actions for a specific repo+subpath context using AI.
#[tauri::command(rename_all = "camelCase")]
pub async fn detect_repo_actions(
    github_repo: String,
    subpath: Option<String>,
    app: AppHandle,
    store: State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Vec<SuggestedAction>, String> {
    let store = get_store(&store)?;
    let context = store
        .get_or_create_action_context(&github_repo, subpath.as_deref())
        .map_err(|e| format!("Failed to get action context: {e}"))?;
    store
        .set_action_context_detecting(&context.id, true)
        .map_err(|e| format!("Failed to set detection status: {e}"))?;
    let _ = app.emit(
        "repo-actions-detection",
        DetectingActionsEvent {
            github_repo: github_repo.clone(),
            subpath: subpath.clone(),
            detecting: true,
        },
    );

    let result = detect_actions_for_repo_context(&github_repo, subpath.as_deref()).await;

    store
        .mark_action_context_detected(&context.id)
        .map_err(|e| format!("Failed to update detection status: {e}"))?;
    let _ = app.emit(
        "repo-actions-detection",
        DetectingActionsEvent {
            github_repo,
            subpath,
            detecting: false,
        },
    );
    result
}

/// Run an action for a branch
#[tauri::command]
pub async fn run_branch_action(
    branch_id: String,
    action_id: String,
    app: AppHandle,
    store: State<'_, Mutex<Option<Arc<Store>>>>,
    executor: State<'_, Arc<ActionExecutor>>,
) -> Result<String, String> {
    log::info!(
        "[run_branch_action] Starting action execution request - branch_id: {}, action_id: {}",
        branch_id,
        action_id
    );

    let store = get_store(&store)?;

    // Get the action
    let action = store
        .get_repo_action(&action_id)
        .map_err(|e| format!("Failed to get action: {e}"))?
        .ok_or_else(|| "Action not found".to_string())?;

    log::info!(
        "[run_branch_action] Found action - name: {}, command: {}",
        action.name,
        action.command
    );

    // Get the branch and its project (for repo context + subpath)
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| format!("Failed to get branch: {e}"))?
        .ok_or_else(|| "Branch not found".to_string())?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| format!("Failed to get project: {e}"))?
        .ok_or_else(|| "Project not found".to_string())?;

    let (github_repo, subpath) = resolve_branch_repo_context(&store, &branch, &project)?;
    let context = store
        .get_or_create_action_context(&github_repo, subpath.as_deref())
        .map_err(|e| format!("Failed to get action context: {e}"))?;
    if action.context_id != context.id {
        return Err("Action does not belong to this repo/subpath context".to_string());
    }

    // Get the worktree path for this branch, then apply the repo subpath
    let workdir = store
        .get_workdir_for_branch(&branch_id)
        .map_err(|e| format!("Failed to get workdir: {e}"))?
        .ok_or_else(|| "No worktree found for branch".to_string())?;

    let working_dir = if let Some(subpath) = &subpath {
        let path = std::path::PathBuf::from(&workdir.path).join(subpath);
        path.to_string_lossy().to_string()
    } else {
        workdir.path
    };

    log::info!(
        "[run_branch_action] Resolved working directory: {}",
        working_dir
    );

    // Create event listener
    let listener = Arc::new(TauriExecutionListener::new(
        app,
        branch_id.clone(),
        action_id.clone(),
        action.name.clone(),
    ));

    // Create metadata
    let metadata = ActionMetadata {
        action_id: action.id.clone(),
        action_name: action.name.clone(),
        auto_commit: action.auto_commit,
    };

    // Execute the action
    log::info!(
        "[run_branch_action] Calling executor.execute - action: {}, working_dir: {}",
        action.name,
        working_dir
    );

    let result = executor
        .execute(action.command, working_dir, metadata, listener)
        .await
        .map_err(|e| format!("Failed to execute action: {e}"));

    match &result {
        Ok(execution_id) => {
            log::info!(
                "[run_branch_action] Action execution started successfully - execution_id: {}, action: {}",
                execution_id,
                action.name
            );
        }
        Err(e) => {
            log::error!(
                "[run_branch_action] Action execution failed - action: {}, error: {}",
                action.name,
                e
            );
        }
    }

    result
}

/// Stop a running action
#[tauri::command]
pub fn stop_branch_action(
    execution_id: String,
    executor: State<'_, Arc<ActionExecutor>>,
) -> Result<(), String> {
    executor
        .stop(&execution_id)
        .map_err(|e| format!("Failed to stop action: {e}"))
}

/// Get all currently running actions for a branch
#[tauri::command]
pub fn get_running_branch_actions(
    _branch_id: String,
    executor: State<'_, Arc<ActionExecutor>>,
) -> Result<Vec<String>, String> {
    // Return list of running execution IDs
    Ok(executor.get_running_ids())
}

/// Get buffered output for an action execution
#[tauri::command]
pub fn get_action_output_buffer(
    execution_id: String,
    executor: State<'_, Arc<ActionExecutor>>,
) -> Result<Option<Vec<builderbot_actions::OutputChunk>>, String> {
    Ok(executor.get_buffered_output(&execution_id))
}

/// Clear buffered output for a completed execution
#[tauri::command]
pub fn clear_action_execution(
    execution_id: String,
    executor: State<'_, Arc<ActionExecutor>>,
) -> Result<bool, String> {
    Ok(executor.clear_execution(&execution_id))
}

/// Run all prerun actions for a branch after creation
#[tauri::command]
pub async fn run_prerun_actions(
    branch_id: String,
    app: AppHandle,
    store: State<'_, Mutex<Option<Arc<Store>>>>,
    executor: State<'_, Arc<ActionExecutor>>,
) -> Result<Vec<String>, String> {
    let store = get_store(&store)?;

    // Get the branch and project (for repo context + subpath)
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| format!("Failed to get branch: {e}"))?
        .ok_or_else(|| "Branch not found".to_string())?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| format!("Failed to get project: {e}"))?
        .ok_or_else(|| "Project not found".to_string())?;

    let (github_repo, subpath) = resolve_branch_repo_context(&store, &branch, &project)?;
    let context = store
        .get_or_create_action_context(&github_repo, subpath.as_deref())
        .map_err(|e| format!("Failed to get action context: {e}"))?;

    // First time we see this repo+subpath, detect actions before running prerun.
    if !context.has_detected_actions {
        store
            .set_action_context_detecting(&context.id, true)
            .map_err(|e| format!("Failed to set detection status: {e}"))?;
        let _ = app.emit(
            "repo-actions-detection",
            DetectingActionsEvent {
                github_repo: github_repo.clone(),
                subpath: subpath.clone(),
                detecting: true,
            },
        );

        let detected = detect_actions_for_repo_context(&github_repo, subpath.as_deref())
            .await
            .unwrap_or_default();

        let existing_actions = store
            .list_repo_actions(&context.id)
            .map_err(|e| format!("Failed to list actions: {e}"))?;
        let mut existing_commands: std::collections::HashSet<String> =
            existing_actions.iter().map(|a| a.command.clone()).collect();
        let mut next_sort_order = existing_actions
            .iter()
            .map(|a| a.sort_order)
            .max()
            .unwrap_or(-1)
            + 1;

        for suggestion in detected {
            if existing_commands.contains(&suggestion.command) {
                continue;
            }
            existing_commands.insert(suggestion.command.clone());
            let action = crate::store::RepoAction::new(
                context.id.clone(),
                suggestion.name,
                suggestion.command,
                suggestion.action_type,
                next_sort_order,
            )
            .with_auto_commit(suggestion.auto_commit);
            store
                .create_repo_action(&action)
                .map_err(|e| format!("Failed to create detected action: {e}"))?;
            next_sort_order += 1;
        }

        store
            .mark_action_context_detected(&context.id)
            .map_err(|e| format!("Failed to update detection status: {e}"))?;
        let _ = app.emit(
            "repo-actions-detection",
            DetectingActionsEvent {
                github_repo: github_repo.clone(),
                subpath: subpath.clone(),
                detecting: false,
            },
        );
    }

    // Get all actions for this context.
    let actions = store
        .list_repo_actions(&context.id)
        .map_err(|e| format!("Failed to list actions: {e}"))?;

    // Filter to prerun actions
    let prerun_actions = actions
        .into_iter()
        .filter(|a| matches!(a.action_type, builderbot_actions::ActionType::Prerun))
        .collect::<Vec<_>>();

    // Get the worktree path for this branch, then apply the repo subpath
    let workdir = store
        .get_workdir_for_branch(&branch_id)
        .map_err(|e| format!("Failed to get workdir: {e}"))?
        .ok_or_else(|| "No worktree found for branch".to_string())?;

    let working_dir = if let Some(subpath) = &subpath {
        let path = std::path::PathBuf::from(&workdir.path).join(subpath);
        path.to_string_lossy().to_string()
    } else {
        workdir.path
    };

    // Execute each prerun action sequentially, waiting for each to complete
    // before starting the next one
    let mut execution_ids = Vec::new();
    for action in prerun_actions {
        let listener = Arc::new(TauriExecutionListener::new(
            app.clone(),
            branch_id.clone(),
            action.id.clone(),
            action.name.clone(),
        ));

        let metadata = ActionMetadata {
            action_id: action.id.clone(),
            action_name: action.name.clone(),
            auto_commit: action.auto_commit,
        };

        let execution_id = executor
            .execute_and_wait(action.command, working_dir.clone(), metadata, listener)
            .await
            .map_err(|e| format!("Failed to execute prerun action: {e}"))?;

        execution_ids.push(execution_id);
    }

    Ok(execution_ids)
}
