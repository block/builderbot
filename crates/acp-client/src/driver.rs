//! Full-featured ACP driver for session management and streaming.
//!
//! This module provides the complete ACP integration including:
//! - Session initialization and resumption
//! - Streaming text and tool calls
//! - Permission handling
//! - Remote workspace support via Blox
//! - Cancellation support

use std::collections::HashSet;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_client_protocol::{
    Agent, ClientSideConnection, ContentBlock as AcpContentBlock, ImageContent, Implementation,
    InitializeRequest, LoadSessionRequest, McpCapabilities, McpServer, NewSessionRequest,
    PermissionOptionId, PromptRequest, ProtocolVersion, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionConfigOption, SessionInfoUpdate, SessionModelState, SessionNotification, SessionUpdate,
    TextContent,
};
use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
use nix::sys::signal::{self, Signal};
#[cfg(unix)]
use nix::unistd::Pid;

use crate::types::blox_acp_command;

// =============================================================================
// Public traits and types
// =============================================================================

/// Protocol-agnostic message writer — streams agent output.
///
/// This trait allows different storage backends (database, in-memory, etc.)
/// to receive streaming agent output without coupling to the ACP protocol.
#[async_trait]
pub trait MessageWriter: Send + Sync {
    /// Append a text chunk to the current assistant message.
    async fn append_text(&self, text: &str);

    /// Flush all buffered text and close the current message block.
    async fn finalize(&self);

    /// Record a tool call with its ID, title, and optional raw input parameters.
    async fn record_tool_call(
        &self,
        tool_call_id: &str,
        title: &str,
        raw_input: Option<&serde_json::Value>,
    );

    /// Update a previously recorded tool call's title and/or raw input.
    ///
    /// When `title` is `None`, the implementation should preserve the
    /// existing title while updating only `raw_input`.
    async fn update_tool_call_title(
        &self,
        tool_call_id: &str,
        title: Option<&str>,
        raw_input: Option<&serde_json::Value>,
    );

    /// Record the result/output of a tool call.
    async fn record_tool_result(&self, content: &str);

    /// Called when session info is updated (title, timestamps, etc.).
    ///
    /// Delivered via `SessionUpdate::SessionInfoUpdate` notifications during a
    /// session, or extracted from setup responses.
    async fn on_session_info_update(&self, _info: &SessionInfoUpdate) {}

    /// Called when model state is received from session setup responses.
    ///
    /// `SessionModelState` is only delivered in `NewSessionResponse` and
    /// `LoadSessionResponse`. Mid-session model changes are surfaced through
    /// `on_config_option_update` via `ConfigOptionUpdate` with category `Model`.
    async fn on_model_state_update(&self, _state: &SessionModelState) {}

    /// Called when session configuration options change.
    async fn on_config_option_update(&self, _options: &[SessionConfigOption]) {}
}

/// Storage interface for persisting agent session data.
///
/// This trait abstracts the storage backend, allowing different implementations
/// (SQLite, PostgreSQL, in-memory, etc.) without changing the driver logic.
#[async_trait]
pub trait Store: Send + Sync {
    /// Save the agent's session ID for resumption.
    fn set_agent_session_id(&self, session_id: &str, agent_session_id: &str) -> Result<(), String>;

    /// Retrieve existing session messages as `(role, content)` pairs.
    ///
    /// Used during session resumption to match replayed notifications
    /// against previously persisted messages.  The default implementation
    /// returns an empty list, which is correct for stores that do not
    /// support message persistence (e.g. `NoOpStore`).
    fn get_session_messages(&self, _session_id: &str) -> Result<Vec<(String, String)>, String> {
        Ok(vec![])
    }
}

/// Everything needed to run one turn of an agent.
///
/// Implementors own the protocol details (spawning a process, connecting,
/// sending the prompt, translating streaming events into [`MessageWriter`]
/// calls).
#[async_trait(?Send)]
#[allow(clippy::too_many_arguments)]
pub trait AgentDriver {
    /// Run a single turn: send `prompt`, stream results via `writer`.
    ///
    /// `images` contains `(base64_data, mime_type)` pairs that are sent as
    /// `ContentBlock::Image` entries alongside the text prompt.
    async fn run(
        &self,
        session_id: &str,
        prompt: &str,
        images: &[(String, String)],
        working_dir: &Path,
        store: &Arc<dyn Store>,
        writer: &Arc<dyn MessageWriter>,
        cancel_token: &CancellationToken,
        agent_session_id: Option<&str>,
    ) -> Result<(), String>;
}

// =============================================================================
// AcpDriver — the main driver implementation
// =============================================================================

pub struct AcpDriver {
    binary_path: PathBuf,
    acp_args: Vec<String>,
    agent_label: String,
    /// When true, this driver proxies through a remote Blox workspace.
    is_remote: bool,
    /// Extra environment variables to pass to the agent process.
    extra_env: Vec<(String, String)>,
    /// Captured interactive-login-shell environment for local sessions.
    ///
    /// When set, the agent binary is spawned *directly* with this environment
    /// (env-cleared first, then these vars applied — the same Hermit-activated
    /// snapshot the caller's `ShellEnvCache` hands pipeline steps and git ops)
    /// instead of launching an interactive `$SHELL -ils` and piping `exec
    /// <binary>` to it. This keeps the agent, its per-command shells, pipeline
    /// steps, and git ops all drawing from one env so their resolved
    /// toolchains can't diverge, and skips the per-session interactive-shell
    /// spawn. `None` (remote sessions, or when the caller couldn't capture a
    /// snapshot) falls back to the `$SHELL -ils` + `exec` spawn.
    env_snapshot: Option<Vec<(String, String)>>,
    /// Captured home/global environment used only to resolve env-shebang
    /// interpreters for bridge startup.
    ///
    /// Npm-installed ACP bridges commonly use `#!/usr/bin/env node`. The
    /// bridge must start with the user's global Node, not a repo-local Hermit
    /// Node that happens to appear first in the working-directory snapshot.
    /// When this snapshot can resolve the interpreter, the driver launches the
    /// bridge as `<interpreter> <bridge>` while still giving the child process
    /// the working-directory `env_snapshot`.
    interpreter_env_snapshot: Option<Vec<(String, String)>>,
    /// MCP servers to inject into the session via NewSessionRequest.
    /// These are *required*: if the agent doesn't support a server's transport,
    /// the session fails.
    mcp_servers: Vec<McpServer>,
    /// Override the working directory sent to the remote agent.
    /// When set, this path is used in the `NewSessionRequest` instead of the
    /// local `working_dir` passed to `run()`. This is needed because the
    /// local `working_dir` is a fallback path on the host machine, while the
    /// remote agent needs the actual workspace path (e.g. `/home/bloxer/cash-server`).
    remote_working_dir: Option<PathBuf>,
}

const REMOTE_ACP_MAX_PENDING_LINE_BYTES: usize = 256 * 1024;
const ACP_SETUP_TIMEOUT: Duration = Duration::from_secs(90);

impl AcpDriver {
    /// Create a driver for the given provider ID (e.g. "goose", "claude").
    ///
    /// Looks up the agent in `KNOWN_AGENTS`, locates the binary on disk,
    /// and returns a ready-to-use driver.
    pub fn new(provider_id: &str) -> Result<Self, String> {
        crate::types::find_acp_agent_by_id(provider_id)
            .map(|agent| Self {
                binary_path: agent.binary_path,
                acp_args: agent.acp_args,
                agent_label: agent.label,
                is_remote: false,
                extra_env: Vec::new(),
                env_snapshot: None,
                interpreter_env_snapshot: None,
                mcp_servers: Vec::new(),
                remote_working_dir: None,
            })
            .ok_or_else(|| format!("Unknown or unavailable agent provider: {provider_id}"))
    }

    /// Create a driver for the first available provider.
    pub fn first_available() -> Result<Self, String> {
        crate::types::find_acp_agent()
            .map(|agent| Self {
                binary_path: agent.binary_path,
                acp_args: agent.acp_args,
                agent_label: agent.label,
                is_remote: false,
                extra_env: Vec::new(),
                env_snapshot: None,
                interpreter_env_snapshot: None,
                mcp_servers: Vec::new(),
                remote_working_dir: None,
            })
            .ok_or_else(|| {
                "No ACP agent found. Install Goose, Claude Code, Codex, Pi, or Amp and ensure it's on your PATH."
                    .to_string()
            })
    }

    /// Create a driver that proxies through `sq blox acp <workspace>`.
    pub fn for_workspace(workspace_name: &str, agent_id: Option<&str>) -> Result<Self, String> {
        let binary_path = blox_cli::find_sq_binary().ok_or_else(|| {
            "Could not find `sq` binary. Install it and ensure it's on your PATH.".to_string()
        })?;

        let command = agent_id.and_then(blox_acp_command);
        let args = blox_cli::acp_proxy_args(workspace_name, command.as_deref());

        Ok(Self {
            binary_path,
            acp_args: args,
            agent_label: "Blox".to_string(),
            is_remote: true,
            extra_env: Vec::new(),
            env_snapshot: None,
            interpreter_env_snapshot: None,
            mcp_servers: Vec::new(),
            remote_working_dir: None,
        })
    }

