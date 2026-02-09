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

use crate::agent::acp::AcpDriver;
use crate::agent::writer::MessageWriter;
use crate::agent::AgentDriver;
use crate::store::{MessageRole, SessionStatus, Store};

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
        self.running
            .lock()
            .unwrap()
            .insert(session_id.to_string(), token.clone());
        token
    }

    /// Remove a session from the registry (called by the background thread
    /// on exit, regardless of success/failure/cancellation).
    fn deregister(&self, session_id: &str) {
        self.running.lock().unwrap().remove(session_id);
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
    // Create the driver eagerly so we fail fast if goose isn't found.
    let driver = AcpDriver::new()?;

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
            let writer = Arc::new(MessageWriter::new(
                config.session_id.clone(),
                Arc::clone(&store),
            ));

            driver
                .run(
                    &config.session_id,
                    &config.prompt,
                    &config.working_dir,
                    &store,
                    &writer,
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
// Post-completion hooks
// =============================================================================

/// Detect artifacts produced by a session and update the DB accordingly.
///
/// - **Commits**: If a pending commit record is linked to this session and
///   HEAD has moved since the session started, record the new SHA.
/// - **Notes**: If an empty note is linked to this session, parse the
///   assistant's last message for content after the first `---`.
fn run_post_completion_hooks(
    session_id: &str,
    working_dir: &std::path::Path,
    pre_head_sha: Option<&str>,
    store: &Arc<Store>,
) {
    // --- Commit detection ---
    if let Some(pre_sha) = pre_head_sha {
        if let Ok(Some(pending_commit)) = store.get_pending_commit_by_session(session_id) {
            match crate::git::get_head_sha(working_dir) {
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
                    log::info!("Session {session_id}: no new commit (HEAD unchanged), cleaning up pending commit");
                    let _ = store.delete_commit(&pending_commit.id);
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
}

/// Extract note content from assistant output.
///
/// Looks for the first `---` (horizontal rule) on its own line and returns
/// everything after it. Returns `None` if no rule is found.
fn extract_note_content(text: &str) -> Option<String> {
    // Look for --- on its own line (possibly with surrounding whitespace).
    // We match the same patterns markdown parsers treat as thematic breaks:
    // a line containing only ---, ***, or ___ (with optional spaces).
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            // Everything after this line
            let remaining: String = text.lines().skip(i + 1).collect::<Vec<_>>().join("\n");
            let trimmed_remaining = remaining.trim().to_string();
            if !trimmed_remaining.is_empty() {
                return Some(trimmed_remaining);
            }
        }
    }
    None
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

fn emit_status(app_handle: &AppHandle, session_id: &str, status: &str, error: Option<String>) {
    let event = SessionStatusEvent {
        session_id: session_id.to_string(),
        status: status.to_string(),
        error_message: error,
    };
    if let Err(e) = app_handle.emit("session-status-changed", &event) {
        log::warn!("Failed to emit session-status-changed: {e}");
    }
}
