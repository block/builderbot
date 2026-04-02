//! Session orchestration — lifecycle, cancellation, status events.
//!
//! This module owns the *lifecycle* of a session but delegates the actual
//! agent protocol work to an [`AgentDriver`](crate::agent::AgentDriver).
//! It:
//!
//! 1. Persists the user message
//! 2. Registers the session for cancellation
//! 3. Spawns a background thread that runs the driver
//! 4. On completion, atomically transitions the DB status
//! 5. Emits a single `session-status-changed` event
//!
//! The frontend never sees streaming events — it polls the DB.
//!
//! ## Cancellation & deletion
//!
//! [`SessionRegistry`] tracks every running session via a
//! [`CancellationToken`]. Cancelling a session signals the token, which
//! causes the driver to exit early. The driver is responsible for killing
//! its child process (e.g. via `kill_on_drop`).
//!
//! **Race on delete:** When a session is cancelled and then immediately
//! deleted, the background thread may still attempt a handful of DB writes
//! (status update, trailing message flushes). These are safe because:
//!
//! - `session_messages.session_id` has `REFERENCES sessions(id) ON DELETE
//!   CASCADE` with `PRAGMA foreign_keys=ON`, so INSERTs referencing a
//!   deleted session fail with an FK error rather than creating orphan rows.
//! - UPDATEs to already-cascade-deleted message rows are no-ops (0 rows
//!   affected).
//! - `UPDATE sessions SET status = … WHERE id = ?` on a deleted session is
//!   also a no-op.
//! - All write paths in the background thread use `let _ =` or
//!   `log::error!`, so FK failures are swallowed gracefully.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;

use acp_client::{McpServer, McpServerHttp};

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::agent::{AcpDriver, AgentDriver, MessageWriter};
use crate::git::Span;
use crate::store::{
    Comment, CommentAuthor, CommentType, CompletionReason, MessageRole, SessionStatus, Store,
};

// =============================================================================
// Event types
// =============================================================================

/// Emitted when a session's status changes. The only event the frontend needs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusEvent {
    pub session_id: String,
    pub status: String,
    pub error_message: Option<String>,
    pub completion_reason: Option<String>,
    /// Set on `"running"` events emitted when an MCP tool starts a repo session,
    /// so the frontend can register the session and refresh the branch timeline.
    pub branch_id: Option<String>,
    pub project_id: Option<String>,
    pub session_type: Option<String>,
    /// When `true`, the session belongs to an automatically triggered review
    /// (not user-initiated). The frontend uses this to suppress UI for auto reviews.
    #[serde(default)]
    pub is_auto_review: bool,
}

// =============================================================================
// Session registry — tracks running sessions for cancellation
// =============================================================================

/// Tracks all running sessions so they can be cancelled from the outside.
///
/// Managed as Tauri state. The background thread for each session removes
/// itself from the registry when it exits (regardless of outcome).
pub struct SessionRegistry {
    running: std::sync::Mutex<HashMap<String, CancellationToken>>,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            running: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Register a new session and return a `CancellationToken` for it.
    fn register(&self, session_id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        let mut running = self.running.lock().unwrap();
        running.insert(session_id.to_string(), token.clone());
        token
    }

    /// Remove a session from the registry (called by the background thread
    /// on exit, regardless of success/failure/cancellation).
    fn deregister(&self, session_id: &str) {
        let mut running = self.running.lock().unwrap();
        running.remove(session_id);
    }

    /// Cancel a running session. Returns true if the session was found and
    /// signalled, false if it wasn't running (already finished or unknown).
    pub fn cancel(&self, session_id: &str) -> bool {
        if let Some(token) = self.running.lock().unwrap().get(session_id) {
            token.cancel();
            true
        } else {
            false
        }
    }

