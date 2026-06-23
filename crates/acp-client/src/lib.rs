//! ACP Client - Full-featured client for Agent Client Protocol (ACP)
//!
//! This library provides comprehensive ACP support including:
//! - Agent discovery and binary lookup
//! - Full session management with history restoration
//! - Streaming events (text, tool calls, updates)
//! - Permission handling
//! - Remote workspace support (via Blox)
//! - Cancellation and graceful shutdown
//!
//! # Architecture
//!
//! The library provides two main interfaces:
//!
//! 1. **High-level driver interface** (`AcpDriver`) - For applications that need
//!    full session orchestration, streaming, and database integration
//! 2. **Simple one-shot interface** (`run_acp_prompt`) - For simple prompting
//!    without session management

mod driver;
mod simple;
mod types;

// Re-export the main API
pub use agent_client_protocol::schema::v1::{
    ConfigOptionUpdate, McpServer, McpServerHttp, McpServerSse, SessionConfigOption,
    SessionConfigOptionCategory, SessionInfoUpdate, SessionModeState as SessionModelState,
};
pub use driver::{
    strip_code_fences, AcpDriver, AcpToolCallMetadata, AgentDriver, BasicMessageWriter,
    MessageWriter, Store,
};
pub use simple::{run_acp_prompt, run_acp_prompt_with_interpreter_env_snapshot};
pub use types::{
    discover_providers, find_acp_agent, find_acp_agent_by_id, find_command, known_agent_commands,
    AcpAgent, AcpProviderInfo,
};
