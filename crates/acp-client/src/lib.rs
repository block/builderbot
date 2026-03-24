//! ACP Client - Full-featured client for Agent Client Protocol (ACP)
//!
//! This library provides comprehensive ACP support including:
//! - Agent discovery and binary lookup
//! - Full session management with history restoration
//! - Streaming events (text, tool calls, updates)
//! - Permission handling
//! - Remote workspace support (via Blox)
//! - Cancellation and graceful shutdown

mod driver;
mod types;

// Re-export the main API
pub use agent_client_protocol::{McpServer, McpServerHttp, McpServerSse};
pub use driver::{
    strip_code_fences, AcpDriver, AgentDriver, BasicMessageWriter, MessageWriter, Store,
};
pub use types::{
    discover_providers, find_acp_agent, find_acp_agent_by_id, find_command, AcpAgent,
    AcpProviderInfo,
};