    /// Returns true if the given session is currently tracked as running.
    pub fn is_running(&self, session_id: &str) -> bool {
        self.running.lock().unwrap().contains_key(session_id)
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Configuration for a session run.
pub struct SessionConfig {
    pub session_id: String,
    pub prompt: String,
    pub working_dir: PathBuf,
    pub agent_session_id: Option<String>,
    /// HEAD SHA before the session starts. When set, post-completion hooks
    /// will check if a new commit was created and update the linked commit
    /// record in the DB.
    pub pre_head_sha: Option<String>,
    /// ACP provider ID (e.g. "goose", "claude"). When `None`, the first
    /// available provider is used.
    pub provider: Option<String>,
    /// When set, the session runs via `blox acp <workspace_name>` instead
    /// of a local agent binary. Commit detection is skipped (no local git).
    pub workspace_name: Option<String>,
    /// Extra environment variables to pass to the agent process.
    pub extra_env: Vec<(String, String)>,
    /// Project ID for MCP tool server (project sessions only).
    /// When set, an MCP server is started and the agent is given access to
    /// `start_repo_session` and `add_project_repo` tools. The MCP server URL
    /// is injected into the ACP session via `NewSessionRequest`.
    pub mcp_project_id: Option<String>,
    /// Action executor for running setup actions in the MCP add_project_repo tool.
    /// Required when `mcp_project_id` is set so the MCP server can run prerun actions.
    pub action_executor: Option<Arc<ActionExecutor>>,
    /// Action registry for tracking running actions in the MCP add_project_repo tool.
    /// Required when `mcp_project_id` is set.
    pub action_registry: Option<Arc<ActionRegistry>>,
    /// Working directory on the remote workspace. When set, this path is sent
    /// to the remote agent in the `NewSessionRequest` instead of `"."`, so
    /// the agent operates in the correct repo directory (e.g.
    /// `/home/bloxer/cash-server` instead of the workspace default).
    pub remote_working_dir: Option<PathBuf>,
    /// Image IDs to include in the prompt. The runner reads the image files,
    /// base64-encodes them, and passes them as content blocks to the driver.
    pub image_ids: Vec<String>,
}

/// Start a session: persist the user message, spawn the agent, stream to DB.
///
/// Returns immediately — the actual agent work happens on a background task.
/// Emits `session-status-changed` when the session reaches a terminal state.
///
/// If `agent_session_id` is provided, the driver uses it to restore the
/// agent's conversation history before sending the new prompt. This is
/// used by `resume_session` for follow-up turns.
pub fn start_session(
    config: SessionConfig,
    store: Arc<Store>,
    app_handle: AppHandle,
    registry: Arc<SessionRegistry>,
) -> Result<(), String> {
    // Create the driver eagerly so we fail fast if the agent isn't found.
    let driver = if let Some(ref ws_name) = config.workspace_name {
        let mut d = AcpDriver::for_workspace(ws_name, config.provider.as_deref())?;
        if let Some(ref remote_dir) = config.remote_working_dir {
            d = d.with_remote_working_dir(remote_dir.clone());
        }
        d
    } else {
        match &config.provider {
            Some(id) => AcpDriver::new(id)?,
            None => AcpDriver::first_available()?,
        }
    };

    // Persist the user message right away so it's visible immediately.
    // Include image IDs so the frontend can display them alongside the text.
    // We also mark attached images as session-scoped immediately after so they
    // don't appear in the branch timeline. Both operations are kept together;
    // if set_images_session_id fails we log a warning rather than aborting the
    // session, since the message was already persisted.
    store
        .add_session_message_with_images(
            &config.session_id,
            MessageRole::User,
            &config.prompt,
            &config.image_ids,
        )
        .map_err(|e| format!("Failed to persist user message: {e}"))?;

    if !config.image_ids.is_empty() {
        if let Err(e) = store.set_images_session_id(&config.image_ids, &config.session_id) {
            log::warn!(
                "Failed to associate images {:?} with session {}: {e}. \
                 Images may appear orphaned in the branch timeline.",
                config.image_ids,
                config.session_id
            );
        }
    }

    let cancel_token = registry.register(&config.session_id);

    // The agent protocol may use !Send futures, so we spin up a dedicated
    // thread with its own single-threaded Tokio runtime + LocalSet.
    let session_id_for_status = config.session_id.clone();
    let store_for_status = Arc::clone(&store);

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime for session");

        let local = tokio::task::LocalSet::new();
        let result = local.block_on(&rt, async {
            // Start MCP server for project sessions, injecting it via the ACP NewSessionRequest.
            let (driver, _mcp_handle) = if let Some(ref proj_id) = config.mcp_project_id {
                match crate::project_mcp::start_project_mcp_server(
                    proj_id.clone(),
                    Arc::clone(&store),
                    Arc::clone(&registry),
                    app_handle.clone(),
                    config.action_executor.clone(),
                    config.action_registry.clone(),
                    cancel_token.clone(),
                    config.workspace_name.clone(),
                )
                .await
                {
                    Ok((port, handle)) => {
                        log::info!(
                            "Session {}: MCP server started on port {port}",
                            config.session_id
                        );
                        let mcp_server = McpServer::Http(McpServerHttp::new(
                            "builderbot",
                            format!("http://127.0.0.1:{port}/mcp"),
                        ));
                        let driver = driver
                            .with_extra_env(config.extra_env.clone())
                            .with_mcp_servers(vec![mcp_server]);
                        (driver, Some(handle))
                    }
                    Err(e) => {
                        log::error!("Failed to start MCP server: {e}");
                        return Err(format!("Failed to start MCP server: {e}"));
                    }
                }
            } else {
                let env = config.extra_env.clone();
                (driver.with_extra_env(env), None)
            };

            let writer = Arc::new(MessageWriter::new(
                config.session_id.clone(),
                Arc::clone(&store),
            ));

            // Read and base64-encode images for the prompt content blocks.
            let mut image_data: Vec<(String, String)> = Vec::new();
            for image_id in &config.image_ids {
                match store.get_image(image_id) {
                    Ok(Some(image)) => {
                        match crate::store::images::image_file_path(
                            &image.project_id,
                            &image.id,
                            &image.filename,
                        ) {
                            Ok(path) => {
                                if let Ok(bytes) = std::fs::read(&path) {
                                    use base64::Engine;
                                    let encoded =
                                        base64::engine::general_purpose::STANDARD.encode(&bytes);
                                    image_data.push((encoded, image.mime_type.clone()));
                                } else {
                                    log::warn!(
                                        "Failed to read image file for image {image_id}: {}",
                                        path.display()
                                    );
                                }
                            }
                            Err(e) => {
                                log::warn!("Failed to resolve file path for image {image_id}: {e}");
                            }
                        }
                    }
                    Ok(None) => {
                        log::warn!("Image {image_id} not found in store, skipping");
                    }
                    Err(e) => {
                        log::warn!("Failed to fetch image {image_id} from store: {e}");
                    }
                }
            }

            // Cast to trait objects for the driver
            let store_trait: Arc<dyn acp_client::Store> = store;
            let writer_trait: Arc<dyn acp_client::MessageWriter> = writer;

            driver
                .run(
                    &config.session_id,
                    &config.prompt,
                    &image_data,
                    &config.working_dir,
                    &store_trait,
                    &writer_trait,
                    &cancel_token,
                    config.agent_session_id.as_deref(),
                )
                .await
        });

        // Always deregister, regardless of outcome.
        registry.deregister(&session_id_for_status);

        // Transition the session to its terminal state, but only if it is
        // still "running". This prevents a late-arriving "completed" from
        // overwriting a "cancelled" that was set by a concurrent cancel
        // request. If the transition returns false the cancel_session
        // command (or delete) already moved the status — we skip the event
        // to avoid a duplicate.
        //
        // If the session was cancelled and then deleted, these DB writes
        // are harmless no-ops — see module-level docs on the race.
        let (new_status, error_msg, completion_reason) = match result {
            Ok(()) if cancel_token.is_cancelled() => {
                ("cancelled", None, CompletionReason::Interrupted)
            }
            Ok(()) => ("completed", None, CompletionReason::TurnComplete),
            Err(ref e) if cancel_token.is_cancelled() => {
                log::info!(
                    "Session {session_id_for_status} cancelled (error during teardown: {e})"
                );
                ("cancelled", None, CompletionReason::Interrupted)
            }
            Err(ref e) => {
                log::error!("Session {session_id_for_status} failed: {e}");
                ("error", Some(e.clone()), CompletionReason::Crashed)
            }
        };

        // Run post-completion hooks before transitioning status.
        // These detect artifacts produced by the session (commits, notes).
        // Returns the branch_id when a new commit was detected.
        let committed_branch_id = if new_status == "completed" {
            run_post_completion_hooks(
                &session_id_for_status,
                &config.working_dir,
                config.pre_head_sha.as_deref(),
                config.workspace_name.as_deref(),
                &store_for_status,
            )
        } else {
            None
        };

        let status_enum = SessionStatus::parse(new_status).unwrap();
        let transitioned = store_for_status
            .transition_from_running(
                &session_id_for_status,
                status_enum,
                error_msg.as_deref(),
                Some(&completion_reason),
            )
            .unwrap_or(false);

        if transitioned {
            emit_status(
                &app_handle,
                &session_id_for_status,
                new_status,
                error_msg,
                Some(&completion_reason),
            );

            let branch_id = store_for_status
                .get_branch_id_for_session(&session_id_for_status)
                .ok()
                .flatten();
            let auto_review_branch_id = committed_branch_id.clone();

            if let Some(branch_id) = branch_id {
                let store_for_follow_up = Arc::clone(&store_for_status);
                let registry_for_follow_up = Arc::clone(&registry);
                let app_handle_for_follow_up = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    match crate::session_commands::drain_queued_sessions_for_branch(
                        Arc::clone(&store_for_follow_up),
                        Arc::clone(&registry_for_follow_up),
                        app_handle_for_follow_up.clone(),
                        branch_id.clone(),
                        None,
                    )
                    .await
                    {
                        Ok(true) => {
                            log::info!("Drained next queued session for branch {branch_id}");
                        }
                        Ok(false) => {
                            if let Some(auto_review_branch_id) = auto_review_branch_id {
                                match crate::session_commands::trigger_auto_review(
                                    store_for_follow_up,
                                    registry_for_follow_up,
                                    app_handle_for_follow_up,
                                    auto_review_branch_id.clone(),
                                    None,
                                )
                                .await
                                {
                                    Ok(resp) => {
                                        log::info!(
                                            "Auto review triggered for branch {auto_review_branch_id}: session={}, review={}",
                                            resp.session_id,
                                            resp.artifact_id,
                                        );
                                    }
                                    Err(e) => {
                                        log::error!(
                                            "Failed to trigger auto review for branch {auto_review_branch_id}: {e}"
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            log::error!(
                                "Failed to drain queued sessions for branch {branch_id}: {e}"
                            );
                        }
                    }
                });
            }
        }
    });

    Ok(())
}

// =============================================================================
// Orphaned session recovery
// =============================================================================

/// On startup, recover any sessions whose owner process is no longer alive.
///
/// Each session records the PID of the Staged process that started it
/// (`owner_pid`). On startup we check each running session:
/// - `owner_pid` is our own PID → shouldn't happen at startup, skip.
/// - `owner_pid` belongs to a live process → another Staged instance owns
///   it; leave it alone.
/// - `owner_pid` is dead (or NULL for pre-migration rows) → transition to
///   error with `AppQuit` reason and emit `session-status-changed` so the
///   frontend learns the outcome.
pub fn recover_dead_sessions(
    store: Arc<Store>,
    registry: Arc<SessionRegistry>,
    app_handle: AppHandle,
) {
    let sessions = match store.get_running_sessions() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[session_runner] Failed to query running sessions: {e}");
            return;
        }
    };