    /// Set extra environment variables to pass to the agent process.
    pub fn with_extra_env(mut self, vars: Vec<(String, String)>) -> Self {
        self.extra_env = vars;
        self
    }

    /// Set the captured interactive-login-shell environment for a local session.
    ///
    /// `vars` is a fully-captured env snapshot (e.g. from the caller's
    /// `ShellEnvCache`, the same one pipeline steps and git ops draw from).
    /// When provided, [`AgentDriver::run`] spawns the agent binary directly
    /// with this environment instead of launching an interactive `$SHELL -ils`
    /// and `exec`-ing the binary — so the agent and its per-command shells
    /// resolve the same Hermit-activated toolchain as everything else, without
    /// paying the per-session shell-spawn cost. Ignored for remote sessions.
    pub fn with_env_snapshot(mut self, vars: Vec<(String, String)>) -> Self {
        if !self.is_remote {
            self.env_snapshot = Some(vars);
        }
        self
    }

    /// Set the captured home/global environment for env-shebang interpreter lookup.
    ///
    /// This does not become the agent process environment. It is consulted only
    /// to resolve launchers such as `#!/usr/bin/env node` to an explicit
    /// interpreter path; the agent still receives the working-directory
    /// environment from [`Self::with_env_snapshot`].
    pub fn with_interpreter_env_snapshot(mut self, vars: Vec<(String, String)>) -> Self {
        if !self.is_remote {
            self.interpreter_env_snapshot = Some(vars);
        }
        self
    }

    /// Set MCP servers to inject into the session via `NewSessionRequest` or `LoadSessionRequest`.
    pub fn with_mcp_servers(mut self, servers: Vec<McpServer>) -> Self {
        self.mcp_servers = servers;
        self
    }

    /// Set the working directory for the remote agent.
    ///
    /// For remote sessions, the `working_dir` passed to `run()` is used as
    /// `current_dir` for spawning the local proxy process. This field
    /// overrides the directory sent to the remote agent in the
    /// `NewSessionRequest`, so the agent operates in the correct repo
    /// directory on the workspace.
    pub fn with_remote_working_dir(mut self, dir: PathBuf) -> Self {
        self.remote_working_dir = Some(dir);
        self
    }
}

/// Shell-escape a value by wrapping it in single quotes with interior quotes
/// escaped via the standard `'\''` trick.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn env_shebang_interpreter(binary_path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(binary_path).ok()?;
    let mut buf = [0_u8; 512];
    let len = file.read(&mut buf).ok()?;
    let first_line = std::str::from_utf8(&buf[..len]).ok()?.lines().next()?;
    let shebang = first_line.strip_prefix("#!")?.trim();

    let mut parts = shebang.split_whitespace();
    let command = parts.next()?;
    let command_name = Path::new(command).file_name()?.to_str()?;
    if command_name != "env" {
        return None;
    }

    for part in parts {
        if part == "-S" || part.starts_with('-') || part.contains('=') {
            continue;
        }
        if part.contains('/') {
            return None;
        }
        return Some(part.to_string());
    }

    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

fn path_contains_executable(path_value: &str, executable: &str) -> bool {
    std::env::split_paths(path_value).any(|dir| is_executable_file(&dir.join(executable)))
}

fn path_value_from_snapshot(snapshot: &[(String, String)]) -> Option<&str> {
    snapshot
        .iter()
        .find_map(|(key, value)| (key == "PATH").then_some(value.as_str()))
}

fn resolve_executable_from_path(path_value: &str, executable: &str) -> Option<PathBuf> {
    std::env::split_paths(path_value)
        .map(|dir| dir.join(executable))
        .find(|path| is_executable_file(path))
}

fn env_shebang_interpreter_from_snapshot(
    binary_path: &Path,
    snapshot: Option<&[(String, String)]>,
) -> Option<PathBuf> {
    let interpreter = env_shebang_interpreter(binary_path)?;
    let path_value = path_value_from_snapshot(snapshot?)?;
    resolve_executable_from_path(path_value, &interpreter)
}

fn is_broad_toolchain_dir(path: &Path) -> bool {
    matches!(
        path.to_str(),
        Some("/opt/homebrew/bin" | "/usr/local/bin" | "/usr/bin" | "/bin" | "/opt/local/bin")
    )
}

fn path_with_inserted_agent_bin_dir(existing_path: &str, bin_dir: &Path) -> Option<String> {
    let mut entries: Vec<PathBuf> = if existing_path.is_empty() {
        Vec::new()
    } else {
        std::env::split_paths(existing_path).collect()
    };

    if entries.iter().any(|entry| entry == bin_dir) {
        return None;
    }

    if is_broad_toolchain_dir(bin_dir) {
        entries.push(bin_dir.to_path_buf());
    } else {
        entries.insert(0, bin_dir.to_path_buf());
    }

    std::env::join_paths(entries).ok()?.into_string().ok()
}

fn guarded_path_for_agent_binary(binary_path: &Path, existing_path: &str) -> Option<String> {
    let interpreter = env_shebang_interpreter(binary_path)?;
    if path_contains_executable(existing_path, &interpreter) {
        return None;
    }

    let bin_dir = binary_path.parent()?;
    if !is_executable_file(&bin_dir.join(&interpreter)) {
        return None;
    }

    path_with_inserted_agent_bin_dir(existing_path, bin_dir)
}

fn shell_path_guard_for_agent_binary(binary_path: &Path) -> Option<String> {
    let interpreter = env_shebang_interpreter(binary_path)?;
    let bin_dir = binary_path.parent()?;
    if !is_executable_file(&bin_dir.join(&interpreter)) {
        return None;
    }

    let quoted_interpreter = shell_quote(&interpreter);
    let quoted_bin_dir = shell_quote(&bin_dir.to_string_lossy());
    let path_assignment = if is_broad_toolchain_dir(bin_dir) {
        format!(
            "if [ -n \"$PATH\" ]; then PATH=\"$PATH\":{quoted_bin_dir}; else PATH={quoted_bin_dir}; fi"
        )
    } else {
        format!(
            "if [ -n \"$PATH\" ]; then PATH={quoted_bin_dir}:\"$PATH\"; else PATH={quoted_bin_dir}; fi"
        )
    };

    Some(format!(
        "command -v {quoted_interpreter} >/dev/null 2>&1 || {{ {path_assignment}; export PATH; }}; "
    ))
}

fn shell_exec_line(
    binary_path: &Path,
    acp_args: &[String],
    interpreter_env_snapshot: Option<&[(String, String)]>,
) -> String {
    let quoted_binary = shell_quote(&binary_path.to_string_lossy());
    let quoted_args = acp_args
        .iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ");

    if let Some(interpreter) =
        env_shebang_interpreter_from_snapshot(binary_path, interpreter_env_snapshot)
    {
        let quoted_interpreter = shell_quote(&interpreter.to_string_lossy());
        if quoted_args.is_empty() {
            return format!("exec {quoted_interpreter} {quoted_binary}\n");
        }
        return format!("exec {quoted_interpreter} {quoted_binary} {quoted_args}\n");
    }

    let guard = shell_path_guard_for_agent_binary(binary_path).unwrap_or_default();

    format!("{guard}exec {quoted_binary} {quoted_args}\n")
}

#[derive(Debug)]
struct AcpSpawnCommand {
    program: PathBuf,
    args: Vec<OsString>,
    uses_explicit_interpreter: bool,
}

fn acp_spawn_command(
    binary_path: &Path,
    acp_args: &[String],
    interpreter_env_snapshot: Option<&[(String, String)]>,
) -> AcpSpawnCommand {
    if let Some(interpreter) =
        env_shebang_interpreter_from_snapshot(binary_path, interpreter_env_snapshot)
    {
        let mut args = vec![binary_path.as_os_str().to_os_string()];
        args.extend(acp_args.iter().map(OsString::from));
        return AcpSpawnCommand {
            program: interpreter,
            args,
            uses_explicit_interpreter: true,
        };
    }

    AcpSpawnCommand {
        program: binary_path.to_path_buf(),
        args: acp_args.iter().map(OsString::from).collect(),
        uses_explicit_interpreter: false,
    }
}

fn resolve_spawn_working_dir(working_dir: &Path, is_remote: bool) -> PathBuf {
    // Remote ACP sessions proxy through `sq blox acp` and don't execute against
    // the local filesystem. Use a guaranteed-existing cwd when the recorded
    // local fallback path doesn't exist, otherwise spawn fails with ENOENT.
    if is_remote && !working_dir.is_dir() {
        return std::env::temp_dir();
    }
    working_dir.to_path_buf()
}

