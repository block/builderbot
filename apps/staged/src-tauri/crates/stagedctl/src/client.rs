//! Discovery, handshake, and transport for talking to a running Staged app.
//!
//! The app writes `~/.staged/control/<pid>/discovery.json` when its control
//! server starts (see `src-tauri/src/control_server.rs`) and injects a
//! control-file path into spawned agent sessions as `$STAGEDCTL_LOCK`. The
//! CLI reads that file through a hardened reader (owner-only directory and
//! file, no symlinks, bounded size — berd's rules), connects over the Unix
//! domain socket it names (or, for the app's loopback-TCP fallback, the port
//! plus the per-start bearer it names), pings the server to verify it is
//! talking to the instance the file describes (pid + generation) and that
//! both sides speak the same protocol version, and only then sends any
//! command payload. Without an explicit path it tries every live-pid
//! instance directory, newest first, until one passes that handshake —
//! file freshness alone can lie (pid reuse).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

/// Version of the CLI ↔ app wire contract. Must equal
/// `control_server::PROTOCOL_VERSION` in the app (pinned by test); a
/// mismatch means the app was updated out from under this binary.
pub const PROTOCOL_VERSION: u32 = 2;

/// Env var carrying the control-file path into spawned agent sessions.
/// Must equal `control_server::STAGEDCTL_LOCK_ENV` (pinned by test).
pub const STAGEDCTL_LOCK_ENV: &str = "STAGEDCTL_LOCK";

/// The `Host` the app's control server requires, verbatim, on both
/// transports. Must equal `control_server::HOST_LITERAL` (pinned by test).
pub const HOST_LITERAL: &str = "stagedctl";

/// Largest control file the reader will take. Discovery and per-session
/// files are a handful of short fields; anything bigger is not ours.
const MAX_CONTROL_FILE_BYTES: u64 = 4096;

/// The `code` values the app sends with a 401/403 that mean "the credential,
/// not the transport, was refused". Exit 4 for each: an agent should report
/// it rather than retry.
pub const AUTHORIZATION_CODES: &[&str] = &[
    "authorization_required",
    "authorization_denied",
    "authorization_revoked",
    "authorization_expired",
    "scope_denied",
];

/// How stagedctl failed, mapped onto the exit-code contract:
/// 1 command error, 2 transport fault, 3 environment (not under a Staged
/// app, app gone, or protocol version mismatch), 4 authorization (the app
/// refused this credential, or the command is outside its scope).
#[derive(Debug)]
pub enum CtlError {
    /// The command reached the app (or was validated client-side) and
    /// failed. Stderr carries `code: message` so callers can act on the
    /// code; messages from the app are passed through verbatim.
    Command { code: &'static str, message: String },
    /// The app was reachable at handshake time but a call later failed.
    Transport(String),
    /// No usable app: not spawned by Staged, the app exited or restarted,
    /// or the app speaks a different protocol version.
    Environment(String),
    /// The app answered but refused the credential: none was sent, it was
    /// revoked or expired, the user denied it, or the command addresses
    /// something outside the credential's scope. `code` is one of
    /// [`AUTHORIZATION_CODES`].
    Authorization { code: String, message: String },
}

impl CtlError {
    pub fn command(code: &'static str, message: impl Into<String>) -> Self {
        Self::Command {
            code,
            message: message.into(),
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Command { .. } => 1,
            Self::Transport(_) => 2,
            Self::Environment(_) => 3,
            Self::Authorization { .. } => 4,
        }
    }
}

impl std::fmt::Display for CtlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command { code, message } => write!(f, "{code}: {message}"),
            Self::Transport(message) => write!(f, "transport: {message}"),
            Self::Environment(message) => write!(f, "{message}"),
            Self::Authorization { code, message } => write!(f, "{code}: {message}"),
        }
    }
}

/// Contents of a control file: the instance's `discovery.json`, or a
/// per-session file with the same shape. Exactly one of `socket` and `port`
/// is set (the app's transport); `token`, when present, is sent as
/// `Authorization: Bearer` on every request.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    #[serde(default)]
    pub socket: Option<PathBuf>,
    #[serde(default)]
    pub port: Option<u16>,
    pub pid: u32,
    pub generation: u64,
    pub protocol_version: u32,
    #[serde(default)]
    pub token: Option<String>,
}

impl Discovery {
    /// Shape checks serde cannot express: one transport, and a token that
    /// looks like the 256-bit hex value the app mints (so a truncated or
    /// hand-edited file fails here rather than as a confusing 401).
    fn validate(&self) -> Result<(), String> {
        match (&self.socket, self.port) {
            (Some(_), Some(_)) => return Err("names both a socket and a port".to_string()),
            (None, None) => return Err("names neither a socket nor a port".to_string()),
            _ => {}
        }
        if let Some(token) = &self.token {
            if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("token must be a 256-bit hexadecimal value".to_string());
            }
        }
        Ok(())
    }

    /// Human-readable endpoint for error messages.
    fn endpoint(&self) -> String {
        match (&self.socket, self.port) {
            (Some(socket), _) => socket.display().to_string(),
            (None, Some(port)) => format!("127.0.0.1:{port}"),
            (None, None) => "<no endpoint>".to_string(),
        }
    }
}

/// Where control files live: `~/.staged`.
pub fn default_data_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".staged"))
}

/// Resolve the control files to try, in order: `--lock-path` beats
/// `$STAGEDCTL_LOCK` (each an exact, single candidate) beats every live-pid
/// `control/<pid>/discovery.json` in the data dir, newest first. No
/// candidate at all means we are not under a Staged app.
pub fn resolve_lock_candidates(
    flag: Option<&Path>,
    env_value: Option<std::ffi::OsString>,
    data_dir: Option<&Path>,
) -> Result<Vec<PathBuf>, CtlError> {
    if let Some(path) = flag {
        return Ok(vec![path.to_path_buf()]);
    }
    if let Some(value) = env_value {
        if !value.is_empty() {
            return Ok(vec![PathBuf::from(value)]);
        }
    }
    let candidates = data_dir.map(live_discovery_files).unwrap_or_default();
    if candidates.is_empty() {
        return Err(CtlError::Environment(
            "not running under a Staged app: $STAGEDCTL_LOCK is not set and no running \
             Staged instance was found (no live control/<pid>/discovery.json under \
             ~/.staged). Start the Staged app, or pass --lock-path."
                .to_string(),
        ));
    }
    Ok(candidates)
}

