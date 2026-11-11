//! Discovery, handshake, and transport for talking to a running Staged app.
//!
//! The app writes `~/.staged/control-<pid>.json` when its control server
//! starts (see `src-tauri/src/control_server.rs`) and injects that path into
//! spawned agent sessions as `$STAGEDCTL_LOCK`. The CLI resolves the file,
//! pings the server to verify it is talking to the instance the file
//! describes (pid + generation) and that both sides speak the same protocol
//! version, and only then sends any command payload. Without an explicit
//! path it tries every live-pid discovery file, newest first, until one
//! passes that handshake — file freshness alone can lie (pid reuse).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

/// Version of the CLI ↔ app wire contract. Must equal
/// `control_server::PROTOCOL_VERSION` in the app (pinned by test); a
/// mismatch means the app was updated out from under this binary.
pub const PROTOCOL_VERSION: u32 = 1;

/// Env var carrying the discovery-file path into spawned agent sessions.
/// Must equal `control_server::STAGEDCTL_LOCK_ENV` (pinned by test).
pub const STAGEDCTL_LOCK_ENV: &str = "STAGEDCTL_LOCK";

/// How stagedctl failed, mapped onto the exit-code contract:
/// 1 command error, 2 transport fault, 3 environment (not under a Staged
/// app, app gone, or protocol version mismatch).
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
        }
    }
}

impl std::fmt::Display for CtlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command { code, message } => write!(f, "{code}: {message}"),
            Self::Transport(message) => write!(f, "transport: {message}"),
            Self::Environment(message) => write!(f, "{message}"),
        }
    }
}

/// Contents of a `control-<pid>.json` discovery file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub port: u16,
    pub pid: u32,
    pub generation: u64,
    pub protocol_version: u32,
}

/// Where discovery files live: `~/.staged`.
pub fn default_data_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".staged"))
}

/// Resolve the discovery files to try, in order: `--lock-path` beats
/// `$STAGEDCTL_LOCK` (each an exact, single candidate) beats every live-pid
/// `control-*.json` in the data dir, newest first. No candidate at all
/// means we are not under a Staged app.
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
             Staged instance was found (no live control-<pid>.json under ~/.staged). \
             Start the Staged app, or pass --lock-path."
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

/// Every `control-<pid>.json` in `dir` whose embedded pid is a live process,
/// newest-mtime first. Dead-pid leftovers are the app's problem to sweep; we
/// just skip them. A live pid is necessary but not sufficient: a crashed
/// instance's file survives every sweep once an unrelated process reclaims
/// its pid, so callers must not commit to a single candidate — they try each
/// in order until the ping handshake picks out a real control server.
fn live_discovery_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(pid) = name
            .strip_prefix("control-")
            .and_then(|rest| rest.strip_suffix(".json"))
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if !is_process_alive(pid) {
            continue;
        }
        let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        candidates.push((mtime, entry.path()));
    }
    candidates.sort_by(|(a, _), (b, _)| b.cmp(a));
    candidates.into_iter().map(|(_, path)| path).collect()
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    // Signal 0 probes existence without touching the process; EPERM still
    // proves something is running under that pid.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn is_process_alive(_pid: u32) -> bool {
    // No cheap probe; a dead pid just fails at ping time instead.
    true
}