#[async_trait(?Send)]
impl AgentDriver for AcpDriver {
    async fn run(
        &self,
        session_id: &str,
        prompt: &str,
        images: &[(String, String)],
        working_dir: &Path,
        store: &Arc<dyn Store>,
        writer: &Arc<dyn MessageWriter>,
        cancel_token: &CancellationToken,
        agent_session_id: Option<&str>,
    ) -> Result<(), String> {
        let spawn_working_dir = resolve_spawn_working_dir(working_dir, self.is_remote);
        if self.is_remote && spawn_working_dir.as_path() != working_dir {
            log::warn!(
                "Remote ACP spawn cwd missing ({}); falling back to {}",
                working_dir.display(),
                spawn_working_dir.display()
            );
        }

        // Local sessions need Hermit (and similar directory-based shell hooks)
        // to have activated before the agent binary runs, so the agent — and
        // every per-command shell it later spawns — resolves the same
        // toolchain as pipeline steps and git ops.
        //
        // The reliable way to get that is to apply a captured
        // interactive-login-shell env snapshot (`env_snapshot`, produced by the
        // caller's `ShellEnvCache` — the very snapshot pipeline steps and git
        // ops use) directly to the agent binary. That starting env is the
        // single determinant for everything the agent runs, so a Hermit-first
        // snapshot propagates into its per-command shells too. It also skips
        // the per-session shell spawn entirely.
        //
        // When no snapshot is supplied (remote sessions, or a capture failure
        // upstream) we fall back to the older dance: spawn an interactive login
        // shell with `-s` (stdin mode) in the working directory with a clean
        // environment. The shell initialises fully (`.zshrc` installs hooks),
        // `precmd` fires in the working directory (activating Hermit), then we
        // write an `exec <binary>` command to stdin. `exec` replaces the shell
        // with the agent binary so all subsequent stdin/stdout traffic is the
        // JSON-RPC protocol. This path is fragile (it depends on `precmd`
        // firing inside an attached interactive shell), which is exactly why
        // the snapshot path is preferred when available.
        let use_shell_spawn = !self.is_remote && self.env_snapshot.is_none();

        let mut cmd = if use_shell_spawn {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
            let mut c = Command::new(&shell);
            c.current_dir(&spawn_working_dir) // start in project dir so precmd sees hermit config
                .env_clear() // clean slate — shell init rebuilds the environment
                .env("HOME", std::env::var("HOME").unwrap_or_default())
                .env("USER", std::env::var("USER").unwrap_or_default())
                .env("SHELL", &shell)
                .arg("-i") // interactive: ensures hooks like precmd/chpwd are installed
                .arg("-l") // login: loads full profile / environment
                .arg("-s"); // read commands from stdin (after init completes)
            c
        } else {
            let spawn_command = acp_spawn_command(
                &self.binary_path,
                &self.acp_args,
                self.interpreter_env_snapshot.as_deref(),
            );
            let mut c = Command::new(&spawn_command.program);
            c.args(&spawn_command.args).current_dir(&spawn_working_dir);
            if !self.is_remote {
                if let Some(ref snapshot) = self.env_snapshot {
                    // Local session with a captured env: start the agent from the
                    // Hermit-activated snapshot. env_clear first so nothing from
                    // Staged's own (possibly Homebrew-first) environment leaks in,
                    // then apply the captured vars — mirroring `ShellEnv::apply_to`.
                    c.env_clear();
                    let mut snapshot_path: Option<String> = None;
                    for (k, v) in snapshot {
                        if k == "PATH" {
                            snapshot_path = Some(v.clone());
                        }
                        c.env(k, v);
                    }
                    // Preserve the captured PATH unless an env-shebang launcher
                    // (for example `#!/usr/bin/env node`) cannot find its
                    // interpreter from the snapshot. Only then add the agent bin
                    // dir as a targeted fallback; broad toolchain dirs are appended
                    // so they cannot jump ahead of Hermit or other project-managed
                    // paths.
                    if !spawn_command.uses_explicit_interpreter {
                        let existing_path = snapshot_path.as_deref().unwrap_or_default();
                        if let Some(new_path) =
                            guarded_path_for_agent_binary(&self.binary_path, existing_path)
                        {
                            c.env("PATH", new_path);
                        }
                    }
                }
            }
            c
        };

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Pipe stderr and log it. Shell init failures (Hermit activation
            // errors, .zshrc syntax errors) and — crucially — a failed shebang
            // (`env: node: No such file or directory` when the agent binary's
            // interpreter isn't on PATH) all surface here. Without this the agent
            // dies before the `initialize` response and the only symptom is an
            // opaque "server shut down unexpectedly".
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Put remote proxies in their own process group so we can send
        // SIGINT to the entire group (sq + its child processes) for graceful
        // shutdown. We must NOT do this for local interactive shells because
        // process_group(0) detaches the child from the controlling terminal,
        // which breaks zsh's job-control / precmd hooks — the shell either
        // hangs or exits immediately without running `exec`.
        if self.is_remote {
            #[cfg(unix)]
            cmd.process_group(0);
        }
        // extra_env is applied last so per-session overrides always win: over
        // the shell's clean environment (shell-spawn path), over the captured
        // snapshot (local direct-spawn path), and over the inherited
        // environment (remote spawns).
        for (k, v) in &self.extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|e| {
            format!(
                "Failed to spawn {} (binary: {}, cwd: {}): {e}",
                self.agent_label,
                self.binary_path.display(),
                spawn_working_dir.display()
            )
        })?;

        // Drain the agent's stderr to the log so spawn/shebang failures are
        // visible instead of vanishing into a generic "server shut down".
        if let Some(stderr) = child.stderr.take() {
            let agent_label = self.agent_label.clone();
            let session_id = session_id.to_string();
            tokio::task::spawn_local(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.trim().is_empty() {
                        continue;
                    }
                    log::warn!("[{agent_label} stderr][session {session_id}] {line}");
                }
            });
        }

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to get stdin".to_string())?;

        // For the shell-spawn fallback, write the exec command to stdin. By the
        // time the shell reads from stdin, init is complete and `precmd` has
        // fired in the working directory (activating Hermit). `exec` replaces
        // the shell with the agent binary — from this point on, stdin belongs
        // to the agent's JSON-RPC transport. When a snapshot was applied the
        // agent binary is already the child process, so there is nothing to
        // exec.
        if use_shell_spawn {
            // If the fallback shell's initialized PATH already provides an
            // env-shebang interpreter, keep it unchanged. Otherwise add the
            // agent bin dir only as an interpreter fallback, preserving broad
            // toolchain dirs behind project-managed PATH entries.
            let exec_line = shell_exec_line(
                &self.binary_path,
                &self.acp_args,
                self.interpreter_env_snapshot.as_deref(),
            );
            stdin
                .write_all(exec_line.as_bytes())
                .await
                .map_err(|e| format!("Failed to write exec command to shell stdin: {e}"))?;
            stdin
                .flush()
                .await
                .map_err(|e| format!("Failed to flush shell stdin: {e}"))?;
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to get stdout".to_string())?;

        let stdin_compat = stdin.compat_write();
        let incoming_reader: Box<dyn tokio::io::AsyncRead + Unpin> = if self.is_remote {
            let (normalized_stdout_writer, normalized_stdout_reader) = tokio::io::duplex(64 * 1024);
            tokio::task::spawn_local(async move {
                if let Err(error) =
                    normalize_remote_acp_stdout(stdout, normalized_stdout_writer).await
                {
                    log::error!("remote ACP stdout normalization failed: {error}");
                }
            });
            Box::new(normalized_stdout_reader)
        } else {
            // On the shell-spawn fallback, shell init (.zshrc, plugin banners,
            // Hermit activation) may write to stdout before `exec` replaces the
            // shell. Filter out any non-JSON lines so they don't reach the
            // JSON-RPC parser. On the direct-spawn (snapshot) path there is no
            // shell banner, so this is a harmless passthrough.
            let (normalized_stdout_writer, normalized_stdout_reader) = tokio::io::duplex(64 * 1024);
            tokio::task::spawn_local(async move {
                if let Err(error) =
                    normalize_local_acp_stdout(stdout, normalized_stdout_writer).await
                {
                    log::error!("local ACP stdout normalization failed: {error}");
                }
            });
            Box::new(normalized_stdout_reader)
        };
        let stdout_compat = incoming_reader.compat();

        let is_resuming = agent_session_id.is_some();
        let db_messages = if is_resuming {
            store.get_session_messages(session_id).unwrap_or_else(|e| {
                log::warn!("Failed to load session messages for replay matching: {e}");
                vec![]
            })
        } else {
            vec![]
        };
        let handler = Arc::new(AcpNotificationHandler::new(
            Arc::clone(writer),
            is_resuming,
            db_messages,
        ));
        let handler_for_conn = Arc::clone(&handler);

        let (connection, io_future) =
            ClientSideConnection::new(handler_for_conn, stdin_compat, stdout_compat, |fut| {
                tokio::task::spawn_local(fut);
            });

        tokio::task::spawn_local(async move {
            if let Err(e) = io_future.await {
                log::error!("ACP IO error: {e:?}");
            }
        });

        let acp_working_dir = if let Some(ref remote_dir) = self.remote_working_dir {
            remote_dir.clone()
        } else if self.is_remote {
            PathBuf::from(".")
        } else {
            working_dir.to_path_buf()
        };

        let protocol_result = tokio::select! {
            _ = cancel_token.cancelled() => {
                log::info!("Session {session_id} cancelled");
                writer.finalize().await;
                graceful_stop(&mut child, self.is_remote).await;
                return Ok(());
            }
            result = run_acp_protocol(
                &connection, &acp_working_dir, prompt, images, store,
                session_id, agent_session_id, &handler, &self.mcp_servers,
            ) => result,
        };

        writer.finalize().await;
        graceful_stop(&mut child, self.is_remote).await;

        protocol_result
    }
}

