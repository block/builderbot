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
//!
//! The credential comes second. A per-session file (`$STAGEDCTL_LOCK`)
//! carries the session's own token; an instance's `discovery.json` carries
//! none (or, on the TCP fallback, only the transport token), so a CLI that
//! found the app that way authenticates with a connected-app grant from
//! `~/.staged/control/grants/` instead — reconnecting with a proof of the
//! grant's secret (see [`crate::grants`]) — and, with no grant either, lets
//! the app say so (`authorization_required`, exit 4).

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

/// Largest control file the reader will take. Discovery, per-session and
/// grant files are a handful of short fields; anything bigger is not ours.
const MAX_CONTROL_FILE_BYTES: u64 = 4096;

/// The instance discovery file's name. Must equal
/// `control_server::files::DISCOVERY_FILE_NAME` (pinned by test): a control
/// file with this name carries no session credential, so the grant lookup
/// applies.
const DISCOVERY_FILE_NAME: &str = "discovery.json";

/// The `code` values the app sends with a 401/403 that mean "the credential,
/// not the transport, was refused". Exit 4 for each: an agent should report
/// it rather than retry.
pub const AUTHORIZATION_CODES: &[&str] = &[
    "authorization_required",
    "authorization_denied",
    "authorization_revoked",
    "authorization_expired",
    "authorization_proof_mismatch",
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
    /// A multi-step write (`project tag`, `project untag`) failed after
    /// some of its steps landed. Each step is its own transaction on the
    /// app, so the earlier ones stay applied; `applied` lists them
    /// (`{ command, args, result }`, in order) and `human` is their lines
    /// as the command would have printed them. The exit code and the
    /// stderr line are the failing step's own (`error`).
    Partial {
        error: Box<CtlError>,
        applied: Vec<Value>,
        human: String,
    },
}

impl CtlError {
    pub fn command(code: &'static str, message: impl Into<String>) -> Self {
        Self::Command {
            code,
            message: message.into(),
        }
    }

    /// The machine-readable code: the app's or the client's for a command
    /// or authorization failure, else the variant's name.
    pub fn code(&self) -> &str {
        match self {
            Self::Command { code, .. } => code,
            Self::Authorization { code, .. } => code,
            Self::Transport(_) => "transport",
            Self::Environment(_) => "environment",
            Self::Partial { error, .. } => error.code(),
        }
    }

    /// The failing step's message, without the code.
    fn message(&self) -> String {
        match self {
            Self::Command { message, .. } | Self::Authorization { message, .. } => message.clone(),
            Self::Transport(message) | Self::Environment(message) => message.clone(),
            Self::Partial { error, .. } => error.message(),
        }
    }

