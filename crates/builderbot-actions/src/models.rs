//! Data models for action execution and detection

use serde::{Deserialize, Serialize};

/// The type of a project action
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActionType {
    Build,
    Test,
    Format,
    Check,
    Prerun,
    Run,
    CleanUp,
}

impl ActionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionType::Build => "build",
            ActionType::Test => "test",
            ActionType::Format => "format",
            ActionType::Check => "check",
            ActionType::Prerun => "prerun",
            ActionType::Run => "run",
            ActionType::CleanUp => "cleanUp",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "build" => Some(ActionType::Build),
            "test" => Some(ActionType::Test),
            "format" => Some(ActionType::Format),
            "check" => Some(ActionType::Check),
            "prerun" => Some(ActionType::Prerun),
            "run" => Some(ActionType::Run),
            "cleanUp" => Some(ActionType::CleanUp),
            _ => None,
        }
    }
}

/// Status of an action execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionStatus {
    Running,
    Completed,
    Failed,
    Stopped,
}

/// Represents a single output chunk with its metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputChunk {
    pub chunk: String,
    pub stream: String, // "stdout" or "stderr"
    pub timestamp: i64,
}

/// A suggested action detected from project files
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedAction {
    pub name: String,
    pub command: String,
    pub action_type: ActionType,
    pub source: String, // e.g., "justfile", "Makefile", "package.json"
}

/// How a "run" action detects whether the process is running and/or its endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RunDetectionMode {
    Autodetect,
    EndpointRegex { pattern: String },
    RunningRegex { pattern: String },
    NoDetection,
}

/// Events emitted during action execution
#[derive(Debug, Clone)]
pub enum ExecutionEvent {
    /// Action has started
    Started {
        execution_id: String,
        started_at: i64,
    },
    /// Output chunk received
    Output {
        execution_id: String,
        chunk: String,
        stream: String, // "stdout" or "stderr"
        timestamp: i64,
    },
    /// Action status changed
    StatusChanged {
        execution_id: String,
        status: ActionStatus,
        exit_code: Option<i32>,
        started_at: Option<i64>,
        completed_at: Option<i64>,
    },
}
