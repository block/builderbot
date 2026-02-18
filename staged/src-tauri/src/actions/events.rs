//! Tauri event adapter for action execution
//!
//! Implements the ExecutionListener trait to emit Tauri events

use async_trait::async_trait;
use builderbot_actions::{ExecutionEvent, ExecutionListener};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use super::registry::ActionRegistry;

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
    registry: Arc<ActionRegistry>,
}

impl TauriExecutionListener {
    pub fn new(
        app: AppHandle,
        branch_id: String,
        action_id: String,
        action_name: String,
        registry: Arc<ActionRegistry>,
    ) -> Self {
        Self {
            app,
            branch_id,
            action_id,
            action_name,
            registry,
        }
    }
}

#[async_trait]
impl ExecutionListener for TauriExecutionListener {
    async fn on_event(&self, event: ExecutionEvent) {
        match event {
            ExecutionEvent::Started {
                execution_id,
                started_at,
            } => {
                log::info!(
                    "[TauriExecutionListener] Action started - execution_id: {}, action: {}, branch_id: {}",
                    execution_id,
                    self.action_name,
                    self.branch_id
                );

                // Register the running action
                self.registry.register(
                    execution_id.clone(),
                    self.branch_id.clone(),
                    self.action_id.clone(),
                    self.action_name.clone(),
                    started_at,
                );

                // We emit running status immediately
                let result = self.app.emit(
                    "action_status",
                    ActionStatusEvent {
                        execution_id: execution_id.clone(),
                        branch_id: self.branch_id.clone(),
                        action_id: self.action_id.clone(),
                        action_name: self.action_name.clone(),
                        status: "running".to_string(),
                        exit_code: None,
                        started_at: Some(started_at),
                        completed_at: None,
                    },
                );

                if let Err(e) = result {
                    log::error!(
                        "[TauriExecutionListener] Failed to emit action_status (running) - execution_id: {}, error: {:?}",
                        execution_id,
                        e
                    );
                } else {
                    log::info!(
                        "[TauriExecutionListener] Emitted action_status (running) - execution_id: {}",
                        execution_id
                    );
                }
            }
            ExecutionEvent::Output {
                execution_id,
                chunk,
                stream,
                ..
            } => {
                // Output events are very frequent, so we only log errors
                let result = self.app.emit(
                    "action_output",
                    ActionOutputEvent {
                        execution_id: execution_id.clone(),
                        chunk,
                        stream: stream.clone(),
                    },
                );

                if let Err(e) = result {
                    log::error!(
                        "[TauriExecutionListener] Failed to emit action_output - execution_id: {}, stream: {}, error: {:?}",
                        execution_id,
                        stream,
                        e
                    );
                }
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

                log::info!(
                    "[TauriExecutionListener] Action status changed - execution_id: {}, action: {}, status: {}, exit_code: {:?}",
                    execution_id,
                    self.action_name,
                    status_str,
                    exit_code
                );

                // Unregister completed/failed/stopped actions from the registry
                if status_str != "running" {
                    self.registry.unregister(&execution_id);
                }

                let result = self.app.emit(
                    "action_status",
                    ActionStatusEvent {
                        execution_id: execution_id.clone(),
                        branch_id: self.branch_id.clone(),
                        action_id: self.action_id.clone(),
                        action_name: self.action_name.clone(),
                        status: status_str.to_string(),
                        exit_code,
                        started_at,
                        completed_at,
                    },
                );

                if let Err(e) = result {
                    log::error!(
                        "[TauriExecutionListener] Failed to emit action_status ({}) - execution_id: {}, error: {:?}",
                        status_str,
                        execution_id,
                        e
                    );
                } else {
                    log::info!(
                        "[TauriExecutionListener] Emitted action_status ({}) - execution_id: {}",
                        status_str,
                        execution_id
                    );
                }
            }
            ExecutionEvent::AutoCommit {
                execution_id,
                action_name,
            } => {
                log::info!(
                    "[TauriExecutionListener] Auto-commit triggered - execution_id: {}, action: {}",
                    execution_id,
                    action_name
                );

                let result = self.app.emit(
                    "action_auto_commit",
                    serde_json::json!({
                        "executionId": execution_id,
                        "branchId": self.branch_id,
                        "actionName": action_name,
                    }),
                );

                if let Err(e) = result {
                    log::error!(
                        "[TauriExecutionListener] Failed to emit action_auto_commit - execution_id: {}, error: {:?}",
                        execution_id,
                        e
                    );
                }
            }
        }
    }
}
