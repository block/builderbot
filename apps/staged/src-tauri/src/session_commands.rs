//! Tauri commands for session management.
//!
//! Separated from `lib.rs` to keep session concerns isolated. These are
//! the commands exposed to the frontend via IPC.
//!
//! ## Design note: minimal surface area
//!
//! Only commands the frontend legitimately needs are exposed here:
//! - `start_session` / `resume_session` — kick off agent work
//! - `start_branch_session` — kick off branch-scoped agent work (note/commit)
//! - `cancel_session` / `delete_session` — lifecycle control
//! - `get_session` / `get_session_messages` / `get_session_messages_since` — reads for polling
//!
//! Internal-only operations (creating bare sessions, inserting messages,
//! updating status) are **not** exposed as Tauri commands. They're used
//! only by the backend (`session_runner` / `agent` modules) via the
//! `Store` directly.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::agent::{self, AcpProviderInfo};
use crate::blox;
use crate::git;
use crate::session_runner::{self, SessionConfig};
use crate::store::{self, Store};

// =============================================================================
// Helper — duplicated from lib.rs to avoid circular deps. If this grows,
// consider extracting a shared `state.rs`.
// =============================================================================

fn get_store(store: &tauri::State<'_, Mutex<Option<Arc<Store>>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Database not initialized — please reset from the startup prompt".into())
}

fn resolve_branch_repo_slug(
    store: &Arc<Store>,
    project: &store::Project,
    branch: &store::Branch,
) -> Option<String> {
    if let Some(repo_id) = &branch.project_repo_id {
        if let Ok(Some(repo)) = store.get_project_repo(repo_id) {
            return Some(repo.github_repo);
        }
    }
    project.primary_repo().map(|s| s.to_string())
}

async fn run_blox_blocking<T, F>(op: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, blox::BloxError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(op)
        .await
        .map_err(|e| format!("blox task failed: {e}"))?
        .map_err(|e| e.to_string())
}

// =============================================================================
// Provider discovery
// =============================================================================

/// Scan the system for installed ACP-compatible agents.
///
/// Returns a list of available providers with their IDs and labels.
/// The frontend uses this for the agent setup modal and selector.
///
/// Marked `async` so Tauri runs it on the async runtime instead of the
/// main thread — `find_command` spawns login shells which can take tens
/// of milliseconds per agent.
#[tauri::command]
pub async fn discover_acp_providers() -> Vec<AcpProviderInfo> {
    tokio::task::spawn_blocking(agent::discover_providers)
        .await
        .unwrap_or_default()
}

// =============================================================================
// Read-only queries (used by frontend polling)
// =============================================================================

