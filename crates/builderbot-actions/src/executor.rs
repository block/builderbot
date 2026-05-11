//! Action execution engine
//!
//! Manages spawning shell commands with real-time output streaming,
//! tracking running processes, and handling termination.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use tokio::sync::oneshot;

use crate::git::{auto_commit_if_changes, auto_commit_if_changes_remote};
use crate::models::{ActionStatus, ExecutionEvent, OutputChunk};

// =============================================================================
// PTY support (unix only)
// =============================================================================

#[cfg(unix)]
mod pty {
    use std::os::fd::RawFd;

    // openpty is not exposed by the libc crate on all platforms, so we
    // declare the extern ourselves. It is available on macOS (<util.h>)
    // and Linux (<pty.h>).
    extern "C" {
        fn openpty(
            amaster: *mut RawFd,
            aslave: *mut RawFd,
            name: *mut libc::c_char,
            termp: *const libc::termios,
            winp: *const libc::winsize,
        ) -> libc::c_int;
    }

    /// Open a PTY pair and return `(master_fd, slave_fd)`.
    pub fn open() -> std::io::Result<(RawFd, RawFd)> {
        let mut master: RawFd = -1;
        let mut slave: RawFd = -1;

        // SAFETY: openpty writes into the provided pointers and returns 0
        // on success, -1 on failure. The NULL pointers for name/termp/winp
        // are explicitly allowed by the API.
        let ret = unsafe {
            openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok((master, slave))
    }

    /// Configure the PTY master: disable echo (so piped stdin commands don't
    /// appear in the output) and set a reasonable terminal size.
    pub fn configure(master_fd: RawFd) -> std::io::Result<()> {
        // Disable echo so commands written to stdin are not reflected in output.
        // SAFETY: tcgetattr/tcsetattr operate on a valid fd returned by openpty.
        unsafe {
            let mut termios: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(master_fd, &mut termios) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            termios.c_lflag &= !(libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ECHONL);
            if libc::tcsetattr(master_fd, libc::TCSANOW, &termios) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }

        // Set a wide terminal size so progress bars don't wrap.
        // SAFETY: ioctl with TIOCSWINSZ writes the winsize struct to the tty.
        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            ws.ws_col = 200;
            ws.ws_row = 50;
            libc::ioctl(master_fd, libc::TIOCSWINSZ, &ws);
        }

        Ok(())
    }
}

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

#[derive(Clone, Copy, Debug)]
pub struct StopOptions {
    pub force_kill_after: Option<Duration>,
}

impl Default for StopOptions {
    fn default() -> Self {
        Self {
            force_kill_after: Some(Duration::from_secs(5)),
        }
    }
}

/// Internal state for a running action
struct RunningActionState {
    child_pid: Option<u32>,
    output_buffer: Arc<Mutex<Vec<OutputChunk>>>,
}

