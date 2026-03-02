//! Lightweight regex matching for run detection.
//!
//! Spawns a background task that polls the action output buffer every 2 seconds,
//! applies a compiled regex against new lines, and transitions `RunPhase` when
//! a match is found. AI-powered autodetection will be added in Phase 3.

use regex::Regex;
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tokio::sync::watch;
use tokio::time::{self, Duration};

use super::events::emit_run_phase_changed;
use super::events::RunPhaseChangedEvent;
use super::registry::{ActionRegistry, RunPhase};

/// Spawns a background task that polls the shared output buffer every 2 seconds,
/// applies the given regex against new lines, and transitions `RunPhase` to
/// `Running` when the pattern matches.
///
/// * `has_endpoint_capture` — when `true`, the regex is expected to contain a
///   named capture group `endpoint` whose value is extracted as the service URL.
/// * `cancel_rx` — the task stops when this receiver yields `true` or the
///   sender is dropped.
pub fn spawn_regex_matcher(
    app_handle: AppHandle,
    registry: Arc<ActionRegistry>,
    execution_id: String,
    branch_id: String,
    action_name: String,
    pattern: String,
    has_endpoint_capture: bool,
    mut cancel_rx: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        // Compile the regex — if it fails, transition straight to NoDetection.
        let re = match Regex::new(&pattern) {
            Ok(re) => re,
            Err(e) => {
                log::warn!("run_detector: invalid regex for execution {execution_id}: {e}");
                registry.set_run_phase(&execution_id, RunPhase::NoDetection);
                emit_run_phase_changed(
                    &app_handle,
                    RunPhaseChangedEvent {
                        execution_id,
                        branch_id,
                        action_name,
                        phase: RunPhase::NoDetection,
                    },
                );
                return;
            }
        };

        // Set initial phase to Building.
        registry.set_run_phase(&execution_id, RunPhase::Building);
        emit_run_phase_changed(
            &app_handle,
            RunPhaseChangedEvent {
                execution_id: execution_id.clone(),
                branch_id: branch_id.clone(),
                action_name: action_name.clone(),
                phase: RunPhase::Building,
            },
        );

        // Get the shared output buffer.
        let buffer: Arc<Mutex<Vec<String>>> = registry.register_output_buffer(&execution_id);

        let mut last_checked_index: usize = 0;
        let mut interval = time::interval(Duration::from_secs(2));

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                result = cancel_rx.changed() => {
                    // Sender dropped or explicitly cancelled.
                    if result.is_err() || *cancel_rx.borrow() {
                        return;
                    }
                }
            }

            // Read new lines since last check.
            let new_lines: Vec<String> = {
                let buf = buffer.lock().unwrap();
                if buf.len() <= last_checked_index {
                    continue;
                }
                let lines = buf[last_checked_index..].to_vec();
                last_checked_index = buf.len();
                lines
            };

            // Apply regex to each new line.
            for line in &new_lines {
                if let Some(caps) = re.captures(line) {
                    let endpoint = if has_endpoint_capture {
                        caps.name("endpoint").map(|m| m.as_str().to_string())
                    } else {
                        None
                    };

                    let phase = RunPhase::Running { endpoint };
                    registry.set_run_phase(&execution_id, phase.clone());
                    emit_run_phase_changed(
                        &app_handle,
                        RunPhaseChangedEvent {
                            execution_id,
                            branch_id,
                            action_name,
                            phase,
                        },
                    );
                    // Match found — task is done.
                    return;
                }
            }
        }
    });
}
