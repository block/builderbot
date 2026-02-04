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
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

use super::client::{self, AcpAgent, AcpPromptResult};
use crate::store::{generate_session_id, MessageRole, Session, Store};

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
}

impl SessionManager {
    /// Create a new session manager
    pub fn new(app_handle: AppHandle, store: Arc<Store>) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            app_handle,
            store,
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
            client::find_acp_agent_by_id(id).ok_or_else(|| format!("Agent '{}' not found", id))?
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
            .map_err(|e| format!("Failed to create session: {}", e))?;

        // Create live session
        let live_session = LiveSession {
            session_id: session_id.clone(),
            acp_session_id: None,
            agent,
            working_dir,
            status: SessionStatus::Idle,
        };

        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id.clone(), Arc::new(RwLock::new(live_session)));

        log::info!("Created session: {}", session_id);
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
            .map_err(|e| format!("Failed to load session: {}", e))?
            .ok_or_else(|| format!("Session '{}' not found", session_id))?;

        let agent = client::find_acp_agent_by_id(&session.agent_id)
            .or_else(client::find_acp_agent)
            .ok_or_else(|| "No AI agent found".to_string())?;

        let live_session = LiveSession {
            session_id: session_id.to_string(),
            acp_session_id: None, // Will be set on first prompt
            agent,
            working_dir: PathBuf::from(&session.working_dir),
            status: SessionStatus::Idle,
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
            .map_err(|e| format!("Failed to load session: {}", e))?
            .ok_or_else(|| format!("Session '{}' not found", session_id))?;

        // Exists but not live = idle
        Ok(SessionStatus::Idle)
    }

    /// Close a live session (keeps history in store)
    pub async fn close_live_session(&self, session_id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
        log::info!("Closed live session: {}", session_id);
        Ok(())
    }

    /// Send a prompt to a session
    pub async fn send_prompt(&self, session_id: &str, prompt: String) -> Result<(), String> {
        // Get or create live session
        let session_arc = self.get_or_create_live_session(session_id).await?;

        // Check status and prepare for prompt
        let (agent, working_dir, acp_session_id) = {
            let mut session = session_arc.write().await;

            if session.status == SessionStatus::Processing {
                return Err("Session is already processing a prompt".to_string());
            }

            // Update status to processing
            session.status = SessionStatus::Processing;
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
            .map_err(|e| format!("Failed to store message: {}", e))?;

        // Spawn background task to run the prompt
        let app_handle = self.app_handle.clone();
        let session_id_owned = session_id.to_string();
        let session_arc_clone = session_arc.clone();
        let store = self.store.clone();

        tokio::spawn(async move {
            // Run the ACP prompt with streaming
            let result = client::run_acp_prompt_streaming(
                &agent,
                &working_dir,
                &prompt,
                acp_session_id.as_deref(),
                &session_id_owned,
                app_handle.clone(),
            )
            .await;

            // Update session and persist based on result
            let mut session = session_arc_clone.write().await;

            match result {
                Ok(acp_result) => {
                    // Store the ACP session ID for future resumption
                    session.acp_session_id = Some(acp_result.session_id.clone());
                    session.status = SessionStatus::Idle;

                    // Persist the assistant response
                    if let Err(e) = persist_assistant_turn(&store, &session_id_owned, &acp_result) {
                        log::error!("Failed to persist assistant turn: {}", e);
                    }

                    // Auto-generate title from first user message if not set
                    if let Err(e) = maybe_set_title(&store, &session_id_owned, &prompt) {
                        log::warn!("Failed to set session title: {}", e);
                    }
                }
                Err(e) => {
                    log::error!("Session {} prompt failed: {}", session_id_owned, e);
                    session.status = SessionStatus::Error { message: e };
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

    fn emit_status(&self, session_id: &str, status: &SessionStatus) {
        let event = SessionStatusEvent {
            session_id: session_id.to_string(),
            status: status.clone(),
        };
        let _ = self.app_handle.emit("session-status", &event);
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