#[tauri::command]
pub fn get_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    session_id: String,
) -> Result<Option<store::Session>, String> {
    get_store(&store)?
        .get_session(&session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_session_messages(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    session_id: String,
) -> Result<Vec<store::SessionMessage>, String> {
    get_store(&store)?
        .get_session_messages(&session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_session_messages_since(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    session_id: String,
    since_id: i64,
) -> Result<Vec<store::SessionMessage>, String> {
    get_store(&store)?
        .get_session_messages_since(&session_id, since_id)
        .map_err(|e| e.to_string())
}

// =============================================================================
// Lifecycle commands
// =============================================================================

/// Create a session and immediately start the agent.
///
/// The prompt is persisted as the first user message, goose is spawned
/// in the background, and messages stream into the DB in real-time.
/// Returns the Session record (status will be "running").
#[tauri::command]
pub fn start_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    prompt: String,
    working_dir: String,
    provider: Option<String>,
) -> Result<store::Session, String> {
    let store = get_store(&store)?;
    let working_dir = PathBuf::from(working_dir);
    let mut session = store::Session::new_running(&prompt, &working_dir);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    store.create_session(&session).map_err(|e| e.to_string())?;

    session_runner::start_session(
        SessionConfig {
            session_id: session.id.clone(),
            prompt,
            working_dir,
            agent_session_id: None,
            pre_head_sha: None,
            provider,
            workspace_name: None,
            extra_env: vec![],
            mcp_project_id: None,
            action_executor: None,
            action_registry: None,
            remote_working_dir: None,
            image_ids: vec![],
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(session)
}

/// Send a follow-up message to an existing session.
///
/// Sets the session status back to "running", persists the user message,
/// and spawns a new goose subprocess. Uses ACP `load_session` to restore
/// the agent's conversation history from the previous turn(s), so the
/// agent has full context when processing the follow-up.
///
/// The working directory is read from the session record (set when the
/// session was first created), so the frontend doesn't need to pass it.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn resume_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    action_executor: tauri::State<'_, Arc<ActionExecutor>>,
    action_registry: tauri::State<'_, Arc<ActionRegistry>>,
    app_handle: tauri::AppHandle,
    session_id: String,
    prompt: String,
    image_ids: Option<Vec<String>>,
    branch_id: Option<String>,
) -> Result<(), String> {
    let store = get_store(&store)?;

    let session = store
        .get_session(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Session not found: {session_id}"))?;

    // Use the provider that originally created this session so the
    // agent's conversation history can be restored correctly.
    let provider = session.provider.clone();
    let agent_session_id = session.agent_id.clone();
    let working_dir = PathBuf::from(&session.working_dir);

    // Check if this session is linked to a project note — if so, we need
    // to start the MCP server so the agent has access to project tools.
    let mcp_project_id = store
        .get_project_note_by_session(&session_id)
        .ok()
        .flatten()
        .map(|note| note.project_id);

    // If a branch_id is provided, look up the branch to get workspace_name
    // and resolve remote_working_dir. This takes priority over the
    // commit-based fallback below.
    let branch_from_id = branch_id
        .as_deref()
        .and_then(|bid| store.get_branch(bid).ok().flatten());

    // If this session is linked to a commit, capture the current HEAD so we
    // can detect new or amended commits when the session completes.
    // This applies both when the commit already has a SHA (amend case) and
    // when it's still pending (no SHA — the previous run didn't produce a
    // commit, so we need to detect if this resumed run does).
    let (pre_head_sha, workspace_name) = {
        // Determine the branch to use: explicit branch_id takes priority,
        // otherwise fall back to the commit-linked branch.
        let branch = if branch_from_id.is_some() {
            branch_from_id.clone()
        } else {
            store
                .get_commit_by_session(&session_id)
                .ok()
                .flatten()
                .and_then(|commit| store.get_branch(&commit.branch_id).ok().flatten())
        };

        if let Some(ref branch) = branch {
            let ws_name = branch.workspace_name.clone();
            let head = if let Some(ref ws) = ws_name {
                let ws = ws.clone();
                run_blox_blocking(move || crate::blox::ws_exec(&ws, &["git", "rev-parse", "HEAD"]))
                    .await
                    .map(|s| s.trim().to_string())
                    .ok()
            } else {
                crate::git::get_head_sha(&working_dir).ok()
            };
            (head, ws_name)
        } else {
            (None, None)
        }
    };

    // For remote branches, resolve the actual workspace path so the remote
    // agent starts in the correct repo directory.
    let remote_working_dir = if let Some(ref branch) = branch_from_id {
        if branch.workspace_name.is_some() {
            let ws_name = branch.workspace_name.as_deref().unwrap().to_string();
            let store_for_resolve = Arc::clone(&store);
            let branch_for_resolve = branch.clone();
            match tauri::async_runtime::spawn_blocking(move || {
                crate::branches::resolve_branch_workspace_subpath(
                    &store_for_resolve,
                    &branch_for_resolve,
                )
                .ok()
                .flatten()
                .and_then(|subpath| {
                    crate::branches::resolve_workspace_repo_path(&ws_name, &subpath).ok()
                })
            })
            .await
            {
                Ok(Some(path)) => Some(PathBuf::from(path)),
                _ => None,
            }
        } else {
            None
        }
    } else {
        None
    };

    let transitioned = store
        .transition_to_running(&session_id)
        .map_err(|e| e.to_string())?;
    if !transitioned {
        return Err("Session is already running".to_string());
    }

    let _ = app_handle.emit(
        "session-status-changed",
        session_runner::SessionStatusEvent {
            session_id: session_id.clone(),
            status: "running".to_string(),
            error_message: None,
            completion_reason: None,
            branch_id: None,
            project_id: mcp_project_id.clone(),
            session_type: None,
            is_auto_review: false,
        },
    );

    session_runner::start_session(
        SessionConfig {
            session_id,
            prompt,
            working_dir,
            agent_session_id,
            pre_head_sha,
            provider,
            workspace_name,
            extra_env: vec![],
            mcp_project_id: mcp_project_id.clone(),
            action_executor: if mcp_project_id.is_some() {
                Some(Arc::clone(&action_executor))
            } else {
                None
            },
            action_registry: if mcp_project_id.is_some() {
                Some(Arc::clone(&action_registry))
            } else {
                None
            },
            remote_working_dir,
            image_ids: image_ids.unwrap_or_default(),
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(())
}

#[tauri::command]
pub fn cancel_session(
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    session_id: String,
) -> Result<(), String> {
    let was_running = registry.cancel(&session_id);
    if !was_running {
        let store = get_store(&store)?;
        if let Ok(Some(session)) = store.get_session(&session_id) {
            if session.status == store::SessionStatus::Running
                || session.status == store::SessionStatus::Queued
            {
                let _ = store.update_session_status(
                    &session_id,
                    store::SessionStatus::Cancelled,
                    None,
                    Some(&store::CompletionReason::Interrupted),
                );
                let _ = app_handle.emit(
                    "session-status-changed",
                    session_runner::SessionStatusEvent {
                        session_id: session_id.clone(),
                        status: "cancelled".to_string(),
                        error_message: None,
                        completion_reason: Some("interrupted".to_string()),
                        branch_id: None,
                        project_id: None,
                        session_type: None,
                        is_auto_review: false,
                    },
                );
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn delete_session(
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    session_id: String,
) -> Result<(), String> {
    registry.cancel(&session_id);

    get_store(&store)?
        .delete_session(&session_id)
        .map_err(|e| e.to_string())
}

// =============================================================================
// Branch-scoped sessions (note / commit)
// =============================================================================

/// The type of branch session to start.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BranchSessionType {
    Note,
    Commit,
    Review,
}

/// Response from starting a branch session.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSessionResponse {
    pub session_id: String,
    /// The ID of the artifact created (commit or note).
    pub artifact_id: String,
}

/// Response from starting a project session.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSessionResponse {
    pub session_id: String,
    /// The ID of the project note created for this session.
    pub note_id: String,
}

/// Start a project-level session.
///
/// Project sessions operate at the project level rather than a specific branch.
/// The agent receives project context (all repos, existing project notes).
/// Sessions receive an MCP server with tools to start repo subagent sessions
/// and add repos.
/// Always creates a ProjectNote stub that is populated when the session completes.
#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)]
pub async fn start_project_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    action_executor: tauri::State<'_, Arc<ActionExecutor>>,
    action_registry: tauri::State<'_, Arc<ActionRegistry>>,
    app_handle: tauri::AppHandle,
    project_id: String,
    prompt: String,
    provider: Option<String>,
    image_ids: Option<Vec<String>>,
) -> Result<ProjectSessionResponse, String> {
    let store = get_store(&store)?;

    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;

    // Build project context for the prompt
    let project_context = build_project_session_context(&store, &project, None);

    // Build the full prompt.
    // The project-session prompt shares most of its structure between local and
    // remote projects. Only the preamble, start_repo_session description, and an
    // optional coordinator reminder differ.
    let is_remote = project.location == store::ProjectLocation::Remote;

    let preamble = if is_remote {
        "This top-level project session runs locally and acts as a coordinator. \
For repository-specific execution, use MCP subagent tools.\n\n\
This is a remote-workspace project. Use the project MCP tools to orchestrate work:"
    } else {
        "You have access to the following tools:"
    };

    let start_repo_session_desc = if is_remote {
        "- start_repo_session: Use this to make changes or run tasks in one of the project's \
repositories. Use `expected_outcome=\"note_in_repo\"` for repo notes and \
`expected_outcome=\"commit\"` for code changes/commits. For remote branches this subagent \
runs on the remote workspace, where file access, notes, and commits must happen."
    } else {
        "- start_repo_session: Use this to make changes or run tasks in one of the project's \
repositories. Use `expected_outcome=\"note_in_repo\"` for repo notes and \
`expected_outcome=\"commit\"` for code changes/commits. Do not ask for both a note and a \
commit in a single start_repo_session request — choose one outcome per call. All reasoning \
specific to a repo must be done within a repo session rather than in this project-wide context. \
You MUST NOT write files directly — all file writes MUST go through start_repo_session with expected_outcome=\"commit\"."
    };

    let coordinator_reminder = if is_remote {
        "\n\nKeep this project session focused on coordination and synthesis. Do not perform \
repository edits directly here; use `start_repo_session` for implementation work."
    } else {
        ""
    };

    let action_instructions = format!(
        "The user is requesting work at the project level. Investigate and \
fulfill the request below, then produce a project note summarizing what you found and any \
actions taken.\n\n\
{preamble}\n\n\
{start_repo_session_desc}\n\n\
- add_project_repo: Use this when the task requires a repository that isn't yet in the \
project. Pass the GitHub repo slug to add it.\n\n\
IMPORTANT: `add_project_repo` and `start_repo_session` are MCP tools, not shell commands. \
Do not run `which`/`type` for these names and do not ask the user to add repos manually \
unless the MCP tool call itself returns an error. If the tool call fails, report the exact \
error and the next action needed.\
{coordinator_reminder}\n\n\
To discover repositories that might be relevant, use `gh` to explore repos in the user's \
GitHub organizations. Only add repos from organizations the user already belongs to.\n\n\
To return the note, include a horizontal rule (---) followed by the note content. \
Begin the note with a markdown H1 heading as the title.\n\n"
    );

    let full_prompt = format!(
        "<action>\n{action_instructions}\n\nProject information:\n{project_context}\n</action>\n\n{prompt}"
    );

    // Resolve working directory — use the project-scoped worktree root (created
    // at project creation time), NOT the repo clone path (~/.staged/repos/…).
    // Project sessions must never have a repos-dir working directory because the
    // agent would see it and start reading/writing files there directly instead
    // of using start_repo_session.
    let working_dir = crate::git::project_worktree_root_for(&project.id)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));

    // Create the session
    let mut session = store::Session::new_running(&full_prompt, &working_dir);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    store.create_session(&session).map_err(|e| e.to_string())?;

    // Always create a project note stub with empty title and content so that the
    // frontend can detect it as "generating" via the !title && !content check.
    let note = store::ProjectNote::new(&project_id, "", "").with_session(&session.id);
    store
        .create_project_note(&note)
        .map_err(|e| e.to_string())?;
    let note_id = note.id.clone();

    session_runner::start_session(
        SessionConfig {
            session_id: session.id.clone(),
            prompt: full_prompt,
            working_dir,
            agent_session_id: None,
            pre_head_sha: None,
            provider,
            workspace_name: None,
            extra_env: vec![],
            mcp_project_id: Some(project_id.clone()),
            action_executor: Some(Arc::clone(&action_executor)),
            action_registry: Some(Arc::clone(&action_registry)),
            remote_working_dir: None,
            image_ids: image_ids.unwrap_or_default(),
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(ProjectSessionResponse {
        session_id: session.id,
        note_id,
    })
}

/// Start a branch-scoped session (note or commit).
///
/// This builds the full prompt (action tag + branch history + user prompt),
/// creates the artifact stub, and kicks off the agent in the branch's workdir.
///
/// For remote branches (those with a `workspace_name`), the session runs via
/// `blox acp` instead of a local agent binary. Branch context and commit
/// detection are skipped since there is no local worktree.
#[allow(clippy::too_many_arguments)]
#[tauri::command(rename_all = "camelCase")]
pub async fn start_branch_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    prompt: String,
    session_type: BranchSessionType,
    provider: Option<String>,
    image_ids: Option<Vec<String>>,
) -> Result<BranchSessionResponse, String> {
    let store = get_store(&store)?;

    // Resolve branch → project
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let is_remote = branch.workspace_name.is_some();

    // Resolve working directory and branch context.
    // Remote branches use ws_exec for git operations; local branches use the worktree directly.
    let (working_dir, branch_context) = if is_remote {
        // For remote branches, use the derived clone path as a fallback working dir.
        // The actual work happens via ws_exec, not local filesystem.
        let fallback_dir = resolve_branch_repo_slug(&store, &project, &branch)
            .and_then(|repo| crate::paths::repos_dir().map(|d| d.join(repo)))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
        let base_branch = branch.base_branch.clone();
        let store_for_context = Arc::clone(&store);
        let branch_id_for_context = branch_id.clone();
        let project_id_for_context = branch.project_id.clone();
        let ctx = tauri::async_runtime::spawn_blocking(move || {
            build_remote_branch_context(
                &workspace_name,
                &base_branch,
                &store_for_context,
                &branch_id_for_context,
                &project_id_for_context,
            )
        })
        .await
        .map_err(|e| format!("Failed to build remote branch context: {e}"))?;
        (fallback_dir, ctx)
    } else {
        let workdir = store
            .get_workdir_for_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

        let mut worktree_path = PathBuf::from(&workdir.path);
        // Use the project_repo's subpath when the branch is attached to a specific
        // repo (e.g. a secondary repo with no subpath), rather than always falling
        // back to the project-level subpath which may belong to a different repo.
        let effective_subpath = if let Some(repo_id) = branch.project_repo_id.as_deref() {
            store
                .get_project_repo(repo_id)
                .ok()
                .flatten()
                .and_then(|repo| repo.subpath)
        } else {
            project.subpath.clone()
        };
        if let Some(ref subpath) = effective_subpath {
            worktree_path = worktree_path.join(subpath);
        }

        let ctx = build_branch_context(
            &worktree_path,
            &branch.base_branch,
            &store,
            &branch_id,
            &branch.project_id,
        );
        (worktree_path, ctx)
    };

    // Build the full prompt with action instructions + project information + branch context.
    let project_information = build_project_context(&store, &project, &branch);
    let full_prompt = build_full_prompt(
        &prompt,
        &project_information,
        &branch_context,
        &session_type,
    );

    // Create the session
    let mut session = store::Session::new_running(&full_prompt, &working_dir);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    store.create_session(&session).map_err(|e| e.to_string())?;

    // Create artifact stub and compute pre-head SHA
    let (artifact_id, pre_head_sha) = match session_type {
        BranchSessionType::Note => {
            let note = store::Note::new(&branch_id, &prompt, "").with_session(&session.id);
            store.create_note(&note).map_err(|e| e.to_string())?;
            (note.id, None)
        }
        BranchSessionType::Commit => {
            let commit = store::Commit::new_pending(&branch_id).with_session(&session.id);
            store.create_commit(&commit).map_err(|e| e.to_string())?;
            // For remote branches, get HEAD via ws_exec; for local, use git directly.
            let head_sha = if is_remote {
                let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
                match run_blox_blocking(move || {
                    blox::ws_exec(&workspace_name, &["git", "rev-parse", "HEAD"])
                })
                .await
                {
                    Ok(sha) => Some(sha.trim().to_string()),
                    Err(e) => {
                        log::warn!("Failed to get remote HEAD SHA via ws_exec: {e}");
                        None
                    }
                }
            } else {
                Some(
                    git::get_head_sha(&working_dir)
                        .map_err(|e| format!("Failed to get HEAD SHA: {e}"))?,
                )
            };
            (commit.id, head_sha)
        }
        BranchSessionType::Review => {
            // Get the current tip SHA for the review anchor
            let tip_sha = if is_remote {
                let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
                run_blox_blocking(move || {
                    blox::ws_exec(&workspace_name, &["git", "rev-parse", "HEAD"])
                })
                .await
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
            } else {
                git::get_head_sha(&working_dir)
                    .map_err(|e| format!("Failed to get HEAD SHA: {e}"))?
            };

            let review = store::Review::new(&branch_id, &tip_sha, store::ReviewScope::Branch)
                .with_session(&session.id);
            store.create_review(&review).map_err(|e| e.to_string())?;
            (review.id, None)
        }
    };

    // For remote branches, use the user's UI selection.
    let effective_provider = provider;

    // Resolve the actual workspace path for remote branches so the remote agent
    // starts in the correct repo directory (not the workspace default).
    let remote_working_dir = if is_remote {
        let ws_name = branch.workspace_name.as_deref().unwrap().to_string();
        let store_for_resolve = Arc::clone(&store);
        let branch_for_resolve = branch.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            crate::branches::resolve_branch_workspace_subpath(
                &store_for_resolve,
                &branch_for_resolve,
            )
            .ok()
            .flatten()
            .and_then(|subpath| {
                crate::branches::resolve_workspace_repo_path(&ws_name, &subpath).ok()
            })
        })
        .await
        {
            Ok(Some(path)) => Some(PathBuf::from(path)),
            _ => None,
        }
    } else {
        None
    };

    session_runner::start_session(
        SessionConfig {
            session_id: session.id.clone(),
            prompt: full_prompt,
            working_dir,
            agent_session_id: None,
            pre_head_sha,
            provider: effective_provider,
            workspace_name: branch.workspace_name.clone(),
            extra_env: vec![],
            mcp_project_id: None,
            action_executor: None,
            action_registry: None,
            remote_working_dir,
            image_ids: image_ids.unwrap_or_default(),
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(BranchSessionResponse {
        session_id: session.id,
        artifact_id,
    })
}

// =============================================================================
// Queued session commands
// =============================================================================

/// Queue a branch session for later execution.
///
/// Creates a session with `Queued` status and links it to an artifact stub
/// (commit or note), but does NOT resolve working directory, git context,
/// or spawn an agent. The session will be started later via `drain_queued_sessions`.
#[tauri::command(rename_all = "camelCase")]
pub fn queue_branch_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    prompt: String,
    session_type: BranchSessionType,
    provider: Option<String>,
    image_ids: Option<Vec<String>>,
) -> Result<BranchSessionResponse, String> {
    let store = get_store(&store)?;

    // Validate that the branch exists.
    let _branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    // Create the queued session — prompt is stored raw (not enriched with
    // context) since context will be built when the session is drained.
    let mut session = store::Session::new_queued(&prompt);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    store.create_session(&session).map_err(|e| e.to_string())?;

    // Link any attached images to the queued session so they are available at drain time.
    if let Some(ref ids) = image_ids {
        store
            .set_images_session_id(ids, &session.id)
            .map_err(|e| e.to_string())?;
    }

    // Create the artifact stub, following the same pattern as start_branch_session.
    let artifact_id = match session_type {
        BranchSessionType::Note => {
            let note = store::Note::new(&branch_id, &prompt, "").with_session(&session.id);
            store.create_note(&note).map_err(|e| e.to_string())?;
            note.id
        }
        BranchSessionType::Commit => {
            let commit = store::Commit::new_pending(&branch_id).with_session(&session.id);
            store.create_commit(&commit).map_err(|e| e.to_string())?;
            commit.id
        }
        BranchSessionType::Review => {
            let review = store::Review::new(&branch_id, "", store::ReviewScope::Branch)
                .with_session(&session.id);
            store.create_review(&review).map_err(|e| e.to_string())?;
            review.id
        }
    };

    Ok(BranchSessionResponse {
        session_id: session.id,
        artifact_id,
    })
}

/// Drain the oldest queued session for a branch by starting it.
///
/// Queries all queued sessions for the given branch and starts only the first
/// one (oldest by `created_at`). Returns whether a session was started.
#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)]
pub async fn drain_queued_sessions(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
) -> Result<bool, String> {
    let store = get_store(&store)?;
    drain_queued_sessions_for_branch(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        provider,
    )
    .await
}

/// Start the oldest queued branch session if one exists and the branch is idle.
///
/// This is shared by the Tauri command and backend lifecycle hooks so queue
/// progression remains owned by the backend.
pub async fn drain_queued_sessions_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
) -> Result<bool, String> {
    // Bail out if the branch already has a running session to prevent
    // concurrent sessions on the same branch.
    if store
        .has_running_session_for_branch(&branch_id)
        .unwrap_or(false)
    {
        return Ok(false);
    }

    let queued = store
        .get_queued_sessions_for_branch(&branch_id)
        .map_err(|e| e.to_string())?;

    let session = match queued.into_iter().next() {
        Some(s) => s,
        None => return Ok(false),
    };

    // Look up which artifact type is linked to this session so we know
    // what kind of branch session to start.
    let (session_type, review_id) =
        if let Ok(Some(_commit)) = store.get_commit_by_session(&session.id) {
            (BranchSessionType::Commit, None)
        } else if let Ok(Some(_note)) = store.get_note_by_session(&session.id) {
            (BranchSessionType::Note, None)
        } else if let Ok(Some(review)) = store.get_review_by_session(&session.id) {
            (BranchSessionType::Review, Some(review.id))
        } else {
            return Err(format!(
                "Queued session {} has no linked artifact",
                session.id
            ));
        };

    // Use the original prompt from the queued session.
    let prompt = session.prompt.clone();
    let session_id = session.id.clone();

    // Resolve branch → project (same as start_branch_session).
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let is_remote = branch.workspace_name.is_some();

    // Resolve working directory and branch context.
    let (working_dir, branch_context) = if is_remote {
        let fallback_dir = resolve_branch_repo_slug(&store, &project, &branch)
            .and_then(|repo| crate::paths::repos_dir().map(|d| d.join(repo)))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
        let base_branch = branch.base_branch.clone();
        let store_for_context = Arc::clone(&store);
        let branch_id_for_context = branch_id.clone();
        let project_id_for_context = branch.project_id.clone();
        let ctx = tauri::async_runtime::spawn_blocking(move || {
            build_remote_branch_context(
                &workspace_name,
                &base_branch,
                &store_for_context,
                &branch_id_for_context,
                &project_id_for_context,
            )
        })
        .await
        .map_err(|e| format!("Failed to build remote branch context: {e}"))?;
        (fallback_dir, ctx)
    } else {
        let workdir = store
            .get_workdir_for_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

        let mut worktree_path = PathBuf::from(&workdir.path);
        let effective_subpath = if let Some(repo_id) = branch.project_repo_id.as_deref() {
            store
                .get_project_repo(repo_id)
                .ok()
                .flatten()
                .and_then(|repo| repo.subpath)
        } else {
            project.subpath.clone()
        };
        if let Some(ref subpath) = effective_subpath {
            worktree_path = worktree_path.join(subpath);
        }

        let ctx = build_branch_context(
            &worktree_path,
            &branch.base_branch,
            &store,
            &branch_id,
            &branch.project_id,
        );
        (worktree_path, ctx)
    };

    // Build the full prompt with context.
    let project_information = build_project_context(&store, &project, &branch);
    let full_prompt = build_full_prompt(
        &prompt,
        &project_information,
        &branch_context,
        &session_type,
    );

    // Atomically transition session from queued to running.
    // If another drain call already claimed this session, bail out.
    let transitioned = store
        .transition_to_running(&session_id)
        .map_err(|e| e.to_string())?;
    if !transitioned {
        return Ok(false);
    }

    // Update the session's working_dir and prompt now that we have context.
    store
        .prepare_queued_session(&session_id, &working_dir.to_string_lossy(), &full_prompt)
        .map_err(|e| e.to_string())?;

    // Update the review's commit_sha now that we have the working directory.
    // At queue time, reviews are created with an empty commit_sha since the
    // workspace may not exist yet.
    if let Some(ref review_id) = review_id {
        let tip_sha = if is_remote {
            let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
            run_blox_blocking(move || blox::ws_exec(&workspace_name, &["git", "rev-parse", "HEAD"]))
                .await
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        } else {
            git::get_head_sha(&working_dir).map_err(|e| format!("Failed to get HEAD SHA: {e}"))?
        };
        store
            .update_review_commit_sha(review_id, &tip_sha)
            .map_err(|e| e.to_string())?;
    }

    // Compute pre-head SHA for commit sessions.
    let pre_head_sha = match session_type {
        BranchSessionType::Commit => {
            if is_remote {
                let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
                match run_blox_blocking(move || {
                    blox::ws_exec(&workspace_name, &["git", "rev-parse", "HEAD"])
                })
                .await
                {
                    Ok(sha) => Some(sha.trim().to_string()),
                    Err(e) => {
                        log::warn!("Failed to get remote HEAD SHA via ws_exec: {e}");
                        None
                    }
                }
            } else {
                Some(
                    git::get_head_sha(&working_dir)
                        .map_err(|e| format!("Failed to get HEAD SHA: {e}"))?,
                )
            }
        }
        _ => None,
    };

    // Resolve remote working dir for remote branches.
    let remote_working_dir = if is_remote {
        let ws_name = branch.workspace_name.as_deref().unwrap().to_string();
        let store_for_resolve = Arc::clone(&store);
        let branch_for_resolve = branch.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            crate::branches::resolve_branch_workspace_subpath(
                &store_for_resolve,
                &branch_for_resolve,
            )
            .ok()
            .flatten()
            .and_then(|subpath| {
                crate::branches::resolve_workspace_repo_path(&ws_name, &subpath).ok()
            })
        })
        .await
        {
            Ok(Some(path)) => Some(PathBuf::from(path)),
            _ => None,
        }
    } else {
        None
    };

    // Use the provider from the queued session, falling back to the one passed in.
    let effective_provider = session.provider.or(provider);

    // Retrieve image IDs linked to this session at queue time.
    let image_ids = store
        .get_image_ids_for_session(&session_id)
        .unwrap_or_default();

    let session_type_str = match session_type {
        BranchSessionType::Commit => "commit",
        BranchSessionType::Note => "note",
        BranchSessionType::Review => "review",
    };

    let _ = app_handle.emit(
        "session-status-changed",
        session_runner::SessionStatusEvent {
            session_id: session_id.clone(),
            status: "running".to_string(),
            error_message: None,
            completion_reason: None,
            branch_id: Some(branch_id.clone()),
            project_id: Some(branch.project_id.clone()),
            session_type: Some(session_type_str.to_string()),
            is_auto_review: false,
        },
    );

    session_runner::start_session(
        SessionConfig {
            session_id: session_id.clone(),
            prompt: full_prompt,
            working_dir,
            agent_session_id: None,
            pre_head_sha,
            provider: effective_provider,
            workspace_name: branch.workspace_name.clone(),
            extra_env: vec![],
            mcp_project_id: None,
            action_executor: None,
            action_registry: None,
            remote_working_dir,
            image_ids,
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(true)
}

// =============================================================================
// Auto review commands
// =============================================================================

/// Core logic for starting an automatic review for a branch.
///
/// Creates a review with `is_auto = true`, starts a session, and emits
/// `session-status-changed` with `isAutoReview: true` so the frontend
/// can track it.
///
/// This is called both from the Tauri command and from the session runner
/// when a commit session completes.
pub async fn trigger_auto_review(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
) -> Result<BranchSessionResponse, String> {
    // Resolve branch → project
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let is_remote = branch.workspace_name.is_some();

    // Resolve working directory and branch context.
    let (working_dir, branch_context) = if is_remote {
        let fallback_dir = resolve_branch_repo_slug(&store, &project, &branch)
            .and_then(|repo| crate::paths::repos_dir().map(|d| d.join(repo)))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
        let base_branch = branch.base_branch.clone();
        let store_for_context = Arc::clone(&store);
        let branch_id_for_context = branch_id.clone();
        let project_id_for_context = branch.project_id.clone();
        let ctx = tauri::async_runtime::spawn_blocking(move || {
            build_remote_branch_context(
                &workspace_name,
                &base_branch,
                &store_for_context,
                &branch_id_for_context,
                &project_id_for_context,
            )
        })
        .await
        .map_err(|e| format!("Failed to build remote branch context: {e}"))?;
        (fallback_dir, ctx)
    } else {
        let workdir = store
            .get_workdir_for_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

        let mut worktree_path = PathBuf::from(&workdir.path);
        let effective_subpath = if let Some(repo_id) = branch.project_repo_id.as_deref() {
            store
                .get_project_repo(repo_id)
                .ok()
                .flatten()
                .and_then(|repo| repo.subpath)
        } else {
            project.subpath.clone()
        };
        if let Some(ref subpath) = effective_subpath {
            worktree_path = worktree_path.join(subpath);
        }

        let ctx = build_branch_context(
            &worktree_path,
            &branch.base_branch,
            &store,
            &branch_id,
            &branch.project_id,
        );
        (worktree_path, ctx)
    };

    // Get the current tip SHA for the review anchor
    let tip_sha = if is_remote {
        let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
        run_blox_blocking(move || blox::ws_exec(&workspace_name, &["git", "rev-parse", "HEAD"]))
            .await
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string())
    } else {
        git::get_head_sha(&working_dir).map_err(|e| format!("Failed to get HEAD SHA: {e}"))?
    };

    // Build the full prompt (reuse Review prompt)
    let prompt = "Review the latest changes on this branch.".to_string();
    let project_information = build_project_context(&store, &project, &branch);
    let full_prompt = build_full_prompt(
        &prompt,
        &project_information,
        &branch_context,
        &BranchSessionType::Review,
    );

    // Create the session
    let mut session = store::Session::new_running(&full_prompt, &working_dir);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    store.create_session(&session).map_err(|e| e.to_string())?;

    // Create auto review
    let review = store::Review::new(&branch_id, &tip_sha, store::ReviewScope::Branch)
        .with_session(&session.id)
        .with_auto();
    store.create_review(&review).map_err(|e| e.to_string())?;

    // Emit session-status-changed with isAutoReview: true
    let _ = app_handle.emit(
        "session-status-changed",
        session_runner::SessionStatusEvent {
            session_id: session.id.clone(),
            status: "running".to_string(),
            error_message: None,
            completion_reason: None,
            branch_id: Some(branch_id.clone()),
            project_id: Some(branch.project_id.clone()),
            session_type: Some("review".to_string()),
            is_auto_review: true,
        },
    );

    // Resolve the remote working dir for remote branches
    let remote_working_dir = if is_remote {
        let ws_name = branch.workspace_name.as_deref().unwrap().to_string();
        let store_for_resolve = Arc::clone(&store);
        let branch_for_resolve = branch.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            crate::branches::resolve_branch_workspace_subpath(
                &store_for_resolve,
                &branch_for_resolve,
            )
            .ok()
            .flatten()
            .and_then(|subpath| {
                crate::branches::resolve_workspace_repo_path(&ws_name, &subpath).ok()
            })
        })
        .await
        {
            Ok(Some(path)) => Some(PathBuf::from(path)),
            _ => None,
        }
    } else {
        None
    };

    session_runner::start_session(
        SessionConfig {
            session_id: session.id.clone(),
            prompt: full_prompt,
            working_dir,
            agent_session_id: None,
            pre_head_sha: None,
            provider,
            workspace_name: branch.workspace_name.clone(),
            extra_env: vec![],
            mcp_project_id: None,
            action_executor: None,
            action_registry: None,
            remote_working_dir,
            image_ids: vec![],
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(BranchSessionResponse {
        session_id: session.id,
        artifact_id: review.id,
    })
}

/// Resolve the latest git committer timestamp (in milliseconds) for a
/// branch by querying the actual git log.  This covers commits made
/// outside the app that are absent from the `commits` table.
///
/// Returns `0` when the branch has no worktree, no commits, or when the
/// git query fails — callers fall back to the DB-only comparison in that
/// case.
fn latest_git_commit_ms(store: &Arc<Store>, branch_id: &str) -> i64 {
    let branch = match store.get_branch(branch_id) {
        Ok(Some(b)) => b,
        _ => return 0,
    };
    let workdir = match store.get_workdir_for_branch(branch_id) {
        Ok(Some(w)) => w,
        _ => return 0,
    };
    let worktree_path = std::path::Path::new(&workdir.path);
    if !worktree_path.exists() {
        return 0;
    }
    let commits = match git::get_commits_since_base(worktree_path, &branch.base_branch) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    // CommitInfo.timestamp is in seconds; convert to milliseconds.
    commits.iter().map(|c| c.timestamp).max().unwrap_or(0) * 1000
}

/// Find an auto review created after all commits on a branch.
#[tauri::command(rename_all = "camelCase")]
pub async fn find_fresh_auto_review(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<Option<store::Review>, String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        let git_ts = latest_git_commit_ms(&store, &branch_id);
        store
            .find_fresh_auto_review(&branch_id, git_ts)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Update the `is_auto` flag on a review.
#[tauri::command(rename_all = "camelCase")]
pub fn set_review_auto(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    is_auto: bool,
) -> Result<(), String> {
    get_store(&store)?
        .set_review_auto(&review_id, is_auto)
        .map_err(|e| e.to_string())
}

// =============================================================================
// Prompt construction helpers
// =============================================================================

/// Build the branch history context block for a local branch.
pub(crate) fn build_branch_context(
    worktree: &Path,
    base_branch: &str,
    store: &Arc<Store>,
    branch_id: &str,
    project_id: &str,
) -> String {
    let mut parts = vec![context_preamble()];
    let mut timeline: Vec<TimelineEntry> = Vec::new();
    let mut commit_error = None;

    // Commits from git log
    match git::get_full_commit_log(worktree, base_branch) {
        Ok(log) if !log.trim().is_empty() => {
            timeline.extend(parse_timestamped_log(&log));
        }
        Ok(_) => {}
        Err(e) => {
            log::warn!("Failed to get commit log for branch context: {e}");
            commit_error = Some(format!("(Error retrieving commit log: {e})"));
        }
    }

    // Notes and reviews from DB
    let max_commit_ts = timeline.iter().map(|e| e.timestamp).max();
    timeline.extend(note_timeline_entries(store, branch_id, None));
    timeline.extend(review_timeline_entries(
        store,
        branch_id,
        None,
        max_commit_ts,
    ));
    timeline.extend(image_timeline_entries(store, branch_id));

    // Project-level notes
    timeline.extend(project_note_timeline_entries(store, project_id, None));

    parts.push(render_timeline(timeline, commit_error));
    parts.join("\n\n")
}

/// Build the branch history context block for a remote branch.
///
/// Uses `blox ws_exec` to run git commands inside the remote workspace,
/// and reads notes from the DB (which works regardless of worktree location).
pub(crate) fn build_remote_branch_context(
    workspace_name: &str,
    base_branch: &str,
    store: &Arc<Store>,
    branch_id: &str,
    project_id: &str,
) -> String {
    let mut parts = vec![context_preamble()];
    let mut timeline: Vec<TimelineEntry> = Vec::new();

    // Full commit log via ws_exec.
    // Use merge-base to find the fork point so that only the branch's own
    // commits are included, even after a rebase or when the base ref has
    // moved forward.
    let range = if let Ok(mb_output) =
        blox::ws_exec(workspace_name, &["git", "merge-base", base_branch, "HEAD"])
    {
        let mb = mb_output.trim().to_string();
        format!("{mb}..HEAD")
    } else {
        format!("{base_branch}..HEAD")
    };
    match blox::ws_exec(
        workspace_name,
        &[
            "git",
            "log",
            "--reverse",
            "--format=%x00%ct%x01commit %H%nAuthor: %an%nDate: %ci%n%n%B",
            &range,
        ],
    ) {
        Ok(log) if !log.trim().is_empty() => {
            timeline.extend(parse_timestamped_log(&log));
        }
        Ok(_) => {}
        Err(e) => {
            log::warn!("Failed to get remote commit log via ws_exec: {e}");
        }
    }

    // Notes written to temp files inside the remote workspace via ws_exec
    let max_commit_ts = timeline.iter().map(|e| e.timestamp).max();
    timeline.extend(note_timeline_entries(
        store,
        branch_id,
        Some(workspace_name),
    ));
    timeline.extend(review_timeline_entries(
        store,
        branch_id,
        Some(workspace_name),
        max_commit_ts,
    ));
    timeline.extend(image_timeline_entries(store, branch_id));

    // Project-level notes
    timeline.extend(project_note_timeline_entries(
        store,
        project_id,
        Some(workspace_name),
    ));

    parts.push(render_timeline(timeline, None));
    parts.join("\n\n")
}

/// Shared preamble for branch context blocks.
fn context_preamble() -> String {
    "This branch represents an ongoing conversation across multiple sessions. \
     Be judicious with your context window, but you are responsible for understanding \
     previous changes or note content from the branch history when they relate to the \
     next step."
        .to_string()
}

fn normalize_subpath(subpath: Option<&str>) -> Option<String> {
    subpath
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn format_repo_label(repo_slug: &str, subpath: Option<&str>) -> String {
    if let Some(subpath) = normalize_subpath(subpath) {
        format!("{repo_slug} (subpath: {subpath})")
    } else {
        repo_slug.to_string()
    }
}

pub(crate) fn build_project_context(
    store: &Arc<Store>,
    project: &store::Project,
    branch: &store::Branch,
) -> String {
    let repos = match store.list_project_repos(&project.id) {
        Ok(repos) => repos,
        Err(e) => {
            log::warn!("Failed to list project repos for prompt context: {e}");
            Vec::new()
        }
    };

    let current_repo_key = if let Some(repo_id) = branch.project_repo_id.as_deref() {
        repos
            .iter()
            .find(|repo| repo.id == repo_id)
            .map(|repo| {
                (
                    repo.github_repo.clone(),
                    normalize_subpath(repo.subpath.as_deref()),
                )
            })
            .or_else(|| {
                store
                    .get_project_repo(repo_id)
                    .ok()
                    .flatten()
                    .map(|repo| (repo.github_repo, normalize_subpath(repo.subpath.as_deref())))
            })
    } else {
        resolve_branch_repo_slug(store, project, branch)
            .map(|repo| (repo, normalize_subpath(project.subpath.as_deref())))
    };

    let current_repo_label = current_repo_key
        .as_ref()
        .map(|(repo, subpath)| format_repo_label(repo, subpath.as_deref()));

    let related_repo_labels = repos
        .iter()
        .filter_map(|repo| {
            let repo_key = (
                repo.github_repo.as_str(),
                normalize_subpath(repo.subpath.as_deref()),
            );
            if let Some((current_repo, current_subpath)) = &current_repo_key {
                if repo_key.0 == current_repo && repo_key.1 == *current_subpath {
                    return None;
                }
            }
            Some(format_repo_label(
                &repo.github_repo,
                repo.subpath.as_deref(),
            ))
        })
        .collect::<Vec<_>>();

    let project_name = project.name.trim();
    let project_name = if project_name.is_empty() {
        "Unnamed Project"
    } else {
        project_name
    };

    let mut lines = vec![format!("You are working in project \"{project_name}\".")];

    if let Some(current_repo_label) = current_repo_label {
        lines.push(format!(
            "This branch is attached to repository `{current_repo_label}`."
        ));
    } else {
        lines.push(
            "This branch is attached to a repository in this project (repo metadata unavailable)."
                .to_string(),
        );
    }

    if related_repo_labels.is_empty() {
        lines
            .push("No additional repositories are currently attached to this project.".to_string());
    } else {
        lines.push("Additional repositories in this project:".to_string());
        for repo_label in related_repo_labels {
            lines.push(format!("- `{repo_label}`"));
        }
    }

    lines.push(
        "You may inspect related repositories for context when relevant. Unless the user explicitly asks for cross-repo changes, only modify files and create commits in this branch's repository.".to_string(),
    );
    lines.join("\n")
}

/// Build the context block for a project-level session.
///
/// Includes: project name, all attached repos (with reasons and per-repo
/// branch timelines), and existing project notes.
fn build_project_session_context(
    store: &Arc<Store>,
    project: &store::Project,
    workspace_name: Option<&str>,
) -> String {
    let project_name = project.name.trim();
    let project_name = if project_name.is_empty() {
        "Unnamed Project"
    } else {
        project_name
    };

    let mut lines = vec![format!("You are working in project \"{project_name}\".")];

    // List all repos
    let repos = store.list_project_repos(&project.id).unwrap_or_default();
    if repos.is_empty() {
        if let Some(ref repo) = project.github_repo {
            lines.push(format!("Primary repository: `{repo}`"));
        } else {
            lines.push("No repositories are attached to this project.".to_string());
        }
    } else {
        lines.push("Repositories in this project:".to_string());
        for repo in &repos {
            let label = format_repo_label(&repo.github_repo, repo.subpath.as_deref());
            let primary_tag = if repo.is_primary { " (primary)" } else { "" };
            let reason_tag = repo
                .reason
                .as_deref()
                .map(|r| format!(" — {r}"))
                .unwrap_or_default();
            lines.push(format!("- `{label}`{primary_tag}{reason_tag}"));
        }
    }

    // Per-repo branch timelines — gives the project-level agent the same
    // awareness of branch activity that branch-level agents receive.
    let all_branches = store
        .list_branches_for_project(&project.id)
        .unwrap_or_default();

    for repo in &repos {
        let repo_branches: Vec<_> = all_branches
            .iter()
            .filter(|b| b.project_repo_id.as_deref() == Some(&repo.id))
            .collect();

        if repo_branches.is_empty() {
            continue;
        }

        let repo_label = format_repo_label(&repo.github_repo, repo.subpath.as_deref());
        lines.push(String::new());
        lines.push(format!("## Repository: {repo_label}"));

        for branch in &repo_branches {
            lines.push(String::new());
            lines.push(format!("### Branch: {}", branch.branch_name));

            let timeline =
                build_branch_timeline_summary(store, branch, branch.workspace_name.as_deref());
            if timeline.is_empty() {
                lines.push("No activity on this branch yet.".to_string());
            } else {
                lines.push(timeline);
            }
        }
    }

    // Also include branches not associated with any repo (legacy or unlinked)
    let unlinked_branches: Vec<_> = all_branches
        .iter()
        .filter(|b| b.project_repo_id.is_none())
        .collect();
    if !unlinked_branches.is_empty() {
        lines.push(String::new());
        lines.push("## Branches (no repo association)".to_string());
        for branch in &unlinked_branches {
            lines.push(String::new());
            lines.push(format!("### Branch: {}", branch.branch_name));

            let timeline =
                build_branch_timeline_summary(store, branch, branch.workspace_name.as_deref());
            if timeline.is_empty() {
                lines.push("No activity on this branch yet.".to_string());
            } else {
                lines.push(timeline);
            }
        }
    }

    // Include existing project notes
    let notes = store.list_project_notes(&project.id).unwrap_or_default();
    let non_empty_notes: Vec<_> = notes.iter().filter(|n| !n.content.is_empty()).collect();
    if !non_empty_notes.is_empty() {
        lines.push(String::new());
        lines.push("## Existing Project Notes".to_string());
        for note in &non_empty_notes {
            let formatted = format_project_note_for_context(
                &note.id,
                &note.title,
                &note.content,
                workspace_name,
            );
            lines.push(formatted);
        }
    }

    lines.join("\n")
}

/// Build a compact timeline summary for a single branch, suitable for
/// inclusion in project-level context.
///
/// Includes commit log (when a local worktree is available), notes, and
/// reviews — but omits project-level notes (those are rendered separately
/// at the project level to avoid duplication).
fn build_branch_timeline_summary(
    store: &Arc<Store>,
    branch: &store::Branch,
    workspace_name: Option<&str>,
) -> String {
    let mut timeline: Vec<TimelineEntry> = Vec::new();
    let mut commit_error = None;

    // Attempt to include commit log if we can resolve a local worktree
    if let Ok(Some(workdir)) = store.get_workdir_for_branch(&branch.id) {
        let worktree = std::path::Path::new(&workdir.path);
        if worktree.exists() {
            match git::get_full_commit_log(worktree, &branch.base_branch) {
                Ok(log) if !log.trim().is_empty() => {
                    timeline.extend(parse_timestamped_log(&log));
                }
                Ok(_) => {}
                Err(e) => {
                    log::warn!(
                        "Failed to get commit log for branch {} in project context: {e}",
                        branch.branch_name
                    );
                    commit_error = Some(format!("(Error retrieving commit log: {e})"));
                }
            }
        }
    }

    // Notes are written to temp files in the matching execution environment:
    // remote workspace when available, otherwise local temp files.
    let max_commit_ts = timeline.iter().map(|e| e.timestamp).max();
    timeline.extend(note_timeline_entries(store, &branch.id, workspace_name));
    timeline.extend(review_timeline_entries(
        store,
        &branch.id,
        workspace_name,
        max_commit_ts,
    ));
    timeline.extend(image_timeline_entries(store, &branch.id));

    if timeline.is_empty() {
        if let Some(err) = commit_error {
            return err;
        }
        return String::new();
    }

    timeline.sort_by_key(|e| (e.timestamp, e.order));

    let mut section = String::new();
    if let Some(err) = commit_error {
        section.push_str(&err);
        section.push('\n');
    }
    for entry in &timeline {
        section.push_str(&entry.content);
        section.push('\n');
    }
    section.trim_end().to_string()
}

// =============================================================================
// Chronological timeline helpers
// =============================================================================

/// A single entry in the branch timeline, sorted by timestamp (Unix seconds).
struct TimelineEntry {
    timestamp: i64,
    /// Position in git's topological order (0 = oldest). Used as a tiebreaker
    /// when multiple commits share the same second-level timestamp.
    order: i64,
    content: String,
}

/// Sort timeline entries and render them into a single section.
fn render_timeline(mut timeline: Vec<TimelineEntry>, error: Option<String>) -> String {
    if timeline.is_empty() {
        let mut s = String::from("## Branch History\n\n");
        if let Some(err) = error {
            s.push_str(&err);
        } else {
            s.push_str("No activity on this branch yet.");
        }
        return s;
    }

    timeline.sort_by_key(|e| (e.timestamp, e.order));

    let mut section = String::from("## Branch History (oldest first)\n");
    if let Some(err) = error {
        section.push_str(&format!("\n{err}\n"));
    }
    for entry in &timeline {
        section.push('\n');
        section.push_str(&entry.content);
        section.push('\n');
    }
    section
}

/// Parse a timestamped git log into timeline entries.
///
/// Expects the format produced by `--format=%x00%ct%x01commit %H…`:
/// `\0<unix_ts>\x01<display_text>` per commit.
fn parse_timestamped_log(output: &str) -> Vec<TimelineEntry> {
    let mut entries = Vec::new();
    // The log is produced with --reverse (oldest-first), so index 0 = oldest.
    let mut order: i64 = 0;
    for record in output.split('\0') {
        let record = record.trim();
        if record.is_empty() {
            continue;
        }
        if let Some((ts_str, display)) = record.split_once('\x01') {
            if let Ok(ts) = ts_str.trim().parse::<i64>() {
                entries.push(TimelineEntry {
                    timestamp: ts,
                    order,
                    content: display.trim().to_string(),
                });
                order += 1;
            }
        }
    }
    entries
}

/// Write note content to a temp file inside a remote workspace via `ws_exec`.
///
/// Uses base64 encoding to avoid shell-escaping issues with arbitrary markdown.
/// Returns the remote path on success, or an error string on failure.
fn write_note_to_remote(
    workspace_name: &str,
    note_id: &str,
    content: &str,
    prefix: &str,
) -> Result<String, String> {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(content);
    let remote_path = format!("/tmp/{prefix}-{note_id}.md");

    // For very large notes, chunk the base64 to stay under ARG_MAX (~1MB on macOS).
    // Each chunk is ~500KB of base64 (~375KB decoded), well within limits.
    const CHUNK_SIZE: usize = 500_000;

    if encoded.len() <= CHUNK_SIZE {
        blox::ws_exec(
            workspace_name,
            &[
                "sh",
                "-c",
                &format!("echo '{}' | base64 -d > '{}'", encoded, remote_path),
            ],
        )
        .map_err(|e| format!("Failed to write note to remote workspace: {e}"))?;
    } else {
        // Chunk: first chunk overwrites, subsequent chunks append
        for (i, chunk) in encoded.as_bytes().chunks(CHUNK_SIZE).enumerate() {
            let chunk_str = std::str::from_utf8(chunk)
                .map_err(|e| format!("Invalid UTF-8 in base64 chunk: {e}"))?;
            let redirect = if i == 0 { ">" } else { ">>" };
            blox::ws_exec(
                workspace_name,
                &[
                    "sh",
                    "-c",
                    &format!(
                        "echo '{}' | base64 -d {} '{}'",
                        chunk_str, redirect, remote_path
                    ),
                ],
            )
            .map_err(|e| format!("Failed to write note chunk {i} to remote workspace: {e}"))?;
        }
    }

    Ok(remote_path)
}

/// Format a note's content for inclusion in an agent context string.
///
/// When `workspace_name` is `Some`, the note is written to a temp file inside
/// the remote workspace via `ws_exec` and referenced by path. When `None`,
/// the note is written to a local temp file. Both produce the same
/// `See: <path>` output format, keeping large notes out of the prompt.
///
/// Falls back to inlining if the write fails (remote or local).
fn format_note_with_heading(
    id: &str,
    title: &str,
    content: &str,
    workspace_name: Option<&str>,
    heading: &str,
) -> String {
    write_content_to_temp_file(id, content, workspace_name, "staged-note", |path| {
        format!("### {heading}: {title}\n\nSee: `{path}`")
    })
    .unwrap_or_else(|| format!("### {heading}: {title}\n\n{content}"))
}

/// Write content to a temp file (local or remote) and return formatted output via `fmt_ok`.
///
/// Returns `None` if the write fails (caller should fall back to inlining).
fn write_content_to_temp_file(
    id: &str,
    content: &str,
    workspace_name: Option<&str>,
    prefix: &str,
    fmt_ok: impl FnOnce(&str) -> String,
) -> Option<String> {
    if let Some(ws_name) = workspace_name {
        match write_note_to_remote(ws_name, id, content, prefix) {
            Ok(remote_path) => Some(fmt_ok(&remote_path)),
            Err(e) => {
                log::warn!("Failed to write to remote workspace, inlining: {e}");
                None
            }
        }
    } else {
        let path = std::env::temp_dir().join(format!("{prefix}-{id}.md"));
        match std::fs::write(&path, content) {
            Ok(()) => Some(fmt_ok(&path.display().to_string())),
            Err(e) => {
                log::warn!("Failed to write to temp file, inlining: {e}");
                None
            }
        }
    }
}

/// Format a single note's content for inclusion in an agent context string.
///
/// Wrapper around `format_note_with_heading` that uses "Note" as the heading
/// and returns `Option<String>` for backward compatibility with timeline filtering.
pub(crate) fn format_note_for_context(
    id: &str,
    title: &str,
    content: &str,
    workspace_name: Option<&str>,
) -> Option<String> {
    Some(format_note_with_heading(
        id,
        title,
        content,
        workspace_name,
        "Note",
    ))
}

/// Convert notes from the DB into timeline entries.
///
/// When `workspace_name` is `Some`, notes are written to temp files inside the
/// remote workspace via `ws_exec`. When `None`, notes are written to local temp
/// files. Both produce path references to keep large notes out of the prompt.
fn note_timeline_entries(
    store: &Arc<Store>,
    branch_id: &str,
    workspace_name: Option<&str>,
) -> Vec<TimelineEntry> {
    let notes = match store.list_notes_for_branch(branch_id) {
        Ok(n) => n,
        Err(e) => {
            log::warn!("Failed to list notes for branch context: {e}");
            return Vec::new();
        }
    };

    let mut entries = Vec::new();
    for note in &notes {
        if note.content.is_empty() {
            continue; // skip notes still generating
        }
        if let Some(content) =
            format_note_for_context(&note.id, &note.title, &note.content, workspace_name)
        {
            entries.push(TimelineEntry {
                timestamp: note.created_at / 1000,
                order: 0,
                content,
            });
        }
    }
    entries
}

/// Format a single project note for inclusion in context.
///
/// Wrapper around `format_note_with_heading` that uses "Project Note" as the heading.
fn format_project_note_for_context(
    id: &str,
    title: &str,
    content: &str,
    workspace_name: Option<&str>,
) -> String {
    format_note_with_heading(id, title, content, workspace_name, "Project Note")
}

/// Convert project notes from the DB into timeline entries.
fn project_note_timeline_entries(
    store: &Arc<Store>,
    project_id: &str,
    workspace_name: Option<&str>,
) -> Vec<TimelineEntry> {
    let notes = match store.list_project_notes(project_id) {
        Ok(n) => n,
        Err(e) => {
            log::warn!("Failed to list project notes for branch context: {e}");
            return Vec::new();
        }
    };

    let mut entries = Vec::new();
    for note in &notes {
        if note.content.is_empty() {
            continue; // skip notes still generating
        }
        let content =
            format_project_note_for_context(&note.id, &note.title, &note.content, workspace_name);
        entries.push(TimelineEntry {
            timestamp: note.created_at / 1000,
            order: 0,
            content,
        });
    }
    entries
}

/// Build the inline comment content for a review (used both for inlining and temp files).
fn format_review_comments(review: &crate::store::models::Review) -> String {
    let mut content = String::new();

    // Group comments by file path
    let mut by_path: std::collections::BTreeMap<&str, Vec<&crate::store::models::Comment>> =
        std::collections::BTreeMap::new();
    for comment in &review.comments {
        by_path.entry(&comment.path).or_default().push(comment);
    }

    for (path, comments) in &by_path {
        for comment in comments {
            if comment.span.start == comment.span.end {
                content.push_str(&format!(
                    "\n- **{}** (line {}): {}",
                    path, comment.span.start, comment.content,
                ));
            } else {
                content.push_str(&format!(
                    "\n- **{}** (lines {}–{}): {}",
                    path, comment.span.start, comment.span.end, comment.content,
                ));
            }
        }
    }

    content
}

/// Count (total_comments, issues) for a review's comments.
fn review_summary_counts(review: &crate::store::models::Review) -> (usize, usize) {
    let total = review.comments.len();
    let issues = review
        .comments
        .iter()
        .filter(|c| {
            matches!(
                c.comment_type,
                Some(crate::store::models::CommentType::Issue)
            )
        })
        .count();
    (total, issues)
}

/// Convert code reviews (with comments) from the DB into timeline entries.
///
/// When `max_commit_ts` is `Some` and a review predates the latest commit,
/// its comments are written to a temp file (like notes) and only a summary
/// line is included in the timeline. Recent reviews are inlined as before.
fn review_timeline_entries(
    store: &Arc<Store>,
    branch_id: &str,
    workspace_name: Option<&str>,
    max_commit_ts: Option<i64>,
) -> Vec<TimelineEntry> {
    let reviews = match store.list_reviews_for_branch(branch_id) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("Failed to list reviews for branch context: {e}");
            return Vec::new();
        }
    };

    let mut entries = Vec::new();
    for review in &reviews {
        if review.is_auto || review.comments.is_empty() {
            continue;
        }
        let short_sha = &review.commit_sha[..review.commit_sha.len().min(7)];
        let review_ts_secs = review.created_at / 1000;
        let is_old = max_commit_ts.is_some_and(|ts| review_ts_secs < ts);

        let heading_title = match review.title.as_deref() {
            Some(title) => format!("Code review: {} — {}", title, short_sha),
            None => format!("Code review: {}", short_sha),
        };

        let content = if is_old {
            let (total, issues) = review_summary_counts(review);
            let comment_detail = format_review_comments(review);
            let full_content = format!("### {heading_title}\n{comment_detail}");

            let summary_suffix = format!("{total} comments, {issues} issues");

            write_content_to_temp_file(
                &review.id,
                &full_content,
                workspace_name,
                "staged-review",
                |path| format!("### {heading_title} — {summary_suffix}\n\nSee: `{path}`"),
            )
            .unwrap_or_else(|| {
                // Fallback: inline if file write fails
                format!("### {heading_title}\n{comment_detail}")
            })
        } else {
            let comment_detail = format_review_comments(review);
            format!("### {heading_title}\n{comment_detail}")
        };

        entries.push(TimelineEntry {
            timestamp: review_ts_secs,
            order: 0,
            content,
        });
    }
    entries
}

/// Convert images from the DB into timeline entries.
///
/// Each entry is a brief mention so the agent knows what images are attached
/// to this branch without embedding the actual image data.
fn image_timeline_entries(store: &Arc<Store>, branch_id: &str) -> Vec<TimelineEntry> {
    let images = match store.list_images_for_branch(branch_id) {
        Ok(imgs) => imgs,
        Err(e) => {
            log::warn!("Failed to list images for branch context: {e}");
            return Vec::new();
        }
    };

    images
        .iter()
        .map(|img| {
            let size_label = if img.size_bytes > 1_000_000 {
                format!("{:.1} MB", img.size_bytes as f64 / 1_000_000.0)
            } else if img.size_bytes > 1_000 {
                format!("{:.0} KB", img.size_bytes as f64 / 1_000.0)
            } else {
                format!("{} B", img.size_bytes)
            };
            TimelineEntry {
                timestamp: img.created_at / 1000,
                order: 0,
                content: format!(
                    "### Image: {}\n\nAttached image ({}, {}). If this image was included in the current prompt, it will appear as an image content block.",
                    img.filename, img.mime_type, size_label
                ),
            }
        })
        .collect()
}

/// Assemble the full prompt from action instructions + branch context + user prompt.
fn build_full_prompt(
    user_prompt: &str,
    project_information: &str,
    branch_context: &str,
    session_type: &BranchSessionType,
) -> String {
    let action_instructions = match session_type {
        BranchSessionType::Note => {
            "The user is requesting a note. Generate a note based on their prompt below.

You may use any tools needed to research and gather information, but do NOT create \
any commits.

To return the note, your final response must include this exact structure:

---
# <Title>
<Body>

Formatting requirements:
- `---` must be on its own line, with a newline immediately before and after it.
- The note content must start immediately after `---` with a markdown H1 (`# Title`).
- Do not wrap the note in code fences."
        }
        BranchSessionType::Commit => {
            "The user is requesting you make a commit based on the prompt below. Make the necessary \
code changes, following any verification or formatting steps as instructed, and then \
create a commit with a conventional commit message. This commit should describe what \
was requested and how it was fulfilled."
        }
        BranchSessionType::Review => {
            "The user is requesting an AI code review of the current branch.

Review the code changes on this branch by running `git diff $(git merge-base origin/HEAD HEAD)..HEAD` \
(or the appropriate base branch) and provide feedback as structured comments.

Do NOT create any commits or modify any files.

## Review philosophy

Your comments should tell the story of the change — focus on the \"why\", potential issues, \
and non-obvious implications. Do NOT exhaustively document every line or restate what the code \
obviously does. It's fine to have no comments for trivial or self-explanatory files. \
Aim for quality over quantity: a few insightful comments are better than many shallow ones.

## Comment types

Each comment MUST include a `type` field. Choose the type carefully:

- `\"information\"` — Contextual explanation, \"why\" behind a change, or architectural observation. \
Use this for comments that help a reader understand the change but don't require action. \
These are shown as subtle hold-to-reveal annotations, not inline comments. \
Examples: explaining a non-obvious design decision, noting how a change fits into the broader architecture, \
describing what the old code was doing and why it changed.

- `\"suggestion\"` — A recommended improvement that isn't strictly necessary. \
The code works but could be better. \
Examples: a more idiomatic approach, better naming, a simplification.

- `\"warning\"` — A potential issue or concern that deserves attention. \
Not a definite bug, but something that could cause problems. \
Examples: missing edge case handling, potential performance issue, fragile assumption.

- `\"issue\"` — A bug or correctness problem that should be fixed. \
Examples: off-by-one error, null pointer risk, logic error, security vulnerability.

Most comments in a typical review should be `\"information\"` or `\"suggestion\"`. \
Reserve `\"warning\"` and `\"issue\"` for genuine concerns.

## Output format

Your response must start directly with the review-title fenced block below — \
do not output any preamble, commentary, or thinking before it.

Provide a single-sentence title (max 15 words) that conveys your overall \
confidence level in the changes. Do not describe what the changes do — instead focus on \
how confident you are that they are correct and safe. Wrap it in a fenced block:

```review-title
Looks solid overall with one minor edge case worth checking
```

Then return your review comments as exactly one fenced JSON block:

```review-comments
[
  {
    \"path\": \"src/foo.ts\",
    \"span\": { \"start\": 10, \"end\": 15 },
    \"type\": \"information\",
    \"content\": \"This refactors the error handling from panicking to returning Results, which aligns with the broader error-handling migration across the codebase.\"
  },
  {
    \"path\": \"src/bar.rs\",
    \"span\": { \"start\": 42, \"end\": 45 },
    \"type\": \"warning\",
    \"content\": \"This unwrap() could panic if the connection pool is exhausted under load.\"
  }
]
```

Formatting requirements:
- The opening fence line for the title must be exactly: ```review-title
- The opening fence line for comments must be exactly: ```review-comments
- Each closing fence line must be exactly: ```
- Put only plain text (no markdown) inside the review-title block.
- Put only the JSON array inside the review-comments block (no prose or markdown).
- Do not wrap these blocks in any additional code fences.

Rules:
- `span` uses 0-indexed line numbers from the \"after\" side of the diff (exclusive end).
- Only comment on changed files.
- Be specific and actionable — reference the actual code, not generic advice."
        }
    };

    let action_tag = format!(
        "<action>\n{action_instructions}\n\nProject information:\n{project_information}\n</action>"
    );

    format!(
        "{action_tag}\n\n\
         <branch-history>\n\
         {branch_context}\n\
         </branch-history>\n\n\
         {user_prompt}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_prompt_requires_strict_fence_lines() {
        let prompt = build_full_prompt(
            "user prompt",
            "project info",
            "branch context",
            &BranchSessionType::Review,
        );

        assert!(
            prompt.contains("Then return your review comments as exactly one fenced JSON block:")
        );
        assert!(prompt
            .contains("The opening fence line for the title must be exactly: ```review-title"));
        assert!(prompt
            .contains("The opening fence line for comments must be exactly: ```review-comments"));
        assert!(prompt.contains("Each closing fence line must be exactly: ```"));
        assert!(prompt.contains(
            "Put only the JSON array inside the review-comments block (no prose or markdown)."
        ));
        assert!(prompt.contains("do not output any preamble, commentary, or thinking before it"));
    }
}
