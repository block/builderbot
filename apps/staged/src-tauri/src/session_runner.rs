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
use std::io;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

use acp_client::{McpServer, McpServerHttp};

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::agent::{AcpDriver, AgentDriver, MessageWriter};
use crate::git::Span;
use crate::shell_env::ShellEnvCache;
use crate::store::{
    Comment, CommentAuthor, CommentType, CompletionReason, FailureStrategy, MessageRole,
    PipelineExecution, PipelineKind, PipelineStep, SessionStatus, StepStatus, StepType, Store,
};

const PIPELINE_STEP_PROMPT_OUTPUT_MAX_CHARS: usize = 30_000;

pub fn git_identity_env_from_global_config() -> Vec<(String, String)> {
    let Some(name) = global_git_config_value("user.name") else {
        return vec![];
    };
    let Some(email) = global_git_config_value("user.email") else {
        return vec![];
    };

    vec![
        ("GIT_AUTHOR_NAME".to_string(), name.clone()),
        ("GIT_AUTHOR_EMAIL".to_string(), email.clone()),
        ("GIT_COMMITTER_NAME".to_string(), name),
        ("GIT_COMMITTER_EMAIL".to_string(), email),
    ]
}

fn global_git_config_value(key: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--global", key])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

// =============================================================================
// Event types
// =============================================================================

/// Emitted when an individual pipeline step changes status.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStepEvent {
    pub session_id: String,
    pub step_index: usize,
    pub label: String,
    pub step_type: StepType,
    pub status: StepStatus,
    pub output: Option<String>,
    pub error: Option<String>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

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
    running: std::sync::Mutex<HashMap<String, Arc<RunningSession>>>,
}

