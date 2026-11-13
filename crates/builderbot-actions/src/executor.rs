//! Action execution engine
//!
//! Manages spawning shell commands with real-time output streaming,
//! tracking running processes, and handling termination.
//!
//! Output is delivered to the listener in batches rather than read by read,
//! and is also buffered so a viewer can catch up on it. That buffer is
//! bounded: each execution keeps only its most recent output, and only the
//! last few finished executions keep theirs.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use tokio::sync::oneshot;

use crate::finished_outputs::{FinishedOutputs, MAX_FINISHED_EXECUTIONS};
use crate::models::{ActionStatus, ExecutionEvent, OutputChunk};
use crate::output_buffer::{OutputBuffer, SharedOutputBuffer, MAX_OUTPUT_BYTES};
use crate::output_emitter::{self, OutputSender, EMIT_INTERVAL, EMIT_MAX_BYTES};
use crate::utf8_stream::Utf8StreamDecoder;

#[cfg(all(test, unix))]
mod tests;

/// How long completion waits, once the process has exited, for the reader
/// threads to reach the end of its output. Only what the kernel still holds
/// is left to read by then, so this elapses only when something that outlived
/// the process keeps the output open.
///
/// Completion then waits as long again, at most, for the listener to be given
/// what was read. That elapses only when the listener is far behind.
const READER_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

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

/// Bounds on what the executor retains and how long it waits. Fixed in
/// production; the tests shrink them.
#[derive(Clone, Copy)]
struct Limits {
    /// Most recent output kept per execution, in bytes.
    max_output_bytes: usize,
    /// How many finished executions keep their output.
    max_finished: usize,
    reader_drain_timeout: Duration,
    /// How long output waits to be merged with what follows it before the
    /// listener is given it.
    emit_interval: Duration,
    /// Size at which a batch of output is delivered without waiting.
    emit_max_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_output_bytes: MAX_OUTPUT_BYTES,
            max_finished: MAX_FINISHED_EXECUTIONS,
            reader_drain_timeout: READER_DRAIN_TIMEOUT,
            emit_interval: EMIT_INTERVAL,
            emit_max_bytes: EMIT_MAX_BYTES,
        }
    }
}

/// Internal state for a running action
struct RunningActionState {
    child_pid: Option<u32>,
    output_buffer: SharedOutputBuffer,
    /// Set by `discard_output`: free the output at exit instead of retaining it.
    discard_output: bool,
}

/// Manages action execution with real-time output streaming
pub struct ActionExecutor {
    running: Arc<Mutex<HashMap<String, RunningActionState>>>,
    /// Output of the most recently finished executions. Locked after
    /// `running` wherever both are held.
    finished: Arc<Mutex<FinishedOutputs>>,
    /// Tracks execution IDs that have been explicitly stopped by the user.
    /// The completion thread checks this to emit `Stopped` instead of `Failed`.
    stopped: Arc<Mutex<HashSet<String>>>,
    limits: Limits,
}

impl Default for ActionExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Spawn a thread that reads from `stream` and queues what it reads on
/// `output`, which buffers it and delivers it to the listener in batches.
///
/// `done` is dropped when the thread reaches the end of the stream, after its
/// last read has been queued, which is how the completion thread learns that
/// all of the output is on its way.
fn spawn_stream_reader(
    mut stream: impl Read + Send + 'static,
    stream_name: &'static str,
    output: OutputSender,
    done: mpsc::Sender<Infallible>,
) {
    thread::spawn(move || {
        let _done = done;

        // A read can end part-way through a multi-byte character, so the
        // decoder carries the unfinished bytes over to the next read.
        let mut decoder = Utf8StreamDecoder::default();
        let mut buf = [0u8; 1024];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => output.send(stream_name, decoder.decode(&buf[..n]), now_timestamp()),
                // PTY master returns EIO (not EOF) when the slave side is
                // closed, which is the normal shutdown path. Treat all read
                // errors as end-of-stream.
                Err(_) => break,
            }
        }
        output.send(stream_name, decoder.finish(), now_timestamp());
    });
}

/// Wait until `deadline` for every reader thread to reach the end of its
/// stream. Returns whether they all did.
///
/// Nothing is ever sent on the channel. Each reader holds a sender and drops
/// it when it ends, so the channel disconnects once the last one is done.
fn wait_for_readers(readers_done: &mpsc::Receiver<Infallible>, deadline: Instant) -> bool {
    matches!(
        readers_done.recv_timeout(deadline.saturating_duration_since(Instant::now())),
        Err(mpsc::RecvTimeoutError::Disconnected)
    )
}

