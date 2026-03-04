//! Action execution engine
//!
//! Manages spawning shell commands with real-time output streaming,
//! tracking running processes, and handling termination.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::sync::oneshot;

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
struct RunningActionState {
    child_pid: Option<u32>,
    output_buffer: Arc<Mutex<Vec<OutputChunk>>>,
}

/// Manages action execution with real-time output streaming
pub struct ActionExecutor {
    running: Arc<Mutex<HashMap<String, RunningActionState>>>,
    completed: Arc<Mutex<HashMap<String, Vec<OutputChunk>>>>,
    /// Tracks execution IDs that have been explicitly stopped by the user.
    /// The completion thread checks this to emit `Stopped` instead of `Failed`.
    stopped: Arc<Mutex<HashSet<String>>>,
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
            completed: Arc::new(Mutex::new(HashMap::new())),
            stopped: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Internal implementation that spawns the command and returns both the
    /// execution ID and a oneshot receiver that fires when the action completes.
    async fn execute_inner(
        &self,
        command: String,
        working_dir: String,
        metadata: ActionMetadata,
        listener: Arc<dyn ExecutionListener>,
    ) -> Result<(String, oneshot::Receiver<()>)> {
        let execution_id = uuid::Uuid::new_v4().to_string();
        let (completion_tx, completion_rx) = oneshot::channel();

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
        let mut cmd = Command::new(&shell);
        cmd.current_dir(&working_dir) // Start in target directory to trigger directory hooks
            .env_clear() // Clear all inherited environment variables
            .env("HOME", std::env::var("HOME").unwrap_or_default()) // Preserve HOME for shell profile loading
            .env("USER", std::env::var("USER").unwrap_or_default()) // Preserve USER for shell profile loading
            .env("SHELL", &shell) // Preserve SHELL so it knows which shell it is
            .arg("-i") // Interactive shell to trigger hooks like chpwd for Hermit
            .arg("-l") // Login shell to load profile
            .arg("-s") // Force shell to read commands from stdin (required for non-TTY)
            .stdin(Stdio::piped()) // Pipe stdin to send commands after initialization
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Create a new process group so we can kill the shell AND all its children.
        // Without this, SIGTERM only reaches the shell process, leaving child
        // processes (e.g. `npm run build`, `cargo build`) running as orphans.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: `setsid()` is an async-signal-safe function that creates a new
            // session and process group. It is safe to call in a pre_exec hook.
            unsafe {
                cmd.pre_exec(|| {
                    // Create a new session (and process group) for this child.
                    // All processes spawned by the shell will inherit this group.
                    libc::setsid();
                    Ok(())
                });
            }
        }

        let mut child = cmd.spawn().context("Failed to spawn action process")?;

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
        let completed_clone = self.completed.clone();
        let stopped_clone = self.stopped.clone();
        let working_dir_clone = working_dir.clone();
        let auto_commit = metadata.auto_commit;
        let action_name = metadata.action_name.clone();
        let action_started_at = started_at;

