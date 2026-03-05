//! Registry for tracking running actions with their metadata
//!
//! This registry maintains a mapping of execution IDs to their associated
//! branch and action information, allowing the UI to restore state when
//! components remount.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// Information about a running action
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningActionInfo {
    pub execution_id: String,
    pub branch_id: String,
    pub action_id: String,
    pub action_name: String,
    pub action_type: String,
    pub started_at: i64,
}

/// Ephemeral run phase tracked per execution (not persisted).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RunPhase {
    Building,
    Running { endpoint: Option<String> },
    AutodetectPending,
    NoDetection,
}

/// Registry for tracking running actions
pub struct ActionRegistry {
    running: Mutex<HashMap<String, RunningActionInfo>>,
    run_phases: Mutex<HashMap<String, RunPhase>>,
    output_buffers: Mutex<HashMap<String, Arc<Mutex<Vec<String>>>>>,
    /// Cancellation senders for regex matcher tasks — dropping or sending
    /// `true` stops the background task.
    cancel_senders: Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl Default for ActionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionRegistry {
    pub fn new() -> Self {
        Self {
            running: Mutex::new(HashMap::new()),
            run_phases: Mutex::new(HashMap::new()),
            output_buffers: Mutex::new(HashMap::new()),
            cancel_senders: Mutex::new(HashMap::new()),
        }
    }

    /// Register a new running action
    pub fn register(
        &self,
        execution_id: String,
        branch_id: String,
        action_id: String,
        action_name: String,
        action_type: String,
        started_at: i64,
    ) {
        let info = RunningActionInfo {
            execution_id: execution_id.clone(),
            branch_id,
            action_id,
            action_name,
            action_type,
            started_at,
        };

        let mut running = self.running.lock().unwrap();
        running.insert(execution_id, info);
    }

    /// Unregister a completed action and clean up associated state.
    pub fn unregister(&self, execution_id: &str) {
        {
            let mut running = self.running.lock().unwrap();
            running.remove(execution_id);
        }
        {
            let mut phases = self.run_phases.lock().unwrap();
            phases.remove(execution_id);
        }
        {
            let mut buffers = self.output_buffers.lock().unwrap();
            buffers.remove(execution_id);
        }
        {
            // Dropping the sender signals the background task to stop.
            let mut senders = self.cancel_senders.lock().unwrap();
            senders.remove(execution_id);
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

    // ----- RunPhase tracking -----

    /// Set the run phase for an execution.
    pub fn set_run_phase(&self, execution_id: &str, phase: RunPhase) {
        let mut phases = self.run_phases.lock().unwrap();
        phases.insert(execution_id.to_string(), phase);
    }

    /// Get the current run phase for an execution.
    pub fn get_run_phase(&self, execution_id: &str) -> Option<RunPhase> {
        let phases = self.run_phases.lock().unwrap();
        phases.get(execution_id).cloned()
    }

    // ----- Output buffer management -----

    /// Create (or retrieve) a shared output buffer for an execution and return
    /// a reference to it.
    pub fn register_output_buffer(&self, execution_id: &str) -> Arc<Mutex<Vec<String>>> {
        let mut buffers = self.output_buffers.lock().unwrap();
        buffers
            .entry(execution_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(Vec::new())))
            .clone()
    }

    /// Append a line to the output buffer for the given execution.
    /// Creates the buffer lazily if it does not yet exist.
    pub fn append_output(&self, execution_id: &str, line: &str) {
        let buf = {
            let mut buffers = self.output_buffers.lock().unwrap();
            buffers
                .entry(execution_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(Vec::new())))
                .clone()
        };
        let mut buf = buf.lock().unwrap();
        buf.push(line.to_string());
    }

    /// Get a snapshot of all output lines for an execution.
    pub fn get_output_lines(&self, execution_id: &str) -> Option<Vec<String>> {
        let buf = {
            let buffers = self.output_buffers.lock().unwrap();
            buffers.get(execution_id).cloned()
        };
        buf.map(|b| {
            let b = b.lock().unwrap();
            b.clone()
        })
    }

    // ----- Cancellation sender management -----

    /// Store a cancellation sender for a regex matcher / detector task.
    pub fn store_cancel_sender(&self, execution_id: &str, sender: watch::Sender<bool>) {
        let mut senders = self.cancel_senders.lock().unwrap();
        senders.insert(execution_id.to_string(), sender);
    }
}
