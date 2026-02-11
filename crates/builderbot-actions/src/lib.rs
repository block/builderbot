//! Builderbot Actions - Reusable action execution and detection engine
//!
//! This crate provides a generic framework for executing shell commands with real-time
//! output streaming and AI-powered action detection from project files. It's designed
//! to work with repo+worktree contexts and is completely framework-agnostic.
//!
//! # Features
//!
//! - **Action Execution**: Spawn shell commands with real-time stdout/stderr streaming
//! - **Process Management**: Track running processes, stop them, retrieve buffered output
//! - **Git Integration**: Auto-commit changes after successful action execution
//! - **AI Detection**: Detect available actions from build files using AI
//! - **Framework Agnostic**: Works with any event system (Tauri, WebSockets, SSE, etc.)
//!
//! # Architecture
//!
//! The crate uses trait abstractions to remain decoupled from specific frameworks:
//! - `ExecutionListener` trait for receiving execution events
//! - `AiProvider` trait for AI-powered action detection
//!
//! # Example Usage
//!
//! ```rust,no_run
//! use builderbot_actions::{ActionExecutor, ActionMetadata, ExecutionListener, ExecutionEvent};
//! use std::sync::Arc;
//! use async_trait::async_trait;
//!
//! // Implement the ExecutionListener trait for your event system
//! struct MyListener;
//!
//! #[async_trait]
//! impl ExecutionListener for MyListener {
//!     async fn on_event(&self, event: ExecutionEvent) {
//!         println!("Event: {:?}", event);
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let executor = ActionExecutor::new();
//!     let listener = Arc::new(MyListener);
//!
//!     let metadata = ActionMetadata {
//!         action_id: "test-action".to_string(),
//!         action_name: "Run Tests".to_string(),
//!         auto_commit: false,
//!     };
//!
//!     let execution_id = executor.execute(
//!         "npm test".to_string(),
//!         "/path/to/project".to_string(),
//!         metadata,
//!         listener,
//!     ).await?;
//!
//!     println!("Started execution: {}", execution_id);
//!     Ok(())
//! }
//! ```

pub mod acp_provider;
pub mod detector;
pub mod executor;
pub mod git;
pub mod models;

// Re-export main types for convenience
pub use acp_provider::AcpAiProvider;
pub use detector::{ActionDetector, AiProvider, SuggestedAction};
pub use executor::{ActionExecutor, ActionMetadata, ExecutionListener};
pub use models::{ActionStatus, ActionType, ExecutionEvent, OutputChunk};
