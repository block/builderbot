//! Action execution engine
//!
//! Manages spawning shell commands with real-time output streaming,
//! tracking running processes, and handling termination.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::git::auto_commit_if_changes;
use crate::models::{ActionStatus, ExecutionEvent, OutputChunk};

/// Trait for receiving execution events
#[async_trait]
pub trait ExecutionListener: Send + Sync {
    /// Called when an execution event occurs
    async fn on_event(&self, event: ExecutionEvent);
}

/// Metadata about an action being executed
#[derive(Clone)]
pub struct ActionMetadata {
    pub action_id: String,
    pub action_name: String,
    pub auto_commit: bool,
}

/// Internal state for a running action
#[allow(dead_code)]
struct RunningActionState {
    execution_id: String,
    action_metadata: ActionMetadata,
    started_at: i64,
    child_pid: Option<u32>,
    output_buffer: Arc<Mutex<Vec<OutputChunk>>>,
}

/// Manages action execution with real-time output streaming
pub struct ActionExecutor {
    running: Arc<Mutex<HashMap<String, RunningActionState>>>,
}

impl Default for ActionExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionExecutor {
    /// Create a new action executor
    pub fn new() -> Self {
        Self {
            running: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Execute a shell command in the specified working directory
    ///
    /// # Arguments
    /// * `command` - The shell command to execute
    /// * `working_dir` - Directory to execute the command in
    /// * `metadata` - Action metadata (id, name, auto_commit flag)
    /// * `listener` - Event listener for execution events
    ///
    /// # Returns
    /// A unique execution ID that can be used to stop the action or retrieve output
    pub async fn execute(
        &self,
        command: String,
        working_dir: String,
        metadata: ActionMetadata,
        listener: Arc<dyn ExecutionListener>,
    ) -> Result<String> {
        let execution_id = uuid::Uuid::new_v4().to_string();

        // Determine which shell to use
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

        // Build commands to pipe to shell stdin
        // We use stdin instead of -c to ensure directory hooks fire before command execution.
        // When using -c, the command runs immediately before hooks can activate Hermit.
        let commands = format!("{}\nexit\n", command);

        // Use interactive (-i) + login (-l) + stdin (-s) with stdin piping to ensure:
        // 1. Interactive mode triggers directory-based hooks (like Hermit's chpwd/precmd)
        // 2. Login shell loads the full environment
        // 3. -s flag forces shell to read commands from stdin (critical for non-TTY context)
        // 4. Stdin commands execute AFTER shell initialization and hook activation
        let mut child = Command::new(&shell)
            .current_dir(&working_dir) // Start in target directory to trigger directory hooks
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
        let started_at = now_timestamp();

        // Record the running action
        {
            let mut running = self.running.lock().unwrap();
            running.insert(
                execution_id.clone(),
                RunningActionState {
                    execution_id: execution_id.clone(),
                    action_metadata: metadata.clone(),
                    started_at,
                    child_pid: Some(child_pid),
                    output_buffer: output_buffer.clone(),
                },
            );
        }

        // Emit initial started event
        listener
            .on_event(ExecutionEvent::Started {
                execution_id: execution_id.clone(),
                started_at,
            })
            .await;

        // Spawn threads to read stdout and stderr
        let exec_id = execution_id.clone();
        let listener_clone = listener.clone();
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
                            let timestamp = now_timestamp();

                            // Store in buffer
                            {
                                let mut buf = buffer_clone.lock().unwrap();
                                buf.push(OutputChunk {
                                    chunk: chunk.clone(),
                                    stream: "stdout".to_string(),
                                    timestamp,
                                });
                            }

                            // Emit event (blocking call in thread is OK)
                            let listener_clone = listener_clone.clone();
                            let exec_id_clone = exec_id.clone();
                            let chunk_clone = chunk.clone();
                            tokio::runtime::Handle::try_current()
                                .unwrap_or_else(|_| {
                                    tokio::runtime::Runtime::new().unwrap().handle().clone()
                                })
                                .block_on(async move {
                                    listener_clone
                                        .on_event(ExecutionEvent::Output {
                                            execution_id: exec_id_clone,
                                            chunk: chunk_clone,
                                            stream: "stdout".to_string(),
                                            timestamp,
                                        })
                                        .await;
                                });
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        let exec_id = execution_id.clone();
        let listener_clone = listener.clone();
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
                            let timestamp = now_timestamp();

                            // Store in buffer
                            {
                                let mut buf = buffer_clone.lock().unwrap();
                                buf.push(OutputChunk {
                                    chunk: chunk.clone(),
                                    stream: "stderr".to_string(),
                                    timestamp,
                                });
                            }

                            // Emit event (blocking call in thread is OK)
                            let listener_clone = listener_clone.clone();
                            let exec_id_clone = exec_id.clone();
                            let chunk_clone = chunk.clone();
                            tokio::runtime::Handle::try_current()
                                .unwrap_or_else(|_| {
                                    tokio::runtime::Runtime::new().unwrap().handle().clone()
                                })
                                .block_on(async move {
                                    listener_clone
                                        .on_event(ExecutionEvent::Output {
                                            execution_id: exec_id_clone,
                                            chunk: chunk_clone,
                                            stream: "stderr".to_string(),
                                            timestamp,
                                        })
                                        .await;
                                });
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        // Spawn thread to wait for completion
        let exec_id = execution_id.clone();
        let running_clone = self.running.clone();
        let working_dir_clone = working_dir.clone();
        let auto_commit = metadata.auto_commit;
        let action_name = metadata.action_name.clone();
        let action_started_at = started_at;

        thread::spawn(move || {
            let exit_status = child.wait();
            let exit_code = exit_status.as_ref().ok().and_then(|s| s.code());
            let completed_at = now_timestamp();

            // Remove from running actions
            {
                let mut running = running_clone.lock().unwrap();
                running.remove(&exec_id);
            }

            let success = exit_status.as_ref().map(|s| s.success()).unwrap_or(false);

            // Emit completion status
            let status = if success {
                ActionStatus::Completed
            } else {
                ActionStatus::Failed
            };

            tokio::runtime::Handle::try_current()
                .unwrap_or_else(|_| tokio::runtime::Runtime::new().unwrap().handle().clone())
                .block_on(async {
                    listener
                        .on_event(ExecutionEvent::StatusChanged {
                            execution_id: exec_id.clone(),
                            status,
                            exit_code,
                            started_at: Some(action_started_at),
                            completed_at: Some(completed_at),
                        })
                        .await;

                    // If auto_commit is enabled and action succeeded, commit changes
                    if auto_commit && success {
                        if let Err(e) = auto_commit_if_changes(&working_dir_clone, &action_name) {
                            eprintln!("Failed to auto-commit changes: {}", e);
                        } else {
                            // Emit auto-commit event
                            listener
                                .on_event(ExecutionEvent::AutoCommit {
                                    execution_id: exec_id.clone(),
                                    action_name: action_name.clone(),
                                })
                                .await;
                        }
                    }
                });
        });

        Ok(execution_id)
    }

    /// Stop a running action by execution ID
    pub fn stop(&self, execution_id: &str) -> Result<()> {
        let state = {
            let mut running = self.running.lock().unwrap();
            running.remove(execution_id)
        };

        if let Some(state) = state {
            if let Some(pid) = state.child_pid {
                #[cfg(unix)]
                {
                    // SAFETY: This unsafe block calls libc::kill to terminate a child process.
                    //
                    // Safety considerations:
                    // 1. PID validity: The PID comes from `std::process::Child::id()` which was
                    //    stored when we spawned the process. While the process may have already
                    //    terminated, calling kill() on a non-existent PID is safe (returns ESRCH).
                    // 2. PID reuse: On Unix systems, PIDs can be reused after process termination.
                    //    However, the window for reuse is typically small, and we only call this
                    //    immediately after removing the process from our tracking map. The risk of
                    //    terminating an unrelated process is minimal in practice.
                    // 3. Signal delivery: SIGTERM is a graceful termination signal that allows
                    //    processes to clean up. This is safer than SIGKILL.
                    // 4. Error handling: We intentionally ignore the return value because:
                    //    - If the process already exited, kill() fails with ESRCH (acceptable)
                    //    - If we lack permissions, we can't do anything about it
                    //    - The process is already removed from our tracking, so we've done our part
                    //
                    // Alternative considered: Using a higher-level library like `sysinfo` or maintaining
                    // a handle to the Child. However, this adds complexity and dependencies without
                    // significantly improving safety for this use case.
                    unsafe {
                        libc::kill(pid as i32, libc::SIGTERM);
                    }
                }

                #[cfg(windows)]
                {
                    // Use taskkill on Windows for graceful termination
                    let _ = Command::new("taskkill")
                        .args(["/PID", &pid.to_string(), "/F"])
                        .status();
                }
            }
        }

        Ok(())
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

    /// Check if an action is currently running
    pub fn is_running(&self, execution_id: &str) -> bool {
        let running = self.running.lock().unwrap();
        running.contains_key(execution_id)
    }

    /// Get all running execution IDs
    pub fn get_running_ids(&self) -> Vec<String> {
        let running = self.running.lock().unwrap();
        running.keys().cloned().collect()
    }
}

/// Get current timestamp in milliseconds
fn now_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