    for session in sessions {
        let should_cancel = match session.owner_pid {
            None => true,
            Some(pid) if pid == std::process::id() => false,
            Some(pid) if !is_process_alive(pid) => true,
            Some(_pid) => false,
        };

        if should_cancel {
            let transitioned = store
                .transition_from_running(
                    &session.id,
                    SessionStatus::Error,
                    None,
                    Some(&CompletionReason::AppQuit),
                )
                .unwrap_or(false);
            if transitioned {
                emit_status(
                    &app_handle,
                    &session.id,
                    "error",
                    None,
                    Some(&CompletionReason::AppQuit),
                );

                let branch_id = store.get_branch_id_for_session(&session.id).ok().flatten();
                if let Some(branch_id) = branch_id {
                    let store_for_follow_up = Arc::clone(&store);
                    let registry_for_follow_up = Arc::clone(&registry);
                    let app_handle_for_follow_up = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        match crate::session_commands::drain_queued_sessions_for_branch(
                            store_for_follow_up,
                            registry_for_follow_up,
                            app_handle_for_follow_up,
                            branch_id.clone(),
                            None,
                        )
                        .await
                        {
                            Ok(true) => {
                                log::info!(
                                    "Drained next queued session after orphan recovery for branch {branch_id}"
                                );
                            }
                            Ok(false) => {}
                            Err(e) => {
                                log::error!(
                                    "Failed to drain queued sessions after orphan recovery for branch {branch_id}: {e}"
                                );
                            }
                        }
                    });
                }
            }
        }
    }
}

/// Check whether a process is alive by sending signal 0 via the `kill` command.
///
/// `kill -0 pid` succeeds (exit 0) if the process exists and we have permission
/// to signal it. It also exits 0 on some systems when the process exists but we
/// lack permission (EPERM). A non-zero exit means the process is gone (ESRCH).
fn is_process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// =============================================================================
// Post-completion hooks
// =============================================================================