struct RunningSession {
    token: CancellationToken,
    cancellation_completion_reason: std::sync::Mutex<Option<CompletionReason>>,
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
        let running_session = Arc::new(RunningSession {
            token: token.clone(),
            cancellation_completion_reason: std::sync::Mutex::new(None),
        });
        let mut running = self.running.lock().unwrap();
        running.insert(session_id.to_string(), running_session);
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
        self.cancel_with_completion_reason(session_id, CompletionReason::Interrupted)
    }

    /// Cancel a running session and remember the completion reason it should persist.
    pub fn cancel_with_completion_reason(
        &self,
        session_id: &str,
        completion_reason: CompletionReason,
    ) -> bool {
        let running_session = self.running.lock().unwrap().get(session_id).cloned();
        if let Some(running_session) = running_session {
            let mut stored_reason = running_session
                .cancellation_completion_reason
                .lock()
                .unwrap();
            if stored_reason.is_none()
                || matches!(
                    completion_reason,
                    CompletionReason::ProjectSessionInterrupted
                )
            {
                *stored_reason = Some(completion_reason);
            }
            running_session.token.cancel();
            true
        } else {
            false
        }
    }

    pub fn cancellation_completion_reason(&self, session_id: &str) -> Option<CompletionReason> {
        self.running
            .lock()
            .unwrap()
            .get(session_id)
            .and_then(|running| {
                running
                    .cancellation_completion_reason
                    .lock()
                    .unwrap()
                    .clone()
            })
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
    /// Branch that owns this session (branch-level sessions only).
    /// Threaded through so terminal events carry the same context as start events.
    pub branch_id: Option<String>,
    /// Project that owns this session. Set for both project-note sessions
    /// (directly) and branch-level sessions (via the branch's project).
    pub project_id: Option<String>,
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
    // Local sessions without an explicit provider resolve the first available
    // provider and persist it on the session. Review-producing callers resolve
    // a concrete provider before creating their session/review rows.
    let driver = if let Some(ref ws_name) = config.workspace_name {
        let mut d = AcpDriver::for_workspace(ws_name, config.provider.as_deref())?;
        if let Some(ref remote_dir) = config.remote_working_dir {
            d = d.with_remote_working_dir(remote_dir.clone());
        }
        d
    } else {
        match &config.provider {
            Some(id) => AcpDriver::new(id)?,
            None => {
                // Resolve the first available provider and backfill it on the
                // local session record so consumers see the provider that
                // actually ran the agent.
                let providers = crate::agent::discover_providers();
                let first = providers.first().ok_or_else(|| {
                    "No ACP agent found. Install Goose, Claude Code, Codex, Pi, or Amp and ensure it's on your PATH.".to_string()
                })?;
                if let Err(e) = store.set_session_provider(&config.session_id, &first.id) {
                    log::warn!(
                        "Failed to backfill provider on session {}: {e}",
                        config.session_id
                    );
                }
                AcpDriver::new(&first.id)?
            }
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
                    config.provider.clone(),
                    cancel_token.clone(),
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

        let cancellation_completion_reason = registry
            .cancellation_completion_reason(&session_id_for_status)
            .unwrap_or(CompletionReason::Interrupted);

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
                ("cancelled", None, cancellation_completion_reason.clone())
            }
            Ok(()) => ("completed", None, CompletionReason::TurnComplete),
            Err(ref e) if cancel_token.is_cancelled() => {
                log::info!(
                    "Session {session_id_for_status} cancelled (error during teardown: {e})"
                );
                ("cancelled", None, cancellation_completion_reason)
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

        // Always emit the terminal status event, even if the DB row was already
        // deleted (e.g. user deleted the pending commit). This lets the frontend
        // clean up sidebar "running" state as a safety net.
        emit_status(
            &app_handle,
            &session_id_for_status,
            new_status,
            error_msg,
            Some(&completion_reason),
            config.branch_id.clone(),
            config.project_id.clone(),
        );

        if transitioned {
            let branch_id = config.branch_id.clone();
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
                            // Check if auto-review is enabled in user preferences
                            let auto_review_enabled = crate::preferences_store_path_buf()
                                .and_then(|path| std::fs::read_to_string(&path).ok())
                                .and_then(|contents| {
                                    serde_json::from_str::<serde_json::Value>(&contents).ok()
                                })
                                .and_then(|json| {
                                    json.get("auto-start-code-reviews")?
                                        .as_str()
                                        .map(String::from)
                                })
                                .map(|mode| mode != "never")
                                .unwrap_or_else(crate::blox::is_sq_available);

                            if let Some(auto_review_branch_id) =
                                auto_review_branch_id.filter(|_| auto_review_enabled)
                            {
                                // Pass None so trigger_auto_review resolves
                                // the user's current preferred agent at
                                // trigger time, rather than reusing the
                                // (possibly stale) commit session provider.
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
// Pipeline execution
// =============================================================================

/// Configuration for a pipeline-driven session.
pub struct PipelineConfig {
    pub session_id: String,
    /// Original user-facing session prompt. Used when a command failure falls
    /// through to a generic AI handoff without a step-specific prompt.
    pub prompt: String,
    pub steps: Vec<PipelineStep>,
    /// The pipeline execution state persisted with the session. Passed in so
    /// `run_pipeline` doesn't reconstruct it from scratch (the caller already
    /// built and persisted one via `PipelineExecution::from_steps`).
    pub pipeline: PipelineExecution,
    pub working_dir: PathBuf,
    /// HEAD before the deterministic pipeline began. Used by rebase pipelines
    /// when they hand off to AI after conflicts.
    pub pre_head_sha: Option<String>,
    /// ACP provider ID (e.g. "goose", "claude").
    pub provider: Option<String>,
    /// Workspace name for remote branches.
    pub workspace_name: Option<String>,
    /// Remote working directory for remote branches.
    pub remote_working_dir: Option<PathBuf>,
    /// Branch that owns this pipeline session. Copied to `SessionConfig` on
    /// AI handoff and used by `emit_status` for terminal events.
    pub branch_id: Option<String>,
    /// Project that owns this pipeline session.
    pub project_id: Option<String>,
}

/// Result of running a pipeline — tells the caller what happened.
pub enum PipelineOutcome {
    /// All steps succeeded without needing AI.
    CompletedWithoutAi,
    /// An AI handoff occurred; a normal AI session was started with this prompt.
    /// `ai_step_index` is `Some(idx)` when the handoff originated from an
    /// explicit `AiHandoff` step (so we can mark it failed if `start_session`
    /// errors). It is `None` when the handoff came from a failed Command step's
    /// `HandoffToAi` failure strategy — in that case remaining steps are already
    /// marked as skipped and no further pipeline updates are needed.
    HandedOffToAi {
        prompt: String,
        ai_step_index: Option<usize>,
    },
    /// The pipeline was aborted because a step failed AND the configured abort
    /// marker was found in the output. This signals a known, expected failure
    /// (e.g. non-fast-forward rejection) that the frontend handles specially.
    ///
    /// Note: if a step uses `Abort { marker: Some(m) }` but the marker is NOT
    /// found in the output, the pipeline falls through to `HandedOffToAi`
    /// instead — only marker-matched failures produce this variant.
    Aborted { step_index: usize },
    /// The pipeline was cancelled externally.
    Cancelled,
}

enum PipelineCommandResult {
    Completed(Output),
    Cancelled { stdout: Vec<u8>, stderr: Vec<u8> },
}

/// Start a pipeline-driven session. Runs deterministic command steps first,
/// then hands off to AI if needed.
///
/// Like `start_session`, this returns immediately and runs in the background.
pub fn start_pipeline_session(
    config: PipelineConfig,
    store: Arc<Store>,
    app_handle: AppHandle,
    registry: Arc<SessionRegistry>,
) -> Result<(), String> {
    let cancel_token = registry.register(&config.session_id);
    let session_id = config.session_id.clone();
    let store_for_status = Arc::clone(&store);

    // We use a dedicated OS thread + single-threaded runtime (matching start_session)
    // because the pipeline may hand off to an AI session that uses !Send futures
    // (via the agent protocol's LocalSet requirement). Keeping the same threading
    // model avoids mixing runtime contexts.
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime for pipeline session");

        let mut config = config;
        let local = tokio::task::LocalSet::new();
        let outcome = local.block_on(&rt, async {
            run_pipeline(&mut config, &store, &app_handle, &cancel_token).await
        });

        match outcome {
            PipelineOutcome::CompletedWithoutAi => {
                // Pipeline completed successfully — transition session to completed.
                resolve_pipeline_artifacts_without_ai(&config, &store_for_status, true);
                let status_enum = SessionStatus::Completed;
                let reason = CompletionReason::TurnComplete;
                registry.deregister(&session_id);
                let transitioned = store_for_status
                    .transition_from_running(&session_id, status_enum, None, Some(&reason))
                    .unwrap_or(false);
                emit_status(
                    &app_handle,
                    &session_id,
                    "completed",
                    None,
                    Some(&reason),
                    config.branch_id.clone(),
                    config.project_id.clone(),
                );
                if transitioned {
                    drain_queued_after_pipeline_terminal(
                        Arc::clone(&store_for_status),
                        Arc::clone(&registry),
                        app_handle.clone(),
                        config.branch_id.clone(),
                    );
                }
            }
            PipelineOutcome::HandedOffToAi {
                prompt,
                ai_step_index,
            } => {
                // The pipeline wants to start an AI session with the built prompt.
                // We reuse the same session_id. The session is still "running".
                //
                // We intentionally skip deregister here: start_session's register()
                // call will atomically replace the old cancel token. This avoids a
                // window where the session has no token registered (during which a
                // cancel request would be silently lost).
                let pre_head_sha = pre_head_for_pipeline_handoff(&config);
                let extra_env = if store_for_status
                    .get_commit_by_session(&session_id)
                    .ok()
                    .flatten()
                    .is_some()
                {
                    git_identity_env_from_global_config()
                } else {
                    vec![]
                };

                // Try to start the AI session now.
                let ai_config = SessionConfig {
                    session_id: session_id.clone(),
                    prompt,
                    working_dir: config.working_dir.clone(),
                    agent_session_id: None,
                    pre_head_sha,
                    provider: config.provider.clone(),
                    workspace_name: config.workspace_name.clone(),
                    extra_env,
                    mcp_project_id: None,
                    action_executor: None,
                    action_registry: None,
                    remote_working_dir: config.remote_working_dir.clone(),
                    image_ids: vec![],
                    branch_id: config.branch_id.clone(),
                    project_id: config.project_id.clone(),
                };
                if let Err(e) = start_session(
                    ai_config,
                    store_for_status.clone(),
                    app_handle.clone(),
                    Arc::clone(&registry),
                ) {
                    log::error!("Failed to start AI session after pipeline handoff: {e}");
                    // If the handoff came from an explicit AiHandoff step, mark
                    // it as failed so the UI doesn't show a perpetual spinner.
                    if let Some(step_idx) = ai_step_index {
                        if let Ok(Some(session)) = store_for_status.get_session(&session_id) {
                            if let Some(mut pipeline) = session.pipeline {
                                if step_idx < pipeline.steps.len() {
                                    pipeline.steps[step_idx].status = StepStatus::Failed;
                                    pipeline.steps[step_idx].error =
                                        Some(format!("Failed to start AI session: {e}"));
                                    pipeline.steps[step_idx].completed_at =
                                        Some(crate::store::now_timestamp());
                                    let _ = store_for_status
                                        .update_session_pipeline(&session_id, &pipeline);
                                    emit_pipeline_step(
                                        &app_handle,
                                        &session_id,
                                        step_idx,
                                        &pipeline.steps[step_idx],
                                    );
                                }
                            }
                        }
                    }
                    resolve_pipeline_artifacts_without_ai(&config, &store_for_status, false);
                    let transitioned = finish_failed_pipeline_handoff_start(
                        &store_for_status,
                        &registry,
                        &session_id,
                        &e,
                    );
                    emit_status(
                        &app_handle,
                        &session_id,
                        "error",
                        Some(e),
                        Some(&CompletionReason::Crashed),
                        config.branch_id.clone(),
                        config.project_id.clone(),
                    );
                    if transitioned {
                        drain_queued_after_pipeline_terminal(
                            Arc::clone(&store_for_status),
                            Arc::clone(&registry),
                            app_handle.clone(),
                            config.branch_id.clone(),
                        );
                    }
                }
            }
            PipelineOutcome::Aborted { .. } => {
                // Pipeline aborted (e.g. non-fast-forward). Mark as completed so
                // the frontend can inspect the pipeline steps for the failure.
                resolve_pipeline_artifacts_without_ai(&config, &store_for_status, false);
                let reason = CompletionReason::TurnComplete;
                registry.deregister(&session_id);
                let transitioned = store_for_status
                    .transition_from_running(
                        &session_id,
                        SessionStatus::Completed,
                        None,
                        Some(&reason),
                    )
                    .unwrap_or(false);
                emit_status(
                    &app_handle,
                    &session_id,
                    "completed",
                    None,
                    Some(&reason),
                    config.branch_id.clone(),
                    config.project_id.clone(),
                );
                if transitioned {
                    drain_queued_after_pipeline_terminal(
                        Arc::clone(&store_for_status),
                        Arc::clone(&registry),
                        app_handle.clone(),
                        config.branch_id.clone(),
                    );
                }
            }
            PipelineOutcome::Cancelled => {
                resolve_pipeline_artifacts_without_ai(&config, &store_for_status, false);
                let reason = registry
                    .cancellation_completion_reason(&session_id)
                    .unwrap_or(CompletionReason::Interrupted);
                registry.deregister(&session_id);
                let transitioned = store_for_status
                    .transition_from_running(
                        &session_id,
                        SessionStatus::Cancelled,
                        None,
                        Some(&reason),
                    )
                    .unwrap_or(false);
                emit_status(
                    &app_handle,
                    &session_id,
                    "cancelled",
                    None,
                    Some(&reason),
                    config.branch_id.clone(),
                    config.project_id.clone(),
                );
                if transitioned {
                    drain_queued_after_pipeline_terminal(
                        Arc::clone(&store_for_status),
                        Arc::clone(&registry),
                        app_handle.clone(),
                        config.branch_id.clone(),
                    );
                }
            }
        }
    });

    Ok(())
}

fn finish_failed_pipeline_handoff_start(
    store: &Store,
    registry: &SessionRegistry,
    session_id: &str,
    error: &str,
) -> bool {
    registry.deregister(session_id);
    store
        .transition_from_running(
            session_id,
            SessionStatus::Error,
            Some(error),
            Some(&CompletionReason::Crashed),
        )
        .unwrap_or(false)
}

fn pre_head_for_pipeline_handoff(config: &PipelineConfig) -> Option<String> {
    match config.pipeline.kind.as_ref() {
        Some(PipelineKind::Rebase) => config.pre_head_sha.clone(),
        Some(PipelineKind::Squash) => match current_pipeline_head(config) {
            Ok(head) => Some(head),
            Err(e) => {
                log::warn!("Failed to capture squash handoff HEAD: {e}");
                None
            }
        },
        None => None,
    }
}

fn current_pipeline_head(config: &PipelineConfig) -> Result<String, String> {
    if let Some(ws_name) = config.workspace_name.as_deref() {
        crate::blox::ws_exec(ws_name, &["git", "rev-parse", "HEAD"])
            .map(|s| s.trim().to_string())
            .map_err(|e| e.to_string())
    } else {
        crate::git::get_head_sha(&config.working_dir).map_err(|e| e.to_string())
    }
}

fn resolve_pipeline_artifacts_without_ai(config: &PipelineConfig, store: &Store, completed: bool) {
    match config.pipeline.kind.as_ref() {
        Some(PipelineKind::Rebase) if completed => {
            finalize_rebase_pipeline_without_ai(config, store);
        }
        Some(PipelineKind::Rebase | PipelineKind::Squash) => {
            if let Err(e) = store.delete_pending_commit_for_session(&config.session_id) {
                log::warn!(
                    "Failed to resolve pending commit for terminal pipeline session {}: {e}",
                    config.session_id
                );
            }
        }
        None => {}
    }
}

fn finalize_rebase_pipeline_without_ai(config: &PipelineConfig, store: &Store) {
    let Some(pre_head_sha) = config.pre_head_sha.as_deref() else {
        let _ = store.delete_pending_commit_for_session(&config.session_id);
        return;
    };

    let commit = match store.get_commit_by_session(&config.session_id) {
        Ok(Some(commit)) => commit,
        Ok(None) => return,
        Err(e) => {
            log::warn!(
                "Failed to load pending commit for rebase pipeline session {}: {e}",
                config.session_id
            );
            return;
        }
    };

    let current_head = match current_pipeline_head(config) {
        Ok(head) => head,
        Err(e) => {
            log::error!(
                "Failed to get HEAD after rebase pipeline session {}: {e}",
                config.session_id
            );
            return;
        }
    };

    if current_head == pre_head_sha {
        if let Err(e) = store.delete_pending_commit_for_session(&config.session_id) {
            log::warn!(
                "Failed to remove no-op rebase pending commit for session {}: {e}",
                config.session_id
            );
        }
        return;
    }

    match store.complete_pending_commit_sha(&commit.id, &commit.branch_id, &current_head) {
        Ok(true) => log::info!(
            "Rebase pipeline session {} updated pending commit to {}",
            config.session_id,
            &current_head[..7.min(current_head.len())]
        ),
        Ok(false) => log::info!(
            "Rebase pipeline session {} resolved duplicate commit SHA {}",
            config.session_id,
            &current_head[..7.min(current_head.len())]
        ),
        Err(e) => log::error!(
            "Failed to complete rebase pending commit for session {}: {e}",
            config.session_id
        ),
    }
}

fn drain_queued_after_pipeline_terminal(
    store: Arc<Store>,
    registry: Arc<SessionRegistry>,
    app_handle: AppHandle,
    branch_id: Option<String>,
) {
    let Some(branch_id) = branch_id else {
        return;
    };

    tauri::async_runtime::spawn(async move {
        match crate::session_commands::drain_queued_sessions_for_branch(
            store,
            registry,
            app_handle,
            branch_id.clone(),
            None,
        )
        .await
        {
            Ok(true) => log::info!("Drained next queued session for branch {branch_id}"),
            Ok(false) => {}
            Err(e) => log::error!(
                "Failed to drain queued sessions after pipeline terminal state for branch {branch_id}: {e}"
            ),
        }
    });
}

/// Execute pipeline steps sequentially, emitting events as each step progresses.
async fn run_pipeline(
    config: &mut PipelineConfig,
    store: &Arc<Store>,
    app_handle: &AppHandle,
    cancel_token: &CancellationToken,
) -> PipelineOutcome {
    // Capture HEAD before the first step for rebase pipelines. This is deferred
    // from session creation so the (potentially slow) remote HEAD lookup doesn't
    // block session visibility in the UI.
    if config.pre_head_sha.is_none() && config.pipeline.kind.as_ref() == Some(&PipelineKind::Rebase)
    {
        match current_pipeline_head(config) {
            Ok(head) => config.pre_head_sha = Some(head),
            Err(e) => log::warn!("Failed to capture pre-rebase HEAD: {e}"),
        }
    }

    // Use the pipeline execution state that was already persisted with the
    // session, rather than reconstructing from step definitions. This keeps
    // the data flow clear: callers build + persist the pipeline, and we
    // mutate that same instance as steps progress.
    let mut execution = config.pipeline.clone();

    // Collect outputs from prior steps for template substitution.
    let mut step_outputs: Vec<String> = Vec::new();

    for (idx, step) in config.steps.iter().enumerate() {
        if cancel_token.is_cancelled() {
            // Mark remaining steps as skipped.
            for remaining in execution.steps[idx..].iter_mut() {
                remaining.status = StepStatus::Skipped;
            }
            let _ = store.update_session_pipeline(&config.session_id, &execution);
            return PipelineOutcome::Cancelled;
        }

        execution.current_step = idx;

        match step {
            PipelineStep::Command {
                label,
                command,
                on_failure,
            } => {
                // Mark step as running.
                execution.steps[idx].status = StepStatus::Running;
                execution.steps[idx].started_at = Some(crate::store::now_timestamp());
                let _ = store.update_session_pipeline(&config.session_id, &execution);
                emit_pipeline_step(app_handle, &config.session_id, idx, &execution.steps[idx]);

                // Execute the shell command — locally or on a remote workspace.
                let result = if let Some(ref ws_name) = config.workspace_name {
                    run_remote_pipeline_command(
                        command,
                        ws_name,
                        config
                            .remote_working_dir
                            .as_deref()
                            .and_then(|p| p.to_str()),
                        cancel_token,
                    )
                    .await
                } else {
                    run_pipeline_command(command, &config.working_dir, cancel_token).await
                };

                match result {
                    Ok(PipelineCommandResult::Completed(output)) => {
                        let combined =
                            combine_normalized_command_output(&output.stdout, &output.stderr);

                        if output.status.success() {
                            execution.steps[idx].status = StepStatus::Succeeded;
                            execution.steps[idx].output = Some(combined.clone());
                            execution.steps[idx].completed_at = Some(crate::store::now_timestamp());
                            let _ = store.update_session_pipeline(&config.session_id, &execution);
                            emit_pipeline_step(
                                app_handle,
                                &config.session_id,
                                idx,
                                &execution.steps[idx],
                            );
                            step_outputs.push(format_step_output_for_prompt(
                                label, command, &combined, false,
                            ));
                        } else {
                            // Command failed — apply failure strategy.
                            execution.steps[idx].status = StepStatus::Failed;
                            execution.steps[idx].output = Some(combined.clone());
                            execution.steps[idx].error =
                                Some(format!("Exit code: {}", output.status));
                            execution.steps[idx].completed_at = Some(crate::store::now_timestamp());

                            step_outputs.push(format_step_output_for_prompt(
                                label, command, &combined, true,
                            ));

                            match on_failure {
                                FailureStrategy::Abort { marker } => {
                                    if let Some(m) = marker {
                                        // combined is already display-normalized, so only
                                        // strip hostile chars for marker matching.
                                        let marker_output =
                                            crate::terminal_output::strip_prompt_hostile_chars(
                                                &combined,
                                            );
                                        if marker_output.contains(m.as_str()) {
                                            // Mark remaining steps as skipped.
                                            for remaining in execution.steps[idx + 1..].iter_mut() {
                                                remaining.status = StepStatus::Skipped;
                                            }
                                            let _ = store.update_session_pipeline(
                                                &config.session_id,
                                                &execution,
                                            );
                                            emit_pipeline_step(
                                                app_handle,
                                                &config.session_id,
                                                idx,
                                                &execution.steps[idx],
                                            );
                                            return PipelineOutcome::Aborted { step_index: idx };
                                        }
                                        // Marker not found — fall through to AI handoff.
                                        let prompt = format!(
                                            "{}\n\nStep '{}' failed. Diagnose and fix using the output below:\n\n{}",
                                            config.prompt,
                                            label,
                                            step_outputs.join("\n\n")
                                        );
                                        for remaining in execution.steps[idx + 1..].iter_mut() {
                                            remaining.status = StepStatus::Skipped;
                                        }
                                        let _ = store.update_session_pipeline(
                                            &config.session_id,
                                            &execution,
                                        );
                                        emit_pipeline_step(
                                            app_handle,
                                            &config.session_id,
                                            idx,
                                            &execution.steps[idx],
                                        );
                                        return PipelineOutcome::HandedOffToAi {
                                            prompt,
                                            ai_step_index: None,
                                        };
                                    }
                                    // No marker — always abort.
                                    for remaining in execution.steps[idx + 1..].iter_mut() {
                                        remaining.status = StepStatus::Skipped;
                                    }
                                    let _ = store
                                        .update_session_pipeline(&config.session_id, &execution);
                                    emit_pipeline_step(
                                        app_handle,
                                        &config.session_id,
                                        idx,
                                        &execution.steps[idx],
                                    );
                                    return PipelineOutcome::Aborted { step_index: idx };
                                }
                                FailureStrategy::HandoffToAi { prompt_template } => {
                                    let prompt = prompt_template
                                        .replace("{step_outputs}", &step_outputs.join("\n\n"));
                                    for remaining in execution.steps[idx + 1..].iter_mut() {
                                        remaining.status = StepStatus::Skipped;
                                    }
                                    let _ = store
                                        .update_session_pipeline(&config.session_id, &execution);
                                    emit_pipeline_step(
                                        app_handle,
                                        &config.session_id,
                                        idx,
                                        &execution.steps[idx],
                                    );
                                    return PipelineOutcome::HandedOffToAi {
                                        prompt,
                                        ai_step_index: None,
                                    };
                                }
                                FailureStrategy::Continue => {
                                    let _ = store
                                        .update_session_pipeline(&config.session_id, &execution);
                                    emit_pipeline_step(
                                        app_handle,
                                        &config.session_id,
                                        idx,
                                        &execution.steps[idx],
                                    );
                                    // Continue to next step.
                                }
                            }
                        }
                    }
                    Ok(PipelineCommandResult::Cancelled { stdout, stderr }) => {
                        let combined = combine_normalized_command_output(&stdout, &stderr);
                        execution.steps[idx].status = StepStatus::Skipped;
                        execution.steps[idx].output = if combined.is_empty() {
                            None
                        } else {
                            Some(combined)
                        };
                        execution.steps[idx].error = Some("Cancelled by user".to_string());
                        execution.steps[idx].completed_at = Some(crate::store::now_timestamp());
                        for remaining in execution.steps[idx + 1..].iter_mut() {
                            remaining.status = StepStatus::Skipped;
                        }
                        let _ = store.update_session_pipeline(&config.session_id, &execution);
                        emit_pipeline_step(
                            app_handle,
                            &config.session_id,
                            idx,
                            &execution.steps[idx],
                        );
                        return PipelineOutcome::Cancelled;
                    }
                    Err(e) => {
                        // Failed to even spawn the command. Always hand off to AI
                        // regardless of the step's configured `on_failure` strategy
                        // — spawn failures (e.g. missing shell, permission denied)
                        // are environmental issues that need AI/human diagnosis,
                        // and the step's failure strategy is designed for command
                        // *output* classification, not spawn-level errors.
                        execution.steps[idx].status = StepStatus::Failed;
                        execution.steps[idx].error = Some(format!("Failed to execute: {e}"));
                        execution.steps[idx].completed_at = Some(crate::store::now_timestamp());
                        let prompt = format!(
                            "{}\n\nStep '{}' failed to execute: {e}\n\n{}",
                            config.prompt,
                            label,
                            step_outputs.join("\n\n")
                        );
                        for remaining in execution.steps[idx + 1..].iter_mut() {
                            remaining.status = StepStatus::Skipped;
                        }
                        let _ = store.update_session_pipeline(&config.session_id, &execution);
                        emit_pipeline_step(
                            app_handle,
                            &config.session_id,
                            idx,
                            &execution.steps[idx],
                        );
                        return PipelineOutcome::HandedOffToAi {
                            prompt,
                            ai_step_index: None,
                        };
                    }
                }
            }
            PipelineStep::AiHandoff {
                prompt_template, ..
            } => {
                // Build the prompt and hand off to AI.
                execution.steps[idx].status = StepStatus::Running;
                execution.steps[idx].started_at = Some(crate::store::now_timestamp());
                let _ = store.update_session_pipeline(&config.session_id, &execution);
                emit_pipeline_step(app_handle, &config.session_id, idx, &execution.steps[idx]);

                let prompt = prompt_template.replace("{step_outputs}", &step_outputs.join("\n\n"));

                // Mark the step as succeeded — the handoff itself completed.
                // The AI session's success/failure is tracked by the session
                // status, not this pipeline step. If start_session later fails,
                // start_pipeline_session will mark this step as failed.
                execution.steps[idx].status = StepStatus::Succeeded;
                execution.steps[idx].completed_at = Some(crate::store::now_timestamp());
                let _ = store.update_session_pipeline(&config.session_id, &execution);
                emit_pipeline_step(app_handle, &config.session_id, idx, &execution.steps[idx]);

                return PipelineOutcome::HandedOffToAi {
                    prompt,
                    ai_step_index: Some(idx),
                };
            }
        }
    }

    // All steps completed without AI handoff.
    execution.completed_without_ai = true;
    let _ = store.update_session_pipeline(&config.session_id, &execution);
    PipelineOutcome::CompletedWithoutAi
}

/// Shared cache of interactive-login-shell env snapshots, keyed by working
/// directory. Spawning `$SHELL -ils` to capture `.zshrc`-driven PATH (e.g.
/// Hermit) on every pipeline step costs ~50–500 ms; this amortises it to
/// once per project per TTL window.
pub fn shell_env_cache() -> &'static Arc<ShellEnvCache> {
    static CACHE: OnceLock<Arc<ShellEnvCache>> = OnceLock::new();
    CACHE.get_or_init(|| Arc::new(ShellEnvCache::new()))
}

async fn run_pipeline_command(
    command: &str,
    working_dir: &PathBuf,
    cancel_token: &CancellationToken,
) -> io::Result<PipelineCommandResult> {
    run_pipeline_command_with_cache(shell_env_cache(), command, working_dir, cancel_token).await
}

/// Same as [`run_pipeline_command`] but lets the caller pass an explicit cache.
/// Used by tests to pre-seed snapshots or point at a hermetic fake `$SHELL`.
async fn run_pipeline_command_with_cache(
    cache: &ShellEnvCache,
    command: &str,
    working_dir: &PathBuf,
    cancel_token: &CancellationToken,
) -> io::Result<PipelineCommandResult> {
    // Apply the cached interactive-login-shell env so Hermit-managed
    // binaries are on PATH (matters for git hooks invoked by pipeline
    // steps). On capture failure fall back to `sh -lc`, which at least
    // sources `/etc/profile`/`~/.profile`.
    let snapshot = match cache.get(working_dir).await {
        Ok(env) => Some(env),
        Err(e) => {
            log::warn!(
                "Failed to capture shell env for {}: {e}; falling back to sh -lc",
                working_dir.display()
            );
            None
        }
    };

    let mut cmd = tokio::process::Command::new("sh");
    let sh_args: &[&str] = if snapshot.is_some() {
        &["-c", command]
    } else {
        &["-lc", command]
    };
    cmd.args(sh_args)
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(snapshot) = &snapshot {
        snapshot.apply_to(&mut cmd);
    }

    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd.spawn()?;
    let stdout_task = child.stdout.take().map(spawn_pipe_reader);
    let stderr_task = child.stderr.take().map(spawn_pipe_reader);

    // Wait for either the child to exit or cancellation. Using `child.wait()`
    // directly (instead of a polling loop with `try_wait()`) avoids both the
    // 50ms poll latency and a potential pipe deadlock: the pipe readers run as
    // spawned tasks on the same single-threaded runtime, so they only make
    // progress when the main task yields. `child.wait()` yields properly,
    // allowing the pipe readers to drain output concurrently.
    tokio::select! {
        result = child.wait() => {
            let status = result?;
            let stdout = collect_pipe_output(stdout_task, "stdout").await;
            let stderr = collect_pipe_output(stderr_task, "stderr").await;
            Ok(PipelineCommandResult::Completed(Output {
                status,
                stdout,
                stderr,
            }))
        }
        _ = cancel_token.cancelled() => {
            terminate_pipeline_child(&mut child);
            match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    log::warn!("Failed to wait for cancelled pipeline command: {e}");
                }
                Err(_) => {
                    force_kill_pipeline_child(&mut child);
                    if let Err(e) = child.wait().await {
                        log::warn!("Failed to wait for killed pipeline command: {e}");
                    }
                }
            }

            let stdout = collect_pipe_output(stdout_task, "stdout").await;
            let stderr = collect_pipe_output(stderr_task, "stderr").await;
            Ok(PipelineCommandResult::Cancelled { stdout, stderr })
        }
    }
}