/// Resolve discovery and connect, trying each candidate in order until one
/// passes the ping handshake. An explicit `--lock-path` or `$STAGEDCTL_LOCK`
/// yields exactly one candidate; the data-dir fallback may yield several —
/// multiple running instances, or a pid-reuse zombie shadowing a real one —
/// and the handshake, not file freshness, decides which is real. If none
/// answers, the newest candidate's error is reported.
pub fn discover_and_connect(
    flag: Option<&Path>,
    env_value: Option<std::ffi::OsString>,
    data_dir: Option<&Path>,
) -> Result<Client, CtlError> {
    let candidates = resolve_lock_candidates(flag, env_value, data_dir)?;
    let mut first_err: Option<CtlError> = None;
    for path in &candidates {
        match Client::connect(path) {
            Ok(client) => return Ok(client),
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    Err(first_err.expect("resolve_lock_candidates never returns an empty list"))
}

/// Every `control/<pid>/discovery.json` under `data_dir` whose pid is a live
/// process, newest-mtime first. Dead-pid leftovers are the app's problem to
/// sweep; we just skip them. A live pid is necessary but not sufficient: a
/// crashed instance's directory survives every sweep once an unrelated
/// process reclaims its pid, so callers must not commit to a single
/// candidate — they try each in order until the ping handshake picks out a
/// real control server.
fn live_discovery_files(data_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(data_dir.join("control")) else {
        return Vec::new();
    };
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|name| !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if !is_process_alive(pid) {
            continue;
        }
        let path = entry.path().join("discovery.json");
        let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            continue;
        };
        candidates.push((mtime, path));
    }
    candidates.sort_by(|(a, _), (b, _)| b.cmp(a));
    candidates.into_iter().map(|(_, path)| path).collect()
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // Signal 0 probes existence without touching the process; EPERM still
    // proves something is running under that pid.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn is_process_alive(_pid: u32) -> bool {
    // No cheap probe; a dead pid just fails at ping time instead.
    true
}

/// Read and parse a control file, with one short retry so a concurrent
/// atomic rewrite never surfaces as a parse error.
pub fn read_discovery(path: &Path) -> Result<Discovery, CtlError> {
    let mut last_err = String::new();
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(200));
        }
        match read_private_file(path) {
            Ok(contents) => match serde_json::from_str::<Discovery>(&contents) {
                Ok(discovery) => match discovery.validate() {
                    Ok(()) => return Ok(discovery),
                    Err(e) => last_err = format!("control file {} {e}", path.display()),
                },
                Err(e) => last_err = format!("cannot parse {}: {e}", path.display()),
            },
            Err(ReadError::Missing(e)) => {
                return Err(CtlError::Environment(format!(
                    "cannot read control file {}: {e} — the Staged app that wrote it has \
                     probably exited; restart Staged or re-run from a fresh session",
                    path.display()
                )))
            }
            Err(ReadError::Unsafe(e)) => {
                return Err(CtlError::Environment(format!(
                    "refusing control file {}: {e}",
                    path.display()
                )))
            }
        }
    }
    Err(CtlError::Environment(format!(
        "{last_err} — the file may be corrupt; restart Staged"
    )))
}

/// Why [`read_private_file`] refused: the file is simply not there (or
/// unreadable), or it is there but not something we should trust.
#[derive(Debug)]
enum ReadError {
    Missing(String),
    Unsafe(String),
}

