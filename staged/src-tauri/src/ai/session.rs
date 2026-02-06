//! Session Manager - manages live ACP agent connections.
//!
//! This is a thin layer that:
//! - Tracks live agent subprocess connections
//! - Buffers the current streaming turn
//! - On turn complete, persists to Store
//!
//! History is stored in SQLite via Store. This module only handles
//! live state that can't be persisted (agent connections, streaming buffers).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

use super::client::{self, AcpAgent, AcpPromptResult};
use crate::store::{generate_session_id, ContentSegment, MessageRole, Session, Store};

// =============================================================================
// Types
// =============================================================================

/// Status of a live session
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum SessionStatus {
    /// Session is idle, ready for prompts
    Idle,
    /// Session is processing a prompt
    Processing,
    /// Session encountered an error
    Error { message: String },
    /// Session was cancelled by the user
    Cancelled,
}

/// Event emitted when session status changes
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusEvent {
    pub session_id: String,
    pub status: SessionStatus,
}

/// Info about a live session (for the frontend)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSessionInfo {
    pub session_id: String,
    pub status: SessionStatus,
}

/// Cancellation handle for an active session.
/// Shared between the session manager and the running task.
#[derive(Debug, Default)]
pub struct CancellationHandle {
    /// Set to true when cancellation is requested
    cancelled: AtomicBool,
    /// PID of the agent subprocess (0 if not yet spawned)
    pid: AtomicU32,
}

impl CancellationHandle {
    pub fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            pid: AtomicU32::new(0),
        }
    }

    /// Request cancellation of the session
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);

        // Kill the subprocess if we have a PID
        let pid = self.pid.load(Ordering::SeqCst);
        if pid != 0 {
            log::info!("Killing agent subprocess with PID {pid}");
            #[cfg(unix)]
            {
                // Send SIGTERM to the process using the kill command
                let _ = std::process::Command::new("kill")
                    .args(["-TERM", &pid.to_string()])
                    .output();
            }
            #[cfg(windows)]
            {
                // On Windows, use taskkill
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .output();
            }
        }
    }

    /// Check if cancellation was requested
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Set the PID of the agent subprocess
    pub fn set_pid(&self, pid: u32) {
        self.pid.store(pid, Ordering::SeqCst);
    }
}

/// Internal live session state
struct LiveSession {
    /// Our session ID (matches Store)
    session_id: String,
    /// ACP session ID (from the agent - for session resumption)
    acp_session_id: Option<String>,
    /// Agent being used
    agent: AcpAgent,
    /// Working directory
    working_dir: PathBuf,
    /// Current status
    status: SessionStatus,
    /// Cancellation handle for the current operation (if any)
    cancellation: Option<Arc<CancellationHandle>>,
}

// =============================================================================
// Session Manager
// =============================================================================

/// Manages live ACP agent connections
pub struct SessionManager {
    /// Live sessions by our session ID
    sessions: RwLock<HashMap<String, Arc<RwLock<LiveSession>>>>,
    /// Tauri app handle for emitting events
    app_handle: AppHandle,
    /// Store for persistence
    store: Arc<Store>,
    /// In-memory buffer for streaming messages (session_id -> segments)
    /// Stores messages as they arrive during streaming, before DB persistence
    streaming_buffer: Arc<RwLock<HashMap<String, Vec<ContentSegment>>>>,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new(app_handle: AppHandle, store: Arc<Store>) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            app_handle,
            store,
            streaming_buffer: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new session (persisted + live)
    pub async fn create_session(
        &self,
        working_dir: PathBuf,
        agent_id: Option<&str>,
    ) -> Result<String, String> {
        // Find the agent
        let agent = if let Some(id) = agent_id {
            client::find_acp_agent_by_id(id).ok_or_else(|| format!("Agent '{id}' not found"))?
        } else {
            client::find_acp_agent().ok_or_else(|| "No AI agent found".to_string())?
        };

        // Generate session ID and create in store
        let session_id = generate_session_id();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let session = Session {
            id: session_id.clone(),
            working_dir: working_dir.to_string_lossy().to_string(),
            agent_id: agent.name().to_string(),
            title: None,
            created_at: now,
            updated_at: now,
        };

        self.store
            .create_session(&session)
            .map_err(|e| format!("Failed to create session: {e}"))?;

        // Create live session
        let live_session = LiveSession {
            session_id: session_id.clone(),
            acp_session_id: None,
            agent,
            working_dir,
            status: SessionStatus::Idle,
            cancellation: None,
        };

        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id.clone(), Arc::new(RwLock::new(live_session)));