/// Execute a pipeline command on a remote Blox workspace via `ws_exec`.
///
/// Wraps the command in `cd <remote_dir> && sh -lc '<command>'` so it runs
/// in the correct working directory on the remote, mirroring how the local
/// path runs `sh -lc` with `current_dir`.
async fn run_remote_pipeline_command(
    command: &str,
    workspace_name: &str,
    remote_working_dir: Option<&str>,
    cancel_token: &CancellationToken,
) -> io::Result<PipelineCommandResult> {
    // Build the remote shell command.
    let shell_command = if let Some(dir) = remote_working_dir {
        // Escape single quotes in the directory and command for the outer sh -c.
        let escaped_dir = dir.replace('\'', "'\\''");
        let escaped_cmd = command.replace('\'', "'\\''");
        format!("cd '{escaped_dir}' && sh -lc '{escaped_cmd}'")
    } else {
        let escaped_cmd = command.replace('\'', "'\\''");
        format!("sh -lc '{escaped_cmd}'")
    };

    let ws = workspace_name.to_string();
    let handle = tokio::task::spawn_blocking(move || {
        crate::blox::ws_exec_output(&ws, &["sh", "-c", &shell_command])
    });

    // Check for pre-existing cancellation before waiting.
    if cancel_token.is_cancelled() {
        handle.abort();
        return Ok(PipelineCommandResult::Cancelled {
            stdout: Vec::new(),
            stderr: Vec::new(),
        });
    }

    tokio::select! {
        result = handle => {
            match result {
                Ok(Ok(output)) => {
                    use std::os::unix::process::ExitStatusExt;
                    let status = if output.success {
                        std::process::ExitStatus::from_raw(0)
                    } else {
                        // Encode exit code 1 in wait-status format (exit code << 8).
                        std::process::ExitStatus::from_raw(1 << 8)
                    };
                    Ok(PipelineCommandResult::Completed(Output {
                        status,
                        stdout: output.stdout,
                        stderr: output.stderr,
                    }))
                }
                Ok(Err(e)) => {
                    // Infrastructure error (CLI not found, timeout, auth failure).
                    Err(io::Error::other(e.to_string()))
                }
                Err(e) => {
                    // spawn_blocking panicked or was cancelled.
                    Err(io::Error::other(e.to_string()))
                }
            }
        }
        _ = cancel_token.cancelled() => {
            // ws_exec is blocking and not cancellable — the background thread
            // will finish naturally, but we return immediately.
            Ok(PipelineCommandResult::Cancelled {
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
    }
}

fn combine_normalized_command_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = crate::terminal_output::normalize_display_bytes(stdout);
    let stderr = crate::terminal_output::normalize_display_bytes(stderr);
    if stderr.is_empty() {
        stdout
    } else if stdout.is_empty() {
        stderr
    } else {
        format!("{stdout}\n{stderr}")
    }
}

fn format_step_output_for_prompt(label: &str, command: &str, output: &str, failed: bool) -> String {
    // Input is already display-normalized (from combine_normalized_command_output),
    // so only strip hostile control chars without re-running CR/ANSI processing.
    let output = crate::terminal_output::strip_prompt_hostile_chars(output);
    let output =
        crate::terminal_output::truncate_for_prompt(&output, PIPELINE_STEP_PROMPT_OUTPUT_MAX_CHARS);
    let status = if failed { " (FAILED)" } else { "" };
    format!("### {label}{status}\nCommand: {command}\n```\n{output}\n```")
}

fn spawn_pipe_reader<R>(mut pipe: R) -> tokio::task::JoinHandle<io::Result<Vec<u8>>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut output = Vec::new();
        pipe.read_to_end(&mut output).await?;
        Ok(output)
    })
}