/// Read a control file only if it is what the app writes: a regular file
/// owned by us with no group/other bits, inside a directory owned by us
/// with no group/other bits, reached without following a symlink, and no
/// larger than [`MAX_CONTROL_FILE_BYTES`]. Modelled on berdctl's discovery
/// reader. The directory is checked first (and opened `O_DIRECTORY |
/// O_NOFOLLOW`) so that once it is known to be owner-private, nobody else
/// can swap the final path component under the open that follows.
#[cfg(unix)]
fn read_private_file(path: &Path) -> Result<String, ReadError> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    // A basename has an empty parent in Rust; it still names a file in the
    // current directory and gets the same checks as `./name`.
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(parent)
        .map_err(|e| ReadError::Missing(format!("cannot open {}: {e}", parent.display())))?;
    let parent_metadata = directory
        .metadata()
        .map_err(|e| ReadError::Missing(format!("cannot inspect {}: {e}", parent.display())))?;
    // SAFETY: `geteuid` takes no arguments and cannot fail.
    let current_uid = unsafe { libc::geteuid() };
    if !parent_metadata.file_type().is_dir()
        || parent_metadata.uid() != current_uid
        || parent_metadata.mode() & 0o077 != 0
    {
        return Err(ReadError::Unsafe(format!(
            "{} is not an owner-private directory (expected mode 0700, owned by the \
             current user)",
            parent.display()
        )));
    }
    #[cfg(target_os = "macos")]
    require_no_extended_acl(&directory).map_err(|e| {
        ReadError::Unsafe(format!("{} is not owner-private: {e}", parent.display()))
    })?;

    // O_NOFOLLOW makes the final symlink check atomic with opening the file.
    // O_NONBLOCK keeps a planted FIFO from blocking before metadata reveals
    // that it is not a regular file.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| ReadError::Missing(format!("cannot open {}: {e}", path.display())))?;
    let metadata = file
        .metadata()
        .map_err(|e| ReadError::Missing(format!("cannot inspect {}: {e}", path.display())))?;
    if !metadata.file_type().is_file() {
        return Err(ReadError::Unsafe(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if metadata.uid() != current_uid {
        return Err(ReadError::Unsafe(format!(
            "{} is not owned by the current user",
            path.display()
        )));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(ReadError::Unsafe(format!(
            "{} is accessible by other users (expected mode 0600)",
            path.display()
        )));
    }
    #[cfg(target_os = "macos")]
    require_no_extended_acl(&file)
        .map_err(|e| ReadError::Unsafe(format!("{} is not owner-private: {e}", path.display())))?;
    if metadata.len() > MAX_CONTROL_FILE_BYTES {
        return Err(ReadError::Unsafe(format!(
            "{} is unexpectedly large",
            path.display()
        )));
    }

    // Bound the read too: the file may grow after the metadata check, but
    // it must never make stagedctl allocate an unbounded control record.
    let mut contents = String::new();
    file.take(MAX_CONTROL_FILE_BYTES + 1)
        .read_to_string(&mut contents)
        .map_err(|e| ReadError::Missing(format!("cannot read {}: {e}", path.display())))?;
    if contents.len() as u64 > MAX_CONTROL_FILE_BYTES {
        return Err(ReadError::Unsafe(format!(
            "{} is unexpectedly large",
            path.display()
        )));
    }
    Ok(contents)
}

/// The app writes ACL-free control files on macOS. Mode bits alone cannot
/// prove privacy there: an extended allow ACE can still expose a 0600 file.
/// Reject extended ACLs rather than implement a second ACL policy engine.
#[cfg(target_os = "macos")]
fn require_no_extended_acl(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    // Darwin sys/acl.h: acl_t is opaque and ACL_TYPE_EXTENDED is 0x100.
    unsafe extern "C" {
        fn acl_get_fd_np(fd: libc::c_int, acl_type: libc::c_int) -> *mut libc::c_void;
        fn acl_free(acl: *mut libc::c_void) -> libc::c_int;
    }
    // SAFETY: `file` owns a live fd; the constant selects the extended ACL.
    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), 0x100) };
    if acl.is_null() {
        let error = std::io::Error::last_os_error();
        // Darwin returns ENOENT for a valid fd with no extended ACL. Other
        // failures (including unsupported filesystems) cannot prove privacy.
        return if error.raw_os_error() == Some(libc::ENOENT) {
            Ok(())
        } else {
            Err(error)
        };
    }
    // SAFETY: acl_get_fd_np allocated this copy; it is not used after freeing.
    unsafe { acl_free(acl) };
    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "extended ACL present on control file",
    ))
}

/// Non-unix builds have no owner-private semantics to check; the app's TCP
/// fallback token is what gates them.
#[cfg(not(unix))]
fn read_private_file(path: &Path) -> Result<String, ReadError> {
    std::fs::read_to_string(path).map_err(|e| ReadError::Missing(e.to_string()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PingResponse {
    pid: u32,
    generation: u64,
    protocol_version: u32,
}

/// A verified connection to one Staged instance's control server.
#[derive(Debug)]
pub struct Client {
    http: reqwest::blocking::Client,
    base: String,
}

impl Client {
    /// Read the control file and perform the ping handshake. No command
    /// payload is ever sent to an endpoint that hasn't passed this check, so
    /// prompts can't be sprayed at whatever local service squats on a stale
    /// port. One re-read + retry absorbs an app restart between file-read
    /// and ping.
    pub fn connect(lock_path: &Path) -> Result<Client, CtlError> {
        let mut discovery = read_discovery(lock_path)?;
        for attempt in 0..2 {
            let client = Client::for_discovery(&discovery)?;
            match client.ping() {
                Ok(pong) => {
                    if pong.protocol_version != PROTOCOL_VERSION {
                        return Err(CtlError::Environment(format!(
                            "app updated — restart Staged (the app speaks stagedctl protocol \
                             v{}, this CLI speaks v{PROTOCOL_VERSION})",
                            pong.protocol_version
                        )));
                    }
                    if pong.pid == discovery.pid && pong.generation == discovery.generation {
                        return Ok(client);
                    }
                    // Another instance answered, or the server restarted
                    // between file-read and ping: re-read the file once.
                }
                Err(e) => {
                    if attempt > 0 {
                        return Err(CtlError::Environment(format!(
                            "cannot reach the Staged app at {} ({e}) — the app has probably \
                             exited; restart Staged or re-run from a fresh session",
                            discovery.endpoint()
                        )));
                    }
                }
            }
            if attempt > 0 {
                return Err(CtlError::Environment(
                    "the Staged app restarted since this control file was written (stale \
                     pid/generation); re-run from a fresh session or pass --lock-path"
                        .to_string(),
                ));
            }
            std::thread::sleep(Duration::from_millis(200));
            discovery = read_discovery(lock_path)?;
        }
        unreachable!("handshake loop returns on every path");
    }

    /// Build the transport a control file describes. No proxy — an
    /// `HTTP_PROXY` in the agent's environment must never route a control
    /// call (and the bearer it carries) through a third party — and no
    /// redirects, so the app's answer can't send a request elsewhere. The
    /// literal `Host` the server pins and the bearer, when the file carries
    /// one, ride on every request as default headers.
    fn for_discovery(discovery: &Discovery) -> Result<Client, CtlError> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::HOST,
            reqwest::header::HeaderValue::from_static(HOST_LITERAL),
        );
        if let Some(token) = &discovery.token {
            let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|e| CtlError::Transport(format!("cannot build bearer: {e}")))?;
            value.set_sensitive(true);
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        let builder = reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(120));
        let (builder, base) = match (&discovery.socket, discovery.port) {
            #[cfg(unix)]
            (Some(socket), _) => (
                builder.unix_socket(socket.clone()),
                format!("http://{HOST_LITERAL}"),
            ),
            #[cfg(not(unix))]
            (Some(socket), _) => {
                return Err(CtlError::Environment(format!(
                    "the Staged app listens on a Unix socket ({}), which this build of \
                     stagedctl cannot open",
                    socket.display()
                )))
            }
            (None, Some(port)) => (builder, format!("http://127.0.0.1:{port}")),
            (None, None) => unreachable!("Discovery::validate requires an endpoint"),
        };
        let http = builder
            .build()
            .map_err(|e| CtlError::Transport(format!("cannot build HTTP client: {e}")))?;
        Ok(Client { http, base })
    }

    fn ping(&self) -> Result<PingResponse, String> {
        let response = self
            .http
            .get(format!("{}/api/ping", self.base))
            .timeout(Duration::from_secs(5))
            .send()
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("ping returned HTTP {}", response.status()));
        }
        response
            .json::<PingResponse>()
            .map_err(|e| format!("malformed ping response: {e}"))
    }

    /// POST one allowlisted command. Command-level failures come back as
    /// HTTP 400/404 with `{"error": message}` and map to exit code 1 with
    /// the app's message verbatim; a 401/403 carrying one of
    /// [`AUTHORIZATION_CODES`] is exit code 4; anything else is a transport
    /// fault.
    pub fn invoke(&self, command: &str, args: Value) -> Result<Value, CtlError> {
        let response = self
            .http
            .post(format!("{}/api/invoke/{command}", self.base))
            .json(&args)
            .send()
            .map_err(|e| CtlError::Transport(format!("{command}: {e}")))?;

        let status = response.status();
        let body: Value = response.json().unwrap_or(Value::Null);
        match status.as_u16() {
            200 => Ok(body),
            400 | 404 => Err(CtlError::command("app_error", error_message(&body))),
            401 | 403 => match authorization_code(&body) {
                Some(code) => Err(CtlError::Authorization {
                    code: code.to_string(),
                    message: error_message(&body),
                }),
                None => Err(CtlError::Transport(format!(
                    "control server rejected the request: {}",
                    error_message(&body)
                ))),
            },
            code => Err(CtlError::Transport(format!(
                "unexpected HTTP {code} from control server: {}",
                error_message(&body)
            ))),
        }
    }
}