/// Detect artifacts produced by a session and update the DB accordingly.
///
/// - **Commits**: If a pending commit record is linked to this session and
///   HEAD has moved since the session started, record the new SHA.
///   For remote workspaces, HEAD is checked via `blox ws_exec`.
/// - **Notes**: If an empty note is linked to this session, parse the
///   assistant's last message for content after the first `---`.
///
/// Returns the `branch_id` when a new commit was successfully detected,
/// so the caller can trigger follow-up work (e.g. auto review).
fn run_post_completion_hooks(
    session_id: &str,
    working_dir: &std::path::Path,
    pre_head_sha: Option<&str>,
    workspace_name: Option<&str>,
    store: &Arc<Store>,
) -> Option<String> {
    let mut committed_branch_id: Option<String> = None;

    // --- Commit detection ---
    if let Some(pre_sha) = pre_head_sha {
        // Look for any commit linked to this session — not just pending (sha IS NULL)
        // ones — so we also detect amended commits on resumed sessions.
        if let Ok(Some(commit)) = store.get_commit_by_session(session_id) {
            // Get current HEAD — either from local worktree or remote workspace.
            let current_head_result = if let Some(ws_name) = workspace_name {
                crate::blox::ws_exec(ws_name, &["git", "rev-parse", "HEAD"])
                    .map(|s| s.trim().to_string())
                    .map_err(|e| format!("{e}"))
            } else {
                crate::git::get_head_sha(working_dir).map_err(|e| format!("{e}"))
            };

            match current_head_result {
                Ok(current_head) if current_head != pre_sha => {
                    log::info!(
                        "Session {session_id}: new commit detected ({} → {})",
                        &pre_sha[..7.min(pre_sha.len())],
                        &current_head[..7.min(current_head.len())]
                    );
                    if let Err(e) = store.update_commit_sha(&commit.id, &current_head) {
                        log::error!("Failed to update commit SHA: {e}");
                    }
                    committed_branch_id = Some(commit.branch_id.clone());

                    // Spawn background diff caching for remote branches.
                    if let Some(ws_name) = workspace_name {
                        let commit_shas: Vec<String> = store
                            .list_commits_for_branch(&commit.branch_id)
                            .unwrap_or_default()
                            .into_iter()
                            .filter_map(|c| c.sha)
                            .collect();
                        crate::diff_cache::spawn_cache_branch_diff(
                            Arc::clone(store),
                            commit.branch_id.clone(),
                            ws_name.to_string(),
                            current_head.clone(),
                            commit_shas,
                        );
                    }
                }
                Ok(_) => {
                    if commit.sha.is_none() {
                        log::info!("Session {session_id}: no new commit (HEAD unchanged), leaving pending commit as failed");
                    } else {
                        log::info!("Session {session_id}: no commit change (HEAD unchanged)");
                    }
                }
                Err(e) => {
                    log::error!("Failed to get HEAD SHA after session: {e}");
                }
            }
        }
    }

    // --- Note extraction (repo notes and project notes) ---
    //
    // Both paths share the same message-scanning and title-resolution logic
    // via `resolve_note_title_and_body`, differing only in which DB record
    // they read/write.
    struct NoteTarget {
        id: String,
        is_amendment: bool,
        kind: NoteKind,
    }
    enum NoteKind {
        Repo,
        Project,
    }

    let note_targets: Vec<NoteTarget> = [
        store
            .get_note_by_session(session_id)
            .ok()
            .flatten()
            .map(|n| NoteTarget {
                id: n.id,
                is_amendment: !n.content.is_empty(),
                kind: NoteKind::Repo,
            }),
        store
            .get_project_note_by_session(session_id)
            .ok()
            .flatten()
            .map(|n| NoteTarget {
                id: n.id,
                is_amendment: !n.content.is_empty(),
                kind: NoteKind::Project,
            }),
    ]
    .into_iter()
    .flatten()
    .collect();

    if !note_targets.is_empty() {
        // Scan assistant messages once for all note targets.
        if let Ok(messages) = store.get_session_messages(session_id) {
            let note_content = messages
                .iter()
                .rev()
                .filter(|m| m.role == MessageRole::Assistant)
                .find_map(|m| extract_note_content(&m.content));

            for target in &note_targets {
                let label = match target.kind {
                    NoteKind::Repo => "note",
                    NoteKind::Project => "project note",
                };
                if let Some(ref note_content) = note_content {
                    let (final_title, body) =
                        resolve_note_title_and_body(note_content, store, session_id);
                    log::info!(
                        "Session {session_id}: {} {label} \"{final_title}\"",
                        if target.is_amendment {
                            "amended"
                        } else {
                            "extracted"
                        }
                    );
                    let result = match target.kind {
                        NoteKind::Repo => {
                            store.update_note_title_and_content(&target.id, &final_title, &body)
                        }
                        NoteKind::Project => store.update_project_note_title_and_content(
                            &target.id,
                            &final_title,
                            &body,
                        ),
                    };
                    if let Err(e) = result {
                        log::error!("Failed to update {label} content: {e}");
                    }
                } else {
                    log::warn!("Session {session_id}: {label} session completed but no --- found in assistant output");
                }
            }
        }
    }

    // --- Review comment and title extraction ---
    if let Ok(Some(review)) = store.get_review_by_session(session_id) {
        if let Ok(messages) = store.get_session_messages(session_id) {
            let full_text: String = messages
                .iter()
                .filter(|m| m.role == MessageRole::Assistant)
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            // Extract and save review title (always attempt, even if comments exist)
            if review.title.is_none() {
                if let Some(title) = extract_review_title(&full_text) {
                    log::info!("Session {session_id}: extracted review title: {title}");
                    if let Err(e) = store.update_review_title(&review.id, &title) {
                        log::error!("Failed to update review title: {e}");
                    }
                } else {
                    log::warn!("Session {session_id}: review session completed but no review-title block found");
                }
            }

            // Extract and save review comments
            if review.comments.is_empty() {
                let comments = extract_review_comments(&full_text);
                if comments.is_empty() {
                    log::warn!("Session {session_id}: review session completed but no review-comments block found");
                } else {
                    log::info!(
                        "Session {session_id}: extracted {} review comments",
                        comments.len()
                    );
                    for comment in comments {
                        if let Err(e) = store.add_comment(&review.id, &comment) {
                            log::error!("Failed to add review comment: {e}");
                        }
                    }
                }
            }
        }
    }

    committed_branch_id
}

/// Extract note content from a single assistant message.
///
/// Callers are responsible for choosing which message to pass — typically the
/// **last** assistant message that contains a `---`. Within that message we
/// use the **first** HR (outside code fences) as the note separator, because
/// the agent places the `---` at the boundary between preamble and note
/// content.
///
/// Primary path: find the first markdown horizontal rule (`---`, `***`, `___`)
/// on its own line (outside code fences) and return everything after it.
///
/// Fallback path: handle model outputs where the rule is accidentally attached
/// to prior text (for example, `Preamble.---\n# Title`) by accepting inline
/// rules only when the remaining content starts with an H1.
fn extract_note_content(text: &str) -> Option<String> {
    extract_note_after_standalone_hr(text).or_else(|| extract_note_after_inline_hr(text))
}