async fn collect_pipe_output(
    handle: Option<tokio::task::JoinHandle<io::Result<Vec<u8>>>>,
    stream_name: &str,
) -> Vec<u8> {
    let Some(mut handle) = handle else {
        return Vec::new();
    };

    match tokio::time::timeout(Duration::from_secs(2), &mut handle).await {
        Ok(Ok(Ok(output))) => output,
        Ok(Ok(Err(e))) => {
            log::warn!("Failed to read pipeline command {stream_name}: {e}");
            Vec::new()
        }
        Ok(Err(e)) => {
            log::warn!("Pipeline command {stream_name} reader task failed: {e}");
            Vec::new()
        }
        Err(_) => {
            handle.abort();
            log::warn!("Timed out reading pipeline command {stream_name}");
            Vec::new()
        }
    }
}

fn terminate_pipeline_child(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        if send_signal_to_pipeline_process_group(pid, libc::SIGTERM).is_ok() {
            return;
        }
    }

    if let Err(e) = child.start_kill() {
        log::debug!("Failed to terminate pipeline command process: {e}");
    }
}

fn force_kill_pipeline_child(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        if send_signal_to_pipeline_process_group(pid, libc::SIGKILL).is_ok() {
            return;
        }
    }

    if let Err(e) = child.start_kill() {
        log::debug!("Failed to kill pipeline command process: {e}");
    }
}

