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
//! 2. **Simple one-shot interface** (`run_acp_prompt_raw`) - For simple prompting
//!    without session management
//!
//! # Example (Simple)
//!
//! ```rust,no_run
//! use acp_client::{find_acp_agent, run_acp_prompt_raw};
//! use std::path::Path;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let agent = find_acp_agent().ok_or_else(|| anyhow::anyhow!("No ACP agent found"))?;
//!     let response = run_acp_prompt_raw(&agent, Path::new("."), "Hello!").await?;
//!     println!("Agent response: {}", response);
//!     Ok(())
//! }
//! ```

mod driver;
mod simple;
mod types;

// Re-export the main API
pub use driver::{AcpDriver, AgentDriver, MessageWriter, Store};
pub use simple::run_acp_prompt_raw;
pub use types::{discover_providers, find_acp_agent, find_acp_agent_by_id, AcpAgent, AcpProviderInfo};
