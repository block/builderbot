//! Registry for tracking running actions with their metadata
//!
//! This registry maintains a mapping of execution IDs to their associated
//! branch and action information, allowing the UI to restore state when
//! components remount.
//!
//! Alongside each running action it keeps what Run detection needs: the run
//! phase, the detector's cancellation handle and, for Run actions only, a
//! normalized copy of the recent output. All of that exists only while the
//! execution is registered. Whatever arrives for an execution that has been
//! unregistered, or never was registered, is dropped rather than stored,
//! because nothing would ever remove it again.

use builderbot_actions::ActionType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

use crate::terminal_output::TerminalOutputProcessor;

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

/// How many recent lines of a Run action's output are kept for detection.
pub(super) const DETECTION_MAX_LINES: usize = 20_000;

/// How much text those lines may hold. The regex matcher reads the buffer
/// every two seconds and the autodetect poller reads its last 200 lines, so
/// this only has to cover what a service prints between two reads.
const DETECTION_MAX_BYTES: usize = 2 * 1024 * 1024;

/// Registry for tracking running actions
///
/// `running` says which executions are live. It is locked before any of the
/// other maps wherever both are held, and they gain an entry only while it
/// is held and has the execution, so `unregister` leaves nothing behind.
pub struct ActionRegistry {
    running: Mutex<HashMap<String, RunningActionInfo>>,
    run_phases: Mutex<HashMap<String, RunPhase>>,
    output_buffers: Mutex<HashMap<String, Arc<Mutex<TerminalOutputProcessor>>>>,
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

    /// Register a new running action.
    ///
    /// A Run action also gets the output buffer Run detection reads. It is
    /// created here, before the action can have produced any output, so that
    /// detection sees the output from its first line even though the detector
    /// itself is wired up only once the action has started.
    pub fn register(
        &self,
        execution_id: String,
        branch_id: String,
        action_id: String,
        action_name: String,
        action_type: String,
        started_at: i64,
    ) {
        let detects_run = action_type == ActionType::Run.as_str();
        let info = RunningActionInfo {
            execution_id: execution_id.clone(),
            branch_id,
            action_id,
            action_name,
            action_type,
            started_at,
        };

        let mut running = self.running.lock().unwrap();
        if detects_run {
            let mut buffers = self.output_buffers.lock().unwrap();
            buffers
                .entry(execution_id.clone())
                .or_insert_with(new_detection_buffer);
        }
        running.insert(execution_id, info);
    }

    /// Unregister a completed action and clean up associated state.
    pub fn unregister(&self, execution_id: &str) {
        // Held throughout, so that nothing can add state for this execution
        // between its removal from `running` and the removals below.
        let mut running = self.running.lock().unwrap();
        running.remove(execution_id);
        self.run_phases.lock().unwrap().remove(execution_id);
        self.output_buffers.lock().unwrap().remove(execution_id);
        // Dropping the sender signals the background task to stop.
        self.cancel_senders.lock().unwrap().remove(execution_id);
    }

