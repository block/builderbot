use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter};

use crate::store::Store;

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
    pub status: ActionStatus,
    pub exit_code: Option<i32>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionStatus {
    Running,
    Completed,
    Failed,
    Stopped,
}

/// Represents a single output chunk with its metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputChunk {
    pub chunk: String,
    pub stream: String, // "stdout" or "stderr"
    pub timestamp: i64,
}

/// Tracks a running action
struct RunningActionState {
    execution_id: String,
    action_id: String,
    action_name: String,
    branch_id: String,
    started_at: i64,
    #[allow(dead_code)]
    child_pid: Option<u32>,
    output_buffer: Arc<Mutex<Vec<OutputChunk>>>,
}

/// Manages action execution
pub struct ActionRunner {
    running: Arc<Mutex<HashMap<String, RunningActionState>>>,
}

impl Default for ActionRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionRunner {
    pub fn new() -> Self {
        Self {
            running: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Execute an action in the given worktree directory
    pub fn run_action(
        &self,
        app: AppHandle,
        store: Arc<Store>,
        branch_id: String,
        action_id: String,
        worktree_path: String,
    ) -> Result<String> {
        let execution_id = uuid::Uuid::new_v4().to_string();

        // Get the action from store
        let action = store
            .get_project_action(&action_id)?
            .context("Action not found")?;

        // Determine which shell to use
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

        // Build commands to pipe to shell stdin
        // We use stdin instead of -c to ensure directory hooks fire before command execution.
        // When using -c, the command runs immediately before hooks can activate Hermit.
        let commands = format!("{}\nexit\n", action.command);

        // Use interactive (-i) + login (-l) + stdin (-s) with stdin piping to ensure:
        // 1. Interactive mode triggers directory-based hooks (like Hermit's chpwd/precmd)
        // 2. Login shell loads the full environment
        // 3. -s flag forces shell to read commands from stdin (critical for non-TTY context)
        // 4. Stdin commands execute AFTER shell initialization and hook activation
        let mut child = Command::new(&shell)
            .current_dir(&worktree_path) // Start in target directory to trigger directory hooks
            .env_clear() // Clear all inherited environment variables
            .env("HOME", std::env::var("HOME").unwrap_or_default()) // Preserve HOME for shell profile loading
            .env("USER", std::env::var("USER").unwrap_or_default()) // Preserve USER for shell profile loading
            .env("SHELL", &shell) // Preserve SHELL so it knows which shell it is
            .arg("-i") // Interactive shell to trigger hooks like chpwd for Hermit
            .arg("-l") // Login shell to load profile
            .arg("-s") // Force shell to read commands from stdin (required for non-TTY)
            .stdin(Stdio::piped()) // Pipe stdin to send commands after initialization
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn action process")?;

        let child_pid = child.id();

        // Write commands to stdin, flush, and close it
        if let Some(mut stdin) = child.stdin.take() {
            let commands_clone = commands.clone();
            // Spawn a thread to write to stdin to avoid blocking
            thread::spawn(move || {
                if let Err(e) = stdin.write_all(commands_clone.as_bytes()) {
                    eprintln!("Failed to write to stdin: {}", e);
                    return;
                }
                // Explicitly flush to ensure commands are sent
                if let Err(e) = stdin.flush() {
                    eprintln!("Failed to flush stdin: {}", e);
                }
                // stdin is automatically closed when dropped
            });
        }

        // Create output buffer
        let output_buffer = Arc::new(Mutex::new(Vec::new()));

        // Record the running action
        {
            let mut running = self.running.lock().unwrap();
            running.insert(
                execution_id.clone(),
                RunningActionState {
                    execution_id: execution_id.clone(),
                    action_id: action_id.clone(),
                    action_name: action.name.clone(),
                    branch_id: branch_id.clone(),
                    started_at: crate::store::now_timestamp(),
                    child_pid: Some(child_pid),
                    output_buffer: output_buffer.clone(),
                },
            );
        }

        // Emit initial status event
        let _ = app.emit(
            "action_status",
            ActionStatusEvent {
                execution_id: execution_id.clone(),
                branch_id: branch_id.clone(),
                action_id: action_id.clone(),
                action_name: action.name.clone(),
                status: ActionStatus::Running,
                exit_code: None,
                started_at: crate::store::now_timestamp(),
                completed_at: None,
            },
        );

        // Spawn threads to read stdout and stderr
        let exec_id = execution_id.clone();
        let app_clone = app.clone();
        let buffer_clone = output_buffer.clone();
        if let Some(mut stdout) = child.stdout.take() {
            thread::spawn(move || {
                let mut buffer = [0u8; 1024];
                loop {
                    match stdout.read(&mut buffer) {
                        Ok(0) => break, // EOF
                        Ok(n) => {
                            // Convert bytes to string, preserving all control characters
                            let chunk = String::from_utf8_lossy(&buffer[..n]).to_string();
                            let timestamp = crate::store::now_timestamp();

                            // Store in buffer
                            {
                                let mut buf = buffer_clone.lock().unwrap();
                                buf.push(OutputChunk {
                                    chunk: chunk.clone(),
                                    stream: "stdout".to_string(),
                                    timestamp,
                                });
                            }

                            // Emit event
                            let _ = app_clone.emit(
                                "action_output",
                                ActionOutputEvent {
                                    execution_id: exec_id.clone(),
                                    chunk,
                                    stream: "stdout".to_string(),
                                },
                            );
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        let exec_id = execution_id.clone();
        let app_clone = app.clone();
        let buffer_clone = output_buffer.clone();
        if let Some(mut stderr) = child.stderr.take() {
            thread::spawn(move || {
                let mut buffer = [0u8; 1024];
                loop {
                    match stderr.read(&mut buffer) {
                        Ok(0) => break, // EOF
                        Ok(n) => {
                            // Convert bytes to string, preserving all control characters
                            let chunk = String::from_utf8_lossy(&buffer[..n]).to_string();
                            let timestamp = crate::store::now_timestamp();

                            // Store in buffer
                            {
                                let mut buf = buffer_clone.lock().unwrap();
                                buf.push(OutputChunk {
                                    chunk: chunk.clone(),
                                    stream: "stderr".to_string(),
                                    timestamp,
                                });
                            }

                            // Emit event
                            let _ = app_clone.emit(
                                "action_output",
                                ActionOutputEvent {
                                    execution_id: exec_id.clone(),
                                    chunk,
                                    stream: "stderr".to_string(),
                                },
                            );
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        // Spawn thread to wait for completion
        let exec_id = execution_id.clone();
        let running_clone = self.running.clone();
        let app_clone = app.clone();
        let _store_clone = store.clone();
        let branch_id_clone = branch_id.clone();
        let worktree_path_clone = worktree_path.clone();
        let auto_commit = action.auto_commit;
        let action_name = action.name.clone();

        thread::spawn(move || {
            let exit_status = child.wait();
            let exit_code = exit_status.as_ref().ok().and_then(|s| s.code());
            let completed_at = crate::store::now_timestamp();

            // Remove from running actions
            {
                let mut running = running_clone.lock().unwrap();
                running.remove(&exec_id);
            }

            let success = exit_status.as_ref().map(|s| s.success()).unwrap_or(false);

            // Emit completion status
            let _ = app_clone.emit(
                "action_status",
                ActionStatusEvent {
                    execution_id: exec_id.clone(),
                    branch_id: branch_id_clone.clone(),
                    action_id: action_id.clone(),
                    action_name: action_name.clone(),
                    status: if success {
                        ActionStatus::Completed
                    } else {
                        ActionStatus::Failed
                    },
                    exit_code,
                    started_at: crate::store::now_timestamp(), // Will be overridden by frontend
                    completed_at: Some(completed_at),
                },
            );

            // If auto_commit is enabled and action succeeded, commit changes
            if auto_commit && success {
                if let Err(e) = Self::auto_commit_changes(&worktree_path_clone, &action_name) {
                    eprintln!("Failed to auto-commit changes: {}", e);
                } else {
                    // Emit event to notify frontend of the commit
                    let _ = app_clone.emit(
                        "action_auto_commit",
                        serde_json::json!({
                            "executionId": exec_id,
                            "branchId": branch_id_clone,
                            "actionName": action_name,
                        }),
                    );
                }
            }
        });

        Ok(execution_id)
    }

    /// Auto-commit changes after a successful action
    fn auto_commit_changes(worktree_path: &str, action_name: &str) -> Result<()> {
        // Check if there are any changes
        let status = Command::new("git")
            .arg("diff")
            .arg("--exit-code")
            .current_dir(worktree_path)
            .status()?;

        // If exit code is 0, no changes exist
        if status.success() {
            return Ok(());
        }

        // Stage all changes
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(worktree_path)
            .status()
            .context("Failed to stage changes")?;

        // Commit with action name
        let commit_message = format!("chore: {}", action_name);
        Command::new("git")
            .args(["commit", "-m", &commit_message])
            .current_dir(worktree_path)
            .status()
            .context("Failed to commit changes")?;

        Ok(())
    }

    /// Stop a running action
    pub fn stop_action(&self, execution_id: &str) -> Result<()> {
        let state = {
            let mut running = self.running.lock().unwrap();
            running.remove(execution_id)
        };

        if let Some(state) = state {
            if let Some(pid) = state.child_pid {
                // Kill the process
                #[cfg(unix)]
                {
                    unsafe {
                        libc::kill(pid as i32, libc::SIGTERM);
                    }
                }

                #[cfg(windows)]
                {
                    Command::new("taskkill")
                        .args(["/PID", &pid.to_string(), "/F"])
                        .status()?;
                }
            }
        }

        Ok(())
    }

    /// Get all running actions for a branch
    pub fn get_running_actions(&self, branch_id: &str) -> Vec<ActionStatusEvent> {
        let running = self.running.lock().unwrap();
        running
            .values()
            .filter(|state| state.branch_id == branch_id)
            .map(|state| ActionStatusEvent {
                execution_id: state.execution_id.clone(),
                branch_id: state.branch_id.clone(),
                action_id: state.action_id.clone(),
                action_name: state.action_name.clone(),
                status: ActionStatus::Running,
                exit_code: None,
                started_at: state.started_at,
                completed_at: None,
            })
            .collect()
    }

    /// Get buffered output for an execution
    pub fn get_buffered_output(&self, execution_id: &str) -> Option<Vec<OutputChunk>> {
        let running = self.running.lock().unwrap();
        if let Some(state) = running.get(execution_id) {
            let buffer = state.output_buffer.lock().unwrap();
            Some(buffer.clone())
        } else {
            None
        }
    }
}