#[cfg(unix)]
fn send_signal_to_pipeline_process_group(pid: u32, signal: libc::c_int) -> io::Result<()> {
    let pgid = i32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "process id exceeds i32"))?;

    // SAFETY: kill(2) does not dereference pointers. A negative pid targets the
    // process group created for the pipeline command.
    let result = unsafe { libc::kill(-pgid, signal) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn emit_pipeline_step(
    app_handle: &AppHandle,
    session_id: &str,
    step_index: usize,
    step: &crate::store::PipelineStepStatus,
) {
    let event = PipelineStepEvent {
        session_id: session_id.to_string(),
        step_index,
        label: step.label.clone(),
        step_type: step.step_type.clone(),
        status: step.status.clone(),
        output: step.output.clone(),
        error: step.error.clone(),
        started_at: step.started_at,
        completed_at: step.completed_at,
    };
    crate::web_server::emit_to_all(app_handle, "pipeline-step-changed", &event);
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
            let recovered_branch_id = store.get_branch_id_for_session(&session.id).ok().flatten();
            let recovered_project_id = store.get_project_id_for_session(&session.id).ok().flatten();
            emit_status(
                &app_handle,
                &session.id,
                "error",
                None,
                Some(&CompletionReason::AppQuit),
                recovered_branch_id.clone(),
                recovered_project_id,
            );
            if transitioned {
                let branch_id = recovered_branch_id;
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
                    let recorded = if commit.sha.is_none() {
                        match store.complete_pending_commit_sha(
                            &commit.id,
                            &commit.branch_id,
                            &current_head,
                        ) {
                            Ok(recorded) => recorded,
                            Err(e) => {
                                log::error!("Failed to update pending commit SHA: {e}");
                                false
                            }
                        }
                    } else {
                        match store.get_commit_by_sha(&commit.branch_id, &current_head) {
                            Ok(Some(existing)) if existing.id != commit.id => {
                                log::warn!(
                                    "Session {session_id}: target commit SHA already has metadata row {}, skipping update",
                                    existing.id
                                );
                                false
                            }
                            Ok(_) => {
                                if let Err(e) = store.update_commit_sha(&commit.id, &current_head) {
                                    log::error!("Failed to update commit SHA: {e}");
                                    false
                                } else {
                                    true
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to check existing commit SHA: {e}");
                                false
                            }
                        }
                    };

                    if recorded {
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

            // Search assistant messages in reverse so the *last* message
            // containing the fenced block wins (mirrors extract_note_content).
            let suggested_next_steps = messages
                .iter()
                .rev()
                .filter(|m| m.role == MessageRole::Assistant)
                .find_map(|m| extract_suggested_next_steps(&m.content));
            if let Some(ref steps) = suggested_next_steps {
                log::info!(
                    "Session {session_id}: extracted suggested next steps — commit: {:?}, note: {:?}",
                    steps.suggested_next_commit_step,
                    steps.suggested_next_note_step,
                );
            }

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
                    let sncs = suggested_next_steps
                        .as_ref()
                        .and_then(|s| s.suggested_next_commit_step.as_deref());
                    let snns = suggested_next_steps
                        .as_ref()
                        .and_then(|s| s.suggested_next_note_step.as_deref());
                    let result = match target.kind {
                        NoteKind::Repo => store.update_note_title_and_content(
                            &target.id,
                            &final_title,
                            &body,
                            sncs,
                            snns,
                        ),
                        NoteKind::Project => store.update_project_note_title_and_content(
                            &target.id,
                            &final_title,
                            &body,
                            sncs,
                            snns,
                        ),
                    };
                    if let Err(e) = result {
                        log::error!("Failed to update {label} content: {e}");
                    }
                } else {
                    log::warn!("Session {session_id}: {label} session completed but no --- found in assistant output");
                    let result = match target.kind {
                        NoteKind::Repo => store.mark_note_completed(&target.id),
                        NoteKind::Project => store.mark_project_note_completed(&target.id),
                    };
                    if let Err(e) = result {
                        log::error!("Failed to mark {label} completed: {e}");
                    }
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
                    // Still mark the review as completed so completed_at is set
                    // for timeline sorting, even without a title.
                    if let Err(e) = store.mark_review_completed(&review.id) {
                        log::error!("Failed to mark review completed: {e}");
                    }
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
    let sanitized = strip_suggested_next_steps_blocks(text);
    let text = sanitized.as_deref().unwrap_or(text);
    extract_note_after_standalone_hr(text).or_else(|| extract_note_after_inline_hr(text))
}

/// Remove assistant response metadata before scanning for the note separator.
/// The note body remains normal markdown and can still contain fenced code.
fn strip_suggested_next_steps_blocks(text: &str) -> Option<String> {
    let marker = "```suggested-next-steps";
    let mut output = String::new();
    let mut last_copied = 0;
    let mut search_from = 0;
    let mut removed_any = false;

    while search_from < text.len() {
        let Some(rel_pos) = find_suggested_next_steps_opening_fence(&text[search_from..], marker)
        else {
            break;
        };
        let start_pos = search_from + rel_pos;
        let block_start = start_pos + marker.len();
        let Some(newline_pos) = text[block_start..].find('\n') else {
            break;
        };
        let content_start = block_start + newline_pos + 1;
        let Some(end_pos) = find_closing_fence(&text[content_start..]) else {
            break;
        };

        let closing_start = content_start + end_pos;
        let after_closing = text[closing_start..]
            .find('\n')
            .map(|newline| closing_start + newline + 1)
            .unwrap_or(text.len());

        output.push_str(&text[last_copied..start_pos]);
        last_copied = after_closing;
        search_from = after_closing;
        removed_any = true;
    }

    if removed_any {
        output.push_str(&text[last_copied..]);
        Some(output)
    } else {
        None
    }
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

/// Structured representation of the suggested next steps extracted from a
/// `suggested-next-steps` fenced block.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuggestedNextSteps {
    suggested_next_commit_step: Option<String>,
    suggested_next_note_step: Option<String>,
}

/// Extract suggested next steps from assistant output.
///
/// Looks for a ```suggested-next-steps fenced block and parses the JSON object
/// inside.  Returns `None` if the block is missing or cannot be parsed.
fn extract_suggested_next_steps(text: &str) -> Option<SuggestedNextSteps> {
    let marker = "```suggested-next-steps";
    let start_pos = find_suggested_next_steps_opening_fence(text, marker)?;
    let block_start = start_pos + marker.len();
    let content_start = block_start + text[block_start..].find('\n')? + 1;
    let end_pos = find_closing_fence(&text[content_start..])?;
    let json_str = text[content_start..content_start + end_pos].trim();
    match serde_json::from_str::<SuggestedNextSteps>(json_str) {
        Ok(steps) => Some(steps),
        Err(e) => {
            log::warn!("Failed to parse suggested-next-steps JSON: {e}");
            None
        }
    }
}

/// Find a `suggested-next-steps` opening fence.
///
/// The normal fence finder requires markers to appear at the start of a line
/// because review extraction should not match markers mentioned in prose. Notes
/// can arrive with this fence attached to the final sentence, so this finder
/// accepts inline markers while still requiring the rest of the marker line to
/// contain only optional whitespace before the newline.
fn find_suggested_next_steps_opening_fence(text: &str, marker: &str) -> Option<usize> {
    let mut pos = 0;
    while pos < text.len() {
        let candidate = text[pos..].find(marker)?;
        let abs = pos + candidate;
        let after_marker = &text[abs + marker.len()..];
        if after_marker
            .find('\n')
            .is_some_and(|newline| after_marker[..newline].trim().is_empty())
        {
            return Some(abs);
        }
        pos = abs + marker.len();
    }
    None
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
    branch_id: Option<String>,
    project_id: Option<String>,
) {
    let event = SessionStatusEvent {
        session_id: session_id.to_string(),
        status: status.to_string(),
        error_message: error,
        completion_reason: completion_reason.map(|r| r.as_str().to_string()),
        branch_id,
        project_id,
        session_type: None,
        is_auto_review: false,
    };
    crate::web_server::emit_to_all(app_handle, "session-status-changed", &event);
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
    crate::web_server::emit_to_all(app_handle, "session-status-changed", &event);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::strip_git_env;

    #[cfg(unix)]
    #[tokio::test]
    async fn pipeline_command_cancellation_stops_current_step() {
        // Pre-warm the global cache so the elapsed-time assertion measures pure
        // cancellation latency rather than first-time shell-env capture (which
        // can take seconds under parallel-test load).
        let _ = shell_env_cache()
            .get(&std::env::temp_dir())
            .await
            .expect("warm cache");

        let cancel_token = CancellationToken::new();
        let cancel_after_start = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel_after_start.cancel();
        });

        let started = std::time::Instant::now();
        let result =
            run_pipeline_command("sleep 5 & wait", &std::env::temp_dir(), &cancel_token).await;

        assert!(matches!(
            result,
            Ok(PipelineCommandResult::Cancelled { .. })
        ));
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    // ---------------------------------------------------------------------
    // Integration: `run_pipeline_command_with_cache` snapshot/fallback paths
    //
    // These tests inject a hermetic `ShellEnvCache` via the test seam so
    // pipeline behaviour can be exercised without depending on the
    // developer's `$SHELL` or `.zshrc`.
    // ---------------------------------------------------------------------

    /// Write `content` to a 0755 tempfile suitable for use as `$SHELL`.
    ///
    /// Returns a `TempPath` (not `NamedTempFile`) so the writable fd is closed
    /// before the script gets exec'd: Linux refuses to exec a file whose inode
    /// still has `i_writecount > 0` and returns ETXTBSY. macOS has no such
    /// check, which is why the prior `NamedTempFile` shape passed locally but
    /// failed on Linux CI.
    #[cfg(unix)]
    fn write_fake_shell(content: &str) -> tempfile::TempPath {
        use std::os::unix::fs::PermissionsExt;
        let temp_path = tempfile::Builder::new()
            .prefix("staged-fake-shell-")
            .suffix(".sh")
            .tempfile()
            .expect("create fake shell tempfile")
            .into_temp_path();
        std::fs::write(&temp_path, content).expect("write script");
        let mut perms = std::fs::metadata(&temp_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&temp_path, perms).expect("chmod 755");
        temp_path
    }

    /// G20: When the cache produces a snapshot, its env vars reach the child
    /// process spawned for the pipeline step.
    #[cfg(unix)]
    #[tokio::test]
    async fn snapshot_path_cached_env_reaches_child() {
        let shell = write_fake_shell(
            "#!/bin/sh\nPATH=/usr/bin:/bin\nPIPELINE_TEST_TOKEN=snapshot-marker-abc\nexport PATH PIPELINE_TEST_TOKEN\nexec /bin/sh -s\n",
        );
        let cache =
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600));
        let dir = tempfile::tempdir().expect("tempdir");

        let cancel = CancellationToken::new();
        let result = run_pipeline_command_with_cache(
            &cache,
            "echo $PIPELINE_TEST_TOKEN",
            &dir.path().to_path_buf(),
            &cancel,
        )
        .await
        .expect("run_pipeline_command_with_cache should succeed");

        let output = match result {
            PipelineCommandResult::Completed(o) => o,
            PipelineCommandResult::Cancelled { .. } => panic!("unexpected cancellation"),
        };
        assert!(output.status.success(), "command should succeed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("snapshot-marker-abc"),
            "child must see PIPELINE_TEST_TOKEN from the snapshot; stdout={stdout:?}"
        );
    }

    /// G21: When the cache returns `Err`, `run_pipeline_command_with_cache`
    /// falls back to `sh -lc` and the command still runs.
    #[cfg(unix)]
    #[tokio::test]
    async fn fallback_path_when_cache_returns_err() {
        let shell = write_fake_shell("#!/bin/sh\nexit 1\n");
        let cache =
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600));
        let dir = std::env::temp_dir();

        let cancel = CancellationToken::new();
        let result = run_pipeline_command_with_cache(&cache, "echo fallback-ok", &dir, &cancel)
            .await
            .expect("fallback path should still spawn and run");

        let output = match result {
            PipelineCommandResult::Completed(o) => o,
            PipelineCommandResult::Cancelled { .. } => panic!("unexpected cancellation"),
        };
        assert!(output.status.success(), "fallback sh -lc should succeed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("fallback-ok"),
            "fallback should still produce the echo output; stdout={stdout:?}"
        );
    }

    /// G22: Cancellation still terminates the child even after a snapshot is
    /// applied — guards against a future refactor that loses `kill_on_drop`
    /// or the cancellation `select!` arm.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_under_snapshot_branch() {
        let shell =
            write_fake_shell("#!/bin/sh\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n");
        let cache =
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600));
        let dir = std::env::temp_dir();

        let cancel_token = CancellationToken::new();
        let cancel_after_start = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel_after_start.cancel();
        });

        let started = std::time::Instant::now();
        let result =
            run_pipeline_command_with_cache(&cache, "sleep 5 & wait", &dir, &cancel_token).await;
        assert!(matches!(
            result,
            Ok(PipelineCommandResult::Cancelled { .. })
        ));
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    /// G23: `current_dir` survives `apply_to` — `pwd` reports the directory
    /// passed to `run_pipeline_command_with_cache`, not the test's cwd.
    #[cfg(unix)]
    #[tokio::test]
    async fn current_dir_survives_apply_to() {
        let shell =
            write_fake_shell("#!/bin/sh\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n");
        let cache =
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600));
        let dir = tempfile::tempdir().expect("tempdir");
        let resolved = std::fs::canonicalize(dir.path()).unwrap_or_else(|_| dir.path().to_owned());

        let cancel = CancellationToken::new();
        let result = run_pipeline_command_with_cache(&cache, "pwd", &resolved, &cancel)
            .await
            .expect("pwd should succeed");
        let output = match result {
            PipelineCommandResult::Completed(o) => o,
            PipelineCommandResult::Cancelled { .. } => panic!("unexpected cancellation"),
        };
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let reported = stdout.trim();
        let reported_path = std::fs::canonicalize(PathBuf::from(reported))
            .unwrap_or_else(|_| PathBuf::from(reported));
        assert_eq!(
            reported_path, resolved,
            "child should run in the requested working_dir; pwd reported {reported:?}"
        );
    }

    #[test]
    fn pipeline_command_output_collapses_progress_for_prompt() {
        let output = combine_normalized_command_output(b"10%\r20%\rdone\n", b"");
        assert_eq!(output, "done");

        let prompt_output =
            format_step_output_for_prompt("Build", "just build --verbose", &output, false);
        assert!(prompt_output.contains("### Build\nCommand: just build --verbose\n```\ndone\n```"));
        let command_position = prompt_output.find("Command: just build --verbose").unwrap();
        let output_position = prompt_output.find("done").unwrap();
        assert!(command_position < output_position);
        assert!(prompt_output.contains("```\ndone\n```"));
        assert!(!prompt_output.contains("10%"));
        assert!(!prompt_output.contains("20%"));
    }

    #[test]
    fn failed_pipeline_handoff_start_cleans_running_state() {
        let store = Store::in_memory().unwrap();
        let session = crate::store::Session::new_running("handoff", std::path::Path::new("/tmp"));
        store.create_session(&session).unwrap();

        let registry = SessionRegistry::new();
        registry.register(&session.id);
        assert!(registry.is_running(&session.id));

        finish_failed_pipeline_handoff_start(
            &store,
            &registry,
            &session.id,
            "provider unavailable",
        );

        assert!(!registry.is_running(&session.id));
        let failed = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(failed.status, SessionStatus::Error);
        assert_eq!(
            failed.error_message.as_deref(),
            Some("provider unavailable")
        );
        assert_eq!(failed.completion_reason, Some(CompletionReason::Crashed));
    }

    #[test]
    fn running_project_session_cancellation_records_completion_reason_override() {
        let registry = SessionRegistry::new();
        registry.register("session-1");
        registry.register("session-2");
        registry.register("session-3");

        assert!(registry.cancel_with_completion_reason(
            "session-1",
            CompletionReason::ProjectSessionInterrupted
        ));
        assert!(registry.cancel("session-2"));
        assert!(registry.cancel("session-3"));
        assert!(registry.cancel("session-1"));
        assert!(registry.cancel_with_completion_reason(
            "session-2",
            CompletionReason::ProjectSessionInterrupted
        ));

        assert_eq!(
            registry.cancellation_completion_reason("session-1"),
            Some(CompletionReason::ProjectSessionInterrupted)
        );
        assert_eq!(
            registry.cancellation_completion_reason("session-2"),
            Some(CompletionReason::ProjectSessionInterrupted)
        );
        assert_eq!(
            registry.cancellation_completion_reason("session-3"),
            Some(CompletionReason::Interrupted)
        );
    }

    fn make_git_repo(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "staged-{test_name}-{}",
            crate::store::now_timestamp()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init"]);
        run_git(&dir, &["config", "user.email", "test@example.com"]);
        run_git(&dir, &["config", "user.name", "Test User"]);
        std::fs::write(dir.join("file.txt"), "one\n").unwrap();
        run_git(&dir, &["add", "file.txt"]);
        run_git(&dir, &["commit", "-m", "initial"]);
        dir
    }

    fn run_git(dir: &std::path::Path, args: &[&str]) -> String {
        let mut command = std::process::Command::new("git");
        command.args(args).current_dir(dir);
        strip_git_env(&mut command);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn rebase_pipeline_config(
        session_id: &str,
        repo: &std::path::Path,
        pre_head: &str,
    ) -> PipelineConfig {
        PipelineConfig {
            session_id: session_id.to_string(),
            prompt: "Rebase branch".to_string(),
            steps: vec![],
            pipeline: PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Rebase),
            working_dir: repo.to_path_buf(),
            pre_head_sha: Some(pre_head.to_string()),
            provider: None,
            workspace_name: None,
            remote_working_dir: None,
            branch_id: None,
            project_id: None,
        }
    }

    #[test]
    fn rebase_pipeline_completion_updates_pending_commit_when_head_changes() {
        let repo = make_git_repo("rebase-updates-pending");
        let pre_head = run_git(&repo, &["rev-parse", "HEAD"]);
        let store = Store::in_memory().unwrap();
        let project = crate::store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = crate::store::Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();
        let mut session = crate::store::Session::new_running("Rebase branch", &repo);
        session.pipeline = Some(PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Rebase));
        store.create_session(&session).unwrap();
        let commit = crate::store::Commit::new_pending(&branch.id).with_session(&session.id);
        store.create_commit(&commit).unwrap();

        std::fs::write(repo.join("file.txt"), "two\n").unwrap();
        run_git(&repo, &["add", "file.txt"]);
        run_git(&repo, &["commit", "-m", "second"]);
        let new_head = run_git(&repo, &["rev-parse", "HEAD"]);

        finalize_rebase_pipeline_without_ai(
            &rebase_pipeline_config(&session.id, &repo, &pre_head),
            &store,
        );

        let updated = store.get_commit(&commit.id).unwrap().unwrap();
        assert_eq!(updated.sha.as_deref(), Some(new_head.as_str()));
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn rebase_pipeline_completion_deletes_noop_pending_commit() {
        let repo = make_git_repo("rebase-noop");
        let pre_head = run_git(&repo, &["rev-parse", "HEAD"]);
        let store = Store::in_memory().unwrap();
        let project = crate::store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = crate::store::Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();
        let mut session = crate::store::Session::new_running("Rebase branch", &repo);
        session.pipeline = Some(PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Rebase));
        store.create_session(&session).unwrap();
        let commit = crate::store::Commit::new_pending(&branch.id).with_session(&session.id);
        store.create_commit(&commit).unwrap();

        finalize_rebase_pipeline_without_ai(
            &rebase_pipeline_config(&session.id, &repo, &pre_head),
            &store,
        );

        assert!(store.get_commit(&commit.id).unwrap().is_none());
        let _ = std::fs::remove_dir_all(repo);
    }

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
    fn note_content_strips_inline_suggested_steps_before_hr_with_code_fence_body() {
        let text = r#"I focused the plan on the parser and tests.```suggested-next-steps
{"suggestedNextCommitStep":"Fix note parsing","suggestedNextNoteStep":null}
```
---
# Harden Note Detection
Strip metadata before scanning for the note separator.

```rust
fn example() {}
```

Keep normal markdown fences in the note body."#;
        let content = extract_note_content(text);
        assert_eq!(
            content,
            Some(
                "# Harden Note Detection\nStrip metadata before scanning for the note separator.\n\n```rust\nfn example() {}\n```\n\nKeep normal markdown fences in the note body.".to_string()
            )
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

    // ── extract_suggested_next_steps ────────────────────────────────────

    #[test]
    fn extract_steps_valid_both_fields() {
        let text = r#"Here are the notes.

```suggested-next-steps
{"suggestedNextCommitStep": "Implement the plan", "suggestedNextNoteStep": "Research alternatives"}
```

---
# Title
Body
"#;
        let steps = extract_suggested_next_steps(text).unwrap();
        assert_eq!(
            steps.suggested_next_commit_step.as_deref(),
            Some("Implement the plan")
        );
        assert_eq!(
            steps.suggested_next_note_step.as_deref(),
            Some("Research alternatives")
        );
    }

    #[test]
    fn extract_steps_valid_inline_opening_fence() {
        let text = "Ready to return the note.```suggested-next-steps\n{\"suggestedNextCommitStep\": \"Reduce churn\", \"suggestedNextNoteStep\": \"Plan IPC fix\"}\n```\n";
        let steps = extract_suggested_next_steps(text).unwrap();
        assert_eq!(
            steps.suggested_next_commit_step.as_deref(),
            Some("Reduce churn")
        );
        assert_eq!(
            steps.suggested_next_note_step.as_deref(),
            Some("Plan IPC fix")
        );
    }

    #[test]
    fn extract_steps_null_fields() {
        let text = "```suggested-next-steps\n{\"suggestedNextCommitStep\": null, \"suggestedNextNoteStep\": null}\n```\n";
        let steps = extract_suggested_next_steps(text).unwrap();
        assert_eq!(steps.suggested_next_commit_step, None);
        assert_eq!(steps.suggested_next_note_step, None);
    }

    #[test]
    fn extract_steps_partial_fields() {
        let text = "```suggested-next-steps\n{\"suggestedNextCommitStep\": \"Fix the bug\", \"suggestedNextNoteStep\": null}\n```\n";
        let steps = extract_suggested_next_steps(text).unwrap();
        assert_eq!(
            steps.suggested_next_commit_step.as_deref(),
            Some("Fix the bug")
        );
        assert_eq!(steps.suggested_next_note_step, None);
    }

    #[test]
    fn extract_steps_missing_block() {
        let text = "Just a normal assistant message with no fenced blocks.";
        assert!(extract_suggested_next_steps(text).is_none());
    }

    #[test]
    fn extract_steps_malformed_json() {
        let text = "```suggested-next-steps\n{not valid json}\n```\n";
        assert!(extract_suggested_next_steps(text).is_none());
    }
}