        log::info!("Created session: {session_id}");
        Ok(session_id)
    }

    /// Get or create a live session for an existing persisted session
    async fn get_or_create_live_session(
        &self,
        session_id: &str,
    ) -> Result<Arc<RwLock<LiveSession>>, String> {
        // Check if already live
        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(session_id) {
                return Ok(session.clone());
            }
        }

        // Load from store and create live session
        let session = self
            .store
            .get_session(session_id)
            .map_err(|e| format!("Failed to load session: {e}"))?
            .ok_or_else(|| format!("Session '{session_id}' not found"))?;

        let agent = client::find_acp_agent_by_id(&session.agent_id)
            .or_else(client::find_acp_agent)
            .ok_or_else(|| "No AI agent found".to_string())?;

        let live_session = LiveSession {
            session_id: session_id.to_string(),
            acp_session_id: None, // Will be set on first prompt
            agent,
            working_dir: PathBuf::from(&session.working_dir),
            status: SessionStatus::Idle,
            cancellation: None,
        };

        let arc = Arc::new(RwLock::new(live_session));
        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id.to_string(), arc.clone());

        Ok(arc)
    }

    /// List live sessions
    pub async fn list_live_sessions(&self) -> Vec<LiveSessionInfo> {
        let sessions = self.sessions.read().await;
        let mut infos = Vec::with_capacity(sessions.len());

        for session in sessions.values() {
            let s = session.read().await;
            infos.push(LiveSessionInfo {
                session_id: s.session_id.clone(),
                status: s.status.clone(),
            });
        }

        infos
    }

    /// Get status of a session (live or from store)
    pub async fn get_session_status(&self, session_id: &str) -> Result<SessionStatus, String> {
        let sessions = self.sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            let s = session.read().await;
            return Ok(s.status.clone());
        }

        // Not live - check if it exists in store
        self.store
            .get_session(session_id)
            .map_err(|e| format!("Failed to load session: {e}"))?
            .ok_or_else(|| format!("Session '{session_id}' not found"))?;

        // Exists but not live = idle
        Ok(SessionStatus::Idle)
    }

    /// Check if a session has a live connection (is in the sessions HashMap).
    /// This is different from get_session_status which returns Idle for sessions
    /// that exist in the store but aren't live.
    pub async fn is_session_live(&self, session_id: &str) -> bool {
        let sessions = self.sessions.read().await;
        sessions.contains_key(session_id)
    }

    /// Close a live session (keeps history in store)
    pub async fn close_live_session(&self, session_id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
        log::info!("Closed live session: {session_id}");
        Ok(())
    }

    /// Send a prompt to a session
    pub async fn send_prompt(&self, session_id: &str, prompt: String) -> Result<(), String> {
        // Get or create live session
        let session_arc = self.get_or_create_live_session(session_id).await?;

        // Create cancellation handle for this operation
        let cancellation = Arc::new(CancellationHandle::new());

        // Check status and prepare for prompt
        let (agent, working_dir, acp_session_id) = {
            let mut session = session_arc.write().await;

            if session.status == SessionStatus::Processing {
                return Err("Session is already processing a prompt".to_string());
            }

            // Update status to processing and store cancellation handle
            session.status = SessionStatus::Processing;
            session.cancellation = Some(cancellation.clone());
            self.emit_status(&session.session_id, &session.status);

            (
                session.agent.clone(),
                session.working_dir.clone(),
                session.acp_session_id.clone(),
            )
        };

        // Store the user message
        self.store
            .add_message(session_id, MessageRole::User, &prompt)
            .map_err(|e| format!("Failed to store message: {e}"))?;

        // Spawn background task to run the prompt
        let app_handle = self.app_handle.clone();
        let session_id_owned = session_id.to_string();
        let session_arc_clone = session_arc.clone();
        let store = self.store.clone();
        let streaming_buffer = Arc::clone(&self.streaming_buffer);

        // Create callback to update buffer during streaming
        let session_id_for_callback = session_id_owned.clone();
        let buffer_for_callback = Arc::clone(&self.streaming_buffer);
        let buffer_callback = Arc::new(move |segments: Vec<ContentSegment>| {
            let session_id = session_id_for_callback.clone();
            let buffer = Arc::clone(&buffer_for_callback);
            // Spawn a task to update the buffer asynchronously
            tokio::spawn(async move {
                let mut buffer = buffer.write().await;
                buffer.insert(session_id, segments);
            });
        });

        tokio::spawn(async move {
            // Run the ACP prompt with streaming
            let result = client::run_acp_prompt_streaming(
                &agent,
                &working_dir,
                &prompt,
                acp_session_id.as_deref(),
                &session_id_owned,
                app_handle.clone(),
                Some(buffer_callback),
                Some(cancellation.clone()),
            )
            .await;

            // Update session and persist based on result
            let mut session = session_arc_clone.write().await;

            // Clear the cancellation handle
            session.cancellation = None;

            // Check if we were cancelled
            if cancellation.is_cancelled() {
                log::info!("Session {session_id_owned} was cancelled");
                session.status = SessionStatus::Cancelled;
                // Clear buffer on cancellation
                let mut buffer = streaming_buffer.write().await;
                buffer.remove(&session_id_owned);
            } else {
                match result {
                    Ok(acp_result) => {
                        // Store the ACP session ID for future resumption
                        session.acp_session_id = Some(acp_result.session_id.clone());
                        session.status = SessionStatus::Idle;

                        // Persist the assistant response
                        if let Err(e) =
                            persist_assistant_turn(&store, &session_id_owned, &acp_result)
                        {
                            log::error!("Failed to persist assistant turn: {e}");
                        }

                        // Clear buffer after persistence attempt (success or failure)
                        // The callback has been updating the buffer during streaming
                        let mut buffer = streaming_buffer.write().await;
                        buffer.remove(&session_id_owned);

                        // Auto-generate title from first user message if not set
                        if let Err(e) = maybe_set_title(&store, &session_id_owned, &prompt) {
                            log::warn!("Failed to set session title: {e}");
                        }
                    }
                    Err(e) => {
                        log::error!("Session {session_id_owned} prompt failed: {e}");
                        session.status = SessionStatus::Error { message: e };
                        // Clear buffer on error too
                        let mut buffer = streaming_buffer.write().await;
                        buffer.remove(&session_id_owned);
                    }
                }
            }

            // Emit status change
            let event = SessionStatusEvent {
                session_id: session_id_owned,
                status: session.status.clone(),
            };
            let _ = app_handle.emit("session-status", &event);
        });

        Ok(())
    }

    /// Cancel an active session by killing the agent subprocess
    pub async fn cancel_session(&self, session_id: &str) -> Result<(), String> {
        let sessions = self.sessions.read().await;
        let session_arc = sessions
            .get(session_id)
            .ok_or_else(|| format!("Session '{session_id}' not found"))?;

        let session = session_arc.read().await;

        if session.status != SessionStatus::Processing {
            return Err("Session is not currently processing".to_string());
        }

        if let Some(ref cancellation) = session.cancellation {
            log::info!("Cancelling session {session_id}");
            cancellation.cancel();
            Ok(())
        } else {
            Err("No active operation to cancel".to_string())
        }
    }

    fn emit_status(&self, session_id: &str, status: &SessionStatus) {
        let event = SessionStatusEvent {
            session_id: session_id.to_string(),
            status: status.clone(),
        };
        let _ = self.app_handle.emit("session-status", &event);
    }

    /// Get buffered streaming segments for a session (before DB persistence).
    ///
    /// Returns:
    /// - Some(segments): Session is actively streaming, these are the latest segments
    /// - None: No buffered segments (either already persisted or never started)
    ///
    /// Note: If None, check the database - segments may have already been persisted.
    pub async fn get_buffered_segments(&self, session_id: &str) -> Option<Vec<ContentSegment>> {
        let buffer = self.streaming_buffer.read().await;
        buffer.get(session_id).cloned()
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Persist an assistant turn to the store
fn persist_assistant_turn(
    store: &Store,
    session_id: &str,
    result: &AcpPromptResult,
) -> Result<(), String> {
    // Store segments directly - they preserve interleaving order
    store
        .add_assistant_turn(session_id, &result.segments)
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Set session title from first prompt if not already set
fn maybe_set_title(store: &Store, session_id: &str, prompt: &str) -> Result<(), String> {
    let session = store
        .get_session(session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Session not found".to_string())?;

    if session.title.is_some() {
        return Ok(());
    }

    // Generate title from first ~50 chars of prompt, first line only
    let first_line = prompt.lines().next().unwrap_or("");
    let truncated: String = first_line.chars().take(50).collect();
    let needs_ellipsis = first_line.chars().count() > 50;

    let title = if needs_ellipsis {
        format!("{}...", truncated.trim())
    } else {
        truncated.trim().to_string()
    };

    if !title.is_empty() {
        store
            .update_session_title(session_id, &title)
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}