/// Read and parse a discovery file, with one short retry so a concurrent
/// atomic rewrite never surfaces as a parse error.
pub fn read_discovery(path: &Path) -> Result<Discovery, CtlError> {
    let mut last_err = String::new();
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(200));
        }
        match std::fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str::<Discovery>(&contents) {
                Ok(discovery) => return Ok(discovery),
                Err(e) => last_err = format!("cannot parse {}: {e}", path.display()),
            },
            Err(e) => {
                return Err(CtlError::Environment(format!(
                    "cannot read control file {}: {e} — the Staged app that wrote it has \
                     probably exited; restart Staged or re-run from a fresh session",
                    path.display()
                )))
            }
        }
    }
    Err(CtlError::Environment(format!(
        "{last_err} — the file may be corrupt; restart Staged"
    )))
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
    /// Read the discovery file and perform the ping handshake. No command
    /// payload is ever sent to a port that hasn't passed this check, so
    /// prompts can't be sprayed at whatever local service squats on a stale
    /// port. One re-read + retry absorbs an app restart between file-read
    /// and ping.
    pub fn connect(lock_path: &Path) -> Result<Client, CtlError> {
        let http = http_client()?;

        let mut discovery = read_discovery(lock_path)?;
        for attempt in 0..2 {
            match ping(&http, discovery.port) {
                Ok(pong) => {
                    if pong.protocol_version != PROTOCOL_VERSION {
                        return Err(CtlError::Environment(format!(
                            "app updated — restart Staged (the app speaks stagedctl protocol \
                             v{}, this CLI speaks v{PROTOCOL_VERSION})",
                            pong.protocol_version
                        )));
                    }
                    if pong.pid == discovery.pid && pong.generation == discovery.generation {
                        return Ok(Client {
                            http,
                            base: format!("http://127.0.0.1:{}", discovery.port),
                        });
                    }
                    // Another instance answered, or the server restarted
                    // between file-read and ping: re-read the file once.
                }
                Err(e) => {
                    if attempt > 0 {
                        return Err(CtlError::Environment(format!(
                            "cannot reach the Staged app on 127.0.0.1:{} ({e}) — the app has \
                             probably exited; restart Staged or re-run from a fresh session",
                            discovery.port
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

    /// POST one allowlisted command. Command-level failures come back as
    /// HTTP 400/404 with `{"error": message}` and map to exit code 1 with
    /// the app's message verbatim; anything else is a transport fault.
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
            403 => Err(CtlError::Transport(format!(
                "control server rejected the request: {}",
                error_message(&body)
            ))),
            code => Err(CtlError::Transport(format!(
                "unexpected HTTP {code} from control server: {}",
                error_message(&body)
            ))),
        }
    }
}

/// The HTTP client every call goes through. No proxy — an `HTTP_PROXY` in
/// the agent's environment must never route a loopback control call (and
/// whatever it carries) through a third party — and no redirects, so the
/// app's answer can't send a request elsewhere.
fn http_client() -> Result<reqwest::blocking::Client, CtlError> {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| CtlError::Transport(format!("cannot build HTTP client: {e}")))
}

fn ping(http: &reqwest::blocking::Client, port: u16) -> Result<PingResponse, String> {
    let response = http
        .get(format!("http://127.0.0.1:{port}/api/ping"))
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

fn error_message(body: &Value) -> String {
    body.get("error")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| body.to_string())
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
    }

    // -------------------------------------------------------------------
    // Exit-code contract
    // -------------------------------------------------------------------

    #[test]
    fn exit_codes_follow_the_contract() {
        assert_eq!(CtlError::command("app_error", "boom").exit_code(), 1);
        assert_eq!(CtlError::Transport("boom".into()).exit_code(), 2);
        assert_eq!(CtlError::Environment("boom".into()).exit_code(), 3);
    }

    #[test]
    fn command_errors_render_as_code_colon_message() {
        let err = CtlError::command("app_error", "Branch not found: xyz");
        assert_eq!(err.to_string(), "app_error: Branch not found: xyz");
    }

    // -------------------------------------------------------------------
    // Lock-path resolution
    // -------------------------------------------------------------------

    fn write_discovery_json(dir: &Path, pid: u32, port: u16, generation: u64) -> PathBuf {
        let path = dir.join(format!("control-{pid}.json"));
        std::fs::write(
            &path,
            serde_json::json!({
                "port": port,
                "pid": pid,
                "generation": generation,
                "protocolVersion": PROTOCOL_VERSION,
            })
            .to_string(),
        )
        .unwrap();
        path
    }

    #[test]
    fn flag_beats_env_beats_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let fallback = write_discovery_json(dir.path(), std::process::id(), 1, 1);

        let flag = PathBuf::from("/flag/control-1.json");
        let env = std::ffi::OsString::from("/env/control-2.json");

        let got =
            resolve_lock_candidates(Some(&flag), Some(env.clone()), Some(dir.path())).unwrap();
        assert_eq!(got, vec![flag]);

        let got = resolve_lock_candidates(None, Some(env), Some(dir.path())).unwrap();
        assert_eq!(got, vec![PathBuf::from("/env/control-2.json")]);

        let got = resolve_lock_candidates(None, None, Some(dir.path())).unwrap();
        assert_eq!(got, vec![fallback]);
    }

    #[test]
    fn empty_env_value_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let fallback = write_discovery_json(dir.path(), std::process::id(), 1, 1);
        let got = resolve_lock_candidates(None, Some(std::ffi::OsString::new()), Some(dir.path()))
            .unwrap();
        assert_eq!(got, vec![fallback]);
    }

    #[test]
    fn fallback_skips_dead_pids_and_non_discovery_files() {
        let dir = tempfile::tempdir().unwrap();
        // A pid beyond macOS/Linux pid ranges — dead.
        write_discovery_json(dir.path(), 999_999_999, 1, 1);
        std::fs::write(dir.path().join("data.db"), "not a discovery file").unwrap();
        std::fs::write(dir.path().join("control-abc.json"), "{}").unwrap();

        let err = resolve_lock_candidates(None, None, Some(dir.path())).unwrap_err();
        assert_eq!(err.exit_code(), 3, "no live instance must exit 3: {err}");

        let live = write_discovery_json(dir.path(), std::process::id(), 1, 1);
        let got = resolve_lock_candidates(None, None, Some(dir.path())).unwrap();
        assert_eq!(got, vec![live]);
    }

    #[test]
    fn fallback_orders_live_candidates_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        // Two live pids: our own, and pid 1 (init/launchd — kill(1, 0)
        // yields EPERM, which counts as alive).
        let ours = write_discovery_json(dir.path(), std::process::id(), 1, 1);
        let other = write_discovery_json(dir.path(), 1, 1, 1);
        set_mtime(&ours, std::time::Duration::from_secs(100));
        set_mtime(&other, std::time::Duration::from_secs(10));

        let got = resolve_lock_candidates(None, None, Some(dir.path())).unwrap();
        assert_eq!(got, vec![other, ours]);
    }

    #[test]
    fn no_candidates_is_an_environment_error() {
        let dir = tempfile::tempdir().unwrap();
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
        let err = read_discovery(Path::new("/nonexistent/control-1.json")).unwrap_err();
        assert_eq!(err.exit_code(), 3);
    }

    // -------------------------------------------------------------------
    // Handshake against a fake control server
    // -------------------------------------------------------------------

    /// Minimal HTTP/1.1 server: reads one request per connection, answers
    /// via the handler, closes. Enough to exercise the reqwest client.
    fn fake_server<F>(mut handler: F) -> (u16, std::thread::JoinHandle<()>)
    where
        F: FnMut(&str) -> (u16, String) + Send + 'static,
    {
        fake_server_with_headers(move |request_line| {
            let (status, body) = handler(request_line);
            (status, body, Vec::new())
        })
    }

    /// [`fake_server`] whose handler also returns extra response headers.
    fn fake_server_with_headers<F>(mut handler: F) -> (u16, std::thread::JoinHandle<()>)
    where
        F: FnMut(&str) -> (u16, String, Vec<(String, String)>) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
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
                let request_line = head.lines().next().unwrap_or_default().to_string();
                let (status, body, extra) = handler(&request_line);
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
                if request_line.is_empty() {
                    break;
                }
            }
        });
        (port, handle)
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

    #[test]
    fn handshake_then_invoke_round_trips() {
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |request_line| {
            if request_line.starts_with("GET /api/ping") {
                (200, ping_body(pid, 7, PROTOCOL_VERSION))
            } else if request_line.starts_with("POST /api/invoke/list_projects") {
                (200, r#"[{"id":"p1","name":"demo"}]"#.to_string())
            } else {
                (404, r#"{"error":"unexpected route"}"#.to_string())
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join(format!("control-{pid}.json"));
        std::fs::write(
            &lock,
            serde_json::json!({
                "port": port, "pid": pid, "generation": 7,
                "protocolVersion": PROTOCOL_VERSION,
            })
            .to_string(),
        )
        .unwrap();

        let client = Client::connect(&lock).unwrap();
        let value = client
            .invoke("list_projects", serde_json::json!({}))
            .unwrap();
        assert_eq!(value[0]["name"], "demo");
    }

    #[test]
    fn protocol_version_mismatch_reads_as_app_updated() {
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |_| (200, ping_body(pid, 1, PROTOCOL_VERSION + 1)));
        let dir = tempfile::tempdir().unwrap();
        let lock = write_discovery_json(dir.path(), pid, port, 1);

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
        let dir = tempfile::tempdir().unwrap();
        let lock = write_discovery_json(dir.path(), pid, port, 1);

        let err = Client::connect(&lock).unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert!(err.to_string().contains("restarted"), "got: {err}");
    }

    #[test]
    fn generation_mismatch_recovers_after_file_rewrite() {
        let pid = std::process::id();
        let dir = tempfile::tempdir().unwrap();
        let lock = write_discovery_json(dir.path(), pid, 0, 1); // port fixed below

        let (port, _handle) = {
            let dir = dir.path().to_path_buf();
            fake_server(move |request_line| {
                if request_line.starts_with("GET /api/ping") {
                    // The app has moved on to generation 2; the fixture
                    // rewrites the discovery file the way a real restart
                    // does, so the client's one re-read should succeed.
                    write_discovery_json(
                        &dir,
                        pid,
                        CURRENT_PORT.load(std::sync::atomic::Ordering::SeqCst),
                        2,
                    );
                    (200, ping_body(pid, 2, PROTOCOL_VERSION))
                } else {
                    (200, "null".to_string())
                }
            })
        };
        static CURRENT_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
        CURRENT_PORT.store(port, std::sync::atomic::Ordering::SeqCst);
        write_discovery_json(dir.path(), pid, port, 1);

        let client = Client::connect(&lock).unwrap();
        assert!(client
            .invoke("cancel_session", serde_json::json!({}))
            .is_ok());
    }

    #[test]
    fn fallback_skips_a_zombie_newest_candidate_via_handshake() {
        // Pid-reuse blind spot: a crashed instance's discovery file whose
        // pid was reclaimed by an unrelated long-lived process looks alive
        // to every liveness probe and survives the app's sweep. If it is
        // also newest by mtime, committing to it would fail even though a
        // real instance is running — discovery must fall through to older
        // live candidates until the handshake succeeds.
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |request_line| {
            if request_line.starts_with("GET /api/ping") {
                (200, ping_body(pid, 1, PROTOCOL_VERSION))
            } else if request_line.starts_with("POST /api/invoke/list_projects") {
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

        let dir = tempfile::tempdir().unwrap();
        let real = write_discovery_json(dir.path(), pid, port, 1);
        // The zombie: pid 1 is alive (it just isn't a Staged app), file is
        // newest, port answers nothing.
        write_discovery_json(dir.path(), 1, closed_port, 1);
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
        let dir = tempfile::tempdir().unwrap();
        let lock = write_discovery_json(dir.path(), std::process::id(), port, 1);

        let err = Client::connect(&lock).unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert!(err.to_string().contains("cannot reach"), "got: {err}");
    }

    #[test]
    fn client_never_follows_redirects() {
        // A redirect from the control port would carry the request (and,
        // from protocol v2 on, its bearer) wherever `Location` points. The
        // decoy records whether anything reached it.
        let decoy_hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (decoy_port, _decoy) = {
            let hits = std::sync::Arc::clone(&decoy_hits);
            fake_server(move |_| {
                hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                (200, "[]".to_string())
            })
        };
        let pid = std::process::id();
        let (port, _handle) = fake_server_with_headers(move |request_line| {
            if request_line.starts_with("GET /api/ping") {
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
        let dir = tempfile::tempdir().unwrap();
        let lock = write_discovery_json(dir.path(), pid, port, 1);

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
        let (port, _handle) = fake_server(move |request_line| {
            if request_line.starts_with("GET /api/ping") {
                (200, ping_body(pid, 1, PROTOCOL_VERSION))
            } else {
                (400, r#"{"error":"Branch not found: b1"}"#.to_string())
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let lock = write_discovery_json(dir.path(), pid, port, 1);

        let client = Client::connect(&lock).unwrap();
        let err = client
            .invoke("get_branch_timeline", serde_json::json!({"branchId": "b1"}))
            .unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert_eq!(err.to_string(), "app_error: Branch not found: b1");
    }
}
