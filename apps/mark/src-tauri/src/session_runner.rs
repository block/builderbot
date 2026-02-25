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
use crate::store::{Comment, CommentAuthor, CommentType, MessageRole, SessionStatus, Store};

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
    /// Set on `"running"` events emitted when an MCP tool starts a repo session,
    /// so the frontend can register the session and refresh the branch timeline.
    pub branch_id: Option<String>,
    pub project_id: Option<String>,
    pub session_type: Option<String>,
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
        AcpDriver::for_workspace(ws_name, config.provider.as_deref())?
    } else {
        match &config.provider {
            Some(id) => AcpDriver::new(id)?,
            None => AcpDriver::first_available()?,
        }
    };

    // Persist the user message right away so it's visible immediately.
    store
        .add_session_message(&config.session_id, MessageRole::User, &config.prompt)
        .map_err(|e| format!("Failed to persist user message: {e}"))?;

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

            // Cast to trait objects for the driver
            let store_trait: Arc<dyn acp_client::Store> = store;
            let writer_trait: Arc<dyn acp_client::MessageWriter> = writer;

            driver
                .run(
                    &config.session_id,
                    &config.prompt,
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
        let (new_status, error_msg) = match result {
            Ok(()) if cancel_token.is_cancelled() => ("cancelled", None),
            Ok(()) => ("completed", None),
            Err(ref e) if cancel_token.is_cancelled() => {
                log::info!(
                    "Session {session_id_for_status} cancelled (error during teardown: {e})"
                );
                ("cancelled", None)
            }
            Err(ref e) => {
                log::error!("Session {session_id_for_status} failed: {e}");
                ("error", Some(e.clone()))
            }
        };

        // Run post-completion hooks before transitioning status.
        // These detect artifacts produced by the session (commits, notes).
        if new_status == "completed" {
            run_post_completion_hooks(
                &session_id_for_status,
                &config.working_dir,
                config.pre_head_sha.as_deref(),
                config.workspace_name.as_deref(),
                &store_for_status,
            );
        }

        let status_enum = SessionStatus::parse(new_status).unwrap();
        let transitioned = store_for_status
            .transition_from_running(&session_id_for_status, status_enum, error_msg.as_deref())
            .unwrap_or(false);

        if transitioned {
            emit_status(&app_handle, &session_id_for_status, new_status, error_msg);
        }
    });

    Ok(())
}

// =============================================================================
// Orphaned session cleanup
// =============================================================================