/// Gracefully stop the ACP child process.
///
/// For remote proxies (spawned with `process_group(0)`), sends SIGINT to the
/// process group so the proxy and its children can run cleanup. Falls back to
/// SIGKILL after a 5-second timeout.
///
/// For local agents (no separate process group), kills immediately.
async fn graceful_stop(child: &mut tokio::process::Child, is_remote: bool) {
    #[cfg(unix)]
    if is_remote {
        let Some(pid) = child.id() else {
            return;
        };
        let Ok(pid) = i32::try_from(pid) else {
            let _ = child.kill().await;
            return;
        };
        // Send SIGINT to the process group (negative PID) so both `sq`
        // and its child processes (the blox acp proxy) receive the signal.
        if signal::kill(Pid::from_raw(-pid), Signal::SIGINT).is_ok() {
            if let Ok(Ok(_status)) =
                tokio::time::timeout(Duration::from_secs(5), child.wait()).await
            {
                return;
            }
        }
    }
    let _ = child.kill().await;
}

#[derive(Debug, PartialEq, Eq)]
enum RemoteLineOutcome {
    Emit(String),
    Pending,
    Dropped,
}

fn sanitize_remote_acp_chunk(chunk: &str) -> String {
    chunk
        .chars()
        .filter(|ch| *ch != '\0' && *ch != '\u{1e}')
        .collect()
}

fn decode_remote_acp_line(raw_line: &[u8]) -> (String, bool) {
    let mut decoded = String::with_capacity(raw_line.len());
    let mut had_invalid_utf8 = false;
    let mut cursor = raw_line;

    while !cursor.is_empty() {
        match std::str::from_utf8(cursor) {
            Ok(valid) => {
                decoded.push_str(valid);
                break;
            }
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                if valid_up_to > 0 {
                    if let Ok(valid) = std::str::from_utf8(&cursor[..valid_up_to]) {
                        decoded.push_str(valid);
                    }
                }

                had_invalid_utf8 = true;
                cursor = if let Some(invalid_len) = error.error_len() {
                    &cursor[valid_up_to + invalid_len..]
                } else {
                    // Incomplete sequence at EOF, which cannot be recovered.
                    break;
                };
            }
        }
    }

    (decoded, had_invalid_utf8)
}

fn consume_remote_acp_line(pending: &mut String, raw_line: &str) -> RemoteLineOutcome {
    let line = raw_line.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        return RemoteLineOutcome::Pending;
    }

    let chunk = sanitize_remote_acp_chunk(line);
    if chunk.is_empty() {
        return RemoteLineOutcome::Pending;
    }

    pending.push_str(&chunk);

    match serde_json::from_str::<serde_json::Value>(pending) {
        Ok(_) => RemoteLineOutcome::Emit(std::mem::take(pending)),
        Err(error) if error.is_eof() => {
            if pending.len() > REMOTE_ACP_MAX_PENDING_LINE_BYTES {
                pending.clear();
                RemoteLineOutcome::Dropped
            } else {
                RemoteLineOutcome::Pending
            }
        }
        Err(_) => {
            // Recovery path: pending may contain stale/corrupted bytes. If the
            // current chunk is a standalone JSON payload, emit it and reset.
            match serde_json::from_str::<serde_json::Value>(&chunk) {
                Ok(_) => {
                    pending.clear();
                    RemoteLineOutcome::Emit(chunk)
                }
                Err(chunk_error) if chunk_error.is_eof() => {
                    pending.clear();
                    pending.push_str(&chunk);
                    if pending.len() > REMOTE_ACP_MAX_PENDING_LINE_BYTES {
                        pending.clear();
                        RemoteLineOutcome::Dropped
                    } else {
                        RemoteLineOutcome::Pending
                    }
                }
                Err(_) => {
                    pending.clear();
                    RemoteLineOutcome::Dropped
                }
            }
        }
    }
}

fn remote_acp_segments(decoded_line: &str) -> impl Iterator<Item = &str> {
    // `sq blox acp` can emit JSON Text Sequences where records are delimited by
    // U+001E (record separator). Keep line-based handling for normal JSON-RPC
    // output, but split RS-delimited frames so concatenated messages are not
    // treated as malformed JSON.
    decoded_line
        .split('\u{1e}')
        .filter(|segment| !segment.trim().is_empty())
}

async fn normalize_remote_acp_stdout<R: tokio::io::AsyncRead + Unpin>(
    stdout: R,
    mut writer: tokio::io::DuplexStream,
) -> Result<(), std::io::Error> {
    let mut reader = BufReader::new(stdout);
    let mut raw_line = Vec::new();
    let mut pending = String::new();

    loop {
        raw_line.clear();
        let bytes_read = reader.read_until(b'\n', &mut raw_line).await?;
        if bytes_read == 0 {
            break;
        }

        let (decoded_line, had_invalid_utf8) = decode_remote_acp_line(&raw_line);
        if had_invalid_utf8 {
            log::warn!("Dropped invalid UTF-8 bytes from remote ACP stdout");
        }

        for segment in remote_acp_segments(&decoded_line) {
            match consume_remote_acp_line(&mut pending, segment) {
                RemoteLineOutcome::Emit(line) => {
                    writer.write_all(line.as_bytes()).await?;
                    writer.write_all(b"\n").await?;
                }
                RemoteLineOutcome::Pending => {}
                RemoteLineOutcome::Dropped => {
                    if !segment.trim().is_empty() {
                        log::warn!("Dropped malformed ACP proxy output line");
                    }
                }
            }
        }
    }

    if !pending.is_empty() {
        log::warn!("Dropped incomplete ACP proxy output at EOF");
    }

    writer.shutdown().await
}

/// Filter local ACP stdout, forwarding only valid JSON lines.
///
/// Local shell initialization (`.zshrc`, Hermit activation, plugin banners)
/// may write non-JSON text to stdout before `exec` replaces the shell with
/// the agent binary. This function reads lines from the child's stdout and
/// only forwards those that parse as valid JSON, discarding everything else.
async fn normalize_local_acp_stdout<R: tokio::io::AsyncRead + Unpin>(
    stdout: R,
    mut writer: tokio::io::DuplexStream,
) -> Result<(), std::io::Error> {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).await?;
        if bytes_read == 0 {
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
            writer.write_all(trimmed.as_bytes()).await?;
            writer.write_all(b"\n").await?;
        } else {
            log::debug!("Dropped non-JSON line from local ACP stdout: {trimmed}");
        }
    }

    writer.shutdown().await
}

// =============================================================================
// ACP notification handler — phase-based replay-sync state machine
// =============================================================================

/// The current phase of the notification handler during session resumption.
enum HandlerPhase {
    /// Accumulating replay notifications and matching against DB messages.
    Replaying(ReplayBuffer),
    /// Replay detected as complete; waiting for prompt to be sent.
    /// All notifications are dropped; tool-call IDs are recorded.
    WaitingForPrompt {
        replayed_tool_call_ids: HashSet<String>,
    },
    /// Prompt has been sent; forwarding live notifications to the writer.
    Live {
        replayed_tool_call_ids: HashSet<String>,
    },
}

/// Accumulates replay notifications and matches them against DB messages.
struct ReplayBuffer {
    /// `(role, content)` pairs from the DB, in order.
    db_messages: Vec<(String, String)>,
    /// Index into `db_messages` of the next message to match.
    match_cursor: usize,
    /// Index of the last non-user message in `db_messages`.
    /// When the cursor passes this, replay is considered complete.
    target_index: Option<usize>,
    /// Text accumulated for the current streaming message.
    current_text: String,
    /// Role of the current streaming message (`"user"` or `"assistant"`).
    current_role: Option<String>,
    /// Tool-call IDs observed during replay (used as a safety-net later).
    replayed_tool_call_ids: HashSet<String>,
    /// Timestamp of the last notification received during replay.
    last_notification_at: Instant,
    /// Whether at least one notification has been received.
    received_any: bool,
}

