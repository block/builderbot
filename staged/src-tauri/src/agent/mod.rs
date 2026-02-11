//! Agent abstraction layer.
//!
//! This module provides adapters between staged's storage layer and the
//! acp-client crate's generic interfaces.

pub mod writer;

use std::sync::Arc;

pub use acp_client::{discover_providers, AcpDriver, AcpProviderInfo, AgentDriver};

use crate::store::Store;

// Implement the acp_client::Store trait for our Store
impl acp_client::driver::Store for Store {
    fn set_agent_session_id(&self, session_id: &str, agent_session_id: &str) -> Result<(), String> {
        self.set_agent_session_id(session_id, agent_session_id)
            .map_err(|e| e.to_string())
    }
}

// Re-export writer for backward compatibility
pub use writer::MessageWriter;