/// On startup, cancel any sessions whose owner process is no longer alive.
///
/// Each session records the PID of the Mark process that started it
/// (`owner_pid`). On startup we check each running session:
/// - `owner_pid` is our own PID → shouldn't happen at startup, skip.
/// - `owner_pid` belongs to a live process → another Mark instance owns
///   it; leave it alone.
/// - `owner_pid` is dead (or NULL for pre-migration rows) → cancel and emit
///   `session-status-changed` so the frontend learns the outcome.
pub fn cancel_dead_sessions(store: Arc<Store>, app_handle: AppHandle) {
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
                .transition_from_running(&session.id, SessionStatus::Cancelled, None)
                .unwrap_or(false);
            if transitioned {
                emit_status(&app_handle, &session.id, "cancelled", None);
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
fn run_post_completion_hooks(
    session_id: &str,
    working_dir: &std::path::Path,
    pre_head_sha: Option<&str>,
    workspace_name: Option<&str>,
    store: &Arc<Store>,
) {
    // --- Commit detection ---
    if let Some(pre_sha) = pre_head_sha {
        if let Ok(Some(pending_commit)) = store.get_pending_commit_by_session(session_id) {
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
                    if let Err(e) = store.update_commit_sha(&pending_commit.id, &current_head) {
                        log::error!("Failed to update commit SHA: {e}");
                    }
                }
                Ok(_) => {
                    log::info!("Session {session_id}: no new commit (HEAD unchanged), leaving pending commit as failed");
                }
                Err(e) => {
                    log::error!("Failed to get HEAD SHA after session: {e}");
                }
            }
        }
    }

    // --- Note extraction ---
    if let Ok(Some(empty_note)) = store.get_empty_note_by_session(session_id) {
        // Collect all assistant messages for this session and find note content.
        if let Ok(messages) = store.get_session_messages(session_id) {
            // Concatenate all assistant messages (the note content could span
            // multiple assistant message chunks if the model was interrupted
            // by tool calls and resumed).
            let full_text: String = messages
                .iter()
                .filter(|m| m.role == MessageRole::Assistant)
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            if let Some(note_content) = extract_note_content(&full_text) {
                let (title, body) = extract_note_title(&note_content);
                let final_title = if title.is_empty() {
                    // Fallback title from the session prompt
                    store
                        .get_session(session_id)
                        .ok()
                        .flatten()
                        .map(|s| {
                            let t: String = s.prompt.chars().take(80).collect();
                            if s.prompt.len() > 80 {
                                format!("{t}…")
                            } else {
                                t
                            }
                        })
                        .unwrap_or_else(|| "Untitled Note".to_string())
                } else {
                    title
                };
                log::info!("Session {session_id}: extracted note \"{final_title}\"");
                if let Err(e) =
                    store.update_note_title_and_content(&empty_note.id, &final_title, &body)
                {
                    log::error!("Failed to update note content: {e}");
                }
            } else {
                log::warn!("Session {session_id}: note session completed but no --- found in assistant output");
            }
        }
    }

    // --- Project note extraction ---
    if let Ok(Some(empty_note)) = store.get_empty_project_note_by_session(session_id) {
        if let Ok(messages) = store.get_session_messages(session_id) {
            let full_text: String = messages
                .iter()
                .filter(|m| m.role == MessageRole::Assistant)
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            if let Some(note_content) = extract_note_content(&full_text) {
                let (title, body) = extract_note_title(&note_content);
                let final_title = if title.is_empty() {
                    store
                        .get_session(session_id)
                        .ok()
                        .flatten()
                        .map(|s| {
                            let t: String = s.prompt.chars().take(80).collect();
                            if s.prompt.len() > 80 {
                                format!("{t}…")
                            } else {
                                t
                            }
                        })
                        .unwrap_or_else(|| "Untitled Note".to_string())
                } else {
                    title
                };
                log::info!("Session {session_id}: extracted project note \"{final_title}\"");
                if let Err(e) =
                    store.update_project_note_title_and_content(&empty_note.id, &final_title, &body)
                {
                    log::error!("Failed to update project note content: {e}");
                }
            } else {
                log::warn!("Session {session_id}: project note session completed but no --- found in assistant output");
            }
        }
    }

    // --- Review comment extraction ---
    if let Ok(Some(review)) = store.get_review_by_session(session_id) {
        if review.comments.is_empty() {
            if let Ok(messages) = store.get_session_messages(session_id) {
                let full_text: String = messages
                    .iter()
                    .filter(|m| m.role == MessageRole::Assistant)
                    .map(|m| m.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");

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
}

/// Extract note content from assistant output.
///
/// Primary path: find the first markdown horizontal rule (`---`, `***`, `___`)
/// on its own line and return everything after it.
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
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            // Everything after this line.
            let remaining: String = text.lines().skip(i + 1).collect::<Vec<_>>().join("\n");
            let trimmed_remaining = remaining.trim().to_string();
            if !trimmed_remaining.is_empty() {
                return Some(trimmed_remaining);
            }
        }
    }
    None
}

fn extract_note_after_inline_hr(text: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;

    for marker in ["---", "***", "___"] {
        let marker_char = marker.chars().next().unwrap();
        for (idx, _) in text.match_indices(marker) {
            let marker_end = idx + marker.len();

            // Ignore markers that are part of longer runs like ----.
            if text[..idx].ends_with(marker_char) || text[marker_end..].starts_with(marker_char) {
                continue;
            }

            let remaining = text[marker_end..].trim_start();
            if !remaining.starts_with("# ") {
                continue;
            }

            match best {
                Some((best_idx, _)) if idx >= best_idx => {}
                _ => best = Some((idx, remaining.to_string())),
            }
        }
    }

    best.map(|(_, content)| content)
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

/// Extract review comments from assistant output.
///
/// Looks for ```review-comments fenced blocks and parses the JSON array inside.
/// Each object should have `path`, `span` (with `start` and `end`), and `content`.
fn extract_review_comments(text: &str) -> Vec<Comment> {
    let mut comments = Vec::new();

    // Find all ```review-comments blocks
    let marker_start = "```review-comments";

    let mut search_from = 0;
    while let Some(start_pos) = text[search_from..].find(marker_start) {
        let block_start = search_from + start_pos + marker_start.len();
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

fn emit_status(app_handle: &AppHandle, session_id: &str, status: &str, error: Option<String>) {
    let event = SessionStatusEvent {
        session_id: session_id.to_string(),
        status: status.to_string(),
        error_message: error,
        branch_id: None,
        project_id: None,
        session_type: None,
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
        branch_id: Some(branch_id.to_string()),
        project_id: Some(project_id.to_string()),
        session_type: Some(session_type.to_string()),
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
}