impl ActionExecutor {
    /// Create a new action executor
    pub fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    fn with_limits(limits: Limits) -> Self {
        Self {
            running: Arc::new(Mutex::new(HashMap::new())),
            finished: Arc::new(Mutex::new(FinishedOutputs::new(limits.max_finished))),
            stopped: Arc::new(Mutex::new(HashSet::new())),
            limits,
        }
    }

    /// Manage a spawned child process: track it, stream its output, and emit
    /// completion events. This is the shared implementation used by both local
    /// and remote execution paths.
    ///
    /// If `pty_reader` is `Some`, it is used as the single output stream
    /// (tagged as `"stdout"`) instead of the child's piped stdout/stderr.
    /// This is used for local execution where a PTY merges both streams.
    async fn manage_child_process(
        &self,
        mut child: Child,
        listener: Arc<dyn ExecutionListener>,
        pty_reader: Option<std::fs::File>,
    ) -> Result<(String, oneshot::Receiver<()>)> {
        let execution_id = uuid::Uuid::new_v4().to_string();
        let (completion_tx, completion_rx) = oneshot::channel();

        let child_pid = child.id();

        // Create output buffer
        let output_buffer = OutputBuffer::shared(self.limits.max_output_bytes);
        let started_at = now_timestamp();

        // Record the running action
        {
            let mut running = self.running.lock().unwrap();
            running.insert(
                execution_id.clone(),
                RunningActionState {
                    child_pid: Some(child_pid),
                    output_buffer: output_buffer.clone(),
                    discard_output: false,
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

        // Everything read from the process goes through one emitter, which
        // appends it to the buffer and hands it to the listener in batches.
        let output = output_emitter::spawn(
            execution_id.clone(),
            listener.clone(),
            output_buffer.clone(),
            handle.clone(),
            self.limits.emit_interval,
            self.limits.emit_max_bytes,
        );

        // Spawn threads to read output.
        //
        // When a PTY reader is provided (local execution), we use it as the
        // single output stream — the PTY merges stdout and stderr, matching
        // real terminal behavior. Otherwise (remote execution), we read from
        // the child's piped stdout and stderr separately.
        let (reader_done, readers_done) = mpsc::channel::<Infallible>();
        if let Some(pty_master) = pty_reader {
            spawn_stream_reader(pty_master, "stdout", output.clone(), reader_done.clone());
        } else {
            if let Some(stdout) = child.stdout.take() {
                spawn_stream_reader(stdout, "stdout", output.clone(), reader_done.clone());
            }

            if let Some(stderr) = child.stderr.take() {
                spawn_stream_reader(stderr, "stderr", output.clone(), reader_done.clone());
            }
        }

        // Only the readers hold a sender from here on.
        drop(reader_done);

        // Spawn thread to wait for completion
        let exec_id = execution_id.clone();
        let running_clone = self.running.clone();
        let finished_clone = self.finished.clone();
        let stopped_clone = self.stopped.clone();
        let action_started_at = started_at;
        let reader_drain_timeout = self.limits.reader_drain_timeout;

        thread::spawn(move || {
            let exit_status = child.wait();
            let exit_code = exit_status.as_ref().ok().and_then(|s| s.code());
            let completed_at = now_timestamp();

            // Check whether this execution was explicitly stopped by the user
            let was_stopped = {
                let mut stopped = stopped_clone.lock().unwrap();
                stopped.remove(&exec_id)
            };

            // Remove from running and hand the output buffer to the finished
            // set. The buffer itself moves: nothing is copied, and the emitter
            // keeps appending to the same one.
            {
                let mut running = running_clone.lock().unwrap();
                if let Some(state) = running.remove(&exec_id) {
                    if !state.discard_output {
                        let mut finished = finished_clone.lock().unwrap();
                        finished.insert(exec_id.clone(), state.output_buffer);
                    }
                }
            }

            // The process has exited, but the last of its output may still be
            // unread, or read and waiting in a batch. Wait for the readers,
            // then have the emitter deliver what it holds, so that the
            // terminal status follows all of the output and the finished log
            // is whole.
            //
            // The wait for the readers is bounded because a process that
            // outlives the action can hold the output open indefinitely;
            // whatever it writes later still lands in the same buffer and is
            // still delivered. The flush has a bound of its own, so that what
            // was read is delivered ahead of the status even when the readers
            // used theirs up.
            wait_for_readers(&readers_done, Instant::now() + reader_drain_timeout);
            output.flush_by(Instant::now() + reader_drain_timeout);
            // The readers keep the emitter alive for as long as they run.
            drop(output);

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

        self.manage_child_process(child, listener, pty_reader).await
    }

    /// Execute a shell command in the specified working directory
    ///
    /// Returns a unique execution ID. The action runs in the background.
    /// Use `execute_and_wait` if you need to wait for completion.
    pub async fn execute(
        &self,
        command: String,
        working_dir: String,
        listener: Arc<dyn ExecutionListener>,
    ) -> Result<String> {
        let (execution_id, _completion_rx) =
            self.execute_inner(command, working_dir, listener).await?;
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
        listener: Arc<dyn ExecutionListener>,
    ) -> Result<String> {
        let (execution_id, completion_rx) =
            self.execute_inner(command, working_dir, listener).await?;

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
        listener: Arc<dyn ExecutionListener>,
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

        let (execution_id, _completion_rx) =
            self.manage_child_process(child, listener, None).await?;
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
        // thread needs the entry to move the output buffer to the finished set
        // and to emit the proper StatusChanged event.
        let pid = {
            let running = self.running.lock().unwrap();
            running.get(execution_id).and_then(|state| state.child_pid)
        };

        if let Some(pid) = pid {
            #[cfg(unix)]
            {
                // Send SIGHUP to the shell's process group.
                //
                // We used `setsid()` in pre_exec, so the shell is the session
                // leader and its PID equals the PGID of its initial process
                // group. Interactive shells (`-i`) use job control, placing
                // child commands in separate process groups, so
                // `kill(-pid, SIGTERM)` misses them. Interactive shells also
                // ignore SIGTERM by default.
                //
                // SIGHUP is the correct signal here: interactive shells handle
                // it by forwarding SIGHUP+SIGCONT to every job they manage,
                // then exiting. This is POSIX-standard behavior across
                // bash/zsh/fish and works on both macOS and Linux.
                // The child's PID equals its session ID (we used setsid()).
                let sid = pid as i32;
                // SAFETY: kill(2) does not dereference pointers. A negative
                // pid targets the process group.
                let hup_result = unsafe { libc::kill(-sid, libc::SIGHUP) };

                if let Some(force_kill_after) = options.force_kill_after {
                    // If SIGHUP failed with ESRCH, the process group is
                    // already gone — no need to escalate.
                    let already_dead = hup_result == -1
                        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);

                    if !already_dead {
                        // Escalate to SIGKILL after a grace period. Use
                        // session-based kill to reach child commands that
                        // the shell may have placed in separate process
                        // groups via job control.
                        thread::spawn(move || {
                            thread::sleep(force_kill_after);
                            kill_session(sid, libc::SIGKILL);
                        });
                    }
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

    /// Get buffered output for an execution.
    ///
    /// Only the most recent output is kept. Once earlier output has been
    /// dropped, the first chunk is a notice saying how much. Returns `None`
    /// for an unknown execution, and for a finished one whose output has been
    /// cleared or evicted.
    pub fn get_buffered_output(&self, execution_id: &str) -> Option<Vec<OutputChunk>> {
        // First check running actions, then finished ones
        let buffer = {
            let running = self.running.lock().unwrap();
            match running.get(execution_id) {
                Some(state) => Some(state.output_buffer.clone()),
                None => self.finished.lock().unwrap().get(execution_id),
            }
        };

        // Copy the output with only the buffer itself locked.
        buffer.map(|buffer| buffer.lock().unwrap().snapshot())
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

    /// Clear buffered output for a finished execution. Returns whether any
    /// was retained. A running execution is left alone.
    pub fn clear_execution(&self, execution_id: &str) -> bool {
        self.finished.lock().unwrap().remove(execution_id)
    }

    /// Give up an execution's output, whether or not it has finished.
    ///
    /// A finished execution's output is freed now. A running execution keeps
    /// buffering for whoever is watching it, and its output is freed when it
    /// exits instead of being retained. Returns whether the execution was
    /// running or retained.
    pub fn discard_output(&self, execution_id: &str) -> bool {
        // Hold `running` across both checks so that an execution cannot
        // finish, and be retained, in between them.
        let mut running = self.running.lock().unwrap();
        if let Some(state) = running.get_mut(execution_id) {
            state.discard_output = true;
            return true;
        }
        self.finished.lock().unwrap().remove(execution_id)
    }
}

/// Get current timestamp in milliseconds
fn now_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

// =============================================================================
// Session-based kill (unix only)
// =============================================================================

/// Send `signal` to every process in session `sid`.
///
/// The child was started with `setsid()`, so its PID equals the session ID.
/// Interactive shells may place child commands in separate process groups,
/// so `kill(-pgid, sig)` can miss them. This function enumerates all PIDs
/// in the session and signals each one individually.
///
/// Returns the number of processes successfully signalled.
#[cfg(target_os = "macos")]
fn kill_session(sid: i32, signal: i32) -> usize {
    // On macOS, sysctl(KERN_PROC_SESSION) returns an array of kinfo_proc
    // structs. The libc crate doesn't expose kinfo_proc for macOS, so we
    // work with raw bytes. On 64-bit macOS (both arm64 and x86_64):
    //   sizeof(struct kinfo_proc) = 648
    //   offsetof(kinfo_proc, kp_proc.p_pid) = 40
    const KINFO_PROC_SIZE: usize = 648;
    const P_PID_OFFSET: usize = 40;

    let mut mib: [libc::c_int; 4] = [
        libc::CTL_KERN,
        libc::KERN_PROC,
        libc::KERN_PROC_SESSION,
        sid,
    ];
    let mut size: libc::size_t = 0;

    // First call: get the buffer size needed.
    // SAFETY: sysctl with a null buffer just returns the needed size.
    let ret = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            4,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret != 0 || size < KINFO_PROC_SIZE {
        return 0;
    }

    let mut buf: Vec<u8> = vec![0u8; size];
    let mut actual_size = size;

    // Second call: fill the buffer.
    // SAFETY: buf is a properly sized byte buffer.
    let ret = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            4,
            buf.as_mut_ptr().cast(),
            &mut actual_size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret != 0 {
        return 0;
    }

    let count = actual_size / KINFO_PROC_SIZE;
    let mut signalled = 0usize;
    for i in 0..count {
        let offset = i * KINFO_PROC_SIZE + P_PID_OFFSET;
        if offset + 4 > actual_size {
            break;
        }
        let pid = i32::from_ne_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]);
        // SAFETY: kill(2) does not dereference pointers.
        if unsafe { libc::kill(pid, signal) } == 0 {
            signalled += 1;
        }
    }
    signalled
}

/// Send `signal` to every process in session `sid`.
///
/// On Linux, enumerates `/proc/*/stat` and parses field 6 (session ID)
/// to find matching processes.
///
/// Returns the number of processes successfully signalled.
#[cfg(target_os = "linux")]
fn kill_session(sid: i32, signal: i32) -> usize {
    let mut signalled = 0usize;
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        let Ok(pid) = name_str.parse::<i32>() else {
            continue;
        };
        let stat_path = format!("/proc/{pid}/stat");
        let Ok(stat) = std::fs::read_to_string(&stat_path) else {
            continue;
        };
        // The stat format is: pid (comm) state ppid pgrp session ...
        // comm can contain spaces and parens, so find the last ')' first.
        let Some(close_paren) = stat.rfind(')') else {
            continue;
        };
        let fields_after_comm = &stat[close_paren + 2..]; // skip ") "
                                                          // fields_after_comm: state ppid pgrp session ...
                                                          //                    [0]   [1]  [2]  [3]
        let mut fields = fields_after_comm.split_whitespace();
        if let Some(session_str) = fields.nth(3) {
            if let Ok(proc_sid) = session_str.parse::<i32>() {
                if proc_sid == sid {
                    // SAFETY: kill(2) does not dereference pointers.
                    if unsafe { libc::kill(pid, signal) } == 0 {
                        signalled += 1;
                    }
                }
            }
        }
    }
    signalled
}

/// Send `signal` to every process in session `sid`.
///
/// Fallback for unix platforms other than macOS and Linux: sends the signal
/// to the session leader's process group via `kill(-sid, signal)`, which may
/// miss child commands in separate process groups under job control.
///
/// Returns 1 if the signal was delivered, 0 otherwise.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn kill_session(sid: i32, signal: i32) -> usize {
    // SAFETY: kill(2) does not dereference pointers.
    if unsafe { libc::kill(-sid, signal) } == 0 {
        1
    } else {
        0
    }
}