fn error_message(body: &Value) -> String {
    body.get("error")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| body.to_string())
}

/// The authorization code on a refusal, if the body carries a known one.
/// A 401/403 without one is the transport's (header or peer rules), not a
/// credential decision.
fn authorization_code(body: &Value) -> Option<&str> {
    body.get("code")
        .and_then(Value::as_str)
        .filter(|code| AUTHORIZATION_CODES.contains(code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    // -------------------------------------------------------------------
    // Protocol constants mirror the app (the crate must not link staged_lib)
    // -------------------------------------------------------------------

    #[test]
    fn protocol_constants_match_control_server() {
        let source = include_str!("../../../src/control_server.rs");
        assert!(
            source.contains(&format!(
                "pub const PROTOCOL_VERSION: u32 = {PROTOCOL_VERSION};"
            )),
            "control_server::PROTOCOL_VERSION diverged from the CLI's compiled-in constant; \
             bump them together"
        );
        assert!(
            source.contains(&format!(
                "pub const STAGEDCTL_LOCK_ENV: &str = \"{STAGEDCTL_LOCK_ENV}\";"
            )),
            "control_server::STAGEDCTL_LOCK_ENV diverged from the CLI's constant"
        );
        assert!(
            source.contains(&format!(
                "pub const HOST_LITERAL: &str = \"{HOST_LITERAL}\";"
            )),
            "control_server::HOST_LITERAL diverged from the CLI's constant"
        );
    }

    // -------------------------------------------------------------------
    // Exit-code contract
    // -------------------------------------------------------------------

    #[test]
    fn exit_codes_follow_the_contract() {
        assert_eq!(CtlError::command("app_error", "boom").exit_code(), 1);
        assert_eq!(CtlError::Transport("boom".into()).exit_code(), 2);
        assert_eq!(CtlError::Environment("boom".into()).exit_code(), 3);
        assert_eq!(
            CtlError::Authorization {
                code: "scope_denied".into(),
                message: "boom".into()
            }
            .exit_code(),
            4
        );
    }

    #[test]
    fn command_errors_render_as_code_colon_message() {
        let err = CtlError::command("app_error", "Branch not found: xyz");
        assert_eq!(err.to_string(), "app_error: Branch not found: xyz");
        let err = CtlError::Authorization {
            code: "authorization_revoked".into(),
            message: "the session has ended".into(),
        };
        assert_eq!(
            err.to_string(),
            "authorization_revoked: the session has ended"
        );
    }

    #[test]
    fn authorization_codes_match_the_control_server() {
        // The server spells each code once; the CLI's list must cover the
        // ones it sends today (the remaining codes are reserved for the
        // consent-based grants).
        let server = include_str!("../../../src/control_server.rs");
        let grants = include_str!("../../../src/control_server/grants.rs");
        for code in [
            "authorization_required",
            "authorization_revoked",
            "scope_denied",
        ] {
            assert!(AUTHORIZATION_CODES.contains(&code));
            assert!(
                server.contains(&format!("\"{code}\"")) || grants.contains(&format!("\"{code}\"")),
                "the app no longer sends {code}; update AUTHORIZATION_CODES"
            );
        }
    }

    // -------------------------------------------------------------------
    // Fixtures: the app's on-disk layout, with its permissions
    // -------------------------------------------------------------------

    /// A data dir the hardened reader accepts: `tempfile` creates
    /// directories with the default (umask) mode, not `0700`.
    fn private_tempdir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        make_private_dir(dir.path());
        dir
    }

    fn make_private_dir(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    /// Write a `0600` file the way the app does.
    fn write_private(path: &Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    /// The instance directory for `pid` under `data_dir`, created private.
    fn instance_dir(data_dir: &Path, pid: u32) -> PathBuf {
        let root = data_dir.join("control");
        make_private_dir(&root);
        let dir = root.join(pid.to_string());
        make_private_dir(&dir);
        dir
    }

    fn discovery_json(endpoint: serde_json::Value, pid: u32, generation: u64) -> String {
        let mut payload = serde_json::json!({
            "pid": pid,
            "generation": generation,
            "protocolVersion": PROTOCOL_VERSION,
        });
        for (key, value) in endpoint.as_object().unwrap() {
            payload[key] = value.clone();
        }
        payload.to_string()
    }

    /// A TCP-fallback discovery file (no token, as a v1-shaped server fake
    /// would answer) under the v2 layout.
    fn write_tcp_discovery(data_dir: &Path, pid: u32, port: u16, generation: u64) -> PathBuf {
        let path = instance_dir(data_dir, pid).join("discovery.json");
        write_private(
            &path,
            &discovery_json(serde_json::json!({ "port": port }), pid, generation),
        );
        path
    }

    #[cfg(unix)]
    fn write_unix_discovery(data_dir: &Path, pid: u32, socket: &Path, generation: u64) -> PathBuf {
        let path = instance_dir(data_dir, pid).join("discovery.json");
        write_private(
            &path,
            &discovery_json(
                serde_json::json!({ "socket": socket.display().to_string() }),
                pid,
                generation,
            ),
        );
        path
    }

    // -------------------------------------------------------------------
    // Lock-path resolution
    // -------------------------------------------------------------------

    #[test]
    fn flag_beats_env_beats_fallback() {
        let dir = private_tempdir();
        let fallback = write_tcp_discovery(dir.path(), std::process::id(), 1, 1);

        let flag = PathBuf::from("/flag/discovery.json");
        let env = std::ffi::OsString::from("/env/discovery.json");

        let got =
            resolve_lock_candidates(Some(&flag), Some(env.clone()), Some(dir.path())).unwrap();
        assert_eq!(got, vec![flag]);

        let got = resolve_lock_candidates(None, Some(env), Some(dir.path())).unwrap();
        assert_eq!(got, vec![PathBuf::from("/env/discovery.json")]);

        let got = resolve_lock_candidates(None, None, Some(dir.path())).unwrap();
        assert_eq!(got, vec![fallback]);
    }

    #[test]
    fn empty_env_value_is_ignored() {
        let dir = private_tempdir();
        let fallback = write_tcp_discovery(dir.path(), std::process::id(), 1, 1);
        let got = resolve_lock_candidates(None, Some(std::ffi::OsString::new()), Some(dir.path()))
            .unwrap();
        assert_eq!(got, vec![fallback]);
    }

    #[test]
    fn fallback_skips_dead_pids_and_non_instance_entries() {
        let dir = private_tempdir();
        // A pid beyond macOS/Linux pid ranges — dead.
        write_tcp_discovery(dir.path(), 999_999_999, 1, 1);
        let root = dir.path().join("control");
        std::fs::create_dir_all(root.join("grants")).unwrap();
        std::fs::write(root.join("grants").join("discovery.json"), "{}").unwrap();
        std::fs::write(root.join("notes.txt"), "not an instance").unwrap();
        // An instance dir with no discovery file yet (mid-startup).
        std::fs::create_dir_all(root.join("1")).unwrap();
        // v1 leftovers in the data dir itself are not candidates.
        std::fs::write(
            dir.path()
                .join(format!("control-{}.json", std::process::id())),
            "{}",
        )
        .unwrap();

        let err = resolve_lock_candidates(None, None, Some(dir.path())).unwrap_err();
        assert_eq!(err.exit_code(), 3, "no live instance must exit 3: {err}");

        let live = write_tcp_discovery(dir.path(), std::process::id(), 1, 1);
        let got = resolve_lock_candidates(None, None, Some(dir.path())).unwrap();
        assert_eq!(got, vec![live]);
    }

    #[test]
    fn fallback_orders_live_candidates_newest_first() {
        let dir = private_tempdir();
        // Two live pids: our own, and pid 1 (init/launchd — kill(1, 0)
        // yields EPERM, which counts as alive).
        let ours = write_tcp_discovery(dir.path(), std::process::id(), 1, 1);
        let other = write_tcp_discovery(dir.path(), 1, 1, 1);
        set_mtime(&ours, std::time::Duration::from_secs(100));
        set_mtime(&other, std::time::Duration::from_secs(10));

        let got = resolve_lock_candidates(None, None, Some(dir.path())).unwrap();
        assert_eq!(got, vec![other, ours]);
    }

    #[test]
    fn no_candidates_is_an_environment_error() {
        let dir = private_tempdir();
        let err = resolve_lock_candidates(None, None, Some(dir.path())).unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert!(
            err.to_string().contains("STAGEDCTL_LOCK"),
            "error should tell the caller how discovery works: {err}"
        );
    }

    /// Backdate a file's mtime by `age` so candidate ordering is
    /// deterministic regardless of filesystem timestamp granularity.
    fn set_mtime(path: &Path, age: Duration) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - age)
            .unwrap();
    }

    #[test]
    fn unreadable_discovery_file_is_an_environment_error() {
        let err = read_discovery(Path::new("/nonexistent/discovery.json")).unwrap_err();
        assert_eq!(err.exit_code(), 3);
    }

    // -------------------------------------------------------------------
    // The hardened reader (berd's rules)
    // -------------------------------------------------------------------

    const VALID_TCP: &str = r#"{"port":4242,"pid":1,"generation":1,"protocolVersion":2,"token":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}"#;

    #[test]
    fn reader_accepts_an_owner_private_file_in_an_owner_private_dir() {
        let dir = private_tempdir();
        let path = dir.path().join("discovery.json");
        write_private(&path, VALID_TCP);
        let discovery = read_discovery(&path).unwrap();
        assert_eq!(discovery.port, Some(4242));
        assert_eq!(discovery.token.as_deref().map(str::len), Some(64));
    }

    #[test]
    #[cfg(unix)]
    fn reader_rejects_a_group_or_world_readable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = private_tempdir();
        let path = dir.path().join("discovery.json");
        write_private(&path, VALID_TCP);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = read_discovery(&path).unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert!(
            err.to_string().contains("accessible by other users"),
            "got: {err}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn reader_rejects_a_file_in_a_shared_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = private_tempdir();
        let path = dir.path().join("discovery.json");
        write_private(&path, VALID_TCP);
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = read_discovery(&path).unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert!(
            err.to_string().contains("not an owner-private directory"),
            "got: {err}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn reader_rejects_a_symlinked_file_and_a_symlinked_directory() {
        let dir = private_tempdir();
        let target = dir.path().join("target.json");
        write_private(&target, VALID_TCP);
        let link = dir.path().join("discovery.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(read_discovery(&link).unwrap_err().exit_code(), 3);

        let real_dir = dir.path().join("real");
        make_private_dir(&real_dir);
        write_private(&real_dir.join("discovery.json"), VALID_TCP);
        let linked_dir = dir.path().join("linked");
        std::os::unix::fs::symlink(&real_dir, &linked_dir).unwrap();
        assert_eq!(
            read_discovery(&linked_dir.join("discovery.json"))
                .unwrap_err()
                .exit_code(),
            3
        );
    }

    #[test]
    #[cfg(unix)]
    fn reader_rejects_a_directory_and_an_oversized_file() {
        let dir = private_tempdir();
        let sub = dir.path().join("discovery.json");
        make_private_dir(&sub);
        assert_eq!(read_discovery(&sub).unwrap_err().exit_code(), 3);

        let big = dir.path().join("big.json");
        write_private(&big, &"x".repeat(MAX_CONTROL_FILE_BYTES as usize + 1));
        let err = read_discovery(&big).unwrap_err();
        assert!(err.to_string().contains("unexpectedly large"), "got: {err}");
    }

    #[test]
    fn reader_rejects_malformed_shapes() {
        let dir = private_tempdir();
        let non_hex = format!(
            r#"{{"port":1,"pid":1,"generation":1,"protocolVersion":2,"token":"{}"}}"#,
            "zz".repeat(32)
        );
        for (name, contents) in [
            (
                "both",
                r#"{"socket":"/s","port":1,"pid":1,"generation":1,"protocolVersion":2}"#,
            ),
            ("neither", r#"{"pid":1,"generation":1,"protocolVersion":2}"#),
            (
                "short-token",
                r#"{"port":1,"pid":1,"generation":1,"protocolVersion":2,"token":"abc"}"#,
            ),
            ("non-hex-token", non_hex.as_str()),
            ("garbage", "{"),
        ] {
            let path = dir.path().join(format!("{name}.json"));
            write_private(&path, contents);
            let err = read_discovery(&path).unwrap_err();
            assert_eq!(err.exit_code(), 3, "{name}: {err}");
        }
    }

    // -------------------------------------------------------------------
    // Handshake against a fake control server
    // -------------------------------------------------------------------

    /// One fake response: status, body, extra headers.
    type FakeResponse = (u16, String, Vec<(String, String)>);

    /// Serve one HTTP/1.1 request per connection from `incoming`, answering
    /// via the handler (which sees the whole request head: request line and
    /// headers), then close. Enough to exercise the reqwest client over
    /// either transport.
    fn run_fake<S, I, F>(incoming: I, mut handler: F)
    where
        S: Read + Write,
        I: Iterator<Item = std::io::Result<S>>,
        F: FnMut(&str) -> FakeResponse,
    {
        for stream in incoming {
            let Ok(mut stream) = stream else { break };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let header_end = loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break None,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break Some(pos + 4);
                        }
                    }
                    Err(_) => break None,
                }
            };
            let Some(header_end) = header_end else {
                continue;
            };
            let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
            let content_length: usize = head
                .lines()
                .find_map(|l| {
                    let (name, value) = l.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().ok())?
                })
                .unwrap_or(0);
            while buf.len() < header_end + content_length {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            let (status, body, extra) = handler(&head);
            let extra: String = extra
                .iter()
                .map(|(name, value)| format!("{name}: {value}\r\n"))
                .collect();
            let response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    }

    /// Loopback-TCP fake (the app's fallback transport).
    fn fake_server<F>(mut handler: F) -> (u16, std::thread::JoinHandle<()>)
    where
        F: FnMut(&str) -> (u16, String) + Send + 'static,
    {
        fake_server_with_headers(move |head| {
            let (status, body) = handler(head);
            (status, body, Vec::new())
        })
    }

    fn fake_server_with_headers<F>(handler: F) -> (u16, std::thread::JoinHandle<()>)
    where
        F: FnMut(&str) -> FakeResponse + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || run_fake(listener.incoming(), handler));
        (port, handle)
    }

    /// Unix-socket fake (the app's primary transport), bound at `socket`.
    #[cfg(unix)]
    fn fake_unix_server<F>(socket: &Path, mut handler: F) -> std::thread::JoinHandle<()>
    where
        F: FnMut(&str) -> (u16, String) + Send + 'static,
    {
        let listener = std::os::unix::net::UnixListener::bind(socket).unwrap();
        std::thread::spawn(move || {
            run_fake(listener.incoming(), move |head| {
                let (status, body) = handler(head);
                (status, body, Vec::new())
            })
        })
    }

    fn ping_body(pid: u32, generation: u64, protocol_version: u32) -> String {
        serde_json::json!({
            "ok": true,
            "pid": pid,
            "generation": generation,
            "protocolVersion": protocol_version,
        })
        .to_string()
    }

    fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
        head.lines().find_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }

    #[test]
    fn handshake_then_invoke_round_trips_over_tcp_with_the_literal_host() {
        let pid = std::process::id();
        let hosts = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let (port, _handle) = {
            let hosts = std::sync::Arc::clone(&hosts);
            fake_server(move |head| {
                hosts
                    .lock()
                    .unwrap()
                    .push(header_value(head, "host").unwrap_or("").to_string());
                if head.starts_with("GET /api/ping") {
                    (200, ping_body(pid, 7, PROTOCOL_VERSION))
                } else if head.starts_with("POST /api/invoke/list_projects") {
                    (200, r#"[{"id":"p1","name":"demo"}]"#.to_string())
                } else {
                    (404, r#"{"error":"unexpected route"}"#.to_string())
                }
            })
        };
        let dir = private_tempdir();
        let lock = write_tcp_discovery(dir.path(), pid, port, 7);

        let client = Client::connect(&lock).unwrap();
        let value = client
            .invoke("list_projects", serde_json::json!({}))
            .unwrap();
        assert_eq!(value[0]["name"], "demo");
        let hosts = hosts.lock().unwrap();
        assert_eq!(hosts.len(), 2);
        assert!(
            hosts.iter().all(|h| h == HOST_LITERAL),
            "every request must carry the literal Host, not the address: {hosts:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn handshake_then_invoke_round_trips_over_a_unix_socket() {
        let pid = std::process::id();
        let dir = private_tempdir();
        let instance = instance_dir(dir.path(), pid);
        let socket = instance.join("sock");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let _handle = {
            let seen = std::sync::Arc::clone(&seen);
            fake_unix_server(&socket, move |head| {
                seen.lock().unwrap().push(head.to_string());
                if head.starts_with("GET /api/ping") {
                    (200, ping_body(pid, 3, PROTOCOL_VERSION))
                } else if head.starts_with("POST /api/invoke/get_session") {
                    (200, r#"{"id":"s1","status":"running"}"#.to_string())
                } else {
                    (404, r#"{"error":"unexpected route"}"#.to_string())
                }
            })
        };
        let lock = write_unix_discovery(dir.path(), pid, &socket, 3);

        let client = Client::connect(&lock).unwrap();
        let value = client
            .invoke("get_session", serde_json::json!({"sessionId": "s1"}))
            .unwrap();
        assert_eq!(value["status"], "running");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        for head in seen.iter() {
            assert_eq!(header_value(head, "host"), Some(HOST_LITERAL), "{head}");
            assert_eq!(
                header_value(head, "authorization"),
                None,
                "no token in the file, no bearer on the wire"
            );
        }
    }

    #[test]
    fn a_token_in_the_control_file_rides_as_the_bearer() {
        let pid = std::process::id();
        let token = "7a".repeat(32);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let (port, _handle) = {
            let seen = std::sync::Arc::clone(&seen);
            fake_server(move |head| {
                seen.lock().unwrap().push(
                    header_value(head, "authorization")
                        .unwrap_or("")
                        .to_string(),
                );
                if head.starts_with("GET /api/ping") {
                    (200, ping_body(pid, 1, PROTOCOL_VERSION))
                } else {
                    (200, "null".to_string())
                }
            })
        };
        let dir = private_tempdir();
        let lock = instance_dir(dir.path(), pid).join("discovery.json");
        write_private(
            &lock,
            &discovery_json(serde_json::json!({ "port": port, "token": token }), pid, 1),
        );

        let client = Client::connect(&lock).unwrap();
        client
            .invoke("list_projects", serde_json::json!({}))
            .unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "ping and invoke");
        assert!(
            seen.iter().all(|a| *a == format!("Bearer {token}")),
            "got: {seen:?}"
        );
    }

    #[test]
    fn a_coded_401_or_403_is_an_authorization_error() {
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |head| {
            if head.starts_with("GET /api/ping") {
                (200, ping_body(pid, 1, PROTOCOL_VERSION))
            } else if head.starts_with("POST /api/invoke/get_session") {
                (
                    403,
                    r#"{"error":"the session belongs to another project","code":"scope_denied"}"#
                        .to_string(),
                )
            } else if head.starts_with("POST /api/invoke/list_projects") {
                (
                    401,
                    r#"{"error":"no credential","code":"authorization_required"}"#.to_string(),
                )
            } else {
                // A made-up code is not an authorization decision.
                (401, r#"{"error":"nope","code":"teapot"}"#.to_string())
            }
        });
        let dir = private_tempdir();
        let lock = write_tcp_discovery(dir.path(), pid, port, 1);

        let client = Client::connect(&lock).unwrap();
        let err = client
            .invoke("get_session", serde_json::json!({"sessionId": "s1"}))
            .unwrap_err();
        assert_eq!(err.exit_code(), 4, "{err}");
        assert_eq!(
            err.to_string(),
            "scope_denied: the session belongs to another project"
        );
        let err = client
            .invoke("list_projects", serde_json::json!({}))
            .unwrap_err();
        assert_eq!(err.exit_code(), 4, "{err}");
        assert!(err.to_string().starts_with("authorization_required: "));
        let err = client
            .invoke("cancel_session", serde_json::json!({}))
            .unwrap_err();
        assert_eq!(
            err.exit_code(),
            2,
            "unknown code stays a transport fault: {err}"
        );
    }

    #[test]
    fn a_401_from_the_server_is_a_transport_fault() {
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |head| {
            if head.starts_with("GET /api/ping") {
                (200, ping_body(pid, 1, PROTOCOL_VERSION))
            } else {
                (
                    401,
                    r#"{"error":"the bearer token is not this Staged instance's"}"#.to_string(),
                )
            }
        });
        let dir = private_tempdir();
        let lock = write_tcp_discovery(dir.path(), pid, port, 1);

        let client = Client::connect(&lock).unwrap();
        let err = client
            .invoke("list_projects", serde_json::json!({}))
            .unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("rejected"), "got: {err}");
    }

    #[test]
    fn protocol_version_mismatch_reads_as_app_updated() {
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |_| (200, ping_body(pid, 1, PROTOCOL_VERSION + 1)));
        let dir = private_tempdir();
        let lock = write_tcp_discovery(dir.path(), pid, port, 1);

        let err = Client::connect(&lock).unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert!(
            err.to_string().contains("app updated — restart Staged"),
            "got: {err}"
        );
    }

    #[test]
    fn persistent_generation_mismatch_reads_as_stale_control_file() {
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |_| (200, ping_body(pid, 99, PROTOCOL_VERSION)));
        let dir = private_tempdir();
        let lock = write_tcp_discovery(dir.path(), pid, port, 1);

        let err = Client::connect(&lock).unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert!(err.to_string().contains("restarted"), "got: {err}");
    }

    #[test]
    fn generation_mismatch_recovers_after_file_rewrite() {
        // The app restarted between the file read and the ping: the ping
        // answers with generation 2 while the file said 1. The fake rewrites
        // the file the way a real restart does, so the client's single
        // re-read should succeed.
        let pid = std::process::id();
        let dir = private_tempdir();
        let data_dir = dir.path().to_path_buf();
        let (port_tx, port_rx) = std::sync::mpsc::channel::<u16>();
        let (port, _handle) = fake_server(move |head| {
            if head.starts_with("GET /api/ping") {
                let port = port_rx.try_recv().ok();
                if let Some(port) = port {
                    write_tcp_discovery(&data_dir, pid, port, 2);
                }
                (200, ping_body(pid, 2, PROTOCOL_VERSION))
            } else {
                (200, "null".to_string())
            }
        });
        port_tx.send(port).unwrap();
        let lock = write_tcp_discovery(dir.path(), pid, port, 1);

        let client = Client::connect(&lock).unwrap();
        assert!(client
            .invoke("cancel_session", serde_json::json!({}))
            .is_ok());
    }

    #[test]
    fn fallback_skips_a_zombie_newest_candidate_via_handshake() {
        // Pid-reuse blind spot: a crashed instance's directory whose pid was
        // reclaimed by an unrelated long-lived process looks alive to every
        // liveness probe and survives the app's sweep. If it is also newest
        // by mtime, committing to it would fail even though a real instance
        // is running — discovery must fall through to older live candidates
        // until the handshake succeeds.
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |head| {
            if head.starts_with("GET /api/ping") {
                (200, ping_body(pid, 1, PROTOCOL_VERSION))
            } else if head.starts_with("POST /api/invoke/list_projects") {
                (200, "[]".to_string())
            } else {
                (404, r#"{"error":"unexpected route"}"#.to_string())
            }
        });
        // Closed port for the zombie: bind and immediately drop.
        let closed_port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();

        let dir = private_tempdir();
        let real = write_tcp_discovery(dir.path(), pid, port, 1);
        // The zombie: pid 1 is alive (it just isn't a Staged app), file is
        // newest, port answers nothing.
        write_tcp_discovery(dir.path(), 1, closed_port, 1);
        set_mtime(&real, Duration::from_secs(100));

        let client = discover_and_connect(None, None, Some(dir.path())).unwrap();
        assert!(client
            .invoke("list_projects", serde_json::json!({}))
            .is_ok());
    }

    #[test]
    fn unreachable_port_is_an_environment_error() {
        // Bind a port and drop the listener so the port is closed.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let dir = private_tempdir();
        let lock = write_tcp_discovery(dir.path(), std::process::id(), port, 1);

        let err = Client::connect(&lock).unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert!(err.to_string().contains("cannot reach"), "got: {err}");
    }

    #[test]
    #[cfg(unix)]
    fn unreachable_socket_is_an_environment_error() {
        let dir = private_tempdir();
        let pid = std::process::id();
        let socket = instance_dir(dir.path(), pid).join("sock");
        let lock = write_unix_discovery(dir.path(), pid, &socket, 1);

        let err = Client::connect(&lock).unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert!(err.to_string().contains("cannot reach"), "got: {err}");
        assert!(
            err.to_string().contains(&socket.display().to_string()),
            "names the socket: {err}"
        );
    }

    #[test]
    fn client_never_follows_redirects() {
        // A redirect from the control port would carry the request (and its
        // bearer) wherever `Location` points. The decoy records whether
        // anything reached it.
        let decoy_hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (decoy_port, _decoy) = {
            let hits = std::sync::Arc::clone(&decoy_hits);
            fake_server(move |_| {
                hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                (200, "[]".to_string())
            })
        };
        let pid = std::process::id();
        let (port, _handle) = fake_server_with_headers(move |head| {
            if head.starts_with("GET /api/ping") {
                (200, ping_body(pid, 1, PROTOCOL_VERSION), Vec::new())
            } else {
                (
                    302,
                    String::new(),
                    vec![(
                        "Location".to_string(),
                        format!("http://127.0.0.1:{decoy_port}/api/invoke/list_projects"),
                    )],
                )
            }
        });
        let dir = private_tempdir();
        let lock = write_tcp_discovery(dir.path(), pid, port, 1);

        let client = Client::connect(&lock).unwrap();
        let err = client
            .invoke("list_projects", serde_json::json!({}))
            .unwrap_err();
        assert_eq!(err.exit_code(), 2, "a redirect is a transport fault: {err}");
        assert!(err.to_string().contains("302"), "got: {err}");
        assert_eq!(
            decoy_hits.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the client followed the redirect"
        );
    }

    #[test]
    fn invoke_maps_app_errors_to_command_errors_verbatim() {
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |head| {
            if head.starts_with("GET /api/ping") {
                (200, ping_body(pid, 1, PROTOCOL_VERSION))
            } else {
                (400, r#"{"error":"Branch not found: b1"}"#.to_string())
            }
        });
        let dir = private_tempdir();
        let lock = write_tcp_discovery(dir.path(), pid, port, 1);

        let client = Client::connect(&lock).unwrap();
        let err = client
            .invoke("get_branch_timeline", serde_json::json!({"branchId": "b1"}))
            .unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert_eq!(err.to_string(), "app_error: Branch not found: b1");
    }
}
