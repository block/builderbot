//! Tauri event adapter for action execution
//!
//! Implements the ExecutionListener trait to emit Tauri events

use async_trait::async_trait;
use builderbot_actions::{ExecutionEvent, ExecutionListener};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use super::registry::{ActionRegistry, RunPhase};

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
    pub action_type: String,
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
    action_type: String,
    registry: Arc<ActionRegistry>,
}

impl TauriExecutionListener {
    pub fn new(
        app: AppHandle,
        branch_id: String,
        action_id: String,
        action_name: String,
        action_type: String,
        registry: Arc<ActionRegistry>,
    ) -> Self {
        Self {
            app,
            branch_id,
            action_id,
            action_name,
            action_type,
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
                // Register the running action
                self.registry.register(
                    execution_id.clone(),
                    self.branch_id.clone(),
                    self.action_id.clone(),
                    self.action_name.clone(),
                    self.action_type.clone(),
                    started_at,
                );

                // We emit running status immediately
                let _ = self.app.emit(
                    "action_status",
                    ActionStatusEvent {
                        execution_id: execution_id.clone(),
                        branch_id: self.branch_id.clone(),
                        action_id: self.action_id.clone(),
                        action_name: self.action_name.clone(),
                        action_type: self.action_type.clone(),
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
                // Feed output lines into the shared buffer for regex matching.
                for line in chunk.lines() {
                    self.registry.append_output(&execution_id, line);
                }

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

                // Unregister completed/failed/stopped actions from the registry
                if status_str != "running" {
                    self.registry.unregister(&execution_id);
                }

                let _ = self.app.emit(
                    "action_status",
                    ActionStatusEvent {
                        execution_id,
                        branch_id: self.branch_id.clone(),
                        action_id: self.action_id.clone(),
                        action_name: self.action_name.clone(),
                        action_type: self.action_type.clone(),
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

// =============================================================================
// Run-phase change event
// =============================================================================

/// Event emitted when an action's run phase changes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPhaseChangedEvent {
    pub execution_id: String,
    pub branch_id: String,
    pub action_name: String,
    pub phase: RunPhase,
}

/// Emit an `action:run-phase-changed` event to the frontend.
pub fn emit_run_phase_changed(app_handle: &AppHandle, event: RunPhaseChangedEvent) {
    let _ = app_handle.emit("action:run-phase-changed", event);
}