    /// What goes on stdout before the error line when a multi-step write
    /// failed part-way: the applied steps' lines, or under `--json`
    /// `{ ok: false, applied: [...], error: { code, message } }`. `None`
    /// for every other error.
    pub fn partial_output(&self, json: bool) -> Option<String> {
        let Self::Partial {
            error,
            applied,
            human,
        } = self
        else {
            return None;
        };
        if json {
            let value = serde_json::json!({
                "ok": false,
                "applied": applied,
                "error": { "code": error.code(), "message": error.message() },
            });
            Some(serde_json::to_string_pretty(&value).expect("error output is always serializable"))
        } else {
            Some(human.clone())
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Command { .. } => 1,
            Self::Transport(_) => 2,
            Self::Environment(_) => 3,
            Self::Authorization { .. } => 4,
            Self::Partial { error, .. } => error.exit_code(),
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
            Self::Partial { error, .. } => write!(f, "{error}"),
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
        // An empty value is set, not unset: inside a spawned session,
        // `STAGEDCTL_LOCK= stagedctl …` must not quietly fall back to the
        // instance file and shed the session's credential.
        if value.is_empty() {
            return Err(CtlError::Environment(format!(
                "${STAGEDCTL_LOCK_ENV} is set but empty; inside a Staged-spawned session it \
                 names the session's credential file — unset it only if you are not in one, or \
                 pass --lock-path"
            )));
        }
        return Ok(vec![PathBuf::from(value)]);
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
    let (mut client, path) = discover_instance(flag, env_value, data_dir)?;
    if is_instance_discovery_file(&path) {
        if let Some(data_dir) = data_dir {
            client.attach_grant_credential(data_dir)?;
        }
    } else {
        // A per-session control file carries a session lease, which the
        // app confines to the session's project.
        client.scope = CredentialScope::Project;
    }
    Ok(client)
}

/// [`discover_and_connect`] without the credential step: the handshaken
/// client plus the control file it came from. `stagedctl connect` uses this
/// — enrolment is how a credential is obtained.
pub fn discover_instance(
    flag: Option<&Path>,
    env_value: Option<std::ffi::OsString>,
    data_dir: Option<&Path>,
) -> Result<(Client, PathBuf), CtlError> {
    let candidates = resolve_lock_candidates(flag, env_value, data_dir)?;
    let mut first_err: Option<CtlError> = None;
    for path in &candidates {
        match Client::connect(path) {
            Ok(client) => return Ok((client, path.clone())),
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    Err(first_err.expect("resolve_lock_candidates never returns an empty list"))
}

/// Whether `path` is an instance's `discovery.json` (no session credential)
/// rather than a per-session file.
pub fn is_instance_discovery_file(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()) == Some(DISCOVERY_FILE_NAME)
}

/// The per-session credential file this invocation would use, if its
/// control file resolves to one: an explicit `--lock-path` or
/// `$STAGEDCTL_LOCK` naming anything but an instance `discovery.json`.
/// `stagedctl connect` and `disconnect` refuse to run under one — a spawned
/// session's credential is confined to its project and cannot request
/// connected-app access — without a round trip to the app. The data-dir
/// fallback only ever yields instance files, so it never answers `Some`.
pub fn session_credential_in_use(
    flag: Option<&Path>,
    env_value: Option<std::ffi::OsString>,
    data_dir: Option<&Path>,
) -> Option<PathBuf> {
    resolve_lock_candidates(flag, env_value, data_dir)
        .ok()?
        .into_iter()
        .find(|path| !is_instance_discovery_file(path))
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
        let path = entry.path().join(DISCOVERY_FILE_NAME);
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

/// [`read_private_file`] with the refusal flattened to its message, for
/// callers (the grant lookup) that skip rather than distinguish.
pub(crate) fn read_private_file_lossy(path: &Path) -> Result<String, String> {
    read_private_file(path).map_err(|e| match e {
        ReadError::Missing(m) | ReadError::Unsafe(m) => m,
    })
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
/// How far the credential this client sends reaches, as far as the client
/// can tell without a round trip. The app confines two kinds of credential
/// to one project: a session lease (a per-session control file) and a
/// grant approved for one project (`connect --project`); both are known at
/// connect time. Anything else is either unconfined or refused by the app
/// as `authorization_required` before any answer is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialScope {
    AllProjects,
    Project,
}

#[derive(Debug)]
pub struct Client {
    http: reqwest::blocking::Client,
    base: String,
    /// The `Authorization: Bearer` value, if any: the control file's token
    /// (a session credential, or the TCP fallback's transport token), later
    /// replaced by a connected-app bearer when one is obtained.
    bearer: Option<String>,
    /// What the handshake confirmed, for the grant the client later writes.
    pub pid: u32,
    pub generation: u64,
    /// See [`CredentialScope`]. Reads the app confines to the credential's
    /// project (`get_tag_state`'s assignments, `list_projects`) are
    /// labelled from this.
    pub scope: CredentialScope,
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
    /// literal `Host` the server pins rides on every request as a default
    /// header; the bearer, when the file carries one, is added per request
    /// so a connected-app credential can replace it.
    fn for_discovery(discovery: &Discovery) -> Result<Client, CtlError> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::HOST,
            reqwest::header::HeaderValue::from_static(HOST_LITERAL),
        );
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
        Ok(Client {
            http,
            base,
            bearer: discovery.token.clone(),
            pid: discovery.pid,
            generation: discovery.generation,
            scope: CredentialScope::AllProjects,
        })
    }

    /// Add the bearer, when there is one, to a request.
    fn authorized(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        match &self.bearer {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    fn ping(&self) -> Result<PingResponse, String> {
        let response = self
            .authorized(self.http.get(format!("{}/api/ping", self.base)))
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
            .authorized(
                self.http
                    .post(format!("{}/api/invoke/{command}", self.base)),
            )
            .json(&args)
            .send()
            .map_err(|e| CtlError::Transport(format!("{command}: {e}")))?;
        let status = response.status().as_u16();
        let body: Value = response.json().unwrap_or(Value::Null);
        classify(command, status, body)
    }

    /// `POST` one of the credential routes (`/api/connect`,
    /// `/api/reconnect/...`, `/api/disconnect`) and classify the answer like
    /// [`invoke`](Self::invoke); a `429` on `connect` is a command error
    /// (the app has too many prompts waiting).
    fn post_credential_route(&self, route: &str, body: &Value) -> Result<Value, CtlError> {
        let response = self
            .authorized(self.http.post(format!("{}{route}", self.base)))
            .json(body)
            .send()
            .map_err(|e| CtlError::Transport(format!("{route}: {e}")))?;
        let status = response.status().as_u16();
        let body: Value = response.json().unwrap_or(Value::Null);
        if status == 429 {
            return Err(CtlError::command("too_many_requests", error_message(&body)));
        }
        classify(route, status, body)
    }

    // ---------------------------------------------------------------------
    // Connected-app grants (class B)
    // ---------------------------------------------------------------------

    /// Authenticate with the freshest usable grant on this machine, if any.
    /// A grant the app no longer knows (`authorization_required`: a reset
    /// database, say) is skipped silently; one it revoked or let expire, one
    /// whose file no longer matches the app's record
    /// (`authorization_proof_mismatch`), or one whose reconnect failed for
    /// any other reason (a transport fault, a malformed answer, a renewal
    /// the client refused), is noted on stderr — the user should hear that —
    /// and skipped for the next, since one bad grant file must not block one
    /// approved later under another name (grant files are only removed by
    /// `disconnect`). If nothing attaches,
    /// the first authorization refusal is the error (exit 4), else the first
    /// other failure (exit 2); with only unknown grants, or none, the client
    /// keeps whatever bearer the control file gave it and the app's own
    /// `authorization_required` message explains what to do.
    pub fn attach_grant_credential(&mut self, data_dir: &Path) -> Result<(), CtlError> {
        let now = crate::grants::now_ms();
        let mut refused: Option<CtlError> = None;
        let mut failed: Option<CtlError> = None;
        for (path, grant) in crate::grants::read_grant_files(data_dir) {
            if grant.is_expired_at(now) {
                continue;
            }
            match self.reconnect(&grant) {
                Ok(renewed) => {
                    if renewed.expires_at != grant.expires_at {
                        let updated = crate::grants::GrantFile {
                            expires_at: renewed.expires_at,
                            ..grant
                        };
                        if let Err(e) = crate::grants::write_grant_file(data_dir, &updated) {
                            eprintln!(
                                "stagedctl: could not record the renewed expiry in {}: {e}",
                                path.display()
                            );
                        }
                    }
                    return Ok(());
                }
                Err(CtlError::Authorization { code, .. }) if code == "authorization_required" => {
                    continue;
                }
                Err(e @ CtlError::Authorization { .. })
                    if matches!(&e, CtlError::Authorization { code, .. }
                        if code == "authorization_proof_mismatch") =>
                {
                    let client = shell_single_quote(&grant.client);
                    eprintln!(
                        "stagedctl: grant {} ({}) no longer matches the app's record ({e}); \
                         trying the next grant — run `stagedctl disconnect --client {client}` \
                         then `stagedctl connect --client {client}` to enrol afresh",
                        grant.grant_id, grant.client
                    );
                    refused.get_or_insert(e);
                }
                Err(e @ CtlError::Authorization { .. }) => {
                    eprintln!(
                        "stagedctl: grant {} ({}) is no longer accepted by the app ({e}); \
                         trying the next grant — `stagedctl disconnect --client {}` removes it",
                        grant.grant_id,
                        grant.client,
                        shell_single_quote(&grant.client)
                    );
                    refused.get_or_insert(e);
                }
                Err(e) => {
                    eprintln!(
                        "stagedctl: grant {} ({}) could not be reconnected ({e}); trying the \
                         next grant",
                        grant.grant_id, grant.client
                    );
                    failed.get_or_insert(e);
                }
            }
        }
        match refused.or(failed) {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Prove possession of `grant`'s secret and take this run's bearer. On
    /// success the bearer is installed and the renewed expiry returned.
    pub fn reconnect(&mut self, grant: &crate::grants::GrantFile) -> Result<Reconnected, CtlError> {
        let challenge = self.post_credential_route(
            "/api/reconnect/challenge",
            &serde_json::json!({ "grantId": grant.grant_id }),
        )?;
        let nonce = challenge
            .get("nonce")
            .and_then(Value::as_str)
            .ok_or_else(|| CtlError::Transport("malformed reconnect challenge".to_string()))?;
        let proof = crate::grants::reconnect_proof(grant, nonce);
        let answer = self.post_credential_route(
            "/api/reconnect",
            &serde_json::json!({
                "grantId": grant.grant_id,
                "nonce": nonce,
                "proof": proof,
            }),
        )?;
        let token = answer
            .get("token")
            .and_then(Value::as_str)
            .ok_or_else(|| CtlError::Transport("malformed reconnect answer".to_string()))?;
        let expires_at = answer
            .get("expiresAt")
            .and_then(Value::as_i64)
            .ok_or_else(|| CtlError::Transport("malformed reconnect answer".to_string()))?;
        grant
            .accept_renewal(expires_at, crate::grants::now_ms())
            .map_err(CtlError::Transport)?;
        self.bearer = Some(token.to_string());
        // The grant file's claim, copied from the approval body at connect
        // time; the app's own confinement is not re-read here.
        self.scope = match grant.scope.project_id {
            Some(_) => CredentialScope::Project,
            None => CredentialScope::AllProjects,
        };
        Ok(Reconnected { expires_at })
    }

    /// Ask the user to approve this client: `POST /api/connect`. Returns the
    /// request id to poll and when the request expires.
    pub fn request_authorization(
        &self,
        client: &str,
        project: Option<&str>,
        read_only: bool,
    ) -> Result<(String, i64), CtlError> {
        let answer = self.post_credential_route(
            "/api/connect",
            &serde_json::json!({
                "client": client,
                "project": project,
                "readOnly": read_only,
            }),
        )?;
        let request_id = answer
            .get("requestId")
            .and_then(Value::as_str)
            .ok_or_else(|| CtlError::Transport("malformed connect answer".to_string()))?;
        let expires_at = answer.get("expiresAt").and_then(Value::as_i64).unwrap_or(0);
        Ok((request_id.to_string(), expires_at))
    }

    /// One long-poll of `GET /api/connect/{id}`: the approval, or `None`
    /// while the user has not answered. A denial or an expired request is
    /// an authorization error (exit 4).
    pub fn poll_authorization(&self, request_id: &str) -> Result<Option<Approval>, CtlError> {
        let response = self
            .authorized(
                self.http
                    .get(format!("{}/api/connect/{request_id}", self.base)),
            )
            .send()
            .map_err(|e| CtlError::Transport(format!("connect poll: {e}")))?;
        let status = response.status().as_u16();
        let body: Value = response.json().unwrap_or(Value::Null);
        if status == 202 {
            return Ok(None);
        }
        let body = classify("connect", status, body)?;
        let field = |name: &str| {
            body.get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| CtlError::Transport(format!("approval is missing {name}")))
        };
        Ok(Some(Approval {
            grant: crate::grants::GrantFile {
                grant_id: field("grantId")?,
                reconnect_secret: field("reconnectSecret")?,
                client: field("client").unwrap_or_default(),
                scope: crate::grants::GrantScope {
                    project_id: body
                        .get("scope")
                        .and_then(|s| s.get("projectId"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
                access: field("access").unwrap_or_else(|_| "write".to_string()),
                expires_at: body.get("expiresAt").and_then(Value::as_i64).unwrap_or(0),
            },
            token: field("token")?,
        }))
    }

    /// Revoke `grant` in the app with a proof of its secret
    /// (`stagedctl disconnect`). Returns whether the app still had it live.
    pub fn revoke_grant(&self, grant: &crate::grants::GrantFile) -> Result<bool, CtlError> {
        let challenge = self.post_credential_route(
            "/api/reconnect/challenge",
            &serde_json::json!({ "grantId": grant.grant_id }),
        );
        let challenge = match challenge {
            Ok(challenge) => challenge,
            // Already revoked or expired app-side: nothing left to revoke.
            Err(CtlError::Authorization { code, .. })
                if code == "authorization_revoked" || code == "authorization_expired" =>
            {
                return Ok(false)
            }
            Err(e) => return Err(e),
        };
        let nonce = challenge
            .get("nonce")
            .and_then(Value::as_str)
            .ok_or_else(|| CtlError::Transport("malformed reconnect challenge".to_string()))?;
        let answer = self.post_credential_route(
            "/api/disconnect",
            &serde_json::json!({
                "grantId": grant.grant_id,
                "nonce": nonce,
                "proof": crate::grants::reconnect_proof(grant, nonce),
            }),
        )?;
        Ok(answer
            .get("revoked")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    #[cfg(test)]
    pub(crate) fn bearer(&self) -> Option<&str> {
        self.bearer.as_deref()
    }
}

/// `value` as one POSIX shell word, so a suggested command line stays
/// literal whatever the server-stored name contains: single quotes, with
/// each embedded `'` closed, escaped and reopened.
pub(crate) fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// What a successful reconnect returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconnected {
    pub expires_at: i64,
}

/// What an approved `connect` returned: the grant to store and this run's
/// bearer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub grant: crate::grants::GrantFile,
    pub token: String,
}

/// Map a control-server answer onto the exit-code contract. Command-level
/// failures come back as HTTP 400/404 with `{"error": message}` and map to
/// exit code 1 with the app's message verbatim; a 401/403 carrying one of
/// [`AUTHORIZATION_CODES`] is exit code 4; anything else is a transport
/// fault.
fn classify(what: &str, status: u16, body: Value) -> Result<Value, CtlError> {
    match status {
        200 => Ok(body),
        400 => Err(CtlError::command("app_error", error_message(&body))),
        404 => {
            // Not allowlisted. For a tag command the likely cause is a Staged
            // from before tags reached the control server, not a typo.
            let mut message = error_message(&body);
            if crate::tags::is_tag_command(what) {
                message.push_str(" (this Staged does not have tags yet; update it)");
            }
            Err(CtlError::command("app_error", message))
        }
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
            "unexpected HTTP {code} from control server ({what}): {}",
            error_message(&body)
        ))),
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
        let files = include_str!("../../../src/control_server/files.rs");
        assert!(
            files.contains(&format!(
                "pub(super) const DISCOVERY_FILE_NAME: &str = \"{DISCOVERY_FILE_NAME}\";"
            )),
            "control_server::files::DISCOVERY_FILE_NAME diverged from the CLI's constant"
        );
        let store = include_str!("../../../src/store/control_grants.rs");
        assert!(
            store.contains(&format!(
                "pub const CONTROL_GRANT_LIFETIME_MS: i64 = {};",
                "30 * 24 * 60 * 60 * 1000"
            )) && crate::grants::GRANT_LIFETIME_MS == 30 * 24 * 60 * 60 * 1000,
            "the grant lifetime diverged between the app and the CLI"
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
    fn a_404_on_a_tag_command_hints_at_updating_staged() {
        // An app from before tags reached the control server answers a tag
        // command the way it answers any unknown command. Only the tag
        // commands get the hint; a 400 from a current app is passed through.
        let refusal = || serde_json::json!({"error": "command not available through stagedctl: get_tag_state"});
        let err = classify("get_tag_state", 404, refusal()).unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert_eq!(
            err.to_string(),
            "app_error: command not available through stagedctl: get_tag_state \
             (this Staged does not have tags yet; update it)"
        );
        let err = classify(
            "list_projects",
            404,
            serde_json::json!({"error": "command not available through stagedctl: list_projects"}),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "app_error: command not available through stagedctl: list_projects"
        );
        let err = classify(
            "set_project_group_tag",
            400,
            serde_json::json!({"error": "Tag group not found: g1"}),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "app_error: Tag group not found: g1");
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
        // The server spells each code once; the CLI's list must cover
        // every one it sends.
        let server = include_str!("../../../src/control_server.rs");
        let grants = include_str!("../../../src/control_server/grants.rs");
        for code in AUTHORIZATION_CODES {
            assert!(AUTHORIZATION_CODES.contains(code));
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
    fn empty_env_value_is_set_but_invalid_not_unset() {
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), std::process::id(), 1, 1);
        let err = resolve_lock_candidates(None, Some(std::ffi::OsString::new()), Some(dir.path()))
            .unwrap_err();
        assert_eq!(err.exit_code(), 3, "{err}");
        assert!(
            err.to_string().contains("set but empty"),
            "never falls back to the instance file: {err}"
        );
        // The flag still wins over an empty env value.
        let flag = PathBuf::from("/flag/discovery.json");
        let got = resolve_lock_candidates(
            Some(&flag),
            Some(std::ffi::OsString::new()),
            Some(dir.path()),
        )
        .unwrap();
        assert_eq!(got, vec![flag]);
    }

    #[test]
    fn shell_single_quoting_keeps_a_name_literal() {
        assert_eq!(shell_single_quote("Claude Code"), "'Claude Code'");
        assert_eq!(shell_single_quote("$(rm -rf ~)"), "'$(rm -rf ~)'");
        assert_eq!(shell_single_quote("`id`"), "'`id`'");
        assert_eq!(shell_single_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_single_quote(""), "''");
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
    /// via the handler (which sees the whole request head — request line
    /// and headers — and the body), then close. Enough to exercise the
    /// reqwest client over either transport.
    fn run_fake<S, I, F>(incoming: I, mut handler: F)
    where
        S: Read + Write,
        I: Iterator<Item = std::io::Result<S>>,
        F: FnMut(&str, &str) -> FakeResponse,
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
            let request_body = String::from_utf8_lossy(&buf[header_end..]).to_string();
            let (status, body, extra) = handler(&head, &request_body);
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

    fn fake_server_with_headers<F>(mut handler: F) -> (u16, std::thread::JoinHandle<()>)
    where
        F: FnMut(&str) -> FakeResponse + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            run_fake(listener.incoming(), move |head, _body| handler(head))
        });
        (port, handle)
    }

    /// Loopback-TCP fake whose handler also sees the request body (the
    /// credential routes carry their arguments there).
    fn fake_server_with_body<F>(mut handler: F) -> (u16, std::thread::JoinHandle<()>)
    where
        F: FnMut(&str, &str) -> (u16, String) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            run_fake(listener.incoming(), move |head, body| {
                let (status, body) = handler(head, body);
                (status, body, Vec::new())
            })
        });
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
            run_fake(listener.incoming(), move |head, _body| {
                let (status, body) = handler(head);
                (status, body, Vec::new())
            })
        })
    }

