//! AI integration via ACP (Agent Client Protocol).
//!
//! This module provides AI chat functionality with persistent sessions.
//!
//! ## Architecture
//!
//! - `session.rs` - SessionManager for live agent connections + streaming
//! - `client.rs` - Core ACP client implementation (agent discovery, protocol)
//! - `analysis/` - Structured diff analysis: prompts, runner, and types for "Analyze with AI"
//!
//! Session/message persistence is handled by the unified Store (see `crate::store`).
//!
//! ## Data Flow
//!
//! 1. Frontend calls `create_session` → creates in SQLite + live session
//! 2. Frontend calls `send_prompt` → stores user message, streams response
//! 3. On turn complete → assistant message persisted to SQLite
//! 4. Frontend can `get_session` to load full history from SQLite
//!
//! Live sessions (agent connections) are ephemeral. History survives app restart.

pub mod analysis;
mod client;
pub mod session;

// Re-export core ACP client functionality
pub use client::{
    discover_acp_providers, find_acp_agent, find_acp_agent_by_id, run_acp_prompt,
    run_acp_prompt_streaming, run_acp_prompt_with_session, AcpAgent, AcpPromptResult,
    AcpProviderInfo,
};

// Re-export session manager types
pub use session::{LiveSessionInfo, SessionManager, SessionStatus, SessionStatusEvent};
