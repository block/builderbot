//! Tauri commands for action execution and detection

use anyhow::Result;
use builderbot_actions::{ActionDetector, ActionExecutor, ActionMetadata, SuggestedAction};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};

use crate::store::{ProjectAction, Store};

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

/// Detect available actions from a project's build files using AI
#[tauri::command]
pub async fn detect_project_actions(
    project_id: String,
    store: State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Vec<SuggestedAction>, String> {
    let store = get_store(&store)?;

    // Get the project
    let project = store
        .get_project(&project_id)
        .map_err(|e| format!("Failed to get project: {e}"))?
        .ok_or_else(|| "Project not found".to_string())?;

    // Ensure the clone exists before trying to detect actions
    let clone_path = crate::git::ensure_local_clone(&project.github_repo)
        .map_err(|e| format!("Failed to ensure local clone: {e}"))?;

    let working_dir = if let Some(subpath) = &project.subpath {
        clone_path.join(subpath)
    } else {
        clone_path
    };

    // Create AI provider with the working directory
    let provider = AcpAiProvider::new(working_dir.clone())
        .map_err(|e| format!("Failed to create AI provider: {e}"))?;

    // Create detector
    let detector = ActionDetector::new(Box::new(provider));

    // Detect actions
    detector
        .detect_actions(&working_dir)
        .await
        .map_err(|e| format!("Action detection failed: {e}"))
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
    let store = get_store(&store)?;

    // Get the action
    let action = store
        .get_project_action(&action_id)
        .map_err(|e| format!("Failed to get action: {e}"))?
        .ok_or_else(|| "Action not found".to_string())?;

    // Get the branch and its project (for subpath)
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| format!("Failed to get branch: {e}"))?
        .ok_or_else(|| "Branch not found".to_string())?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| format!("Failed to get project: {e}"))?
        .ok_or_else(|| "Project not found".to_string())?;

    // Get the worktree path for this branch, then apply the project subpath
    let workdir = store
        .get_workdir_for_branch(&branch_id)
        .map_err(|e| format!("Failed to get workdir: {e}"))?
        .ok_or_else(|| "No worktree found for branch".to_string())?;

    let working_dir = if let Some(subpath) = &project.subpath {
        let path = std::path::PathBuf::from(&workdir.path).join(subpath);
        path.to_string_lossy().to_string()
    } else {
        workdir.path
    };

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
    executor
        .execute(action.command, working_dir, metadata, listener)
        .await
        .map_err(|e| format!("Failed to execute action: {e}"))
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
    project_id: String,
    app: AppHandle,
    store: State<'_, Mutex<Option<Arc<Store>>>>,
    executor: State<'_, Arc<ActionExecutor>>,
) -> Result<Vec<String>, String> {
    let store = get_store(&store)?;

    // Get the project (for subpath)
    let project = store
        .get_project(&project_id)
        .map_err(|e| format!("Failed to get project: {e}"))?
        .ok_or_else(|| "Project not found".to_string())?;

    // Get all actions for the project
    let actions = store
        .list_project_actions(&project_id)
        .map_err(|e| format!("Failed to list actions: {e}"))?;

    // Filter to prerun actions
    let prerun_actions: Vec<ProjectAction> = actions
        .into_iter()
        .filter(|a| matches!(a.action_type, builderbot_actions::ActionType::Prerun))
        .collect();

    // Get the worktree path for this branch, then apply the project subpath
    let workdir = store
        .get_workdir_for_branch(&branch_id)
        .map_err(|e| format!("Failed to get workdir: {e}"))?
        .ok_or_else(|| "No worktree found for branch".to_string())?;

    let working_dir = if let Some(subpath) = &project.subpath {
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