fn extract_note_after_standalone_hr(text: &str) -> Option<String> {
    // Look for --- on its own line (possibly with surrounding whitespace).
    // We match the same patterns markdown parsers treat as thematic breaks:
    // a line containing only ---, ***, or ___ (with optional spaces).
    //
    // We scan from the **top** so the *first* HR wins. The caller already
    // selects the last assistant message that contains a separator, so
    // within that message the first `---` marks the start of the note.
    //
    // We track whether we are inside a fenced code block (``` or ~~~) and
    // skip any HR that appears inside one.
    let lines: Vec<&str> = text.lines().collect();

    // Pre-compute which lines are inside a code fence.
    let mut in_fence = vec![false; lines.len()];
    let mut inside = false;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            inside = !inside;
        }
        in_fence[i] = inside;
    }

    for i in 0..lines.len() {
        if in_fence[i] {
            continue;
        }
        let trimmed = lines[i].trim();
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            let remaining: String = lines[i + 1..].join("\n");
            let trimmed_remaining = remaining.trim().to_string();
            if !trimmed_remaining.is_empty() {
                return Some(trimmed_remaining);
            }
        }
    }
    None
}

fn extract_note_after_inline_hr(text: &str) -> Option<String> {
    // Pre-compute a set of byte offsets that fall inside fenced code blocks
    // so we can skip inline HRs that appear in code examples.
    let fence_ranges = compute_fence_ranges(text);

    let mut best: Option<(usize, String)> = None;

    for marker in ["---", "***", "___"] {
        let marker_char = marker.chars().next().unwrap();
        for (idx, _) in text.match_indices(marker) {
            // Skip markers inside fenced code blocks.
            if fence_ranges
                .iter()
                .any(|(start, end)| idx >= *start && idx < *end)
            {
                continue;
            }

            let marker_end = idx + marker.len();

            // Ignore markers that are part of longer runs like ----.
            if text[..idx].ends_with(marker_char) || text[marker_end..].starts_with(marker_char) {
                continue;
            }

            let remaining = text[marker_end..].trim_start();
            if !remaining.starts_with("# ") {
                continue;
            }

            // Keep the *first* (earliest) match — the note starts at the
            // first separator in the message.
            match best {
                Some((best_idx, _)) if idx >= best_idx => {}
                _ => best = Some((idx, remaining.to_string())),
            }
        }
    }

    best.map(|(_, content)| content)
}

/// Return byte-offset ranges `(start, end)` for content inside fenced code
/// blocks (``` or ~~~). The ranges cover from the character after the opening
/// fence line's newline to the start of the closing fence line.
fn compute_fence_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut fence_start: Option<usize> = None;

    for (line_start, line) in line_byte_offsets(text) {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            if fence_start.is_some() {
                // Closing fence — record the range.
                ranges.push((fence_start.unwrap(), line_start));
                fence_start = None;
            } else {
                // Opening fence — content starts after this line.
                let after_line = line_start + line.len() + 1; // +1 for newline
                fence_start = Some(after_line.min(text.len()));
            }
        }
    }
    ranges
}

/// Iterate over `(byte_offset, line_str)` pairs for each line in `text`.
fn line_byte_offsets(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    text.lines().map(move |line| {
        let start = offset;
        // +1 accounts for the newline character (or end of string for last line)
        offset += line.len() + 1;
        (start, line)
    })
}

/// Strip leading `<action>...</action>` blocks from a prompt so that fallback
/// title generation uses the user's actual text rather than injected XML.
fn strip_action_wrapper(prompt: &str) -> &str {
    let trimmed = prompt.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<action>") {
        if let Some(end) = rest.find("</action>") {
            return rest[end + "</action>".len()..].trim_start();
        }
    }
    prompt
}

/// Extract a title (leading `# H1`) from note content.
///
/// Returns `(title, body_without_title)`. If no H1 is found, title is empty
/// and body is the full content.
fn extract_note_title(content: &str) -> (String, String) {
    let trimmed = content.trim_start();
    if let Some(rest) = trimmed.strip_prefix("# ") {
        // Find the end of the first line (the title)
        if let Some(newline_pos) = rest.find('\n') {
            let title = rest[..newline_pos].trim().to_string();
            let body = rest[newline_pos + 1..].trim_start().to_string();
            (title, body)
        } else {
            // The entire content is just the title
            (rest.trim().to_string(), String::new())
        }
    } else {
        (String::new(), content.to_string())
    }
}

/// Parse note content into a final `(title, body)` pair, using the session
/// prompt as a fallback title when the note has no H1 heading.
///
/// Shared by both repo-note and project-note extraction paths.
fn resolve_note_title_and_body(
    note_content: &str,
    store: &Store,
    session_id: &str,
) -> (String, String) {
    let (title, body) = extract_note_title(note_content);
    let final_title = if title.is_empty() {
        store
            .get_session(session_id)
            .ok()
            .flatten()
            .map(|s| {
                let prompt = strip_action_wrapper(&s.prompt);
                let t: String = prompt.chars().take(80).collect();
                if prompt.len() > 80 {
                    format!("{t}…")
                } else {
                    t
                }
            })
            .unwrap_or_else(|| "Untitled Note".to_string())
    } else {
        title
    };
    (final_title, body)
}

