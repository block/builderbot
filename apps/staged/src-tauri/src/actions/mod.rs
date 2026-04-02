//! Actions module - orchestrates action execution and detection
//!
//! This module integrates the reusable `builderbot-actions` crate with
//! the Tauri app, providing Tauri commands, event emission, and AI provider
//! implementation using ACP.

pub mod ai_provider;
pub mod commands;
pub mod events;
pub mod registry;
pub mod run_detector;

// Re-export types for convenience
pub use builderbot_actions::{
    ActionDetector, ActionExecutor, ActionMetadata, ActionStatus, ActionType, OutputChunk,
    StopOptions, SuggestedAction,
};
pub use registry::{ActionRegistry, RunPhase, RunningActionInfo};