impl ReplayBuffer {
    fn new(db_messages: Vec<(String, String)>) -> Self {
        // Find index of last non-user message.
        let target_index = db_messages
            .iter()
            .enumerate()
            .rev()
            .find(|(_, (role, _))| role != "user")
            .map(|(i, _)| i);

        Self {
            db_messages,
            match_cursor: 0,
            target_index,
            current_text: String::new(),
            current_role: None,
            replayed_tool_call_ids: HashSet::new(),
            last_notification_at: Instant::now(),
            received_any: false,
        }
    }

    /// Finalize the current streaming text and try to match it against DB.
    /// Called when the role transitions (e.g. from assistant text to tool call).
    /// Returns `true` if replay is now considered complete.
    fn finalize_current(&mut self) -> bool {
        if let Some(role) = self.current_role.take() {
            if !self.current_text.is_empty() {
                self.current_text.clear();
                return self.try_match(&role);
            }
        }
        false
    }

    /// Try to match a role against `db_messages[match_cursor]`.
    /// Returns `true` if replay is now considered complete.
    fn try_match(&mut self, role: &str) -> bool {
        if self.match_cursor >= self.db_messages.len() {
            return self.is_complete();
        }

        let (db_role, _) = &self.db_messages[self.match_cursor];

        if role == db_role {
            self.match_cursor += 1;
        }
        // Don't advance cursor on role mismatch.

        self.is_complete()
    }

    /// Returns `true` if the match cursor has passed the target index.
    fn is_complete(&self) -> bool {
        match self.target_index {
            Some(target) => self.match_cursor > target,
            None => true, // No non-user messages → complete immediately
        }
    }
}

struct AcpNotificationHandler {
    writer: Arc<dyn MessageWriter>,
    phase: Mutex<HandlerPhase>,
    /// Signalled when replay matching determines all DB messages have been replayed.
    replay_done: tokio::sync::Notify,
}

impl AcpNotificationHandler {
    fn new(
        writer: Arc<dyn MessageWriter>,
        replaying: bool,
        db_messages: Vec<(String, String)>,
    ) -> Self {
        let phase = if replaying {
            HandlerPhase::Replaying(ReplayBuffer::new(db_messages))
        } else {
            HandlerPhase::Live {
                replayed_tool_call_ids: HashSet::new(),
            }
        };

        Self {
            writer,
            phase: Mutex::new(phase),
            replay_done: tokio::sync::Notify::new(),
        }
    }

    /// Check whether the replay phase has been idle for at least `timeout`.
    /// Returns `false` if not in the Replaying phase or no notification received yet.
    async fn is_replay_idle(&self, timeout: Duration) -> bool {
        let phase = self.phase.lock().await;
        if let HandlerPhase::Replaying(buf) = &*phase {
            buf.received_any && buf.last_notification_at.elapsed() >= timeout
        } else {
            false
        }
    }

    /// Transition from Replaying to WaitingForPrompt.
    /// Extracts the replayed_tool_call_ids from the ReplayBuffer.
    async fn transition_to_waiting_for_prompt(&self) {
        let mut phase = self.phase.lock().await;
        let ids = match &mut *phase {
            HandlerPhase::Replaying(buf) => std::mem::take(&mut buf.replayed_tool_call_ids),
            HandlerPhase::WaitingForPrompt { .. } | HandlerPhase::Live { .. } => return,
        };
        *phase = HandlerPhase::WaitingForPrompt {
            replayed_tool_call_ids: ids,
        };
    }

    /// Transition from WaitingForPrompt (or Replaying) to Live.
    async fn transition_to_live(&self) {
        let mut phase = self.phase.lock().await;
        let ids = match &mut *phase {
            HandlerPhase::WaitingForPrompt {
                replayed_tool_call_ids,
            } => std::mem::take(replayed_tool_call_ids),
            HandlerPhase::Replaying(buf) => std::mem::take(&mut buf.replayed_tool_call_ids),
            HandlerPhase::Live { .. } => return,
        };
        *phase = HandlerPhase::Live {
            replayed_tool_call_ids: ids,
        };
    }
}

#[async_trait(?Send)]
impl agent_client_protocol::Client for AcpNotificationHandler {
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        let option_id = args
            .options
            .first()
            .map(|opt| opt.option_id.clone())
            .unwrap_or_else(|| PermissionOptionId::new("approve"));

        Ok(RequestPermissionResponse::new(
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id)),
        ))
    }

    async fn session_notification(
        &self,
        notification: SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        // Session metadata events are forwarded regardless of phase.
        match &notification.update {
            SessionUpdate::SessionInfoUpdate(info) => {
                self.writer.on_session_info_update(info).await;
                return Ok(());
            }
            SessionUpdate::ConfigOptionUpdate(update) => {
                self.writer
                    .on_config_option_update(&update.config_options)
                    .await;
                return Ok(());
            }
            _ => {}
        }

        // Determine the action to take under the lock, then drop the lock
        // before calling into the writer to avoid holding it across await points.
        enum LiveAction {
            AppendText(String),
            RecordToolCall {
                id: String,
                title: String,
                raw_input: Option<serde_json::Value>,
            },
            ToolCallUpdate {
                id: String,
                title: Option<String>,
                raw_input: Option<serde_json::Value>,
                result: Option<String>,
            },
            Ignore,
            Drop,
        }

        let live_action = {
            let mut phase = self.phase.lock().await;

            match &mut *phase {
                // ── Replaying phase: accumulate chunks, match against DB ──
                HandlerPhase::Replaying(buf) => {
                    buf.last_notification_at = Instant::now();
                    buf.received_any = true;

                    // Record tool-call IDs for the safety-net.
                    if let Some(id) = notification_tool_call_id(&notification.update) {
                        buf.replayed_tool_call_ids.insert(id);
                    }

                    let completed = match &notification.update {
                        SessionUpdate::AgentMessageChunk(chunk) => {
                            if let AcpContentBlock::Text(text) = &chunk.content {
                                // If switching from non-assistant role, finalize previous.
                                let mut done = false;
                                if buf.current_role.as_deref() != Some("assistant") {
                                    done = buf.finalize_current();
                                    buf.current_role = Some("assistant".to_string());
                                }
                                buf.current_text.push_str(&text.text);
                                done
                            } else {
                                false
                            }
                        }
                        SessionUpdate::UserMessageChunk(chunk) => {
                            if let AcpContentBlock::Text(text) = &chunk.content {
                                let mut done = false;
                                if buf.current_role.as_deref() != Some("user") {
                                    done = buf.finalize_current();
                                    buf.current_role = Some("user".to_string());
                                }
                                buf.current_text.push_str(&text.text);
                                done
                            } else {
                                false
                            }
                        }
                        SessionUpdate::ToolCall(_tc) => {
                            buf.finalize_current();
                            buf.try_match("tool_call")
                        }
                        SessionUpdate::ToolCallUpdate(update)
                            if update.fields.content.is_some() =>
                        {
                            buf.finalize_current();
                            buf.try_match("tool_result")
                        }
                        SessionUpdate::AgentThoughtChunk(_) => {
                            // Thinking is not persisted — ignore.
                            false
                        }
                        _ => false,
                    };

                    if completed {
                        self.replay_done.notify_one();
                    }
                    return Ok(());
                }

                // ── WaitingForPrompt phase: drop everything, record tool-call IDs ──
                HandlerPhase::WaitingForPrompt {
                    replayed_tool_call_ids,
                } => {
                    if let Some(id) = notification_tool_call_id(&notification.update) {
                        replayed_tool_call_ids.insert(id);
                    }
                    return Ok(());
                }

                // ── Live phase: determine action, then release lock ──
                HandlerPhase::Live {
                    replayed_tool_call_ids,
                } => {
                    // Safety net: drop notifications for tool-call IDs seen during replay.
                    if let Some(id) = notification_tool_call_id(&notification.update) {
                        if replayed_tool_call_ids.contains(&id) {
                            return Ok(());
                        }
                    }

                    match &notification.update {
                        SessionUpdate::AgentMessageChunk(chunk) => {
                            if let AcpContentBlock::Text(text) = &chunk.content {
                                LiveAction::AppendText(text.text.clone())
                            } else {
                                LiveAction::Drop
                            }
                        }
                        SessionUpdate::ToolCall(tool_call) => LiveAction::RecordToolCall {
                            id: tool_call.tool_call_id.0.to_string(),
                            title: tool_call.title.clone(),
                            raw_input: tool_call.raw_input.clone(),
                        },
                        SessionUpdate::ToolCallUpdate(update) => {
                            let tc_id = update.tool_call_id.0.to_string();
                            let title = update.fields.title.clone();
                            let raw_input = update.fields.raw_input.clone();
                            let result = update
                                .fields
                                .content
                                .as_ref()
                                .and_then(|c| extract_content_preview(c));
                            if title.is_some() || raw_input.is_some() || result.is_some() {
                                LiveAction::ToolCallUpdate {
                                    id: tc_id,
                                    title,
                                    raw_input,
                                    result,
                                }
                            } else {
                                LiveAction::Drop
                            }
                        }
                        _ => LiveAction::Ignore,
                    }
                }
            }
            // phase lock is dropped here
        };

        // Execute the live action without holding the phase lock.
        match live_action {
            LiveAction::AppendText(text) => {
                self.writer.append_text(&text).await;
            }
            LiveAction::RecordToolCall {
                id,
                title,
                raw_input,
            } => {
                self.writer
                    .record_tool_call(&id, &title, raw_input.as_ref())
                    .await;
            }
            LiveAction::ToolCallUpdate {
                id,
                title,
                raw_input,
                result,
            } => {
                if title.is_some() || raw_input.is_some() {
                    self.writer
                        .update_tool_call_title(&id, title.as_deref(), raw_input.as_ref())
                        .await;
                }
                if let Some(preview) = result {
                    self.writer.record_tool_result(&preview).await;
                }
            }
            LiveAction::Ignore => {
                log::debug!("Ignoring session update: {:?}", notification.update);
            }
            LiveAction::Drop => {}
        }
        Ok(())
    }
}