/// Extract review comments from assistant output.
///
/// Looks for ```review-comments fenced blocks and parses the JSON array inside.
/// Each object should have `path`, `span` (with `start` and `end`), and `content`.
fn extract_review_comments(text: &str) -> Vec<Comment> {
    let mut comments = Vec::new();

    // Find all ```review-comments blocks
    let marker_start = "```review-comments";

    let mut search_from = 0;
    while let Some(rel_pos) = find_opening_fence(&text[search_from..], marker_start) {
        let start_pos = search_from + rel_pos;
        let block_start = start_pos + marker_start.len();
        // Skip to the next line after the opening marker
        let json_start = match text[block_start..].find('\n') {
            Some(pos) => block_start + pos + 1,
            None => break,
        };

        // Find the closing ``` — must be on its own line (start of line),
        // not an embedded code fence inside JSON string values like ```rust.
        // We look for "\n```" where the ``` is followed by EOF, newline, or
        // only whitespace (not an info-string like "rust" or "sql").
        if let Some(end_pos) = find_closing_fence(&text[json_start..]) {
            let json_str = &text[json_start..json_start + end_pos];

            // Parse the JSON array
            if let Ok(parsed) = serde_json::from_str::<Vec<serde_json::Value>>(json_str.trim()) {
                for item in parsed {
                    let path = item.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let content = item.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    let comment_type_str = item.get("type").and_then(|v| v.as_str());
                    let span_obj = item.get("span");
                    let start = span_obj
                        .and_then(|s| s.get("start"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    let end = span_obj
                        .and_then(|s| s.get("end"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;

                    if !path.is_empty() && !content.is_empty() {
                        let mut comment = Comment::new(path, Span::new(start, end), content)
                            .with_author(CommentAuthor::Agent);
                        if let Some(ct) = comment_type_str.and_then(CommentType::parse) {
                            comment = comment.with_comment_type(ct);
                        }
                        comments.push(comment);
                    }
                }
            } else {
                log::warn!("Failed to parse review-comments JSON block");
            }

            search_from = json_start + end_pos + 3; // skip past the closing ```
        } else {
            break;
        }
    }

    comments
}

/// Extract the review title from assistant output.
///
/// Looks for a ```review-title fenced block and returns the trimmed text inside.
/// The opening fence must appear at the start of a line to avoid matching the
/// marker when it appears inside regular prose (e.g. the LLM discussing the
/// extraction logic itself).
fn extract_review_title(text: &str) -> Option<String> {
    let marker = "```review-title";
    let start_pos = find_opening_fence(text, marker)?;
    let block_start = start_pos + marker.len();
    let content_start = block_start + text[block_start..].find('\n')? + 1;
    let end_pos = find_closing_fence(&text[content_start..])?;
    let title = text[content_start..content_start + end_pos].trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

/// Find an opening fence marker (e.g. ` ```review-title `) that appears at the
/// start of a line (position 0 or immediately after `\n`).  Returns the byte
/// offset of the marker within `text`, or `None` if no line-start match exists.
fn find_opening_fence(text: &str, marker: &str) -> Option<usize> {
    let mut pos = 0;
    while pos < text.len() {
        let candidate = text[pos..].find(marker)?;
        let abs = pos + candidate;
        if abs == 0 || text.as_bytes()[abs - 1] == b'\n' {
            return Some(abs);
        }
        pos = abs + marker.len();
    }
    None
}

/// Find the closing ``` fence for a code block.
///
/// A closing fence must appear at the start of a line (after a newline) and
/// must consist of exactly ``` followed by EOF, a newline, or only whitespace.
/// This distinguishes it from opening fences like ```rust or ``` embedded
/// within JSON string values (where the ``` appears mid-line after `\n` escape
/// sequences in the JSON, not actual newlines in the text).
fn find_closing_fence(text: &str) -> Option<usize> {
    let fence = "```";
    let mut pos = 0;
    while pos < text.len() {
        let remaining = &text[pos..];
        let candidate = remaining.find(fence)?;
        let abs = pos + candidate;

        // Must be at column 0: either start of text or preceded by '\n'
        let at_line_start = abs == 0 || text.as_bytes()[abs - 1] == b'\n';
        if at_line_start {
            // Check what follows the ```: must be EOF, newline, or whitespace-only to EOL
            let after = &text[abs + fence.len()..];
            let is_closing = if after.is_empty() {
                true
            } else {
                match after.find('\n') {
                    Some(nl) => after[..nl].trim().is_empty(),
                    None => after.trim().is_empty(),
                }
            };
            if is_closing {
                return Some(abs);
            }
        }

        pos = abs + fence.len();
    }
    None
}

fn emit_status(
    app_handle: &AppHandle,
    session_id: &str,
    status: &str,
    error: Option<String>,
    completion_reason: Option<&CompletionReason>,
) {
    let event = SessionStatusEvent {
        session_id: session_id.to_string(),
        status: status.to_string(),
        error_message: error,
        completion_reason: completion_reason.map(|r| r.as_str().to_string()),
        branch_id: None,
        project_id: None,
        session_type: None,
        is_auto_review: false,
    };
    if let Err(e) = app_handle.emit("session-status-changed", &event) {
        log::warn!("Failed to emit session-status-changed: {e}");
    }
}

/// Emit a `session-status-changed` event with `"running"` status and branch/project
/// context. Called by the MCP tool when it starts a repo session on behalf of a project
/// session, so the frontend can register the session in its state stores and refresh
/// the branch card timeline immediately (without waiting for completion).
pub fn emit_session_running(
    app_handle: &AppHandle,
    session_id: &str,
    branch_id: &str,
    project_id: &str,
    session_type: &str,
) {
    let event = SessionStatusEvent {
        session_id: session_id.to_string(),
        status: "running".to_string(),
        error_message: None,
        completion_reason: None,
        branch_id: Some(branch_id.to_string()),
        project_id: Some(project_id.to_string()),
        session_type: Some(session_type.to_string()),
        is_auto_review: false,
    };
    if let Err(e) = app_handle.emit("session-status-changed", &event) {
        log::warn!("Failed to emit session-status-changed (running): {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── find_closing_fence ──────────────────────────────────────────────

    #[test]
    fn closing_fence_simple() {
        let text = "some json\n```\n";
        assert_eq!(find_closing_fence(text), Some(10));
    }

    #[test]
    fn closing_fence_at_eof_without_newline() {
        let text = "some json\n```";
        assert_eq!(find_closing_fence(text), Some(10));
    }

    #[test]
    fn closing_fence_skips_opening_fences() {
        // ```rust is an opening fence (has info-string), should be skipped
        let text = "before\n```rust\ncode\n```\nafter";
        assert_eq!(find_closing_fence(text), Some(20));
    }

    #[test]
    fn closing_fence_skips_mid_line_backticks() {
        // ``` appearing mid-line (not at column 0) should be skipped
        let text = "some text ``` not a fence\n```\n";
        assert_eq!(find_closing_fence(text), Some(26));
    }

    #[test]
    fn closing_fence_none_when_missing() {
        let text = "no closing fence here\n```rust\ncode";
        assert_eq!(find_closing_fence(text), None);
    }

    #[test]
    fn closing_fence_with_trailing_whitespace() {
        let text = "json\n```  \nmore";
        assert_eq!(find_closing_fence(text), Some(5));
    }

    // ── extract_review_title ──────────────────────────────────────────

    #[test]
    fn extract_title_simple() {
        let text = "Here is my review:\n\n```review-title\nSolid refactor with one edge case\n```\n\nAnd comments...";
        assert_eq!(
            extract_review_title(text),
            Some("Solid refactor with one edge case".to_string())
        );
    }

    #[test]
    fn extract_title_trims_whitespace() {
        let text = "```review-title\n  Clean changes, no concerns  \n```\n";
        assert_eq!(
            extract_review_title(text),
            Some("Clean changes, no concerns".to_string())
        );
    }

    #[test]
    fn extract_title_none_when_missing() {
        let text = "Just a normal message with no review title.";
        assert_eq!(extract_review_title(text), None);
    }

    #[test]
    fn extract_title_none_when_empty() {
        let text = "```review-title\n\n```\n";
        assert_eq!(extract_review_title(text), None);
    }

    #[test]
    fn extract_title_with_review_comments() {
        let text = r#"Here is my review:

```review-title
Risky changes to auth flow need closer look
```

```review-comments
[
  {
    "path": "src/auth.rs",
    "span": { "start": 10, "end": 15 },
    "content": "Missing validation."
  }
]
```
"#;
        assert_eq!(
            extract_review_title(text),
            Some("Risky changes to auth flow need closer look".to_string())
        );
    }

    #[test]
    fn extract_title_ignores_llm_preamble() {
        let text = r#"I now have a complete picture of the changes. Let me produce the review.

```review-title
Clean, well-tested feature addition with good backward compatibility
```

```review-comments
[
  {
    "path": "src/foo.rs",
    "span": { "start": 10, "end": 15 },
    "type": "suggestion",
    "content": "Consider adding a test."
  }
]
```
"#;
        assert_eq!(
            extract_review_title(text),
            Some(
                "Clean, well-tested feature addition with good backward compatibility".to_string()
            )
        );
    }

    #[test]
    fn extract_title_marker_mentioned_in_preamble() {
        // The LLM discusses the ```review-title marker in its preamble before
        // actually producing the fenced block.  The extractor must skip the
        // mid-line mention and find the real opening fence.
        let text = r#"Let me check whether `"```review-title"` would match inside `"```review-title-v2"`:
I now have a complete picture. Let me produce the review.

```review-title
Solid changes with minor nit
```

```review-comments
[]
```
"#;
        assert_eq!(
            extract_review_title(text),
            Some("Solid changes with minor nit".to_string())
        );
    }

    // ── extract_review_comments ─────────────────────────────────────────

    #[test]
    fn extract_simple_comments() {
        let text = r#"Here is my review:

```review-comments
[
  {
    "path": "src/main.rs",
    "span": { "start": 10, "end": 15 },
    "content": "This function is missing error handling."
  }
]
```

That's all!"#;

        let comments = extract_review_comments(text);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].path, "src/main.rs");
        assert_eq!(comments[0].span.start, 10);
        assert_eq!(comments[0].span.end, 15);
        assert_eq!(
            comments[0].content,
            "This function is missing error handling."
        );
    }

    #[test]
    fn extract_comments_with_embedded_code_blocks() {
        // This is the exact bug scenario: JSON content contains markdown
        // code blocks with triple backticks. The old parser would match the
        // first ``` inside the JSON string, truncating the JSON.
        let text = r#"Review:

```review-comments
[
  {
    "path": "src/store/models.rs",
    "span": { "start": 82, "end": 91 },
    "content": "Consider implementing `FromStr`:\n```rust\nimpl FromStr for Status {\n    type Err = Error;\n    fn from_str(s: &str) -> Result<Self, Self::Err> {\n        match s {\n            \"ready\" => Ok(Self::Ready),\n            _ => Err(Error::InvalidStatus(s.to_string())),\n        }\n    }\n}\n```\nThis would be more idiomatic."
  },
  {
    "path": "src/store/pool.rs",
    "span": { "start": 10, "end": 25 },
    "content": "Missing validation for worktree path."
  }
]
```

Done."#;

        let comments = extract_review_comments(text);
        assert_eq!(comments.len(), 2, "should parse both comments");
        assert_eq!(comments[0].path, "src/store/models.rs");
        assert!(comments[0].content.contains("FromStr"));
        assert_eq!(comments[1].path, "src/store/pool.rs");
    }

    #[test]
    fn extract_multiple_review_blocks() {
        let text = r#"First batch:

```review-comments
[
  {
    "path": "a.rs",
    "span": { "start": 1, "end": 2 },
    "content": "Issue A"
  }
]
```

Second batch:

```review-comments
[
  {
    "path": "b.rs",
    "span": { "start": 3, "end": 4 },
    "content": "Issue B"
  }
]
```
"#;

        let comments = extract_review_comments(text);
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].path, "a.rs");
        assert_eq!(comments[1].path, "b.rs");
    }

    #[test]
    fn extract_no_review_block() {
        let text = "Just a normal message with no review comments.";
        let comments = extract_review_comments(text);
        assert!(comments.is_empty());
    }

    #[test]
    fn extract_skips_empty_path_or_content() {
        let text = r#"```review-comments
[
  {
    "path": "",
    "span": { "start": 0, "end": 0 },
    "content": "Has content but no path"
  },
  {
    "path": "file.rs",
    "span": { "start": 0, "end": 0 },
    "content": ""
  }
]
```"#;

        let comments = extract_review_comments(text);
        assert!(
            comments.is_empty(),
            "should skip entries with empty path or content"
        );
    }

    #[test]
    fn extract_comment_types() {
        let text = r#"```review-comments
[
  {
    "path": "src/main.rs",
    "span": { "start": 1, "end": 5 },
    "content": "FYI: this is informational.",
    "type": "information"
  },
  {
    "path": "src/main.rs",
    "span": { "start": 10, "end": 12 },
    "content": "Consider renaming this variable.",
    "type": "suggestion"
  },
  {
    "path": "src/lib.rs",
    "span": { "start": 20, "end": 25 },
    "content": "This could panic at runtime.",
    "type": "warning"
  },
  {
    "path": "src/lib.rs",
    "span": { "start": 30, "end": 35 },
    "content": "Off-by-one error here.",
    "type": "issue"
  },
  {
    "path": "src/lib.rs",
    "span": { "start": 40, "end": 45 },
    "content": "No type field at all."
  }
]
```"#;

        let comments = extract_review_comments(text);
        assert_eq!(comments.len(), 5);

        assert_eq!(comments[0].comment_type, Some(CommentType::Information));
        assert_eq!(comments[1].comment_type, Some(CommentType::Suggestion));
        assert_eq!(comments[2].comment_type, Some(CommentType::Warning));
        assert_eq!(comments[3].comment_type, Some(CommentType::Issue));
        assert_eq!(
            comments[4].comment_type, None,
            "missing type field should result in None"
        );
    }

    // ── extract_note_content ────────────────────────────────────────────

    #[test]
    fn note_content_after_hr() {
        let text = "Preamble\n---\n# My Note\nBody here.";
        let content = extract_note_content(text);
        assert_eq!(content, Some("# My Note\nBody here.".to_string()));
    }

    #[test]
    fn note_content_none_without_hr() {
        let text = "Just some text without a horizontal rule.";
        assert_eq!(extract_note_content(text), None);
    }

    #[test]
    fn note_content_inline_hr_before_h1() {
        let text =
            "I gathered enough context.---\n# Repo Purpose\nThis repo ships desktop tooling.";
        let content = extract_note_content(text);
        assert_eq!(
            content,
            Some("# Repo Purpose\nThis repo ships desktop tooling.".to_string())
        );
    }

    #[test]
    fn note_content_inline_hr_without_h1_is_ignored() {
        let text = "Two reasons:--- this session is read-only.";
        assert_eq!(extract_note_content(text), None);
    }

    #[test]
    fn note_content_first_hr_wins_within_message() {
        // Within a single message the first --- is the note separator.
        // The caller is responsible for picking the right message.
        let text = "Here is the format:\n---\n# <Title>\n<Body>\n\nNow here is my actual note:\n---\n# Real Title\nReal body.";
        let content = extract_note_content(text);
        assert_eq!(
            content,
            Some(
                "# <Title>\n<Body>\n\nNow here is my actual note:\n---\n# Real Title\nReal body."
                    .to_string()
            ),
        );
    }

    #[test]
    fn note_content_hr_inside_code_fence_is_skipped() {
        // A --- inside a fenced code block should not be treated as a note separator.
        let text = "Here is an example:\n```\n---\n# <Title>\n<Body>\n```\n---\n# Actual Note\nActual body.";
        let content = extract_note_content(text);
        assert_eq!(content, Some("# Actual Note\nActual body.".to_string()));
    }

    #[test]
    fn note_content_hr_inside_tilde_fence_is_skipped() {
        let text = "Example:\n~~~\n---\n# Fake\n~~~\n---\n# Real\nBody.";
        let content = extract_note_content(text);
        assert_eq!(content, Some("# Real\nBody.".to_string()));
    }

    #[test]
    fn note_content_only_hr_is_inside_code_fence() {
        // If the only --- is inside a code fence, no note should be detected.
        let text = "Some reasoning:\n```\n---\n# Title\nBody\n```\nDone.";
        assert_eq!(extract_note_content(text), None);
    }

    #[test]
    fn note_content_multiple_hrs_picks_first() {
        // Multiple standalone HRs — the first one delimits the note.
        let text = "Section 1\n---\nSection 2\n---\n# Final Note\nFinal body.";
        let content = extract_note_content(text);
        assert_eq!(
            content,
            Some("Section 2\n---\n# Final Note\nFinal body.".to_string()),
        );
    }

    // ── extract_note_title ──────────────────────────────────────────────

    #[test]
    fn note_title_from_h1() {
        let (title, body) = extract_note_title("# My Title\nBody text.");
        assert_eq!(title, "My Title");
        assert_eq!(body, "Body text.");
    }

    #[test]
    fn note_title_empty_when_no_h1() {
        let (title, body) = extract_note_title("No heading here.\nJust text.");
        assert!(title.is_empty());
        assert_eq!(body, "No heading here.\nJust text.");
    }

    // ── strip_action_wrapper ────────────────────────────────────────────

    #[test]
    fn strip_action_no_wrapper() {
        assert_eq!(strip_action_wrapper("plain prompt"), "plain prompt");
    }

    #[test]
    fn strip_action_normal_wrapper() {
        let input = "<action>\nSome injected context\n</action>\nActual user prompt";
        assert_eq!(strip_action_wrapper(input), "Actual user prompt");
    }

    #[test]
    fn strip_action_wrapper_with_leading_whitespace() {
        let input = "  \n<action>injected</action>  user text";
        assert_eq!(strip_action_wrapper(input), "user text");
    }

    #[test]
    fn strip_action_missing_closing_tag() {
        // If the closing tag is absent, return the original prompt unchanged.
        let input = "<action>unclosed block\nuser text";
        assert_eq!(strip_action_wrapper(input), input);
    }

    #[test]
    fn strip_action_whitespace_only_after_stripping() {
        // After stripping the wrapper, only whitespace remains — should trim to empty.
        let input = "<action>stuff</action>   \n  ";
        assert_eq!(strip_action_wrapper(input), "");
    }

    #[test]
    fn strip_action_nested_action_tags() {
        // The function uses the *first* </action> it finds, so a nested
        // <action> inside the wrapper content would split there.
        let input = "<action>outer <action>inner</action> leftover</action>\nreal prompt";
        assert_eq!(
            strip_action_wrapper(input),
            "leftover</action>\nreal prompt"
        );
    }
}
