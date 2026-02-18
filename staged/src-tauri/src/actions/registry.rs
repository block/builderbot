//! Registry for tracking running actions with their metadata
//!
//! This registry maintains a mapping of execution IDs to their associated
//! branch and action information, allowing the UI to restore state when
//! components remount.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Information about a running action
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningActionInfo {
    pub execution_id: String,
    pub branch_id: String,
    pub action_id: String,
    pub action_name: String,
    pub started_at: i64,
}

/// Registry for tracking running actions
#[derive(Default)]
pub struct ActionRegistry {
    running: Arc<Mutex<HashMap<String, RunningActionInfo>>>,
}

impl ActionRegistry {
    pub fn new() -> Self {
        Self {
            running: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a new running action
    pub fn register(
        &self,
        execution_id: String,
        branch_id: String,
        action_id: String,
        action_name: String,
        started_at: i64,
    ) {
        let info = RunningActionInfo {
            execution_id: execution_id.clone(),
            branch_id,
            action_id,
            action_name,
            started_at,
        };

        let mut running = self.running.lock().unwrap();
        running.insert(execution_id.clone(), info);

        log::info!(
            "[ActionRegistry] Registered running action - execution_id: {}, total_running: {}",
            execution_id,
            running.len()
        );
    }

    /// Unregister a completed action
    pub fn unregister(&self, execution_id: &str) {
        let mut running = self.running.lock().unwrap();
        if running.remove(execution_id).is_some() {
            log::info!(
                "[ActionRegistry] Unregistered action - execution_id: {}, remaining_running: {}",
                execution_id,
                running.len()
            );
        }
    }

    /// Get all running actions for a specific branch
    pub fn get_running_for_branch(&self, branch_id: &str) -> Vec<RunningActionInfo> {
        let running = self.running.lock().unwrap();
        running
            .values()
            .filter(|info| info.branch_id == branch_id)
            .cloned()
            .collect()
    }

    /// Get all running actions
    pub fn get_all_running(&self) -> Vec<RunningActionInfo> {
        let running = self.running.lock().unwrap();
        running.values().cloned().collect()
    }
}
