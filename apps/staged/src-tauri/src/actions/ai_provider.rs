//! ACP AI provider implementation for action detection
//!
//! This module provides the bridge between the app and the builderbot-actions crate,
//! using the acp-client library for reliable ACP communication.

// Re-export the AcpAiProvider from builderbot-actions
// This allows the app to use it without importing builderbot-actions directly
pub use builderbot_actions::AcpAiProvider;
