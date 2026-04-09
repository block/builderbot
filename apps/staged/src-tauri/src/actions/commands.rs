//! Tauri commands for action execution and detection

use anyhow::Result;
use builderbot_actions::{
    ActionDetector, ActionExecutor, ActionMetadata, ActionType, FileExplorationMode,
    RunDetectionMode, StopOptions, SuggestedAction,
};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::watch;

use crate::store::Store;

use super::ai_provider::AcpAiProvider;
use super::events::{emit_run_phase_changed, RunPhaseChangedEvent, TauriExecutionListener};
use super::registry::{ActionRegistry, RunPhase, RunningActionInfo};
use super::run_detector;

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

pub(crate) async fn detect_actions_for_repo_context(
    github_repo: &str,
    subpath: Option<&str>,
) -> Result<Vec<SuggestedAction>, String> {
    log::info!(
        "[detect_actions_for_repo_context] starting detection for repo={} subpath={:?}",
        github_repo,
        subpath
    );

    // Check whether a local clone already exists on disk.
    let local_clone = crate::paths::repos_dir()
        .map(|d| d.join(github_repo))
        .filter(|p| p.exists());

    // If we have a local clone, update its working tree to the latest remote
    // default branch so that action detection sees the current file layout.
    // This is essential when the upstream repo has been restructured (e.g. a
    // subpath moved) — without this the stale working tree would be missing
    // the expected directories. Only the main checkout is affected; worktrees
    // are separate directories and remain untouched.
    if let Some(clone_path) = &local_clone {
        crate::git::update_clone_to_remote_head(clone_path, github_repo);
    }

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
        Some(_) => {
            log::info!(
                "[detect_actions_for_repo_context] using Local mode, working_dir={}",
                provider_dir.display()
            );
            FileExplorationMode::Local {
                working_dir: provider_dir,
            }
        }
        None => {
            log::info!(
                "[detect_actions_for_repo_context] using GitHub mode, repo={} subpath={:?}",
                github_repo,
                subpath
            );
            FileExplorationMode::GitHub {
                repo: github_repo.to_string(),
                subpath: subpath.map(str::to_string),
            }
        }
    };

    let result = detector
        .detect_actions_with_mode(mode)
        .await
        .map_err(|e| format!("Action detection failed: {e}"));

    match &result {
        Ok(actions) => {
            log::info!(
                "[detect_actions_for_repo_context] detection complete: {} actions found",
                actions.len()
            );
        }
        Err(e) => {
            log::info!("[detect_actions_for_repo_context] detection failed: {e}");
        }
    }

    result
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
    registry: State<'_, Arc<ActionRegistry>>,
) -> Result<String, String> {
    let store = get_store(&store)?;

    // Get the action
    let action = store
        .get_repo_action(&action_id)
        .map_err(|e| format!("Failed to get action: {e}"))?
        .ok_or_else(|| "Action not found".to_string())?;

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

    let is_remote = branch.branch_type == crate::store::BranchType::Remote;

    let reg = registry.inner().clone();

    // Create event listener
    let listener = Arc::new(TauriExecutionListener::new(
        app.clone(),
        branch_id.clone(),
        action_id.clone(),
        action.name.clone(),
        action.action_type.as_str().to_string(),
        reg.clone(),
    ));

    // Create metadata
    let metadata = ActionMetadata {
        action_id: action.id.clone(),
        action_name: action.name.clone(),
        auto_commit: action.auto_commit,
    };

    // Clone values needed after execute() moves them.
    let command_for_detection = action.command.clone();

    // Execute the action — local vs remote paths
    let (execution_id, working_dir_for_detection) = if is_remote {
        // Remote branch: execute via `sq blox ws exec`
        let workspace_name = branch
            .workspace_name
            .as_deref()
            .ok_or_else(|| "Remote branch has no workspace name".to_string())?;

        // Check workspace status before running. A `None` status means
        // the workspace hasn't been polled yet — treat it as an error to
        // avoid a confusing `sq blox ws exec` failure.
        match branch.workspace_status {
            Some(crate::store::WorkspaceStatus::Running) => {} // OK
            Some(crate::store::WorkspaceStatus::Starting) => {
                return Err(
                    "Workspace is still starting. Please wait until it is running.".to_string(),
                );
            }
            Some(crate::store::WorkspaceStatus::Stopped) => {
                return Err(
                    "Workspace is stopped. Please restart it before running actions.".to_string(),
                );
            }
            Some(crate::store::WorkspaceStatus::Suspended) => {
                return Err(
                    "Workspace is suspended. Please resume it before running actions.".to_string(),
                );
            }
            Some(crate::store::WorkspaceStatus::Error) => {
                return Err("Workspace is in an error state.".to_string());
            }
            None => {
                return Err(
                    "Workspace status is unknown. Please wait for status to be determined."
                        .to_string(),
                );
            }
        }

        let repo_subpath = crate::branches::resolve_branch_workspace_subpath(&store, &branch)
            .map_err(|e| format!("Failed to resolve workspace subpath: {e}"))?;

        // Resolve the full path inside the workspace for this repo+subpath.
        let resolved_repo_path = match &repo_subpath {
            Some(subpath) => Some(
                crate::branches::resolve_workspace_repo_path(workspace_name, subpath)
                    .map_err(|e| format!("Failed to resolve workspace repo path: {e}"))?,
            ),
            None => None,
        };

        // Build the shell command to run inside the workspace.
        // If there's a subpath, cd into it first.
        // Note: `action.command` comes from the action config (not user input)
        // so it is trusted. The `resolved` path is shell-escaped for safety.
        let shell_command = match &resolved_repo_path {
            Some(resolved) => {
                format!(
                    "cd '{}' && {}",
                    resolved.replace('\'', "'\\''"),
                    action.command
                )
            }
            None => action.command.clone(),
        };

        // Find the sq binary and build args for `sq blox ws exec`
        let sq_binary = blox_cli::find_sq_binary().ok_or_else(|| {
            "Could not find `sq` binary. Is it installed and on your PATH?".to_string()
        })?;

        let args = vec![
            "blox".to_string(),
            "ws".to_string(),
            "exec".to_string(),
            workspace_name.to_string(),
            "--".to_string(),
            "sh".to_string(),
            "-lc".to_string(),
            shell_command,
        ];

        // Provide auto-commit context so that after a successful action,
        // git commands run on the remote workspace via `sq blox ws exec`.
        // When there's no resolved path we can't determine the git working
        // directory, so auto-commit is skipped (unlikely for remote branches).
        let auto_commit_info = resolved_repo_path
            .map(|resolved| (sq_binary.clone(), workspace_name.to_string(), resolved));

        let eid = executor
            .execute_remote(sq_binary, args, metadata, listener, auto_commit_info)
            .await
            .map_err(|e| format!("Failed to execute remote action: {e}"))?;

        // Remote actions don't have a local working dir for autodetect polling.
        // Use an empty string as a placeholder — Run detection that needs a
        // local path will gracefully degrade.
        (eid, String::new())
    } else {
        // Local branch: resolve worktree path
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

        let wd = working_dir.clone();
        let eid = executor
            .execute(action.command, working_dir, metadata, listener)
            .await
            .map_err(|e| format!("Failed to execute action: {e}"))?;

        (eid, wd)
    };

    // --- Run detection wiring (only for Run actions) ---
    if matches!(action.action_type, ActionType::Run) {
        // Ensure the output buffer for this execution_id exists so the
        // regex matcher can obtain a reference to it.
        reg.register_output_buffer(&execution_id);

        let detection_mode = action.run_detection_mode.clone();

        match detection_mode {
            Some(RunDetectionMode::EndpointRegex { pattern }) => {
                let (cancel_tx, cancel_rx) = watch::channel(false);
                reg.store_cancel_sender(&execution_id, cancel_tx);
                run_detector::spawn_regex_matcher(
                    app,
                    reg,
                    execution_id.clone(),
                    branch_id,
                    action.name.clone(),
                    pattern,
                    true,
                    cancel_rx,
                );
            }
            Some(RunDetectionMode::RunningRegex { pattern }) => {
                let (cancel_tx, cancel_rx) = watch::channel(false);
                reg.store_cancel_sender(&execution_id, cancel_tx);
                run_detector::spawn_regex_matcher(
                    app,
                    reg,
                    execution_id.clone(),
                    branch_id,
                    action.name.clone(),
                    pattern,
                    false,
                    cancel_rx,
                );
            }
            Some(RunDetectionMode::NoDetection) => {
                reg.set_run_phase(&execution_id, RunPhase::NoDetection);
                emit_run_phase_changed(
                    &app,
                    RunPhaseChangedEvent {
                        execution_id: execution_id.clone(),
                        branch_id,
                        action_name: action.name.clone(),
                        phase: RunPhase::NoDetection,
                    },
                );
            }
            Some(RunDetectionMode::Autodetect) | None => {
                reg.set_run_phase(&execution_id, RunPhase::AutodetectPending);
                emit_run_phase_changed(
                    &app,
                    RunPhaseChangedEvent {
                        execution_id: execution_id.clone(),
                        branch_id: branch_id.clone(),
                        action_name: action.name.clone(),
                        phase: RunPhase::AutodetectPending,
                    },
                );

                let (cancel_tx, cancel_rx) = watch::channel(false);
                reg.store_cancel_sender(&execution_id, cancel_tx);
                run_detector::spawn_autodetect_poller(
                    app,
                    store,
                    reg,
                    execution_id.clone(),
                    branch_id,
                    action_id,
                    action.name.clone(),
                    command_for_detection,
                    std::path::PathBuf::from(&working_dir_for_detection),
                    cancel_rx,
                );
            }
        }
    }

    Ok(execution_id)
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

/// Stop all running actions for the given branch IDs (best-effort).
pub fn stop_actions_for_branches(
    executor: &ActionExecutor,
    registry: &ActionRegistry,
    branch_ids: &[&str],
) {
    for branch_id in branch_ids {
        for info in registry.get_running_for_branch(branch_id) {
            if executor.is_running(&info.execution_id) {
                if let Err(e) = executor.stop(&info.execution_id) {
                    log::warn!("Failed to stop action {}: {e}", info.execution_id);
                }
            }
        }
    }
}

/// Stop all running actions across all branches (best-effort).
pub fn stop_all_actions(
    executor: &ActionExecutor,
    registry: &ActionRegistry,
    stop_options: StopOptions,
) -> Vec<String> {
    let mut stopped_execution_ids = Vec::new();

    for info in registry.get_all_running() {
        if executor.is_running(&info.execution_id) {
            if let Err(e) = executor.stop_with_options(&info.execution_id, stop_options) {
                log::warn!("Failed to stop action {}: {e}", info.execution_id);
            } else {
                stopped_execution_ids.push(info.execution_id);
            }
        }
    }

    stopped_execution_ids
}

/// Get all currently running actions for a branch
#[tauri::command]
pub fn get_running_branch_actions(
    branch_id: String,
    executor: State<'_, Arc<ActionExecutor>>,
    registry: State<'_, Arc<ActionRegistry>>,
) -> Result<Vec<RunningActionInfo>, String> {
    // Get running actions from registry for this branch
    let running_actions = registry.get_running_for_branch(&branch_id);

    // Filter to only actions that are still actually running in the executor
    let executor_ids: std::collections::HashSet<String> =
        executor.get_running_ids().into_iter().collect();

    let active_actions: Vec<RunningActionInfo> = running_actions
        .into_iter()
        .filter(|info| executor_ids.contains(&info.execution_id))
        .collect();

    Ok(active_actions)
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
    registry: State<'_, Arc<ActionRegistry>>,
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
        log::info!(
            "[run_prerun_actions] detecting actions for repo {} (subpath: {:?}), context_id={}",
            github_repo,
            subpath,
            context.id
        );
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

        let detected = match detect_actions_for_repo_context(&github_repo, subpath.as_deref()).await
        {
            Ok(actions) => {
                log::info!(
                    "[run_prerun_actions] action detection returned {} actions for repo {} (subpath: {:?})",
                    actions.len(),
                    github_repo,
                    subpath
                );
                for a in &actions {
                    log::info!(
                        "[run_prerun_actions]   detected action: name={:?} command={:?} type={:?}",
                        a.name,
                        a.command,
                        a.action_type
                    );
                }
                actions
            }
            Err(e) => {
                log::warn!(
                    "[run_prerun_actions] action detection failed for repo {} (subpath: {:?}): {e}",
                    github_repo,
                    subpath
                );
                Vec::new()
            }
        };

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
    } else {
        log::info!(
            "[run_prerun_actions] actions already detected for context_id={}, skipping detection",
            context.id
        );
    }

    // Get all actions for this context.
    let actions = store
        .list_repo_actions(&context.id)
        .map_err(|e| format!("Failed to list actions: {e}"))?;

    log::info!(
        "[run_prerun_actions] found {} total actions for context {} (repo: {}, subpath: {:?})",
        actions.len(),
        context.id,
        github_repo,
        subpath
    );
    for a in &actions {
        log::info!(
            "[run_prerun_actions]   action: id={} name={:?} command={:?} type={:?}",
            a.id,
            a.name,
            a.command,
            a.action_type
        );
    }

    // Filter to prerun actions
    let prerun_actions = actions
        .into_iter()
        .filter(|a| matches!(a.action_type, builderbot_actions::ActionType::Prerun))
        .collect::<Vec<_>>();

    log::info!(
        "[run_prerun_actions] {} prerun actions to execute for branch {}",
        prerun_actions.len(),
        branch_id
    );

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
        "[run_prerun_actions] working_dir for prerun actions: {}",
        working_dir
    );

    // Execute each prerun action sequentially, waiting for each to complete
    // before starting the next one
    let mut execution_ids = Vec::new();
    for action in prerun_actions {
        log::info!(
            "[run_prerun_actions] executing prerun action: id={} name={:?} command={:?}",
            action.id,
            action.name,
            action.command
        );
        let listener = Arc::new(TauriExecutionListener::new(
            app.clone(),
            branch_id.clone(),
            action.id.clone(),
            action.name.clone(),
            action.action_type.as_str().to_string(),
            registry.inner().clone(),
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

// =============================================================================
// Run detection commands
// =============================================================================

/// Get the current run phase for an execution.
#[tauri::command]
pub async fn get_run_phase(
    registry: State<'_, Arc<ActionRegistry>>,
    execution_id: String,
) -> Result<Option<RunPhase>, String> {
    Ok(registry.get_run_phase(&execution_id))
}

/// Update the run detection mode for a repo action.
#[tauri::command]
pub async fn update_run_detection_mode(
    store: State<'_, Mutex<Option<Arc<Store>>>>,
    action_id: String,
    mode: RunDetectionMode,
) -> Result<(), String> {
    let store = get_store(&store)?;
    let mut action = store
        .get_repo_action(&action_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Action not found".to_string())?;
    action.run_detection_mode = Some(mode);
    store
        .update_repo_action(&action)
        .map_err(|e| e.to_string())?;
    Ok(())
}