        thread::spawn(move || {
            let exit_status = child.wait();
            let exit_code = exit_status.as_ref().ok().and_then(|s| s.code());
            let completed_at = now_timestamp();

            // Check whether this execution was explicitly stopped by the user
            let was_stopped = {
                let mut stopped = stopped_clone.lock().unwrap();
                stopped.remove(&exec_id)
            };

            // Move output buffer to completed actions map and remove from running
            {
                let mut running = running_clone.lock().unwrap();
                if let Some(state) = running.remove(&exec_id) {
                    let output = state.output_buffer.lock().unwrap().clone();
                    let mut completed = completed_clone.lock().unwrap();
                    completed.insert(exec_id.clone(), output);
                }
            }

            let success = exit_status.as_ref().map(|s| s.success()).unwrap_or(false);

            // Determine the correct status:
            // - If explicitly stopped by the user → Stopped
            // - If exited successfully → Completed
            // - Otherwise → Failed
            let status = if was_stopped {
                ActionStatus::Stopped
            } else if success {
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
                    if auto_commit && success && !was_stopped {
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

            // Signal completion (ignore error if receiver was dropped)
            let _ = completion_tx.send(());
        });

        Ok((execution_id, completion_rx))
    }

    /// Execute a shell command in the specified working directory
    ///
    /// Returns a unique execution ID. The action runs in the background.
    /// Use `execute_and_wait` if you need to wait for completion.
    pub async fn execute(
        &self,
        command: String,
        working_dir: String,
        metadata: ActionMetadata,
        listener: Arc<dyn ExecutionListener>,
    ) -> Result<String> {
        let (execution_id, _completion_rx) = self
            .execute_inner(command, working_dir, metadata, listener)
            .await?;
        Ok(execution_id)
    }

    /// Execute a shell command and wait for it to complete
    ///
    /// Returns the execution ID after the action has finished.
    /// This is useful for running actions sequentially.
    pub async fn execute_and_wait(
        &self,
        command: String,
        working_dir: String,
        metadata: ActionMetadata,
        listener: Arc<dyn ExecutionListener>,
    ) -> Result<String> {
        let (execution_id, completion_rx) = self
            .execute_inner(command, working_dir, metadata, listener)
            .await?;

        // Wait for the background thread to signal completion
        let _ = completion_rx.await;

        Ok(execution_id)
    }

    /// Execute a pre-built command (e.g. for remote workspace execution via
    /// `sq blox ws exec`). Instead of wrapping the command in an interactive
    /// login shell, the given `program` is spawned directly with `args`.
    ///
    /// Streaming output, stop, and completion tracking work identically to
    /// local execution since it is still a local child process under the hood.
    ///
    /// Returns a unique execution ID. The action runs in the background.
    pub async fn execute_remote(
        &self,
        program: PathBuf,
        args: Vec<String>,
        metadata: ActionMetadata,
        listener: Arc<dyn ExecutionListener>,
    ) -> Result<String> {
        let (execution_id, _completion_rx) = self
            .execute_remote_inner(program, args, metadata, listener)
            .await?;
        Ok(execution_id)
    }

    /// Internal implementation for remote execution.
    async fn execute_remote_inner(
        &self,
        program: PathBuf,
        args: Vec<String>,
        _metadata: ActionMetadata,
        listener: Arc<dyn ExecutionListener>,
    ) -> Result<(String, oneshot::Receiver<()>)> {
        let execution_id = uuid::Uuid::new_v4().to_string();
        let (completion_tx, completion_rx) = oneshot::channel();

        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Create a new process group so we can kill the proxy AND all its
        // children together, same as local execution.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: `setsid()` is async-signal-safe.
            unsafe {
                cmd.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }

        let mut child = cmd
            .spawn()
            .context("Failed to spawn remote action process")?;

        let child_pid = child.id();

        // Create output buffer
        let output_buffer = Arc::new(Mutex::new(Vec::new()));
        let started_at = now_timestamp();

        // Record the running action
        {
            let mut running = self.running.lock().unwrap();
            running.insert(
                execution_id.clone(),
                RunningActionState {
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

        // Spawn threads to read stdout and stderr (identical to local execution)
        let exec_id = execution_id.clone();
        let listener_clone = listener.clone();
        let buffer_clone = output_buffer.clone();
        if let Some(mut stdout) = child.stdout.take() {
            thread::spawn(move || {
                let mut buffer = [0u8; 1024];
                loop {
                    match stdout.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = String::from_utf8_lossy(&buffer[..n]).to_string();
                            let timestamp = now_timestamp();

                            {
                                let mut buf = buffer_clone.lock().unwrap();
                                buf.push(OutputChunk {
                                    chunk: chunk.clone(),
                                    stream: "stdout".to_string(),
                                    timestamp,
                                });
                            }

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
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = String::from_utf8_lossy(&buffer[..n]).to_string();
                            let timestamp = now_timestamp();

                            {
                                let mut buf = buffer_clone.lock().unwrap();
                                buf.push(OutputChunk {
                                    chunk: chunk.clone(),
                                    stream: "stderr".to_string(),
                                    timestamp,
                                });
                            }

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
        let completed_clone = self.completed.clone();
        let stopped_clone = self.stopped.clone();
        let action_started_at = started_at;

        thread::spawn(move || {
            let exit_status = child.wait();
            let exit_code = exit_status.as_ref().ok().and_then(|s| s.code());
            let completed_at = now_timestamp();

            let was_stopped = {
                let mut stopped = stopped_clone.lock().unwrap();
                stopped.remove(&exec_id)
            };

            {
                let mut running = running_clone.lock().unwrap();
                if let Some(state) = running.remove(&exec_id) {
                    let output = state.output_buffer.lock().unwrap().clone();
                    let mut completed = completed_clone.lock().unwrap();
                    completed.insert(exec_id.clone(), output);
                }
            }

            let success = exit_status.as_ref().map(|s| s.success()).unwrap_or(false);

            let status = if was_stopped {
                ActionStatus::Stopped
            } else if success {
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

                    // Note: auto_commit is not supported for remote execution
                    // since the working directory is on the remote workspace.
                });

            let _ = completion_tx.send(());
        });

        Ok((execution_id, completion_rx))
    }

    /// Stop a running action by execution ID.
    ///
    /// Marks the execution as stopped and sends SIGTERM to the entire process
    /// group. The completion thread will see the stopped flag and emit a
    /// `Stopped` status event once the process actually exits.
    pub fn stop(&self, execution_id: &str) -> Result<()> {
        // Mark as stopped BEFORE sending the signal so the completion thread
        // knows this was an intentional stop (not a crash/failure).
        {
            let mut stopped = self.stopped.lock().unwrap();
            stopped.insert(execution_id.to_string());
        }

        // Read the PID but do NOT remove from the running map. The completion
        // thread needs the entry to move the output buffer to the completed map
        // and to emit the proper StatusChanged event.
        let pid = {
            let running = self.running.lock().unwrap();
            running.get(execution_id).and_then(|state| state.child_pid)
        };

        if let Some(pid) = pid {
            #[cfg(unix)]
            {
                // Send SIGTERM to the entire process group (negative PID).
                //
                // Because we used `setsid()` in pre_exec, the shell and all its
                // children share a process group whose PGID equals the shell's PID.
                // Sending the signal to `-pid` reaches every process in the group,
                // ensuring child processes (npm, cargo, etc.) are also terminated.
                //
                // SAFETY: Calling `libc::kill` with a negative PID targets a process
                // group. The PID came from `Child::id()` at spawn time. If the group
                // no longer exists, kill() returns ESRCH which we safely ignore.
                // After SIGTERM, we spawn a background thread that waits briefly and
                // escalates to SIGKILL if the group is still alive.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGTERM);
                }

                // Escalate to SIGKILL after a short grace period in case the
                // process group ignores SIGTERM (e.g. a process traps the signal).
                thread::spawn(move || {
                    thread::sleep(std::time::Duration::from_secs(5));
                    // SAFETY: Same considerations as above. If the process group
                    // already exited, kill() harmlessly returns ESRCH.
                    unsafe {
                        // Check if the process group still exists before sending SIGKILL
                        let ret = libc::kill(-(pid as i32), 0);
                        if ret == 0 {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                    }
                });
            }

            #[cfg(windows)]
            {
                // Use taskkill with /T to kill the process tree on Windows
                let _ = Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .status();
            }
        }

        Ok(())
    }

    /// Get buffered output for an execution
    pub fn get_buffered_output(&self, execution_id: &str) -> Option<Vec<OutputChunk>> {
        // First check running actions
        let running = self.running.lock().unwrap();
        if let Some(state) = running.get(execution_id) {
            let buffer = state.output_buffer.lock().unwrap();
            return Some(buffer.clone());
        }
        drop(running);

        // If not running, check completed actions
        let completed = self.completed.lock().unwrap();
        completed.get(execution_id).cloned()
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

    /// Clear buffered output for a completed execution
    /// This removes the output from the completed actions map
    pub fn clear_execution(&self, execution_id: &str) -> bool {
        let mut completed = self.completed.lock().unwrap();
        completed.remove(execution_id).is_some()
    }
}

/// Get current timestamp in milliseconds
fn now_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
