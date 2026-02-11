//! Tauri event adapter for action execution
//!
//! Implements the ExecutionListener trait to emit Tauri events

use async_trait::async_trait;
use builderbot_actions::{ExecutionEvent, ExecutionListener};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// Event emitted when action output is produced
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionOutputEvent {
    pub execution_id: String,
    pub chunk: String,
    pub stream: String, // "stdout" or "stderr"
}

/// Event emitted when action status changes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionStatusEvent {
    pub execution_id: String,
    pub branch_id: String,
    pub action_id: String,
    pub action_name: String,
    pub status: String, // "running", "completed", "failed", "stopped"
    pub exit_code: Option<i32>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

/// Tauri event listener adapter
pub struct TauriExecutionListener {
    app: AppHandle,
    branch_id: String,
    action_id: String,
    action_name: String,
}

impl TauriExecutionListener {
    pub fn new(app: AppHandle, branch_id: String, action_id: String, action_name: String) -> Self {
        Self {
            app,
            branch_id,
            action_id,
            action_name,
        }
    }
}

#[async_trait]
impl ExecutionListener for TauriExecutionListener {
    async fn on_event(&self, event: ExecutionEvent) {
        match event {
            ExecutionEvent::Started { execution_id, started_at } => {
                // We emit running status immediately
                let _ = self.app.emit(
                    "action_status",
                    ActionStatusEvent {
                        execution_id,
                        branch_id: self.branch_id.clone(),
                        action_id: self.action_id.clone(),
                        action_name: self.action_name.clone(),
                        status: "running".to_string(),
                        exit_code: None,
                        started_at: Some(started_at),
                        completed_at: None,
                    },
                );
            }
            ExecutionEvent::Output {
                execution_id,
                chunk,
                stream,
                ..
            } => {
                let _ = self.app.emit(
                    "action_output",
                    ActionOutputEvent {
                        execution_id,
                        chunk,
                        stream,
                    },
                );
            }
            ExecutionEvent::StatusChanged {
                execution_id,
                status,
                exit_code,
                started_at,
                completed_at,
            } => {
                let status_str = match status {
                    builderbot_actions::ActionStatus::Running => "running",
                    builderbot_actions::ActionStatus::Completed => "completed",
                    builderbot_actions::ActionStatus::Failed => "failed",
                    builderbot_actions::ActionStatus::Stopped => "stopped",
                };

                let _ = self.app.emit(
                    "action_status",
                    ActionStatusEvent {
                        execution_id,
                        branch_id: self.branch_id.clone(),
                        action_id: self.action_id.clone(),
                        action_name: self.action_name.clone(),
                        status: status_str.to_string(),
                        exit_code,
                        started_at,
                        completed_at,
                    },
                );
            }
            ExecutionEvent::AutoCommit {
                execution_id,
                action_name,
            } => {
                let _ = self.app.emit(
                    "action_auto_commit",
                    serde_json::json!({
                        "executionId": execution_id,
                        "branchId": self.branch_id,
                        "actionName": action_name,
                    }),
                );
            }
        }
    }
}