    // -----------------------------------------------------------------
    // Connected-app grants
    // -----------------------------------------------------------------

    fn grant_file(id: &str, secret: &str, expires_at: i64) -> crate::grants::GrantFile {
        crate::grants::GrantFile {
            grant_id: id.to_string(),
            reconnect_secret: secret.to_string(),
            client: "my-agent".to_string(),
            scope: crate::grants::GrantScope { project_id: None },
            access: "write".to_string(),
            expires_at,
        }
    }

    /// A `known` secret with this value stands for a grant the fake app has
    /// revoked: its challenge answers `authorization_revoked`.
    const REVOKED_SECRET: &str = "<revoked>";
    /// A `known` secret with this value stands for a grant whose thirty
    /// days ran out app-side: its challenge answers `authorization_expired`.
    const EXPIRED_SECRET: &str = "<expired>";
    /// A `known` secret with this value stands for a grant whose challenge
    /// fails inside the app (an HTTP 500 — a transport fault to the CLI).
    const BROKEN_SECRET: &str = "<broken>";

    /// A fake app that knows the grants in `known` (id → secret) and issues
    /// one fixed nonce per challenge, checking the proof on reconnect
    /// exactly as `control_server::enrol` does. Records every
    /// `Authorization` header it sees.
    fn grant_aware_server(
        pid: u32,
        known: Vec<(String, String)>,
        renew_to: i64,
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> (u16, std::thread::JoinHandle<()>) {
        fake_server_with_body(move |head, body| {
            seen.lock().unwrap().push(
                header_value(head, "authorization")
                    .unwrap_or("")
                    .to_string(),
            );
            let args: serde_json::Value = serde_json::from_str(body).unwrap_or(Value::Null);
            let grant_id = args.get("grantId").and_then(Value::as_str).unwrap_or("");
            let secret = known.iter().find(|(id, _)| id == grant_id).map(|(_, s)| s);
            if head.starts_with("GET /api/ping") {
                (200, ping_body(pid, 1, PROTOCOL_VERSION))
            } else if head.starts_with("POST /api/reconnect/challenge") {
                match secret {
                    Some(secret) if secret == REVOKED_SECRET => (
                        401,
                        r#"{"error":"the grant was revoked in Staged","code":"authorization_revoked"}"#
                            .to_string(),
                    ),
                    Some(secret) if secret == EXPIRED_SECRET => (
                        401,
                        r#"{"error":"the approval expired","code":"authorization_expired"}"#
                            .to_string(),
                    ),
                    Some(secret) if secret == BROKEN_SECRET => {
                        (500, r#"{"error":"store task failed"}"#.to_string())
                    }
                    Some(_) => (
                        200,
                        serde_json::json!({ "nonce": format!("nonce-for-{grant_id}"), "expiresAt": 0 })
                            .to_string(),
                    ),
                    None => (
                        401,
                        r#"{"error":"unknown grant","code":"authorization_required"}"#.to_string(),
                    ),
                }
            } else if head.starts_with("POST /api/reconnect") {
                let Some(secret) = secret else {
                    return (
                        401,
                        r#"{"error":"unknown grant","code":"authorization_required"}"#.to_string(),
                    );
                };
                let expected = crate::grants::reconnect_proof(
                    &grant_file(grant_id, secret, 0),
                    &format!("nonce-for-{grant_id}"),
                );
                let proof = args.get("proof").and_then(Value::as_str).unwrap_or("");
                let nonce = args.get("nonce").and_then(Value::as_str).unwrap_or("");
                if proof == expected && nonce == format!("nonce-for-{grant_id}") {
                    (
                        200,
                        serde_json::json!({
                            "grantId": grant_id,
                            "token": format!("live-{grant_id}"),
                            "expiresAt": renew_to,
                            "client": "my-agent",
                            "scope": { "projectId": null },
                            "access": "write",
                        })
                        .to_string(),
                    )
                } else {
                    (
                        401,
                        r#"{"error":"the reconnect proof did not verify","code":"authorization_proof_mismatch"}"#
                            .to_string(),
                    )
                }
            } else if head.starts_with("POST /api/disconnect") {
                (200, r#"{"revoked":true}"#.to_string())
            } else {
                (200, "null".to_string())
            }
        })
    }

    #[test]
    fn an_instance_discovery_file_attaches_the_freshest_grant_by_proof() {
        let pid = std::process::id();
        let now = crate::grants::now_ms();
        let renew_to = now + crate::grants::GRANT_LIFETIME_MS - 1000;
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let (port, _handle) = grant_aware_server(
            pid,
            vec![
                ("01de5".to_string(), "s-older".to_string()),
                ("4e77e4".to_string(), "s-newer".to_string()),
            ],
            renew_to,
            std::sync::Arc::clone(&seen),
        );
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        crate::grants::write_grant_file(dir.path(), &grant_file("01de5", "s-older", now + 1000))
            .unwrap();
        crate::grants::write_grant_file(dir.path(), &grant_file("4e77e4", "s-newer", now + 2000))
            .unwrap();
        // An expired grant is never tried, however fresh it looks.
        crate::grants::write_grant_file(dir.path(), &grant_file("5a1e", "s-stale", now - 1))
            .unwrap();

        let client = discover_and_connect(None, None, Some(dir.path())).unwrap();
        assert_eq!(client.bearer(), Some("live-4e77e4"));
        client
            .invoke("list_projects", serde_json::json!({}))
            .unwrap();
        let seen = seen.lock().unwrap();
        // ping (instance token), challenge, reconnect, invoke (grant bearer).
        assert_eq!(seen.len(), 4, "{seen:?}");
        assert_eq!(seen[3], "Bearer live-4e77e4");

        // The renewed expiry was recorded; the others are untouched.
        let files: Vec<(String, i64)> = crate::grants::read_grant_files(dir.path())
            .into_iter()
            .map(|(_, g)| (g.grant_id, g.expires_at))
            .collect();
        assert_eq!(
            files,
            [
                ("4e77e4".to_string(), renew_to),
                ("01de5".to_string(), now + 1000),
                ("5a1e".to_string(), now - 1),
            ]
        );
    }

    #[test]
    fn a_grant_the_app_does_not_know_is_skipped_for_the_next() {
        let pid = std::process::id();
        let now = crate::grants::now_ms();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let (port, _handle) = grant_aware_server(
            pid,
            vec![("c0de".to_string(), "s-known".to_string())],
            now + crate::grants::GRANT_LIFETIME_MS,
            seen,
        );
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        crate::grants::write_grant_file(dir.path(), &grant_file("c0de", "s-known", now + 1000))
            .unwrap();
        crate::grants::write_grant_file(dir.path(), &grant_file("f046", "s-forgotten", now + 5000))
            .unwrap();

        let client = discover_and_connect(None, None, Some(dir.path())).unwrap();
        assert_eq!(client.bearer(), Some("live-c0de"));
    }

    #[test]
    fn a_revoked_or_expired_grant_is_reported_and_a_bad_renewal_refused() {
        let pid = std::process::id();
        let now = crate::grants::now_ms();
        let (port, _handle) = fake_server(move |head| {
            if head.starts_with("GET /api/ping") {
                (200, ping_body(pid, 1, PROTOCOL_VERSION))
            } else {
                (
                    401,
                    r#"{"error":"the grant was revoked in Staged","code":"authorization_revoked"}"#
                        .to_string(),
                )
            }
        });
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        crate::grants::write_grant_file(dir.path(), &grant_file("a1", "s", now + 1000)).unwrap();
        let err = discover_and_connect(None, None, Some(dir.path())).unwrap_err();
        assert_eq!(err.exit_code(), 4, "{err}");
        assert_eq!(
            err.to_string(),
            "authorization_revoked: the grant was revoked in Staged"
        );

        // A renewal further out than thirty days is not something the app
        // would send; refuse it rather than record it.
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (port, _handle) = grant_aware_server(
            pid,
            vec![("a1".to_string(), "s".to_string())],
            now + crate::grants::GRANT_LIFETIME_MS + 24 * 60 * 60 * 1000,
            seen,
        );
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        crate::grants::write_grant_file(dir.path(), &grant_file("a1", "s", now + 1000)).unwrap();
        let err = discover_and_connect(None, None, Some(dir.path())).unwrap_err();
        assert_eq!(err.exit_code(), 2, "{err}");
        assert!(err.to_string().contains("thirty days"), "{err}");
    }

    #[test]
    fn a_revoked_grant_file_is_noted_and_the_next_grant_attaches() {
        // The user revoked "01d0" in Settings and later approved "4e77"; the
        // stale file stays until `disconnect`, and must not block "4e77".
        let pid = std::process::id();
        let now = crate::grants::now_ms();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let (port, _handle) = grant_aware_server(
            pid,
            vec![
                ("01d0".to_string(), REVOKED_SECRET.to_string()),
                ("4e77".to_string(), "s-new".to_string()),
            ],
            now + crate::grants::GRANT_LIFETIME_MS,
            std::sync::Arc::clone(&seen),
        );
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        crate::grants::write_grant_file(dir.path(), &grant_file("4e77", "s-new", now + 1000))
            .unwrap();
        // Fresher, so tried first.
        crate::grants::write_grant_file(dir.path(), &grant_file("01d0", "x", now + 5000)).unwrap();

        let client = discover_and_connect(None, None, Some(dir.path())).unwrap();
        assert_eq!(client.bearer(), Some("live-4e77"));
        // ping, old's challenge (refused), new's challenge, new's reconnect.
        assert_eq!(seen.lock().unwrap().len(), 4);

        // With only refused grants, the first refusal is the error.
        let (port, _handle) = grant_aware_server(
            pid,
            vec![("01d0".to_string(), REVOKED_SECRET.to_string())],
            now + crate::grants::GRANT_LIFETIME_MS,
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        );
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        crate::grants::write_grant_file(dir.path(), &grant_file("01d0", "x", now + 5000)).unwrap();
        crate::grants::write_grant_file(dir.path(), &grant_file("f046", "y", now + 1000)).unwrap();
        let err = discover_and_connect(None, None, Some(dir.path())).unwrap_err();
        assert_eq!(err.exit_code(), 4, "{err}");
        assert!(
            err.to_string().starts_with("authorization_revoked:"),
            "{err}"
        );
    }

    #[test]
    fn a_session_credential_is_recognised_by_its_file_name_before_any_io() {
        let session = PathBuf::from("/data/control/4242/sessions/abc-3.json");
        let instance = PathBuf::from("/data/control/4242/discovery.json");
        assert_eq!(
            session_credential_in_use(Some(&session), None, None).as_deref(),
            Some(session.as_path())
        );
        assert_eq!(
            session_credential_in_use(None, Some(session.clone().into()), None).as_deref(),
            Some(session.as_path()),
            "$STAGEDCTL_LOCK is how a spawned session gets its credential"
        );
        assert_eq!(session_credential_in_use(Some(&instance), None, None), None);
        // The data-dir fallback finds a live instance here, and still says
        // no: it yields only instance files.
        let dir = private_tempdir();
        let live = write_tcp_discovery(dir.path(), std::process::id(), 1, 1);
        assert!(is_instance_discovery_file(&live));
        assert_eq!(
            resolve_lock_candidates(None, None, Some(dir.path())).unwrap(),
            vec![live],
            "the fallback does see the live instance"
        );
        assert_eq!(
            session_credential_in_use(None, None, Some(dir.path())),
            None,
            "…and an instance file is not a session credential"
        );
        // An empty $STAGEDCTL_LOCK is not a session credential either (it
        // is an error at discovery), so connect's local check defers to
        // the environment error rather than inventing a path.
        assert_eq!(
            session_credential_in_use(None, Some(std::ffi::OsString::new()), Some(dir.path())),
            None
        );
    }

    /// `authorization_expired` continues to the next grant like a revoked
    /// one, and when nothing attaches the *first* refusal is reported
    /// whatever came after it.
    #[test]
    fn an_expired_grant_file_is_noted_and_the_first_refusal_wins() {
        let pid = std::process::id();
        let now = crate::grants::now_ms();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (port, _handle) = grant_aware_server(
            pid,
            vec![
                ("5a1e".to_string(), EXPIRED_SECRET.to_string()),
                ("600d".to_string(), "s".to_string()),
            ],
            now + crate::grants::GRANT_LIFETIME_MS,
            std::sync::Arc::clone(&seen),
        );
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        // The expired-app-side grant is fresher locally (its file never
        // learnt of the expiry), so it is tried first and passed over.
        crate::grants::write_grant_file(dir.path(), &grant_file("5a1e", "x", now + 9_000)).unwrap();
        crate::grants::write_grant_file(dir.path(), &grant_file("600d", "s", now + 5_000)).unwrap();
        let client = discover_and_connect(None, None, Some(dir.path())).unwrap();
        assert_eq!(client.bearer(), Some("live-600d"));
        // ping, stale's challenge (expired), good's challenge, good's reconnect.
        assert_eq!(seen.lock().unwrap().len(), 4);

        // Two refusals: expired first, revoked second → expired is reported.
        let (port, _handle) = grant_aware_server(
            pid,
            vec![
                ("5a1e".to_string(), EXPIRED_SECRET.to_string()),
                ("60e0".to_string(), REVOKED_SECRET.to_string()),
            ],
            now + crate::grants::GRANT_LIFETIME_MS,
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        );
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        crate::grants::write_grant_file(dir.path(), &grant_file("5a1e", "x", now + 9_000)).unwrap();
        crate::grants::write_grant_file(dir.path(), &grant_file("60e0", "y", now + 5_000)).unwrap();
        let err = discover_and_connect(None, None, Some(dir.path())).unwrap_err();
        assert_eq!(err.exit_code(), 4, "{err}");
        assert!(
            err.to_string().starts_with("authorization_expired:"),
            "first refusal wins: {err}"
        );
    }

    /// A reconnect that fails for a non-authorization reason on one grant
    /// (here the app's challenge 500s) is noted and the search continues;
    /// it is the reported error only when no grant attaches and none was
    /// refused outright.
    #[test]
    fn a_transport_failure_on_one_grant_does_not_hide_the_next() {
        let pid = std::process::id();
        let now = crate::grants::now_ms();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (port, _handle) = grant_aware_server(
            pid,
            vec![
                ("b40c".to_string(), BROKEN_SECRET.to_string()),
                ("600d".to_string(), "s".to_string()),
            ],
            now + crate::grants::GRANT_LIFETIME_MS,
            std::sync::Arc::clone(&seen),
        );
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        crate::grants::write_grant_file(dir.path(), &grant_file("b40c", "x", now + 9_000)).unwrap();
        crate::grants::write_grant_file(dir.path(), &grant_file("600d", "s", now + 5_000)).unwrap();
        let client = discover_and_connect(None, None, Some(dir.path())).unwrap();
        assert_eq!(client.bearer(), Some("live-600d"));
        assert_eq!(seen.lock().unwrap().len(), 4);

        // Only a broken grant: the transport error is reported (exit 2)…
        let (port, _handle) = grant_aware_server(
            pid,
            vec![("b40c".to_string(), BROKEN_SECRET.to_string())],
            now + crate::grants::GRANT_LIFETIME_MS,
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        );
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        crate::grants::write_grant_file(dir.path(), &grant_file("b40c", "x", now + 9_000)).unwrap();
        let err = discover_and_connect(None, None, Some(dir.path())).unwrap_err();
        assert_eq!(err.exit_code(), 2, "{err}");
        assert!(err.to_string().contains("HTTP 500"), "{err}");

        // …unless some grant was refused outright, which is the more
        // specific answer whatever the order.
        let (port, _handle) = grant_aware_server(
            pid,
            vec![
                ("b40c".to_string(), BROKEN_SECRET.to_string()),
                ("60e0".to_string(), REVOKED_SECRET.to_string()),
            ],
            now + crate::grants::GRANT_LIFETIME_MS,
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        );
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        crate::grants::write_grant_file(dir.path(), &grant_file("b40c", "x", now + 9_000)).unwrap();
        crate::grants::write_grant_file(dir.path(), &grant_file("60e0", "y", now + 5_000)).unwrap();
        let err = discover_and_connect(None, None, Some(dir.path())).unwrap_err();
        assert_eq!(err.exit_code(), 4, "{err}");
        assert!(
            err.to_string().starts_with("authorization_revoked:"),
            "{err}"
        );
    }

    /// A grant file whose secret no longer matches the app's row (the app
    /// knows the grant id, so the challenge succeeds, but the proof fails)
    /// is a refusal in its own right — `authorization_proof_mismatch`, not
    /// the silent skip an unknown grant gets — so when nothing else
    /// attaches the user learns to `disconnect` then `connect`, and a later
    /// grant under another name still attaches.
    #[test]
    fn a_grant_file_that_no_longer_matches_is_a_refusal_not_a_silent_skip() {
        let pid = std::process::id();
        let now = crate::grants::now_ms();
        // Only the mismatched grant: the refusal is the error, exit 4.
        let (port, _handle) = grant_aware_server(
            pid,
            vec![("c0de".to_string(), "app-side-secret".to_string())],
            now + crate::grants::GRANT_LIFETIME_MS,
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        );
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        crate::grants::write_grant_file(
            dir.path(),
            &grant_file("c0de", "stale-local-secret", now + 9_000),
        )
        .unwrap();
        let err = discover_and_connect(None, None, Some(dir.path())).unwrap_err();
        assert_eq!(err.exit_code(), 4, "{err}");
        assert!(
            err.to_string().starts_with("authorization_proof_mismatch:"),
            "{err}"
        );

        // A matching grant written earlier (so searched second) attaches.
        let (port, _handle) = grant_aware_server(
            pid,
            vec![
                ("c0de".to_string(), "app-side-secret".to_string()),
                ("600d".to_string(), "s2".to_string()),
            ],
            now + crate::grants::GRANT_LIFETIME_MS,
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        );
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        crate::grants::write_grant_file(
            dir.path(),
            &grant_file("c0de", "stale-local-secret", now + 9_000),
        )
        .unwrap();
        crate::grants::write_grant_file(dir.path(), &grant_file("600d", "s2", now + 5_000))
            .unwrap();
        let client = discover_and_connect(None, None, Some(dir.path())).unwrap();
        assert_eq!(client.bearer(), Some("live-600d"));
    }

    #[test]
    fn a_per_session_file_never_consults_the_grants() {
        let pid = std::process::id();
        let now = crate::grants::now_ms();
        let token = "5e".repeat(32);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let (port, _handle) = grant_aware_server(
            pid,
            vec![("a1".to_string(), "s".to_string())],
            now + crate::grants::GRANT_LIFETIME_MS,
            std::sync::Arc::clone(&seen),
        );
        let dir = private_tempdir();
        crate::grants::write_grant_file(dir.path(), &grant_file("a1", "s", now + 1000)).unwrap();
        let session_file = instance_dir(dir.path(), pid)
            .join("sessions")
            .join("abc.json");
        make_private_dir(session_file.parent().unwrap());
        write_private(
            &session_file,
            &discovery_json(serde_json::json!({ "port": port, "token": token }), pid, 1),
        );

        let client = discover_and_connect(Some(&session_file), None, Some(dir.path())).unwrap();
        assert_eq!(client.bearer(), Some(token.as_str()));
        assert_eq!(seen.lock().unwrap().len(), 1, "ping only; no challenge");
        // A session lease is confined to its project; the client knows
        // without asking.
        assert_eq!(client.scope, CredentialScope::Project);
    }

    #[test]
    fn the_credential_scope_follows_the_grant_that_attached() {
        let pid = std::process::id();
        let now = crate::grants::now_ms();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let (port, _handle) = grant_aware_server(
            pid,
            vec![("a1".to_string(), "s".to_string())],
            now + crate::grants::GRANT_LIFETIME_MS,
            seen,
        );
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        let unconfined = grant_file("a1", "s", now + 1000);
        crate::grants::write_grant_file(dir.path(), &unconfined).unwrap();
        let client = discover_and_connect(None, None, Some(dir.path())).unwrap();
        assert_eq!(client.scope, CredentialScope::AllProjects);

        let scoped = crate::grants::GrantFile {
            scope: crate::grants::GrantScope {
                project_id: Some("p-1".to_string()),
            },
            ..unconfined
        };
        crate::grants::write_grant_file(dir.path(), &scoped).unwrap();
        let client = discover_and_connect(None, None, Some(dir.path())).unwrap();
        assert_eq!(client.scope, CredentialScope::Project);
    }

    #[test]
    fn a_partial_failure_reports_the_applied_steps_then_the_inner_error() {
        let inner = CtlError::command("app_error", "Tag not found: t-x");
        let err = CtlError::Partial {
            error: Box::new(inner),
            applied: vec![serde_json::json!({ "command": "set_project_group_tag" })],
            human: "tagged Widgets  id:p1\n  Status: Done".to_string(),
        };
        assert_eq!(err.exit_code(), 1);
        assert_eq!(err.code(), "app_error");
        assert_eq!(err.to_string(), "app_error: Tag not found: t-x");
        assert_eq!(
            err.partial_output(false).as_deref(),
            Some("tagged Widgets  id:p1\n  Status: Done")
        );
        let json: Value = serde_json::from_str(&err.partial_output(true).unwrap()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "ok": false,
                "applied": [{ "command": "set_project_group_tag" }],
                "error": { "code": "app_error", "message": "Tag not found: t-x" },
            })
        );
        // An authorization failure part-way keeps its own exit code.
        let err = CtlError::Partial {
            error: Box::new(CtlError::Authorization {
                code: "scope_denied".to_string(),
                message: "outside".to_string(),
            }),
            applied: Vec::new(),
            human: String::new(),
        };
        assert_eq!(err.exit_code(), 4);
        assert_eq!(err.code(), "scope_denied");
        // Other errors have nothing to put on stdout.
        assert_eq!(CtlError::command("x", "y").partial_output(true), None);
        assert_eq!(CtlError::Transport("t".into()).partial_output(false), None);
    }

    #[test]
    fn connect_requests_then_polls_and_a_denial_is_exit_4() {
        let pid = std::process::id();
        let polls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (port, _handle) = {
            let polls = std::sync::Arc::clone(&polls);
            fake_server_with_body(move |head, body| {
                if head.starts_with("GET /api/ping") {
                    (200, ping_body(pid, 1, PROTOCOL_VERSION))
                } else if head.starts_with("POST /api/connect") {
                    let args: Value = serde_json::from_str(body).unwrap();
                    assert_eq!(args["client"], "my-agent");
                    assert_eq!(args["project"], "Widgets");
                    assert_eq!(args["readOnly"], true);
                    (200, r#"{"requestId":"req-1","expiresAt":99}"#.to_string())
                } else if head.starts_with("GET /api/connect/req-1") {
                    let n = polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n == 0 {
                        (202, r#"{"status":"pending","expiresAt":99}"#.to_string())
                    } else {
                        (
                            200,
                            serde_json::json!({
                                "status": "approved",
                                "grantId": "g-1",
                                "token": "live-1",
                                "reconnectSecret": "secret-1",
                                "expiresAt": 12345,
                                "client": "my-agent",
                                "scope": { "projectId": "p-1" },
                                "access": "read",
                            })
                            .to_string(),
                        )
                    }
                } else if head.starts_with("GET /api/connect/req-denied") {
                    (
                        403,
                        r#"{"error":"the Staged user denied the request","code":"authorization_denied"}"#
                            .to_string(),
                    )
                } else if head.starts_with("GET /api/connect/req-expired") {
                    (
                        401,
                        r#"{"error":"the request expired","code":"authorization_required"}"#
                            .to_string(),
                    )
                } else {
                    (404, r#"{"error":"unknown request"}"#.to_string())
                }
            })
        };
        let dir = private_tempdir();
        let lock = write_tcp_discovery(dir.path(), pid, port, 1);
        let (client, path) = discover_instance(None, None, Some(dir.path())).unwrap();
        assert_eq!(path, lock);

        let (request_id, expires_at) = client
            .request_authorization("my-agent", Some("Widgets"), true)
            .unwrap();
        assert_eq!((request_id.as_str(), expires_at), ("req-1", 99));
        assert_eq!(client.poll_authorization("req-1").unwrap(), None, "pending");
        let approval = client.poll_authorization("req-1").unwrap().unwrap();
        assert_eq!(approval.token, "live-1");
        assert_eq!(
            approval.grant,
            crate::grants::GrantFile {
                grant_id: "g-1".to_string(),
                reconnect_secret: "secret-1".to_string(),
                client: "my-agent".to_string(),
                scope: crate::grants::GrantScope {
                    project_id: Some("p-1".to_string())
                },
                access: "read".to_string(),
                expires_at: 12345,
            }
        );

        let err = client.poll_authorization("req-denied").unwrap_err();
        assert_eq!(err.exit_code(), 4, "{err}");
        assert_eq!(
            err.to_string(),
            "authorization_denied: the Staged user denied the request"
        );
        let err = client.poll_authorization("req-expired").unwrap_err();
        assert_eq!(err.exit_code(), 4, "{err}");
        let err = client.poll_authorization("req-unknown").unwrap_err();
        assert_eq!(err.exit_code(), 1, "{err}");
    }

    #[test]
    fn a_full_prompt_queue_is_a_command_error() {
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |head| {
            if head.starts_with("GET /api/ping") {
                (200, ping_body(pid, 1, PROTOCOL_VERSION))
            } else {
                (429, r#"{"error":"too many requests waiting"}"#.to_string())
            }
        });
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        let (client, _) = discover_instance(None, None, Some(dir.path())).unwrap();
        let err = client
            .request_authorization("my-agent", None, false)
            .unwrap_err();
        assert_eq!(err.exit_code(), 1, "{err}");
        assert_eq!(
            err.to_string(),
            "too_many_requests: too many requests waiting"
        );
    }

    #[test]
    fn revoke_grant_proves_the_secret_and_accepts_an_already_dead_grant() {
        let pid = std::process::id();
        let now = crate::grants::now_ms();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (port, _handle) = grant_aware_server(
            pid,
            vec![("a1".to_string(), "s".to_string())],
            now + crate::grants::GRANT_LIFETIME_MS,
            seen,
        );
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        let (client, _) = discover_instance(None, None, Some(dir.path())).unwrap();
        assert!(client.revoke_grant(&grant_file("a1", "s", now)).unwrap());

        let (port, _handle) = fake_server(move |head| {
            if head.starts_with("GET /api/ping") {
                (200, ping_body(pid, 1, PROTOCOL_VERSION))
            } else {
                (
                    401,
                    r#"{"error":"the grant expired","code":"authorization_expired"}"#.to_string(),
                )
            }
        });
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        let (client, _) = discover_instance(None, None, Some(dir.path())).unwrap();
        assert!(
            !client.revoke_grant(&grant_file("a1", "s", now)).unwrap(),
            "nothing left to revoke"
        );
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
