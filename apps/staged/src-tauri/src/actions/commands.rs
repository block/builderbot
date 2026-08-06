//! Tauri commands for action execution and detection

use anyhow::Result;
use builderbot_actions::{
    ActionDetector, ActionExecutor, ActionMetadata, ActionType, FileExplorationMode,
    RunDetectionMode, StopOptions, SuggestedAction,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};
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

/// Build an [`AcpAiProvider`] for action detection, honoring the user's
/// preferred agent when `provider_id` is `None`.
///
/// An explicit `provider_id` always wins. When it is `None` — which the
/// automatic first-touch worktree setup and the project-MCP `add_project_repo`
/// path both pass — detection resolves the user's most-recently-used available
/// agent via
/// [`discover_preferred_provider_id`](crate::session_commands::discover_preferred_provider_id),
/// the shared helper behind the badge and action-detection fallbacks, instead
/// of silently picking the first installed agent in `KNOWN_AGENTS` order
/// (Goose).
///
/// Falls back to [`AcpAiProvider::new`] (first installed agent) only when no
/// provider can be resolved at all — i.e. no agents are installed, in which
/// case construction would fail regardless.
pub(crate) async fn build_action_provider(
    provider_id: Option<&str>,
    working_dir: PathBuf,
) -> Result<AcpAiProvider> {
    let provider = match crate::session_commands::discover_preferred_provider_id(provider_id) {
        Some(id) => AcpAiProvider::with_agent(&id, working_dir),
        None => AcpAiProvider::new(working_dir),
    }?;
    let home_snapshot = crate::shell_env::home_env_vars_with_extended_path(
        crate::session_runner::shell_env_cache().as_ref(),
    )
    .await;

    Ok(provider.with_interpreter_env_snapshot(home_snapshot))
}