/// Extract the tool-call ID from a session update, if it carries one.
fn notification_tool_call_id(update: &SessionUpdate) -> Option<String> {
    match update {
        SessionUpdate::ToolCall(tc) => Some(tc.tool_call_id.0.to_string()),
        SessionUpdate::ToolCallUpdate(tcu) => Some(tcu.tool_call_id.0.to_string()),
        _ => None,
    }
}

// =============================================================================
// Protocol helpers
// =============================================================================

#[allow(clippy::too_many_arguments)]
async fn run_acp_protocol(
    connection: &ClientSideConnection,
    working_dir: &Path,
    prompt: &str,
    images: &[(String, String)],
    store: &Arc<dyn Store>,
    our_session_id: &str,
    acp_session_id: Option<&str>,
    handler: &Arc<AcpNotificationHandler>,
    mcp_servers: &[McpServer],
) -> Result<(), String> {
    let agent_session_id = tokio::time::timeout(
        ACP_SETUP_TIMEOUT,
        setup_acp_session(
            connection,
            working_dir,
            store,
            &handler.writer,
            our_session_id,
            acp_session_id,
            mcp_servers,
        ),
    )
    .await
    .map_err(|_| {
        format!(
            "Timed out waiting for ACP protocol startup after {}s",
            ACP_SETUP_TIMEOUT.as_secs()
        )
    })??;

    // If resuming, wait for replay to complete (content match OR idle timeout).
    // An absolute 10s timeout prevents a hang if the server sends zero replay
    // notifications (e.g. the remote session was garbage-collected).
    if acp_session_id.is_some() {
        let absolute_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            tokio::select! {
                _ = handler.replay_done.notified() => {
                    break;
                }
                _ = tokio::time::sleep_until(absolute_deadline) => {
                    log::warn!("Replay-wait absolute timeout reached (10s) — proceeding");
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if handler.is_replay_idle(Duration::from_secs(1)).await {
                        break;
                    }
                }
            }
        }
        handler.transition_to_waiting_for_prompt().await;
    }

    let mut content_blocks = vec![AcpContentBlock::Text(TextContent::new(prompt))];
    for (data, mime_type) in images {
        content_blocks.push(AcpContentBlock::Image(ImageContent::new(
            data.as_str(),
            mime_type.as_str(),
        )));
    }
    let prompt_request = PromptRequest::new(agent_session_id, content_blocks);

    handler.transition_to_live().await;

    connection
        .prompt(prompt_request)
        .await
        .map_err(|e| format!("Prompt failed: {e:?}"))?;

    Ok(())
}