/// Context for auto-committing changes after a successful action execution.
enum AutoCommitContext {
    /// Local execution: run git commands directly against the worktree.
    Local { working_dir: String },
    /// Remote execution: run git commands via `sq blox ws exec`.
    Remote {
        sq_binary: PathBuf,
        workspace_name: String,
        working_dir: String,
    },
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

/// Spawn a thread that reads from `stream` and emits output events via `listener`.
///
/// Each chunk is also appended to `buffer` for later retrieval.
fn spawn_stream_reader(
    mut stream: impl Read + Send + 'static,
    stream_name: &'static str,
    exec_id: String,
    listener: Arc<dyn ExecutionListener>,
    buffer: Arc<Mutex<Vec<OutputChunk>>>,
    handle: Handle,
) {
    thread::spawn(move || {
        let mut buf = [0u8; 1024];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                    let timestamp = now_timestamp();

                    // Store in buffer
                    {
                        let mut b = buffer.lock().unwrap();
                        b.push(OutputChunk {
                            chunk: chunk.clone(),
                            stream: stream_name.to_string(),
                            timestamp,
                        });
                    }

                    // Emit event
                    let listener = listener.clone();
                    let exec_id = exec_id.clone();
                    handle.block_on(async move {
                        listener
                            .on_event(ExecutionEvent::Output {
                                execution_id: exec_id,
                                chunk,
                                stream: stream_name.to_string(),
                                timestamp,
                            })
                            .await;
                    });
                }
                // PTY master returns EIO (not EOF) when the slave side is
                // closed, which is the normal shutdown path. Treat all read
                // errors as end-of-stream.
                Err(_) => break,
            }
        }
    });
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

    /// Manage a spawned child process: track it, stream its output, and emit
    /// completion events. This is the shared implementation used by both local
    /// and remote execution paths.
    ///
    /// If `auto_commit_ctx` is `Some`, auto-commit will be attempted after a
    /// successful execution (local only).
    ///
    /// If `pty_reader` is `Some`, it is used as the single output stream
    /// (tagged as `"stdout"`) instead of the child's piped stdout/stderr.
    /// This is used for local execution where a PTY merges both streams.
    async fn manage_child_process(
        &self,
        mut child: Child,
        metadata: ActionMetadata,
        listener: Arc<dyn ExecutionListener>,
        auto_commit_ctx: Option<AutoCommitContext>,
        pty_reader: Option<std::fs::File>,
    ) -> Result<(String, oneshot::Receiver<()>)> {
        let execution_id = uuid::Uuid::new_v4().to_string();
        let (completion_tx, completion_rx) = oneshot::channel();

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

        // Capture the current Tokio runtime handle for use in background threads.
        // These threads are always spawned from within a Tokio context (a Tauri
        // command handler), so `Handle::current()` is guaranteed to succeed.
        let handle = Handle::current();

        // Spawn threads to read output.
        //
        // When a PTY reader is provided (local execution), we use it as the
        // single output stream — the PTY merges stdout and stderr, matching
        // real terminal behavior. Otherwise (remote execution), we read from
        // the child's piped stdout and stderr separately.
        if let Some(pty_master) = pty_reader {
            spawn_stream_reader(
                pty_master,
                "stdout",
                execution_id.clone(),
                listener.clone(),
                output_buffer.clone(),
                handle.clone(),
            );
        } else {
            if let Some(stdout) = child.stdout.take() {
                spawn_stream_reader(
                    stdout,
                    "stdout",
                    execution_id.clone(),
                    listener.clone(),
                    output_buffer.clone(),
                    handle.clone(),
                );
            }

            if let Some(stderr) = child.stderr.take() {
                spawn_stream_reader(
                    stderr,
                    "stderr",
                    execution_id.clone(),
                    listener.clone(),
                    output_buffer.clone(),
                    handle.clone(),
                );
            }
        }

        // Spawn thread to wait for completion
        let exec_id = execution_id.clone();
        let running_clone = self.running.clone();
        let completed_clone = self.completed.clone();
        let stopped_clone = self.stopped.clone();
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

            handle.block_on(async {
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
                    if let Some(ctx) = &auto_commit_ctx {
                        let result = match ctx {
                            AutoCommitContext::Local { working_dir } => {
                                auto_commit_if_changes(working_dir, &action_name)
                            }
                            AutoCommitContext::Remote {
                                sq_binary,
                                workspace_name,
                                working_dir,
                            } => auto_commit_if_changes_remote(
                                sq_binary,
                                workspace_name,
                                working_dir,
                                &action_name,
                            ),
                        };
                        match result {
                            Ok(true) => {
                                // Emit auto-commit event
                                listener
                                    .on_event(ExecutionEvent::AutoCommit {
                                        execution_id: exec_id.clone(),
                                        action_name: action_name.clone(),
                                    })
                                    .await;
                            }
                            Ok(false) => {} // No changes to commit
                            Err(e) => {
                                eprintln!("Failed to auto-commit changes: {}", e);
                            }
                        }
                    }
                }
            });

            // Signal completion (ignore error if receiver was dropped)
            let _ = completion_tx.send(());
        });

        Ok((execution_id, completion_rx))
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
        // Determine which shell to use
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

        // Build commands to pipe to shell stdin
        // We use stdin instead of -c to ensure directory hooks fire before command execution.
        // When using -c, the command runs immediately before hooks can activate Hermit.
        let commands = format!("{}\nexit\n", command);

        // ---- PTY setup (unix) -------------------------------------------------
        // Open a PTY so child processes see a real terminal and use \r for
        // progress bars instead of falling back to \n-separated output.
        #[cfg(unix)]
        let (pty_master_fd, pty_slave_fd) = pty::open().context("Failed to open PTY")?;
        #[cfg(unix)]
        {
            pty::configure(pty_master_fd).context("Failed to configure PTY")?;
        }

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
            .stdin(Stdio::piped()); // Pipe stdin to send commands after initialization

        // On unix, redirect stdout/stderr to the PTY slave in the child process.
        // On other platforms, fall back to piped stdout/stderr.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;

            // stdout/stderr will be overridden by dup2 in pre_exec, so use null
            // as a placeholder (the child never writes to these fds directly).
            cmd.stdout(Stdio::null()).stderr(Stdio::null());

            // SAFETY: All functions called here are async-signal-safe:
            // - setsid(): creates new session/process group
            // - ioctl(TIOCSCTTY): sets controlling terminal
            // - dup2(): duplicates file descriptors
            // - close(): closes file descriptor
            unsafe {
                cmd.pre_exec(move || {
                    // Create a new session (and process group) for this child.
                    // All processes spawned by the shell will inherit this group.
                    libc::setsid();

                    // Make the PTY slave the controlling terminal for the session.
                    libc::ioctl(pty_slave_fd, libc::TIOCSCTTY as _, 0);

                    // Redirect stdout and stderr to the PTY slave so child
                    // processes see a real terminal (isatty() returns true).
                    libc::dup2(pty_slave_fd, libc::STDOUT_FILENO);
                    libc::dup2(pty_slave_fd, libc::STDERR_FILENO);

                    // Close the original slave fd (it has been duplicated above).
                    if pty_slave_fd > libc::STDERR_FILENO {
                        libc::close(pty_slave_fd);
                    }

                    Ok(())
                });
            }
        }

        #[cfg(not(unix))]
        {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        }

        let mut child = cmd.spawn().context("Failed to spawn action process")?;

        // Close the slave fd in the parent — the child inherited it via fork.
        #[cfg(unix)]
        {
            // SAFETY: close() on a valid fd returned by openpty is safe.
            // The fd is no longer needed in the parent process.
            unsafe {
                libc::close(pty_slave_fd);
            }
        }

        // Wrap the PTY master fd in a File for reading.
        #[cfg(unix)]
        let pty_reader = {
            use std::os::fd::FromRawFd;
            // SAFETY: master_fd is a valid fd returned by openpty. Wrapping it
            // in a File transfers ownership — File will close it on drop.
            Some(unsafe { std::fs::File::from_raw_fd(pty_master_fd) })
        };
        #[cfg(not(unix))]
        let pty_reader: Option<std::fs::File> = None;

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

        let auto_commit_ctx = AutoCommitContext::Local { working_dir };

        self.manage_child_process(child, metadata, listener, Some(auto_commit_ctx), pty_reader)
            .await
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
    /// If `auto_commit_info` is provided (sq binary path, workspace name,
    /// working dir), auto-commit will be attempted after a successful
    /// execution by running git commands on the remote workspace.
    ///
    /// Returns a unique execution ID. The action runs in the background.
    pub async fn execute_remote(
        &self,
        program: PathBuf,
        args: Vec<String>,
        metadata: ActionMetadata,
        listener: Arc<dyn ExecutionListener>,
        auto_commit_info: Option<(PathBuf, String, String)>,
    ) -> Result<String> {
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

        let child = cmd
            .spawn()
            .context("Failed to spawn remote action process")?;

        let auto_commit_ctx = auto_commit_info.map(|(sq_binary, workspace_name, working_dir)| {
            AutoCommitContext::Remote {
                sq_binary,
                workspace_name,
                working_dir,
            }
        });

        let (execution_id, _completion_rx) = self
            .manage_child_process(child, metadata, listener, auto_commit_ctx, None)
            .await?;
        Ok(execution_id)
    }

    /// Stop a running action by execution ID.
    ///
    /// Marks the execution as stopped and sends SIGTERM to the entire process
    /// group. The completion thread will see the stopped flag and emit a
    /// `Stopped` status event once the process actually exits.
    pub fn stop(&self, execution_id: &str) -> Result<()> {
        self.stop_with_options(execution_id, StopOptions::default())
    }

    /// Stop a running action with configurable shutdown timing.
    pub fn stop_with_options(&self, execution_id: &str, options: StopOptions) -> Result<()> {
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
                // Send SIGTERM to every process in the session.
                //
                // Because we used `setsid()` in pre_exec, the shell is the
                // session leader and all descendant processes share its SID.
                // However, the interactive shell (`-i`) uses job control and
                // places child commands in their own process groups. A simple
                // `kill(-pid, SIGTERM)` only reaches the shell's process group,
                // missing child process groups entirely. Additionally,
                // interactive shells ignore SIGTERM by default.
                //
                // `pkill -s <sid>` targets every process in the session
                // regardless of process group, which reliably reaches the
                // shell, its child commands, and their descendants.
                let pid_str = pid.to_string();
                let _ = Command::new("pkill")
                    .args(["-TERM", "-s", &pid_str])
                    .status();

                if let Some(force_kill_after) = options.force_kill_after {
                    // Escalate to SIGKILL after a short grace period in case
                    // processes ignore SIGTERM (e.g. a process traps the signal).
                    thread::spawn(move || {
                        thread::sleep(force_kill_after);
                        // Check if any process in the session still exists
                        // before escalating. pkill exits 0 when it matches
                        // at least one process.
                        let probe = Command::new("pkill").args(["-0", "-s", &pid_str]).status();
                        if probe.map(|s| s.success()).unwrap_or(false) {
                            let _ = Command::new("pkill")
                                .args(["-KILL", "-s", &pid_str])
                                .status();
                        }
                    });
                }
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

    /// Wait until all listed executions are no longer running, or until the timeout elapses.
    ///
    /// Returns `true` if every execution completed before the timeout.
    pub fn wait_for_executions(&self, execution_ids: &[String], timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;

        loop {
            let all_stopped = {
                let running = self.running.lock().unwrap();
                execution_ids
                    .iter()
                    .all(|execution_id| !running.contains_key(execution_id))
            };

            if all_stopped {
                return true;
            }

            if Instant::now() >= deadline {
                return false;
            }

            thread::sleep(Duration::from_millis(25));
        }
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