pub(crate) async fn detect_actions_for_repo_context(
    github_repo: &str,
    subpath: Option<&str>,
    provider_id: Option<&str>,
) -> Result<Vec<SuggestedAction>, String> {
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

    let provider = build_action_provider(provider_id, provider_dir.clone())
        .await
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

/// Persist detected suggestions into an action context, skipping commands the
/// context already has and continuing its sort order.
///
/// Persistence belongs inside the caller's detection window: every surface
/// treats the `detecting: false` half of the `repo-actions-detection` broadcast
/// as "this context's action list is final", so a caller that detects here and
/// persists afterwards — as the repo card's Detect Actions button used to —
/// reopens its own in-progress guard while the writes are still landing, and a
/// second run dedupes against a list the first one hasn't finished writing.
/// Both callers (this module's [`detect_repo_actions_impl`] and the
/// prerun-actions path in [`crate::branches`]) write through here before
/// marking the context detected.
pub(crate) fn persist_suggested_actions(
    store: &Store,
    context_id: &str,
    suggestions: Vec<SuggestedAction>,
) -> Result<(), String> {
    let existing_actions = store
        .list_repo_actions(context_id)
        .map_err(|e| format!("Failed to list actions: {e}"))?;
    let mut existing_commands: std::collections::HashSet<String> =
        existing_actions.iter().map(|a| a.command.clone()).collect();
    let mut next_sort_order = existing_actions
        .iter()
        .map(|a| a.sort_order)
        .max()
        .unwrap_or(-1)
        + 1;

    for suggestion in suggestions {
        if existing_commands.contains(&suggestion.command) {
            continue;
        }
        existing_commands.insert(suggestion.command.clone());
        let action = crate::store::RepoAction::new(
            context_id.to_string(),
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

    Ok(())
}

/// Detect actions for a repo+subpath context, persist the new suggestions, and
/// return the context's resulting action list.
///
/// Detection and persistence both happen while `detecting_actions` is set, so
/// the flag — and the `detecting: false` event that clears it — only drop once
/// the list callers are about to load is complete. That also makes the
/// already-in-progress guard below cover the writes, so a second click while
/// the first run persists is rejected rather than starting another AI call.
pub(crate) async fn detect_repo_actions_impl(
    github_repo: String,
    subpath: Option<String>,
    provider: Option<String>,
    app: AppHandle,
    store: Arc<Store>,
) -> Result<Vec<crate::store::RepoAction>, String> {
    let context = store
        .get_or_create_action_context(&github_repo, subpath.as_deref())
        .map_err(|e| format!("Failed to get action context: {e}"))?;
    if context.detecting_actions {
        return Err("Detection is already in progress for this repository".into());
    }
    store
        .set_action_context_detecting(&context.id, true)
        .map_err(|e| format!("Failed to set detection status: {e}"))?;
    crate::web_server::emit_to_all(
        &app,
        "repo-actions-detection",
        DetectingActionsEvent {
            github_repo: github_repo.clone(),
            subpath: subpath.clone(),
            detecting: true,
        },
    );

    let detected =
        detect_actions_for_repo_context(&github_repo, subpath.as_deref(), provider.as_deref())
            .await;
    if let Err(ref e) = detected {
        log::warn!("Action detection failed for {github_repo}: {e}");
    }

    let result = detected.and_then(|suggestions| {
        persist_suggested_actions(&store, &context.id, suggestions)?;
        store
            .list_repo_actions(&context.id)
            .map_err(|e| format!("Failed to list actions: {e}"))
    });

    // Clear the flag and announce the end of detection even when detection or
    // the persist failed — otherwise every surface keeps spinning on a run
    // that is over. A flag left set while the event says detection is over is
    // worse than either alone: the guard above would reject every later
    // detection for this context and no UI path clears it, so fall back to
    // clearing just the flag before giving up on it.
    if let Err(e) = store.mark_action_context_detected(&context.id) {
        log::error!(
            "Failed to mark action context {} detected after detection: {e}",
            context.id
        );
        if let Err(e) = store.set_action_context_detecting(&context.id, false) {
            log::error!(
                "Failed to clear the detecting flag for action context {}: {e} — \
                 further detection for this repo will be rejected as already in progress",
                context.id
            );
        }
    }
    crate::web_server::emit_to_all(
        &app,
        "repo-actions-detection",
        DetectingActionsEvent {
            github_repo,
            subpath,
            detecting: false,
        },
    );
    result
}

/// Detect available actions for a specific repo+subpath context using AI and
/// persist them; returns the context's actions afterwards.
#[tauri::command(rename_all = "camelCase")]
pub async fn detect_repo_actions(
    github_repo: String,
    subpath: Option<String>,
    provider: Option<String>,
    app: AppHandle,
    store: State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Vec<crate::store::RepoAction>, String> {
    let store = get_store(&store)?;
    detect_repo_actions_impl(github_repo, subpath, provider, app, store).await
}

/// Wire up run detection for a just-started Run-type action execution.
///
/// `scope_id` is the routing id echoed into run-phase events — a branch id
/// for branch runs, or the synthetic id from [`repo_action_scope_id`] for
/// repo runs; the registry and event stream treat it as an opaque string.
/// `working_dir` is the local directory the autodetect poller inspects
/// (empty for remote executions, where detection gracefully degrades).
#[allow(clippy::too_many_arguments)]
fn wire_run_detection(
    app: AppHandle,
    store: Arc<Store>,
    registry: Arc<ActionRegistry>,
    execution_id: String,
    scope_id: String,
    action: &crate::store::RepoAction,
    working_dir: String,
    provider_id: Option<String>,
) {
    // Ensure the output buffer for this execution_id exists so the
    // regex matcher can obtain a reference to it.
    registry.register_output_buffer(&execution_id);

    match action.run_detection_mode.clone() {
        Some(RunDetectionMode::EndpointRegex { pattern }) => {
            let (cancel_tx, cancel_rx) = watch::channel(false);
            registry.store_cancel_sender(&execution_id, cancel_tx);
            run_detector::spawn_regex_matcher(
                app,
                registry,
                execution_id,
                scope_id,
                action.name.clone(),
                pattern,
                true,
                cancel_rx,
            );
        }
        Some(RunDetectionMode::RunningRegex { pattern }) => {
            let (cancel_tx, cancel_rx) = watch::channel(false);
            registry.store_cancel_sender(&execution_id, cancel_tx);
            run_detector::spawn_regex_matcher(
                app,
                registry,
                execution_id,
                scope_id,
                action.name.clone(),
                pattern,
                false,
                cancel_rx,
            );
        }
        Some(RunDetectionMode::NoDetection) => {
            registry.set_run_phase(&execution_id, RunPhase::NoDetection);
            emit_run_phase_changed(
                &app,
                RunPhaseChangedEvent {
                    execution_id,
                    branch_id: scope_id,
                    action_name: action.name.clone(),
                    phase: RunPhase::NoDetection,
                },
            );
        }
        Some(RunDetectionMode::Autodetect) | None => {
            registry.set_run_phase(&execution_id, RunPhase::AutodetectPending);
            emit_run_phase_changed(
                &app,
                RunPhaseChangedEvent {
                    execution_id: execution_id.clone(),
                    branch_id: scope_id.clone(),
                    action_name: action.name.clone(),
                    phase: RunPhase::AutodetectPending,
                },
            );

            let (cancel_tx, cancel_rx) = watch::channel(false);
            registry.store_cancel_sender(&execution_id, cancel_tx);
            run_detector::spawn_autodetect_poller(
                app,
                store,
                registry,
                execution_id,
                scope_id,
                action.id.clone(),
                action.name.clone(),
                action.command.clone(),
                std::path::PathBuf::from(&working_dir),
                provider_id,
                cancel_rx,
            );
        }
    }
}

pub(crate) async fn run_branch_action_impl(
    branch_id: String,
    action_id: String,
    provider_id: Option<String>,
    app: AppHandle,
    store: Arc<Store>,
    executor: Arc<ActionExecutor>,
    registry: Arc<ActionRegistry>,
) -> Result<String, String> {
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

    // Create event listener
    let listener = Arc::new(TauriExecutionListener::new(
        app.clone(),
        branch_id.clone(),
        action_id.clone(),
        action.name.clone(),
        action.action_type.as_str().to_string(),
        Arc::clone(&registry),
    ));

    // Create metadata
    let metadata = ActionMetadata {
        action_id: action.id.clone(),
        action_name: action.name.clone(),
        auto_commit: action.auto_commit,
    };

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
            .execute(action.command.clone(), working_dir, metadata, listener)
            .await
            .map_err(|e| format!("Failed to execute action: {e}"))?;

        (eid, wd)
    };

    // --- Run detection wiring (only for Run actions) ---
    if matches!(action.action_type, ActionType::Run) {
        wire_run_detection(
            app,
            store,
            registry,
            execution_id.clone(),
            branch_id,
            &action,
            working_dir_for_detection,
            provider_id,
        );
    }

    Ok(execution_id)
}

/// Run an action for a branch
#[tauri::command]
pub async fn run_branch_action(
    branch_id: String,
    action_id: String,
    provider: Option<String>,
    app: AppHandle,
    store: State<'_, Mutex<Option<Arc<Store>>>>,
    executor: State<'_, Arc<ActionExecutor>>,
    registry: State<'_, Arc<ActionRegistry>>,
) -> Result<String, String> {
    let store = get_store(&store)?;
    run_branch_action_impl(
        branch_id,
        action_id,
        provider,
        app,
        store,
        executor.inner().clone(),
        registry.inner().clone(),
    )
    .await
}

/// Build the synthetic scope id under which repo-scoped executions are
/// routed: `repo:{github_repo}` or `repo:{github_repo}:{subpath}`.
///
/// The registry, execution events, and running-actions queries all treat
/// the branch id as an opaque routing string, so repo runs reuse them
/// untouched by passing this id where branch runs pass a branch id. The
/// frontend mirrors this format in `repoActionScopeId` (src/lib/commands.ts).
pub(crate) fn repo_action_scope_id(github_repo: &str, subpath: Option<&str>) -> String {
    match subpath.filter(|s| !s.is_empty()) {
        Some(subpath) => format!("repo:{github_repo}:{subpath}"),
        None => format!("repo:{github_repo}"),
    }
}

/// Check that `action` belongs to the (`github_repo`, `subpath`) context the
/// caller claims it does.
///
/// The lookup is read-only on purpose: a context that does not exist cannot own
/// the action, so a missing one is rejected exactly like an unrelated one.
/// Getting-or-creating here would insert an empty context row for the repo on
/// the way to refusing the run — the same reason `list_all_repo_actions` only
/// reads.
fn validate_repo_action_context(
    store: &Store,
    action: &crate::store::RepoAction,
    github_repo: &str,
    subpath: Option<&str>,
) -> Result<(), String> {
    let owns_action = store
        .get_action_context_by_repo_and_subpath(github_repo, subpath)
        .map_err(|e| format!("Failed to get action context: {e}"))?
        .is_some_and(|context| context.id == action.context_id);
    if !owns_action {
        return Err("Action does not belong to this repo/subpath context".to_string());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_repo_action_impl(
    github_repo: String,
    subpath: Option<String>,
    action_id: String,
    provider_id: Option<String>,
    app: AppHandle,
    store: Arc<Store>,
    executor: Arc<ActionExecutor>,
    registry: Arc<ActionRegistry>,
) -> Result<String, String> {
    // Normalize empty subpaths so the context lookup and scope id agree.
    let subpath = subpath.filter(|s| !s.is_empty());

    // Get the action and validate it belongs to this repo+subpath context
    let action = store
        .get_repo_action(&action_id)
        .map_err(|e| format!("Failed to get action: {e}"))?
        .ok_or_else(|| "Action not found".to_string())?;

    validate_repo_action_context(&store, &action, &github_repo, subpath.as_deref())?;

    // Repo runs execute against the repo's main local clone; unlike branch
    // runs there is no worktree or remote-workspace fallback, so the clone
    // must already exist on disk.
    let clone_path = crate::paths::clone_path_for(&github_repo)
        .ok_or_else(|| "Cannot determine clone path (no home directory)".to_string())?;
    if !clone_path.exists() {
        return Err(format!(
            "Repository {github_repo} has not been cloned locally. Clone it before running actions."
        ));
    }

    let working_dir_path = match subpath.as_deref() {
        Some(subpath) => clone_path.join(subpath),
        None => clone_path,
    };
    if !working_dir_path.exists() {
        return Err(format!(
            "Path {} does not exist in the local clone",
            working_dir_path.to_string_lossy()
        ));
    }
    let working_dir = working_dir_path.to_string_lossy().to_string();

    let scope_id = repo_action_scope_id(&github_repo, subpath.as_deref());

    let listener = Arc::new(TauriExecutionListener::new(
        app.clone(),
        scope_id.clone(),
        action.id.clone(),
        action.name.clone(),
        action.action_type.as_str().to_string(),
        Arc::clone(&registry),
    ));

    // Auto-commit is always stripped for repo runs: the executor would
    // commit into the working dir, which here is the user's default-branch
    // checkout rather than a disposable worktree.
    let metadata = ActionMetadata {
        action_id: action.id.clone(),
        action_name: action.name.clone(),
        auto_commit: false,
    };

    let execution_id = executor
        .execute(
            action.command.clone(),
            working_dir.clone(),
            metadata,
            listener,
        )
        .await
        .map_err(|e| format!("Failed to execute action: {e}"))?;

    // --- Run detection wiring (only for Run actions) ---
    if matches!(action.action_type, ActionType::Run) {
        wire_run_detection(
            app,
            store,
            registry,
            execution_id.clone(),
            scope_id,
            &action,
            working_dir,
            provider_id,
        );
    }

    Ok(execution_id)
}

/// Run a repo-scoped action against the repo's local clone.
#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)]
pub async fn run_repo_action(
    github_repo: String,
    subpath: Option<String>,
    action_id: String,
    provider: Option<String>,
    app: AppHandle,
    store: State<'_, Mutex<Option<Arc<Store>>>>,
    executor: State<'_, Arc<ActionExecutor>>,
    registry: State<'_, Arc<ActionRegistry>>,
) -> Result<String, String> {
    let store = get_store(&store)?;
    run_repo_action_impl(
        github_repo,
        subpath,
        action_id,
        provider,
        app,
        store,
        executor.inner().clone(),
        registry.inner().clone(),
    )
    .await
}

pub(crate) fn stop_branch_action_impl(
    execution_id: String,
    executor: &ActionExecutor,
) -> Result<(), String> {
    executor
        .stop(&execution_id)
        .map_err(|e| format!("Failed to stop action: {e}"))
}

/// Stop a running action
#[tauri::command]
pub fn stop_branch_action(
    execution_id: String,
    executor: State<'_, Arc<ActionExecutor>>,
) -> Result<(), String> {
    stop_branch_action_impl(execution_id, &executor)
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

pub(crate) fn get_running_branch_actions_impl(
    branch_id: String,
    executor: &ActionExecutor,
    registry: &ActionRegistry,
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

/// Get all currently running actions for a branch
#[tauri::command]
pub fn get_running_branch_actions(
    branch_id: String,
    executor: State<'_, Arc<ActionExecutor>>,
    registry: State<'_, Arc<ActionRegistry>>,
) -> Result<Vec<RunningActionInfo>, String> {
    get_running_branch_actions_impl(branch_id, &executor, &registry)
}

/// A live execution paired with its current run phase. Carrying the phase
/// inline spares callers a `get_run_phase` round trip per execution.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningActionSnapshot {
    #[serde(flatten)]
    pub info: RunningActionInfo,
    pub phase: Option<RunPhase>,
}

/// Pair each registry entry that is still live in the executor with its run
/// phase. `live_execution_ids` is the executor's liveness set, passed in so the
/// filter and phase join are testable without a real execution.
fn snapshot_running_actions(
    registry: &ActionRegistry,
    live_execution_ids: &std::collections::HashSet<String>,
) -> Vec<RunningActionSnapshot> {
    registry
        .get_all_running()
        .into_iter()
        .filter(|info| live_execution_ids.contains(&info.execution_id))
        .map(|info| RunningActionSnapshot {
            phase: registry.get_run_phase(&info.execution_id),
            info,
        })
        .collect()
}

pub(crate) fn get_all_running_actions_impl(
    executor: &ActionExecutor,
    registry: &ActionRegistry,
) -> Result<Vec<RunningActionSnapshot>, String> {
    let live_execution_ids: std::collections::HashSet<String> =
        executor.get_running_ids().into_iter().collect();
    Ok(snapshot_running_actions(registry, &live_execution_ids))
}

/// Get every currently running action across all scopes, each with its run
/// phase. Cards slice the result by their own scope id, so a surface rendering
/// N of them hydrates from one call instead of one (plus a phase call per
/// execution) each.
#[tauri::command]
pub fn get_all_running_actions(
    executor: State<'_, Arc<ActionExecutor>>,
    registry: State<'_, Arc<ActionRegistry>>,
) -> Result<Vec<RunningActionSnapshot>, String> {
    get_all_running_actions_impl(&executor, &registry)
}

pub(crate) fn get_action_output_buffer_impl(
    execution_id: String,
    executor: &ActionExecutor,
) -> Result<Option<Vec<builderbot_actions::OutputChunk>>, String> {
    Ok(executor.get_buffered_output(&execution_id))
}

/// Get buffered output for an action execution
#[tauri::command]
pub fn get_action_output_buffer(
    execution_id: String,
    executor: State<'_, Arc<ActionExecutor>>,
) -> Result<Option<Vec<builderbot_actions::OutputChunk>>, String> {
    get_action_output_buffer_impl(execution_id, &executor)
}

pub(crate) fn clear_action_execution_impl(
    execution_id: String,
    executor: &ActionExecutor,
) -> Result<bool, String> {
    Ok(executor.clear_execution(&execution_id))
}

/// Clear buffered output for a completed execution
#[tauri::command]
pub fn clear_action_execution(
    execution_id: String,
    executor: State<'_, Arc<ActionExecutor>>,
) -> Result<bool, String> {
    clear_action_execution_impl(execution_id, &executor)
}

pub(crate) async fn run_prerun_actions_impl(
    branch_id: String,
    provider_id: Option<String>,
    app: AppHandle,
    store: Arc<Store>,
    executor: Arc<ActionExecutor>,
    registry: Arc<ActionRegistry>,
) -> Result<Vec<String>, String> {
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
        crate::web_server::emit_to_all(
            &app,
            "repo-actions-detection",
            DetectingActionsEvent {
                github_repo: github_repo.clone(),
                subpath: subpath.clone(),
                detecting: true,
            },
        );

        let detected = match detect_actions_for_repo_context(
            &github_repo,
            subpath.as_deref(),
            provider_id.as_deref(),
        )
        .await
        {
            Ok(actions) => actions,
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
        crate::web_server::emit_to_all(
            &app,
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
            action.action_type.as_str().to_string(),
            Arc::clone(&registry),
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

/// Run all prerun actions for a branch after creation
#[tauri::command]
pub async fn run_prerun_actions(
    branch_id: String,
    provider: Option<String>,
    app: AppHandle,
    store: State<'_, Mutex<Option<Arc<Store>>>>,
    executor: State<'_, Arc<ActionExecutor>>,
    registry: State<'_, Arc<ActionRegistry>>,
) -> Result<Vec<String>, String> {
    let store = get_store(&store)?;
    run_prerun_actions_impl(
        branch_id,
        provider,
        app,
        store,
        executor.inner().clone(),
        registry.inner().clone(),
    )
    .await
}

// =============================================================================
// Run detection commands
// =============================================================================

pub(crate) fn get_run_phase_impl(
    registry: &ActionRegistry,
    execution_id: String,
) -> Result<Option<RunPhase>, String> {
    Ok(registry.get_run_phase(&execution_id))
}

/// Get the current run phase for an execution.
#[tauri::command]
pub async fn get_run_phase(
    registry: State<'_, Arc<ActionRegistry>>,
    execution_id: String,
) -> Result<Option<RunPhase>, String> {
    get_run_phase_impl(&registry, execution_id)
}

pub(crate) fn update_run_detection_mode_impl(
    store: Arc<Store>,
    action_id: String,
    mode: RunDetectionMode,
) -> Result<(), String> {
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

/// Update the run detection mode for a repo action.
#[tauri::command]
pub async fn update_run_detection_mode(
    store: State<'_, Mutex<Option<Arc<Store>>>>,
    action_id: String,
    mode: RunDetectionMode,
) -> Result<(), String> {
    let store = get_store(&store)?;
    update_run_detection_mode_impl(store, action_id, mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn register(registry: &ActionRegistry, execution_id: &str, scope_id: &str, action_type: &str) {
        registry.register(
            execution_id.to_string(),
            scope_id.to_string(),
            format!("action-{execution_id}"),
            format!("Action {execution_id}"),
            action_type.to_string(),
            0,
        );
    }

    #[test]
    fn snapshot_running_actions_drops_dead_executions_and_joins_phases() {
        let registry = ActionRegistry::new();
        let repo_scope = repo_action_scope_id("block/builderbot", Some("apps/staged"));
        register(&registry, "live-run", &repo_scope, "run");
        register(&registry, "live-test", "branch-1", "test");
        register(&registry, "dead", "branch-1", "build");
        registry.set_run_phase(
            "live-run",
            RunPhase::Running {
                endpoint: Some("http://localhost:5173".to_string()),
            },
        );

        let live: HashSet<String> = ["live-run", "live-test"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut snapshots = snapshot_running_actions(&registry, &live);
        snapshots.sort_by(|a, b| a.info.execution_id.cmp(&b.info.execution_id));

        // "dead" is still registered but no longer live in the executor.
        assert_eq!(
            snapshots
                .iter()
                .map(|s| s.info.execution_id.as_str())
                .collect::<Vec<_>>(),
            vec!["live-run", "live-test"]
        );
        assert!(matches!(
            &snapshots[0].phase,
            Some(RunPhase::Running { endpoint: Some(e) }) if e == "http://localhost:5173"
        ));
        assert!(snapshots[1].phase.is_none());
        // Repo- and branch-scoped executions come back together; callers slice
        // the result by their own scope id.
        assert_eq!(snapshots[0].info.branch_id, repo_scope);
        assert_eq!(snapshots[1].info.branch_id, "branch-1");

        // The info fields flatten alongside the phase, so a snapshot reads like
        // a RunningActionInfo with one extra key.
        let json = serde_json::to_value(&snapshots[0]).unwrap();
        assert_eq!(json["executionId"], "live-run");
        assert_eq!(json["actionType"], "run");
        assert_eq!(json["phase"]["type"], "running");
    }

    #[test]
    fn get_all_running_actions_is_empty_when_the_executor_runs_nothing() {
        let registry = ActionRegistry::new();
        register(&registry, "stale", "branch-1", "test");

        let executor = ActionExecutor::new();
        assert!(get_all_running_actions_impl(&executor, &registry)
            .unwrap()
            .is_empty());
    }

    fn suggestion(name: &str, command: &str, action_type: ActionType) -> SuggestedAction {
        SuggestedAction {
            name: name.to_string(),
            command: command.to_string(),
            action_type,
            auto_commit: false,
            source: "justfile".to_string(),
        }
    }

    #[test]
    fn persist_suggested_actions_skips_known_commands_and_continues_sort_order() {
        let store = Store::in_memory().unwrap();
        let context = store
            .get_or_create_action_context("block/builderbot", Some("apps/staged"))
            .unwrap();

        persist_suggested_actions(
            &store,
            &context.id,
            vec![
                suggestion("Dev", "just dev", ActionType::Run),
                suggestion("Test", "just test", ActionType::Test),
            ],
        )
        .unwrap();

        // A re-detection that turns up one known command and one new one only
        // writes the new one, appended after the existing sort orders.
        persist_suggested_actions(
            &store,
            &context.id,
            vec![
                suggestion("Test (renamed)", "just test", ActionType::Test),
                suggestion("Build", "just build", ActionType::Build),
            ],
        )
        .unwrap();

        let actions = store.list_repo_actions(&context.id).unwrap();
        assert_eq!(
            actions
                .iter()
                .map(|a| (a.name.as_str(), a.sort_order))
                .collect::<Vec<_>>(),
            vec![("Dev", 0), ("Test", 1), ("Build", 2)]
        );
    }

    #[test]
    fn validating_a_repo_action_context_never_creates_one() {
        let store = Store::in_memory().unwrap();
        let context = store
            .get_or_create_action_context("block/builderbot", Some("apps/staged"))
            .unwrap();
        let action = crate::store::RepoAction::new(
            context.id.clone(),
            "Dev".to_string(),
            "just dev".to_string(),
            ActionType::Run,
            0,
        );
        store.create_repo_action(&action).unwrap();

        validate_repo_action_context(&store, &action, "block/builderbot", Some("apps/staged"))
            .unwrap();

        // A repo with no context of its own is rejected without minting one,
        // and so is the same repo at a different subpath.
        assert!(validate_repo_action_context(&store, &action, "block/goose", None).is_err());
        assert!(validate_repo_action_context(
            &store,
            &action,
            "block/builderbot",
            Some("apps/other")
        )
        .is_err());
        assert_eq!(
            store.count_action_contexts_for_repo("block/goose").unwrap(),
            0
        );
        assert_eq!(
            store
                .count_action_contexts_for_repo("block/builderbot")
                .unwrap(),
            1
        );
    }

    #[test]
    fn repo_action_scope_id_includes_subpath_when_present() {
        assert_eq!(
            repo_action_scope_id("block/builderbot", Some("apps/staged")),
            "repo:block/builderbot:apps/staged"
        );
        assert_eq!(
            repo_action_scope_id("block/goose", None),
            "repo:block/goose"
        );
        // Empty subpaths normalize to the no-subpath form.
        assert_eq!(
            repo_action_scope_id("block/goose", Some("")),
            "repo:block/goose"
        );
    }
}