    /// Run `store` if the execution is registered, with `running` held so
    /// that it cannot be unregistered meanwhile.
    fn while_registered(&self, execution_id: &str, store: impl FnOnce()) {
        let running = self.running.lock().unwrap();
        if running.contains_key(execution_id) {
            store();
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

    /// Set the run phase for an execution. Ignored once the execution has
    /// ended.
    pub fn set_run_phase(&self, execution_id: &str, phase: RunPhase) {
        self.while_registered(execution_id, || {
            let mut phases = self.run_phases.lock().unwrap();
            phases.insert(execution_id.to_string(), phase);
        });
    }

    /// Get the current run phase for an execution.
    pub fn get_run_phase(&self, execution_id: &str) -> Option<RunPhase> {
        let phases = self.run_phases.lock().unwrap();
        phases.get(execution_id).cloned()
    }

    // ----- Output buffer management -----

    /// Make sure a running execution has an output buffer for Run detection
    /// to read. Run actions have had one since they were registered; this
    /// covers any other execution a detector is attached to. Does nothing
    /// once the execution has ended.
    pub(crate) fn register_output_buffer(&self, execution_id: &str) {
        self.while_registered(execution_id, || {
            let mut buffers = self.output_buffers.lock().unwrap();
            buffers
                .entry(execution_id.to_string())
                .or_insert_with(new_detection_buffer);
        });
    }

    /// Append a raw output chunk to the normalized buffer for the given
    /// execution, if it has one.
    ///
    /// Only executions Run detection reads have a buffer. Output of any
    /// other action is not kept here, and neither is output that arrives
    /// after the execution was unregistered: a process that outlives its
    /// action can keep writing indefinitely.
    pub(crate) fn append_output_chunk(&self, execution_id: &str, chunk: &str) {
        if let Some(buf) = self.output_buffer(execution_id) {
            buf.lock().unwrap().process_chunk(chunk);
        }
    }

    /// The output lines numbered `from` and up, with the number of lines
    /// there have been so far. Lines are numbered from the start of the
    /// output, and those no longer retained are left out. `None` if the
    /// execution has no output buffer.
    pub(crate) fn get_output_lines_from(
        &self,
        execution_id: &str,
        from: usize,
    ) -> Option<(usize, Vec<String>)> {
        self.output_buffer(execution_id).map(|buf| {
            let buf = buf.lock().unwrap();
            (buf.line_count(), buf.lines_from(from))
        })
    }

    /// The last `count` output lines for an execution. `None` if it has no
    /// output buffer.
    pub(crate) fn get_last_output_lines(
        &self,
        execution_id: &str,
        count: usize,
    ) -> Option<Vec<String>> {
        self.output_buffer(execution_id)
            .map(|buf| buf.lock().unwrap().last_lines(count))
    }

    fn output_buffer(&self, execution_id: &str) -> Option<Arc<Mutex<TerminalOutputProcessor>>> {
        let buffers = self.output_buffers.lock().unwrap();
        buffers.get(execution_id).cloned()
    }

    // ----- Cancellation sender management -----

    /// Store a cancellation sender for a regex matcher / detector task.
    ///
    /// If the execution has already ended the sender is dropped instead,
    /// which stops the task just as `unregister` would have.
    pub fn store_cancel_sender(&self, execution_id: &str, sender: watch::Sender<bool>) {
        self.while_registered(execution_id, || {
            let mut senders = self.cancel_senders.lock().unwrap();
            senders.insert(execution_id.to_string(), sender);
        });
    }
}

fn new_detection_buffer() -> Arc<Mutex<TerminalOutputProcessor>> {
    Arc::new(Mutex::new(TerminalOutputProcessor::keeping_recent(
        DETECTION_MAX_LINES,
        DETECTION_MAX_BYTES,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn register(registry: &ActionRegistry, execution_id: &str, action_type: ActionType) {
        registry.register(
            execution_id.to_string(),
            "branch".to_string(),
            format!("action-{execution_id}"),
            format!("Action {execution_id}"),
            action_type.as_str().to_string(),
            0,
        );
    }

    fn output_lines(registry: &ActionRegistry, execution_id: &str) -> Option<Vec<String>> {
        registry
            .get_output_lines_from(execution_id, 0)
            .map(|(_, lines)| lines)
    }

    /// How many entries the registry holds beyond the running actions.
    fn side_entries(registry: &ActionRegistry) -> usize {
        registry.run_phases.lock().unwrap().len()
            + registry.output_buffers.lock().unwrap().len()
            + registry.cancel_senders.lock().unwrap().len()
    }

    #[test]
    fn output_buffer_renders_progress_across_chunks() {
        let registry = ActionRegistry::new();
        register(&registry, "run", ActionType::Run);

        registry.append_output_chunk("run", "10%\r");
        assert_eq!(output_lines(&registry, "run").unwrap(), vec!["10%"]);

        registry.append_output_chunk("run", "20%\r");
        registry.append_output_chunk("run", "done\n");

        assert_eq!(output_lines(&registry, "run").unwrap(), vec!["done"]);
    }

    #[test]
    fn output_buffer_handles_crlf_split_across_chunks() {
        let registry = ActionRegistry::new();
        register(&registry, "run", ActionType::Run);

        registry.append_output_chunk("run", "hello\r");
        registry.append_output_chunk("run", "\nworld\n");

        assert_eq!(
            output_lines(&registry, "run").unwrap(),
            vec!["hello", "world"]
        );
    }

    #[test]
    fn a_run_action_is_buffered_from_its_first_output() {
        let registry = ActionRegistry::new();
        register(&registry, "run", ActionType::Run);

        // Output that arrives before the detector is wired up is kept, and
        // wiring it up keeps what is already there.
        registry.append_output_chunk("run", "Listening on :3000\n");
        registry.register_output_buffer("run");
        registry.append_output_chunk("run", "ready\n");

        assert_eq!(
            output_lines(&registry, "run").unwrap(),
            vec!["Listening on :3000", "ready"]
        );
        assert_eq!(
            registry.get_last_output_lines("run", 1).unwrap(),
            vec!["ready"]
        );
    }

    #[test]
    fn only_executions_run_detection_reads_are_buffered() {
        let registry = ActionRegistry::new();
        for action_type in [
            ActionType::Build,
            ActionType::Test,
            ActionType::Format,
            ActionType::Check,
            ActionType::Prerun,
            ActionType::CleanUp,
        ] {
            let execution_id = action_type.as_str();
            register(&registry, execution_id, action_type);
            registry.append_output_chunk(execution_id, "output nothing reads\n");
            assert_eq!(output_lines(&registry, execution_id), None);
            assert_eq!(registry.get_last_output_lines(execution_id, 200), None);
        }
        assert_eq!(side_entries(&registry), 0);

        // A detector attached to one of them gets a buffer from then on.
        registry.register_output_buffer("build");
        registry.append_output_chunk("build", "now read\n");
        assert_eq!(output_lines(&registry, "build").unwrap(), vec!["now read"]);
        assert_eq!(side_entries(&registry), 1);
    }

    #[test]
    fn output_after_unregister_does_not_bring_the_entry_back() {
        let registry = ActionRegistry::new();
        register(&registry, "run", ActionType::Run);
        registry.append_output_chunk("run", "before\n");
        assert_eq!(side_entries(&registry), 1);

        registry.unregister("run");
        assert_eq!(side_entries(&registry), 0);

        // A process that outlived the action is still writing.
        registry.append_output_chunk("run", "after\n");
        assert_eq!(output_lines(&registry, "run"), None);
        assert_eq!(side_entries(&registry), 0);
    }

    #[test]
    fn nothing_is_stored_for_an_execution_that_is_not_running() {
        let registry = ActionRegistry::new();
        register(&registry, "ended", ActionType::Run);
        registry.unregister("ended");

        for execution_id in ["ended", "never-started"] {
            // Everything Run detection stores when it is wired up, late.
            registry.register_output_buffer(execution_id);
            registry.append_output_chunk(execution_id, "output\n");
            registry.set_run_phase(execution_id, RunPhase::Building);
            let (cancel_tx, cancel_rx) = watch::channel(false);
            registry.store_cancel_sender(execution_id, cancel_tx);

            assert_eq!(output_lines(&registry, execution_id), None);
            assert!(registry.get_run_phase(execution_id).is_none());
            // The sender was dropped, which is what stops a detector task.
            assert!(cancel_rx.has_changed().is_err());
        }
        assert_eq!(side_entries(&registry), 0);
    }

    #[test]
    fn a_running_execution_keeps_its_detection_state_until_it_ends() {
        let registry = ActionRegistry::new();
        register(&registry, "run", ActionType::Run);
        registry.set_run_phase("run", RunPhase::Building);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        registry.store_cancel_sender("run", cancel_tx);

        assert!(matches!(
            registry.get_run_phase("run"),
            Some(RunPhase::Building)
        ));
        assert!(cancel_rx.has_changed().is_ok());
        assert_eq!(side_entries(&registry), 3);

        registry.unregister("run");
        assert!(registry.get_run_phase("run").is_none());
        assert!(cancel_rx.has_changed().is_err());
        assert_eq!(side_entries(&registry), 0);
    }

    #[test]
    fn the_detection_buffer_keeps_only_recent_lines() {
        let registry = ActionRegistry::new();
        register(&registry, "run", ActionType::Run);

        let written = DETECTION_MAX_LINES + 500;
        let chunk: String = (0..written).map(|line| format!("line {line}\n")).collect();
        registry.append_output_chunk("run", &chunk);

        let (count, lines) = registry.get_output_lines_from("run", 0).unwrap();
        assert_eq!(count, written);
        assert_eq!(lines.len(), DETECTION_MAX_LINES);
        assert_eq!(lines[0], "line 500");
        // Line numbers still address the lines that remain.
        let (_, last) = registry.get_output_lines_from("run", written - 1).unwrap();
        assert_eq!(last, vec![format!("line {}", written - 1)]);

        // Long lines are bounded by their size instead.
        let long = format!("{}\n", "x".repeat(DETECTION_MAX_BYTES / 4));
        for _ in 0..8 {
            registry.append_output_chunk("run", &long);
        }
        let (_, lines) = registry.get_output_lines_from("run", 0).unwrap();
        assert_eq!(lines.len(), 4);
    }
}