/// Whether the agent advertises support for the transport an MCP server needs.
/// Stdio is always supported per the ACP spec.
fn mcp_server_transport_supported(server: &McpServer, caps: &McpCapabilities) -> bool {
    match server {
        McpServer::Http(_) => caps.http,
        McpServer::Sse(_) => caps.sse,
        McpServer::Stdio(_) => true,
        // `McpServer` is non_exhaustive. A transport we don't recognize can't be
        // reasoned about, so we conservatively treat it as unsupported, which
        // fails the session rather than silently shipping an unvalidated server.
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
async fn setup_acp_session(
    connection: &ClientSideConnection,
    working_dir: &Path,
    store: &Arc<dyn Store>,
    writer: &Arc<dyn MessageWriter>,
    our_session_id: &str,
    acp_session_id: Option<&str>,
    mcp_servers: &[McpServer],
) -> Result<String, String> {
    let client_info = Implementation::new("acp-client", env!("CARGO_PKG_VERSION"));
    let init_request = InitializeRequest::new(ProtocolVersion::LATEST).client_info(client_info);

    let init_response = connection
        .initialize(init_request)
        .await
        .map_err(|e| format!("ACP init failed: {e:?}"))?;

    let mcp_caps = &init_response.agent_capabilities.mcp_capabilities;

    // Required servers must all have a supported transport, or the session
    // fails. Route the decision through mcp_server_transport_supported so the
    // transport->capability mapping lives in exactly one place — a newly added
    // transport stays validated here instead of silently slipping through.
    if mcp_servers
        .iter()
        .any(|server| !mcp_server_transport_supported(server, mcp_caps))
    {
        let requires_http = mcp_servers
            .iter()
            .any(|server| matches!(server, McpServer::Http(_)));
        let requires_sse = mcp_servers
            .iter()
            .any(|server| matches!(server, McpServer::Sse(_)));

        return Err(format!(
            "Agent does not support required MCP transports (required: http={}, sse={}; agent: http={}, sse={}). Select a provider that supports MCP over HTTP/SSE.",
            requires_http, requires_sse, mcp_caps.http, mcp_caps.sse
        ));
    }

    match acp_session_id {
        Some(existing_id) => {
            if !init_response.agent_capabilities.load_session {
                return Err(
                    "Agent does not support load_session — cannot resume conversation".to_string(),
                );
            }

            log::info!(
                "Resuming ACP session {existing_id} via load_session for session {our_session_id}"
            );

            let load_response = connection
                .load_session(
                    LoadSessionRequest::new(existing_id.to_string(), working_dir.to_path_buf())
                        .mcp_servers(mcp_servers.to_vec()),
                )
                .await
                .map_err(|e| format!("Failed to load ACP session: {e:?}"))?;

            if let Some(ref models) = load_response.models {
                writer.on_model_state_update(models).await;
            }
            if let Some(ref options) = load_response.config_options {
                writer.on_config_option_update(options).await;
            }

            Ok(existing_id.to_string())
        }
        None => {
            let new_session_request =
                NewSessionRequest::new(working_dir.to_path_buf()).mcp_servers(mcp_servers.to_vec());
            let session_response = connection
                .new_session(new_session_request)
                .await
                .map_err(|e| format!("Failed to create ACP session: {e:?}"))?;

            let new_id = session_response.session_id.to_string();
            store
                .set_agent_session_id(our_session_id, &new_id)
                .map_err(|e| format!("Failed to save agent session ID: {e}"))?;

            if let Some(ref models) = session_response.models {
                writer.on_model_state_update(models).await;
            }
            if let Some(ref options) = session_response.config_options {
                writer.on_config_option_update(options).await;
            }

            Ok(new_id)
        }
    }
}

/// Strip outer markdown code fences from tool-result content.
/// Agents often wrap results in ``` fences which are redundant in our `<pre>` display.
/// The closing fence may be absent when content was truncated by the preview limit.
pub fn strip_code_fences(content: &str) -> String {
    let trimmed = content.trim();
    if let Some(after_open) = trimmed.strip_prefix("```") {
        if let Some(nl) = after_open.find('\n') {
            let body = after_open[nl + 1..].trim_end();
            return body
                .strip_suffix("```")
                .unwrap_or(body)
                .trim_end()
                .to_string();
        }
    }
    content.to_string()
}

fn extract_content_preview(content: &[agent_client_protocol::ToolCallContent]) -> Option<String> {
    for item in content {
        match item {
            agent_client_protocol::ToolCallContent::Content(c) => {
                if let AcpContentBlock::Text(text) = &c.content {
                    let preview: String = text.text.chars().take(500).collect();
                    return Some(if text.text.len() > 500 {
                        format!("{preview}…")
                    } else {
                        preview
                    });
                }
            }
            agent_client_protocol::ToolCallContent::Diff(d) => {
                return Some(format!(
                    "{}{}",
                    d.path.display(),
                    if d.old_text.is_some() {
                        " (modified)"
                    } else {
                        " (new)"
                    }
                ));
            }
            agent_client_protocol::ToolCallContent::Terminal(t) => {
                return Some(format!("Terminal: {}", t.terminal_id.0));
            }
            _ => {}
        }
    }
    None
}

// =============================================================================
// Basic MessageWriter implementation
// =============================================================================

/// Simple in-memory message writer for basic usage.
pub struct BasicMessageWriter {
    text: Mutex<String>,
    last_flush_at: Mutex<Instant>,
}

impl BasicMessageWriter {
    pub fn new() -> Self {
        Self {
            text: Mutex::new(String::new()),
            last_flush_at: Mutex::new(Instant::now()),
        }
    }

    pub async fn get_text(&self) -> String {
        self.text.lock().await.clone()
    }
}

impl Default for BasicMessageWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MessageWriter for BasicMessageWriter {
    async fn append_text(&self, text: &str) {
        let mut current = self.text.lock().await;
        current.push_str(text);
        *self.last_flush_at.lock().await = Instant::now();
    }

    async fn finalize(&self) {
        // Nothing to do for basic implementation
    }

    async fn record_tool_call(
        &self,
        _tool_call_id: &str,
        title: &str,
        _raw_input: Option<&serde_json::Value>,
    ) {
        let mut current = self.text.lock().await;
        current.push_str(&format!("\n[Tool: {}]\n", title));
    }

    async fn update_tool_call_title(
        &self,
        _tool_call_id: &str,
        _title: Option<&str>,
        _raw_input: Option<&serde_json::Value>,
    ) {
        // Nothing to do for basic implementation
    }

    async fn record_tool_result(&self, content: &str) {
        let mut current = self.text.lock().await;
        current.push_str(&format!("\n[Result: {}]\n", content));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        acp_spawn_command, consume_remote_acp_line, decode_remote_acp_line,
        env_shebang_interpreter, guarded_path_for_agent_binary, is_broad_toolchain_dir,
        mcp_server_transport_supported, path_with_inserted_agent_bin_dir, remote_acp_segments,
        resolve_spawn_working_dir, sanitize_remote_acp_chunk, shell_exec_line, shell_quote,
        AcpDriver, McpCapabilities, McpServer, RemoteLineOutcome,
    };
    use agent_client_protocol::{McpServerHttp, McpServerSse, McpServerStdio};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    fn write_executable(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create executable parent");
        }
        std::fs::write(path, content).expect("write executable");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms).expect("chmod executable");
        }
    }

    fn join_path_entries(entries: &[PathBuf]) -> String {
        std::env::join_paths(entries)
            .expect("join path entries")
            .into_string()
            .expect("path entries should be utf8")
    }

    #[test]
    fn consumes_wrapped_json_line_across_multiple_chunks() {
        let mut pending = String::new();
        let first = r#"{"jsonrpc":"2.0","id":1,"result":{"text":"Bypass all permiss"#;
        let second = r#"ion checks"}}"#;

        assert_eq!(
            consume_remote_acp_line(&mut pending, first),
            RemoteLineOutcome::Pending
        );

        assert_eq!(
            consume_remote_acp_line(&mut pending, second),
            RemoteLineOutcome::Emit(format!("{first}{second}"))
        );
    }

    #[test]
    fn strips_record_separator_and_nul_bytes() {
        let chunk = "\u{1e}{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}\0";
        assert_eq!(
            sanitize_remote_acp_chunk(chunk),
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}"
        );
    }

    #[test]
    fn drops_noise_and_recovers_with_next_valid_json_message() {
        let mut pending = String::new();
        assert_eq!(
            consume_remote_acp_line(&mut pending, "this is not json"),
            RemoteLineOutcome::Dropped
        );

        assert_eq!(
            consume_remote_acp_line(
                &mut pending,
                "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}"
            ),
            RemoteLineOutcome::Emit("{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}".to_string())
        );
    }

    #[test]
    fn splits_record_separator_delimited_messages_in_one_stdout_line() {
        let mut pending = String::new();
        let line = "\u{1e}{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}\u{1e}{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":null}\n";

        let outcomes: Vec<RemoteLineOutcome> = remote_acp_segments(line)
            .map(|segment| consume_remote_acp_line(&mut pending, segment))
            .collect();

        assert_eq!(
            outcomes,
            vec![
                RemoteLineOutcome::Emit(
                    "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}".to_string()
                ),
                RemoteLineOutcome::Emit(
                    "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":null}".to_string()
                ),
            ]
        );
    }

    #[test]
    fn remote_utf8_decoder_strips_invalid_bytes() {
        let (decoded, had_invalid_utf8) =
            decode_remote_acp_line(b"\xff{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}\n");
        assert!(had_invalid_utf8);
        assert_eq!(decoded, "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}\n");
    }

    #[test]
    fn remote_utf8_decoder_preserves_valid_replacement_character() {
        let (decoded, had_invalid_utf8) =
            decode_remote_acp_line("\u{FFFD}{\"jsonrpc\":\"2.0\",\"id\":1}\n".as_bytes());
        assert!(!had_invalid_utf8);
        assert_eq!(decoded, "\u{FFFD}{\"jsonrpc\":\"2.0\",\"id\":1}\n");
    }

    #[test]
    fn remote_spawn_dir_falls_back_when_working_dir_is_missing() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be after epoch")
            .as_nanos();
        let missing_path =
            std::env::temp_dir().join(format!("acp-client-missing-{}-{nonce}", std::process::id()));
        assert!(!missing_path.exists());

        assert_eq!(
            resolve_spawn_working_dir(&missing_path, true),
            std::env::temp_dir()
        );
        assert_eq!(
            resolve_spawn_working_dir(&missing_path, false),
            missing_path
        );
    }

    #[test]
    fn remote_spawn_dir_uses_existing_working_dir() {
        let existing = std::env::temp_dir();
        assert_eq!(resolve_spawn_working_dir(&existing, true), existing);
    }

    #[test]
    fn env_snapshot_is_ignored_for_remote_drivers() {
        let snapshot = vec![(String::from("PATH"), String::from("/snapshot/bin"))];
        let local = AcpDriver {
            binary_path: PathBuf::from("/usr/local/bin/codex-acp"),
            acp_args: vec![],
            agent_label: String::from("Codex"),
            is_remote: false,
            extra_env: vec![],
            env_snapshot: None,
            interpreter_env_snapshot: None,
            mcp_servers: vec![],
            remote_working_dir: None,
        }
        .with_env_snapshot(snapshot.clone());
        let remote = AcpDriver {
            binary_path: PathBuf::from("/usr/local/bin/sq"),
            acp_args: vec![String::from("blox"), String::from("acp")],
            agent_label: String::from("Blox"),
            is_remote: true,
            extra_env: vec![],
            env_snapshot: None,
            interpreter_env_snapshot: None,
            mcp_servers: vec![],
            remote_working_dir: None,
        }
        .with_env_snapshot(snapshot);

        assert!(local.env_snapshot.is_some());
        assert!(
            remote.env_snapshot.is_none(),
            "remote sq proxy launches must keep their inherited environment"
        );
    }

    #[test]
    fn interpreter_env_snapshot_is_ignored_for_remote_drivers() {
        let snapshot = vec![(String::from("PATH"), String::from("/home/bin"))];
        let local = AcpDriver {
            binary_path: PathBuf::from("/usr/local/bin/codex-acp"),
            acp_args: vec![],
            agent_label: String::from("Codex"),
            is_remote: false,
            extra_env: vec![],
            env_snapshot: None,
            interpreter_env_snapshot: None,
            mcp_servers: vec![],
            remote_working_dir: None,
        }
        .with_interpreter_env_snapshot(snapshot.clone());
        let remote = AcpDriver {
            binary_path: PathBuf::from("/usr/local/bin/sq"),
            acp_args: vec![String::from("blox"), String::from("acp")],
            agent_label: String::from("Blox"),
            is_remote: true,
            extra_env: vec![],
            env_snapshot: None,
            interpreter_env_snapshot: None,
            mcp_servers: vec![],
            remote_working_dir: None,
        }
        .with_interpreter_env_snapshot(snapshot);

        assert!(local.interpreter_env_snapshot.is_some());
        assert!(
            remote.interpreter_env_snapshot.is_none(),
            "remote sq proxy launches must keep their inherited environment"
        );
    }

    #[test]
    fn shell_quote_simple_value() {
        assert_eq!(
            shell_quote("/usr/local/bin/goose"),
            "'/usr/local/bin/goose'"
        );
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("it's here"), "'it'\\''s here'");
    }

    #[test]
    fn shell_quote_preserves_spaces() {
        assert_eq!(shell_quote("/path/with space"), "'/path/with space'");
    }

    #[test]
    fn env_shebang_interpreter_detects_env_launcher() {
        let dir = unique_test_dir("acp-env-shebang");
        let launcher = dir.join("codex-acp");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        assert_eq!(env_shebang_interpreter(&launcher).as_deref(), Some("node"));

        std::fs::remove_dir_all(dir).expect("cleanup test dir");
    }

    #[test]
    fn guarded_path_keeps_snapshot_when_interpreter_is_already_available() {
        let dir = unique_test_dir("acp-guarded-path-present");
        let project_bin = dir.join("project-bin");
        let agent_bin = dir.join("agent-bin");
        let launcher = agent_bin.join("codex-acp");
        write_executable(&project_bin.join("node"), "#!/bin/sh\n");
        write_executable(&agent_bin.join("node"), "#!/bin/sh\n");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        let snapshot_path = join_path_entries(&[project_bin.clone(), PathBuf::from("/usr/bin")]);

        assert_eq!(
            guarded_path_for_agent_binary(&launcher, &snapshot_path),
            None,
            "agent bin dir must not be inserted ahead of a snapshot PATH that already provides node"
        );

        std::fs::remove_dir_all(dir).expect("cleanup test dir");
    }

    #[test]
    fn guarded_path_prepends_private_agent_dir_when_interpreter_is_missing() {
        let dir = unique_test_dir("acp-guarded-path-missing");
        let snapshot_bin = dir.join("snapshot-bin");
        let agent_bin = dir.join("agent-bin");
        let launcher = agent_bin.join("claude-agent-acp");
        std::fs::create_dir_all(&snapshot_bin).expect("create snapshot bin");
        write_executable(&agent_bin.join("node"), "#!/bin/sh\n");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        let snapshot_path = join_path_entries(std::slice::from_ref(&snapshot_bin));
        let updated = guarded_path_for_agent_binary(&launcher, &snapshot_path)
            .expect("missing interpreter should add agent bin dir");
        let entries: Vec<PathBuf> = std::env::split_paths(&updated).collect();

        assert_eq!(entries, vec![agent_bin, snapshot_bin]);

        std::fs::remove_dir_all(dir).expect("cleanup test dir");
    }

    #[test]
    fn spawn_command_uses_home_interpreter_for_env_shebang_bridge() {
        let dir = unique_test_dir("acp-home-interpreter");
        let home_bin = dir.join("home-bin");
        let project_bin = dir.join("project-bin");
        let agent_bin = dir.join("agent-bin");
        let launcher = agent_bin.join("claude-agent-acp");
        let home_node = home_bin.join("node");
        write_executable(&home_node, "#!/bin/sh\n");
        write_executable(&project_bin.join("node"), "#!/bin/sh\n");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        let home_snapshot = vec![(
            String::from("PATH"),
            join_path_entries(std::slice::from_ref(&home_bin)),
        )];
        let command = acp_spawn_command(
            &launcher,
            &[String::from("--stdio")],
            Some(home_snapshot.as_slice()),
        );

        assert_eq!(command.program, home_node);
        assert_eq!(
            command.args,
            vec![
                launcher.as_os_str().to_os_string(),
                OsString::from("--stdio")
            ]
        );
        assert!(command.uses_explicit_interpreter);

        std::fs::remove_dir_all(dir).expect("cleanup test dir");
    }

    #[test]
    fn spawn_command_falls_back_to_launcher_without_interpreter_snapshot() {
        let dir = unique_test_dir("acp-no-home-interpreter");
        let agent_bin = dir.join("agent-bin");
        let launcher = agent_bin.join("codex-acp");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        let command = acp_spawn_command(&launcher, &[String::from("--stdio")], None);

        assert_eq!(command.program, launcher);
        assert_eq!(command.args, vec![OsString::from("--stdio")]);
        assert!(!command.uses_explicit_interpreter);

        std::fs::remove_dir_all(dir).expect("cleanup test dir");
    }

    #[test]
    fn broad_toolchain_dirs_are_appended_not_prepended() {
        let hermit_bin = PathBuf::from("/repo/bin");
        let existing_path = join_path_entries(std::slice::from_ref(&hermit_bin));
        let updated =
            path_with_inserted_agent_bin_dir(&existing_path, Path::new("/opt/homebrew/bin"))
                .expect("broad dir should still be added as a fallback");
        let entries: Vec<PathBuf> = std::env::split_paths(&updated).collect();

        assert!(is_broad_toolchain_dir(Path::new("/opt/homebrew/bin")));
        assert_eq!(
            entries,
            vec![hermit_bin, PathBuf::from("/opt/homebrew/bin")]
        );
    }

    #[test]
    fn shell_exec_line_guards_interpreter_before_mutating_path() {
        let dir = unique_test_dir("acp-shell-guard");
        let agent_bin = dir.join("agent-bin");
        let launcher = agent_bin.join("amp-acp");
        write_executable(&agent_bin.join("node"), "#!/bin/sh\n");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        let line = shell_exec_line(&launcher, &[String::from("--stdio")], None);

        assert!(
            line.starts_with(
                "command -v 'node' >/dev/null 2>&1 || { if [ -n \"$PATH\" ]; then PATH='"
            ),
            "shell fallback should check the initialized PATH before adding the agent dir: {line}"
        );
        assert!(
            line.contains(":\"$PATH\"; else PATH='"),
            "private agent dir should be prepended only inside the missing-interpreter guard: {line}"
        );
        assert!(
            line.contains("; fi; export PATH; }; exec '"),
            "shell fallback should export the guarded PATH before exec: {line}"
        );
        assert!(
            line.ends_with("' '--stdio'\n"),
            "exec should preserve the binary and args after the guard: {line}"
        );

        std::fs::remove_dir_all(dir).expect("cleanup test dir");
    }

    #[test]
    fn shell_exec_line_uses_home_interpreter_when_available() {
        let dir = unique_test_dir("acp-shell-home-interpreter");
        let home_bin = dir.join("home-bin");
        let agent_bin = dir.join("agent-bin");
        let launcher = agent_bin.join("claude-agent-acp");
        let home_node = home_bin.join("node");
        write_executable(&home_node, "#!/bin/sh\n");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        let home_snapshot = vec![(
            String::from("PATH"),
            join_path_entries(std::slice::from_ref(&home_bin)),
        )];

        let line = shell_exec_line(
            &launcher,
            &[String::from("--stdio")],
            Some(home_snapshot.as_slice()),
        );

        assert_eq!(
            line,
            format!(
                "exec '{}' '{}' '--stdio'\n",
                home_node.display(),
                launcher.display()
            )
        );

        std::fs::remove_dir_all(dir).expect("cleanup test dir");
    }

    #[test]
    fn mcp_transport_support_maps_each_transport_to_its_capability() {
        let stdio = McpServer::Stdio(McpServerStdio::new("local", "/usr/bin/server"));
        let http = McpServer::Http(McpServerHttp::new("remote", "https://example.com"));
        let sse = McpServer::Sse(McpServerSse::new("remote", "https://example.com/events"));

        // Stdio needs no capability — it is always supported, even with all caps off.
        let none = McpCapabilities::new();
        assert!(mcp_server_transport_supported(&stdio, &none));
        assert!(!mcp_server_transport_supported(&http, &none));
        assert!(!mcp_server_transport_supported(&sse, &none));

        // Each remote transport is gated on its own capability.
        let http_only = McpCapabilities::new().http(true);
        assert!(mcp_server_transport_supported(&http, &http_only));
        assert!(!mcp_server_transport_supported(&sse, &http_only));

        let sse_only = McpCapabilities::new().sse(true);
        assert!(mcp_server_transport_supported(&sse, &sse_only));
        assert!(!mcp_server_transport_supported(&http, &sse_only));
    }

    #[tokio::test]
    async fn local_stdout_normalization_filters_non_json() {
        use super::normalize_local_acp_stdout;
        use tokio::io::AsyncReadExt;

        let input = b"Hermit environment /home/user/.hermit activated\n\
                       {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}\n\
                       some banner text\n\
                       {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":null}\n";

        let (writer, mut reader) = tokio::io::duplex(64 * 1024);
        let input_reader = &input[..];

        normalize_local_acp_stdout(input_reader, writer)
            .await
            .expect("normalization should succeed");

        let mut output = String::new();
        reader
            .read_to_string(&mut output)
            .await
            .expect("read should succeed");

        assert_eq!(
            output,
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}\n\
             {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":null}\n"
        );
    }

    #[tokio::test]
    async fn local_stdout_normalization_passes_empty_input() {
        use super::normalize_local_acp_stdout;
        use tokio::io::AsyncReadExt;

        let input = b"";
        let (writer, mut reader) = tokio::io::duplex(64 * 1024);

        normalize_local_acp_stdout(&input[..], writer)
            .await
            .expect("normalization should succeed");

        let mut output = String::new();
        reader
            .read_to_string(&mut output)
            .await
            .expect("read should succeed");

        assert!(output.is_empty());
    }
}
