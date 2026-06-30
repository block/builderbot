//! Run detection: regex matching and AI-powered autodetection.
//!
//! * `spawn_regex_matcher` — polls the output buffer every 2 seconds and applies
//!   a compiled regex to detect when the service is running.
//! * `spawn_autodetect_poller` — uses an AI model on a backoff schedule to
//!   derive a regex pattern from terminal output, then hands off to the regex
//!   matcher once a valid pattern is found.

use regex::Regex;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::watch;
use tokio::time::{self, Duration};

use builderbot_actions::{AiProvider, RunDetectionMode};

use super::events::emit_run_phase_changed;
use super::events::RunPhaseChangedEvent;
use super::registry::{ActionRegistry, RunPhase};

use crate::store::Store;

/// Spawns a background task that polls the shared output buffer every 2 seconds,
/// applies the given regex against new lines, and transitions `RunPhase` to
/// `Running` when the pattern matches.
///
/// * `has_endpoint_capture` — when `true`, the regex is expected to contain a
///   named capture group `endpoint` whose value is extracted as the service URL.
/// * `cancel_rx` — the task stops when this receiver yields `true` or the
///   sender is dropped.
#[allow(clippy::too_many_arguments)]
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

        // Ensure the shared output buffer exists before polling.
        registry.register_output_buffer(&execution_id);

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

            // Read new lines since last check. Re-check the previous line too:
            // it may have been an in-progress line that gained more text since
            // the last poll.
            let new_lines: Vec<String> = {
                let Some(lines) = registry.get_output_lines(&execution_id) else {
                    continue;
                };
                let start_index = last_checked_index.saturating_sub(1);
                if lines.len() <= start_index {
                    continue;
                }
                let new_lines = lines[start_index..].to_vec();
                last_checked_index = lines.len();
                new_lines
            };

            // Apply regex to each normalized plain-text line.
            for line in &new_lines {
                let clean = crate::terminal_output::normalize_for_prompt(line);
                if let Some(caps) = re.captures(&clean) {
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

// =============================================================================
// AI autodetect poller (Phase 3)
// =============================================================================

/// JSON shape returned by the AI model.
#[derive(Deserialize)]
struct AutodetectResponse {
    status: String,
    regex: Option<String>,
    #[serde(default)]
    has_endpoint_capture: bool,
}

/// Build the polling schedule: intervals (in seconds) at which to poll.
///
/// * 0–150 s → every 30 s (5 polls)
/// * 150–1200 s → every 120 s (~9 more polls)
fn build_poll_schedule() -> Vec<u64> {
    let mut schedule = vec![30; 5]; // 5 polls at 30s intervals (covers 0–150s)
    schedule.resize(5 + 9, 120); // ~9 polls at 120s intervals (covers 150–1200s → 150+120*9 = 1230s)
    schedule
}

/// Strip optional markdown code-fence wrappers (```json ... ```) that an AI
/// model may include around its JSON response.
fn strip_markdown_json(raw: &str) -> &str {
    let trimmed = raw.trim();
    // Strip ```json ... ``` or ``` ... ```
    if let Some(rest) = trimmed.strip_prefix("```json") {
        return rest.strip_suffix("```").unwrap_or(rest).trim();
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        return rest.strip_suffix("```").unwrap_or(rest).trim();
    }
    trimmed
}

/// Spawns a background task that periodically sends terminal output to an AI
/// model which attempts to derive a regex pattern for run-state detection.
///
/// When the AI identifies a valid, matching regex the poller:
/// 1. Persists the appropriate `RunDetectionMode` to the database.
/// 2. Transitions the `RunPhase` to `Running` (with an optional endpoint).
/// 3. Stops polling.
///
/// If no pattern is found within 20 minutes the poller gives up and persists
/// `RunDetectionMode::NoDetection`.
#[allow(clippy::too_many_arguments)]
pub fn spawn_autodetect_poller(
    app_handle: AppHandle,
    store: Arc<Store>,
    registry: Arc<ActionRegistry>,
    execution_id: String,
    branch_id: String,
    action_id: String,
    action_name: String,
    command: String,
    working_dir: PathBuf,
    provider_id: Option<String>,
    mut cancel_rx: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        let schedule = build_poll_schedule();

        for wait_secs in &schedule {
            // ---- Wait the scheduled interval, or bail on cancel ----
            tokio::select! {
                _ = time::sleep(Duration::from_secs(*wait_secs)) => {}
                result = cancel_rx.changed() => {
                    if result.is_err() || *cancel_rx.borrow() {
                        return;
                    }
                }
            }

            // ---- Read the last ~200 lines of output ----
            let lines = match registry.get_output_lines(&execution_id) {
                Some(l) => l,
                None => continue, // buffer not yet created
            };
            let tail: Vec<&String> = if lines.len() > 200 {
                lines[lines.len() - 200..].iter().collect()
            } else {
                lines.iter().collect()
            };
            if tail.is_empty() {
                continue;
            }

            // Keep AI prompts and regex validation on normalized plain text.
            let clean_lines: Vec<String> = tail
                .iter()
                .map(|s| crate::terminal_output::normalize_for_prompt(s))
                .collect();
            let output = clean_lines.join("\n");

            // ---- Build and send the AI prompt ----
            let prompt = format!(
                r#"I'm running a development command and need to determine a regex pattern
that will match the terminal output line indicating the service is ready.

Command: `{command}`

Recent terminal output (last ~200 lines):
---
{output}
---

Analyze this output and determine:
1. Is the application still building/compiling, or has it reached a running/ready state?
2. If running: identify the specific output line from the server or build tool that
   indicates readiness (e.g., "Listening on http://0.0.0.0:3000", "Server started
   on port 8080", "ready in 300ms", "Local: http://localhost:1234/").
   - IMPORTANT: Pick the server/framework readiness message, NOT application-level
     log output. For example, Vite prints "Local: http://localhost:PORT/" when ready —
     use that, not subsequent browser console logs or webview messages that happen
     to contain URLs.
   - Prefer the EARLIEST line that indicates the service is up and accepting
     connections.
3. Provide a regex pattern that would match this readiness line in future runs.
   The regex is tested against each output line individually (single-line matching),
   so it must match within a single line.
   Be careful to avoid volatile values like timestamps, PIDs, version numbers,
   or build durations.
   - If the readiness line contains a URL/endpoint, include a named capture group
     `(?P<endpoint>...)` for it.
   - The regex should be general enough to work across restarts but specific enough
     to avoid matching unrelated log lines that happen to contain URLs.

Respond ONLY with JSON, no other text:
{{
  "status": "building" | "running",
  "regex": "<pattern or null>",
  "has_endpoint_capture": true | false
}}

If still building, set regex and has_endpoint_capture to null/false."#,
            );

            let ai_response = {
                let provider_result = super::commands::build_action_provider(
                    provider_id.as_deref(),
                    working_dir.clone(),
                );
                let provider = match provider_result {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!(
                            "autodetect_poller: failed to create AI provider for {execution_id}: {e}"
                        );
                        continue;
                    }
                };

                match provider.prompt(prompt).await {
                    Ok(resp) => resp,
                    Err(e) => {
                        log::warn!("autodetect_poller: AI prompt failed for {execution_id}: {e}");
                        continue;
                    }
                }
            };

            // ---- Parse the AI response ----
            let cleaned = strip_markdown_json(&ai_response);
            let parsed: AutodetectResponse = match serde_json::from_str(cleaned) {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("autodetect_poller: invalid JSON from AI for {execution_id}: {e}");
                    continue;
                }
            };

            // ---- Handle "building" status ----
            if parsed.status != "running" {
                log::info!(
                    "autodetect_poller: AI says still building for {execution_id}, continuing"
                );
                continue;
            }

            // ---- Handle "running" status ----
            let pattern = match &parsed.regex {
                Some(p) if !p.is_empty() => p.clone(),
                _ => {
                    log::info!(
                        "autodetect_poller: AI said running but no regex for {execution_id}, continuing"
                    );
                    continue;
                }
            };

            // Validate regex compiles.
            let re = match Regex::new(&pattern) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!(
                        "autodetect_poller: AI returned invalid regex for {execution_id}: {e}"
                    );
                    continue;
                }
            };

            // Validate that the regex matches at least one line in the current
            // output (using the already-stripped lines).
            let matched_line = clean_lines.iter().find(|line| re.is_match(line));
            if matched_line.is_none() {
                log::warn!(
                    "autodetect_poller: AI regex does not match any output line for {execution_id}"
                );
                continue;
            }

            // ---- Determine detection mode and extract endpoint ----
            let has_endpoint = parsed.has_endpoint_capture;
            let endpoint = if has_endpoint {
                matched_line.and_then(|line| {
                    re.captures(line)
                        .and_then(|caps| caps.name("endpoint").map(|m| m.as_str().to_string()))
                })
            } else {
                None
            };

            let mode = if has_endpoint && endpoint.is_some() {
                RunDetectionMode::EndpointRegex {
                    pattern: pattern.clone(),
                }
            } else {
                RunDetectionMode::RunningRegex {
                    pattern: pattern.clone(),
                }
            };

            // ---- Persist the detection mode to the database ----
            match store.get_repo_action(&action_id) {
                Ok(Some(mut action)) => {
                    action.run_detection_mode = Some(mode);
                    if let Err(e) = store.update_repo_action(&action) {
                        log::error!(
                            "autodetect_poller: failed to persist detection mode for {action_id}: {e}"
                        );
                    }
                }
                Ok(None) => {
                    log::warn!("autodetect_poller: action {action_id} not found in store");
                }
                Err(e) => {
                    log::error!("autodetect_poller: failed to read action {action_id}: {e}");
                }
            }

            // ---- Transition RunPhase to Running ----
            let phase = RunPhase::Running {
                endpoint: endpoint.clone(),
            };
            registry.set_run_phase(&execution_id, phase.clone());
            emit_run_phase_changed(
                &app_handle,
                RunPhaseChangedEvent {
                    execution_id: execution_id.clone(),
                    branch_id: branch_id.clone(),
                    action_name: action_name.clone(),
                    phase,
                },
            );

            log::info!(
                "autodetect_poller: detected running state for {execution_id} \
                 (endpoint={endpoint:?}, pattern={pattern})"
            );

            // Done — stop the poller.
            return;
        }

        // ---- Timeout: 20-minute schedule exhausted ----
        log::info!("autodetect_poller: giving up after schedule exhausted for {execution_id}");

        // Persist NoDetection.
        match store.get_repo_action(&action_id) {
            Ok(Some(mut action)) => {
                action.run_detection_mode = Some(RunDetectionMode::NoDetection);
                if let Err(e) = store.update_repo_action(&action) {
                    log::error!(
                        "autodetect_poller: failed to persist NoDetection for {action_id}: {e}"
                    );
                }
            }
            Ok(None) => {
                log::warn!("autodetect_poller: action {action_id} not found in store");
            }
            Err(e) => {
                log::error!("autodetect_poller: failed to read action {action_id}: {e}");
            }
        }

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
    });
}
