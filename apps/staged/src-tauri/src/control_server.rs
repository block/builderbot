//! Loopback control server for `stagedctl`.
//!
//! A plain-HTTP listener on `127.0.0.1:0` that lets a same-host CLI drive the
//! running app through the existing `web_server::dispatch()` logic:
//!
//! - `GET  /api/ping`              — handshake: pid, generation, protocolVersion
//! - `POST /api/invoke/{command}`  — dispatch, restricted to [`ALLOWED_COMMANDS`]
//!
//! **Security model (berd's):** no token. Every request is rejected with 403
//! if it carries an `Origin` header, any `Sec-Fetch-*` header, or a `Host`
//! other than the literal `127.0.0.1:<port>`. That closes browser-JS-to-
//! localhost and DNS rebinding; it deliberately does not exclude other
//! same-user processes. What makes that sound is the fixed verb table:
//! [`ALLOWED_COMMANDS`] maps the v1 `stagedctl` surface to specific dispatch
//! commands — every one UI-visible and read-only or reversible — and nothing
//! else is reachable. No deletes, no history rewrites, no `create_pr` /
//! `push_branch`. Exposing a destructive verb reopens the auth design and
//! must come with a `PROTOCOL_VERSION` bump.
//!
//! Loopback TCP bounds callers to the same *host*, not the same *user*:
//! another local account can find this listener with a port scan of
//! 127.0.0.1 and pass every header check — the 0600 discovery file only
//! hides the port, it does not gate the connection. v1 accepts that,
//! matching berd: Staged targets effectively single-user dev machines, and
//! the allowlist caps what such a caller gets (timeline reads and session
//! starts, nothing destructive). If that assumption stops holding, the
//! upgrade path is a Unix-domain socket with 0600 perms — or a
//! peer-credential (`LOCAL_PEERCRED`/`SO_PEERCRED`) check on this listener
//! — not a reintroduced token.
//!
//! **Discovery:** once the listener is bound — synchronously at app setup,
//! before anything can start a session (see [`start`]) — the server
//! atomically writes `~/.staged/control-<pid>.json` (0600) containing
//! `{port, pid, generation, protocolVersion}`. The pid suffix exists because
//! multiple Staged instances share `~/.staged`; stale files from dead pids
//! are swept on startup and our own file is removed on exit. Agent sessions
//! spawned by Staged receive the file's path in `STAGEDCTL_LOCK` so the CLI
//! finds its owning instance with zero configuration.
//!
//! **Sidecar reachability:** the `stagedctl` binary ships as a Tauri sidecar
//! next to the app executable. At startup this module resolves it, exposes
//! its path to sessions as `STAGEDCTL_BIN`, and builds a per-instance PATH
//! shim dir (`~/.staged/control-bin-<pid>/stagedctl` symlink) that
//! `session_runner` prepends to every local agent session's PATH so a bare
//! `stagedctl` just works. Shim dirs follow the discovery file's lifecycle
//! (pid-suffixed, swept when dead, removed on exit).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::Value;

/// Version of the CLI ↔ app wire contract. Bump on any breaking reshape of
/// the discovery file, the endpoints, or the semantics of an allowlisted
/// command — the CLI refuses to talk to a mismatched app.
pub const PROTOCOL_VERSION: u32 = 1;

/// Env var carrying the discovery-file path into spawned agent sessions.
pub const STAGEDCTL_LOCK_ENV: &str = "STAGEDCTL_LOCK";

/// Env var carrying the absolute path of this instance's bundled `stagedctl`
/// sidecar into spawned agent sessions. The PATH shim dir makes the bare
/// `stagedctl` name resolve; this is the exact-path escape hatch (berd's
/// `BERDCTL_BIN` equivalent).
pub const STAGEDCTL_BIN_ENV: &str = "STAGEDCTL_BIN";

/// The dispatch commands backing the v1 `stagedctl` command table — the only
/// verbs reachable through the control server. Every entry must be visible in
/// the UI and either read-only or reversible (berd's eligibility rule); a
/// destructive verb cannot be added here without reopening the auth design
/// (see module docs).
pub const ALLOWED_COMMANDS: &[&str] = &[
    // project list / project get
    "list_projects",
    "list_project_repos",
    "list_branches_for_project",
    // timeline show
    "get_branch_timeline",
    "get_project_timeline",
    "list_project_notes",
    // session start / status / wait / cancel / list
    "start_branch_session",
    "start_project_session",
    "get_session",
    "get_active_sessions",
    "cancel_session",
    // image add
    "create_image",
];

/// Request body cap for `POST /api/invoke/{command}`. Control payloads are
/// prompts and ids — `create_image` takes a *path*, never file bytes — so
/// this sits far below `web_server`'s cap for base64 PDF uploads, and well
/// above any prompt an agent would reasonably send. axum's default (2 MiB)
/// would otherwise apply implicitly, and a cap that exists only by default
/// is one a refactor can lose.
const CONTROL_BODY_LIMIT: usize = 1024 * 1024;

/// Monotonic per-process start counter, mirrored into the discovery file and
/// the ping response so the CLI can detect a restart between file-read and
/// call.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Discovery-file path, set once the listener is up. Consulted by the env
/// injection in `session_runner` and the exit cleanup; `None` means the
/// server never started (bind failure) and sessions get no `STAGEDCTL_LOCK`.
static DISCOVERY_FILE: OnceLock<PathBuf> = OnceLock::new();

/// Path of the bundled `stagedctl` sidecar, resolved at control-server start.
/// `None` means the sidecar wasn't found next to the app executable (e.g. a
/// bare `cargo run` outside the tauri CLI) and sessions get no
/// `STAGEDCTL_BIN` or PATH shim.
static STAGEDCTL_BIN: OnceLock<PathBuf> = OnceLock::new();

/// Per-instance PATH shim dir (`~/.staged/control-bin-<pid>/`) holding a
/// `stagedctl` symlink to the sidecar, prepended to every local agent
/// session's PATH. Pid-suffixed for the same reason as the discovery file:
/// `~/.staged` is shared, and a dev instance and a production install must
/// never point each other's sessions at the wrong binary (or at a symlink
/// dangling into a cleaned dev target dir). Swept and removed alongside the
/// discovery file.
static STAGEDCTL_SHIM_DIR: OnceLock<PathBuf> = OnceLock::new();

#[derive(Clone)]
struct ControlState {
    app_handle: tauri::AppHandle,
    port: u16,
    generation: u64,
}

/// The `STAGEDCTL_LOCK` env pair for spawned agent sessions, once the control
/// server is up.
pub fn stagedctl_lock_env() -> Option<(String, String)> {
    DISCOVERY_FILE
        .get()
        .map(|path| (STAGEDCTL_LOCK_ENV.to_string(), path.display().to_string()))
}

/// All stagedctl env pairs for spawned agent sessions: `STAGEDCTL_LOCK` once
/// the control server is up, plus `STAGEDCTL_BIN` when the sidecar resolved.
pub fn stagedctl_env() -> Vec<(String, String)> {
    let mut vars: Vec<(String, String)> = stagedctl_lock_env().into_iter().collect();
    if let Some(bin) = STAGEDCTL_BIN.get() {
        vars.push((STAGEDCTL_BIN_ENV.to_string(), bin.display().to_string()));
    }
    vars
}

/// Prepend this instance's shim dir to a captured env snapshot's PATH so a
/// bare `stagedctl` resolves inside spawned agent sessions. No-op when the
/// sidecar (and therefore the shim) is unavailable.
pub fn apply_stagedctl_path(vars: &mut Vec<(String, String)>) {
    if let Some(shim_dir) = STAGEDCTL_SHIM_DIR.get() {
        crate::acp_tools::prepend_dirs_to_path(vars, std::slice::from_ref(shim_dir));
    }
}

/// Whether spawned local sessions can actually run `stagedctl`: the control
/// server is reachable (discovery file written) and the sidecar resolved.
/// Prompt scaffolding consults this so sessions are never told about a CLI
/// they cannot use.
pub fn stagedctl_session_ready() -> bool {
    DISCOVERY_FILE.get().is_some() && STAGEDCTL_BIN.get().is_some()
}

/// Start the control server. Called unconditionally at app setup, after every
/// piece of state a dispatch reads is managed; a failure to bind or to write
/// the discovery file is logged and leaves the app fully functional without a
/// control surface.
///
/// Everything a session start reads — the discovery file, the sidecar path,
/// the PATH shim — is in place when this returns; only accepting connections
/// runs on a background task, and a connection that arrives first waits in
/// the listen backlog. That matters because an agent's env is fixed for its
/// whole life, and startup recovery and the `KickSource::Startup` queue drain
/// start local sessions with no window open: `lib.rs` calls this ahead of
/// them so none of those sessions misses `STAGEDCTL_*`, the shim, or the
/// guidance paragraph.
pub fn start(app_handle: tauri::AppHandle) {
    let prepared = match prepare() {
        Ok(prepared) => prepared,
        Err(e) => {
            log::error!("[control_server] not started: {e}");
            return;
        }
    };
    tauri::async_runtime::spawn(async move {
        if let Err(e) = serve(prepared, app_handle).await {
            log::error!("[control_server] stopped: {e}");
            // Don't leave sessions and CLIs pointed at a port nobody answers.
            remove_instance_files();
        }
    });
}

/// A bound control listener whose discovery file is written. Built by
/// [`prepare`]; [`serve`] only has to accept on it.
struct PreparedListener {
    listener: std::net::TcpListener,
    port: u16,
    generation: u64,
}

/// The synchronous half of [`start`]: sweep, bind, write the discovery file,
/// build the shim, and publish them to [`stagedctl_env`] and
/// [`stagedctl_session_ready`].
fn prepare() -> Result<PreparedListener, String> {
    let dir = crate::paths::data_dir().ok_or("cannot determine ~/.staged")?;
    let pid = std::process::id();
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let prepared = bind_and_write_discovery(&dir, pid, generation)?;
    let path = discovery_file_path(&dir, pid);
    let _ = DISCOVERY_FILE.set(path.clone());
    setup_stagedctl_shim(&dir, pid);
    log::info!(
        "[control_server] bound 127.0.0.1:{}, discovery file {path:?}",
        prepared.port
    );
    Ok(prepared)
}

/// The filesystem and socket work of [`prepare`], kept free of the
/// process-wide statics so tests can run it against a temp dir.
fn bind_and_write_discovery(
    dir: &std::path::Path,
    pid: u32,
    generation: u64,
) -> Result<PreparedListener, String> {
    sweep_stale_discovery_files(dir);

    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("failed to bind control listener: {e}"))?;
    // `serve` hands the socket to tokio, which requires non-blocking mode.
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("failed to make control listener non-blocking: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("failed to get local address: {e}"))?
        .port();

    write_discovery_file(&discovery_file_path(dir, pid), port, pid, generation)?;
    Ok(PreparedListener {
        listener,
        port,
        generation,
    })
}

/// The async half of [`start`]: accept and route requests until the app exits.
async fn serve(prepared: PreparedListener, app_handle: tauri::AppHandle) -> Result<(), String> {
    let PreparedListener {
        listener,
        port,
        generation,
    } = prepared;
    let listener = tokio::net::TcpListener::from_std(listener)
        .map_err(|e| format!("failed to adopt control listener: {e}"))?;

    let state = ControlState {
        app_handle,
        port,
        generation,
    };
    let router = Router::new()
        .route("/api/ping", get(ping))
        .merge(invoke_router(invoke))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            reject_non_local,
        ))
        .with_state(state);

    axum::serve(listener, router)
        .await
        .map_err(|e| format!("control server error: {e}"))
}

/// The invoke route with [`CONTROL_BODY_LIMIT`] applied to it. Generic over
/// the handler so a test can mount a stand-in without a Tauri `AppHandle`
/// and exercise the exact layering [`serve`] uses.
fn invoke_router<S, H, T>(handler: H) -> Router<S>
where
    H: axum::handler::Handler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    Router::new().route(
        "/api/invoke/{command}",
        post(handler).layer(DefaultBodyLimit::max(CONTROL_BODY_LIMIT)),
    )
}

// =============================================================================
// Request handling
// =============================================================================

/// Middleware applying the header rules on every route (see module docs).
async fn reject_non_local(
    State(state): State<ControlState>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(reason) = header_violation(request.headers(), state.port) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": reason })),
        )
            .into_response();
    }
    next.run(request).await
}

/// Returns why a request must be rejected, or `None` if it may proceed:
/// any `Origin` header, any `Sec-Fetch-*` header, or a `Host` other than
/// the literal `127.0.0.1:<port>`.
fn header_violation(headers: &HeaderMap, port: u16) -> Option<&'static str> {
    if headers.contains_key("origin") {
        return Some("requests with an Origin header are not allowed");
    }
    if headers
        .keys()
        .any(|name| name.as_str().starts_with("sec-fetch-"))
    {
        return Some("requests with Sec-Fetch-* headers are not allowed");
    }
    let expected_host = format!("127.0.0.1:{port}");
    match headers.get("host").and_then(|v| v.to_str().ok()) {
        Some(host) if host == expected_host => None,
        _ => Some("Host header must be the literal loopback address"),
    }
}

async fn ping(State(state): State<ControlState>) -> Response {
    Json(serde_json::json!({
        "ok": true,
        "pid": std::process::id(),
        "generation": state.generation,
        "protocolVersion": PROTOCOL_VERSION,
    }))
    .into_response()
}

async fn invoke(
    Path(command): Path<String>,
    State(state): State<ControlState>,
    Json(args): Json<Value>,
) -> Response {
    if !ALLOWED_COMMANDS.contains(&command.as_str()) {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("command not available through stagedctl: {command}")
            })),
        )
            .into_response();
    }

    match crate::web_server::dispatch(
        &command,
        args,
        &state.app_handle,
        crate::web_server::DispatchCaller::Control,
    )
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(msg) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response(),
    }
}

// =============================================================================
// stagedctl sidecar + PATH shim
// =============================================================================

/// Locate the bundled `stagedctl` sidecar. The tauri CLI copies
/// `bundle.externalBin` entries next to the app executable — into
/// `target/{debug,release}` for dev runs and `Contents/MacOS/` in the bundle
/// — with the target-triple suffix stripped, so it always sits beside
/// `current_exe`.
fn locate_stagedctl_sidecar() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join("stagedctl");
    candidate.is_file().then_some(candidate)
}

/// Resolve the sidecar and build this instance's PATH shim dir. A missing
/// sidecar or failed shim is logged and degrades gracefully: sessions still
/// get `STAGEDCTL_LOCK`, just no `STAGEDCTL_BIN` / bare-name resolution.
fn setup_stagedctl_shim(dir: &std::path::Path, pid: u32) {
    let Some(bin) = locate_stagedctl_sidecar() else {
        log::warn!(
            "[control_server] stagedctl sidecar not found next to the app executable; \
             agent sessions get STAGEDCTL_LOCK only"
        );
        return;
    };
    match create_stagedctl_shim_dir(&stagedctl_shim_dir_path(dir, pid), &bin) {
        Ok(()) => {
            let _ = STAGEDCTL_SHIM_DIR.set(stagedctl_shim_dir_path(dir, pid));
        }
        Err(e) => log::warn!("[control_server] failed to create stagedctl PATH shim: {e}"),
    }
    let _ = STAGEDCTL_BIN.set(bin);
}

fn stagedctl_shim_dir_path(dir: &std::path::Path, pid: u32) -> PathBuf {
    dir.join(format!("control-bin-{pid}"))
}

/// Create (or refresh, after a same-pid restart of the control server) the
/// shim dir containing a `stagedctl` symlink to the bundled sidecar.
#[cfg(unix)]
fn create_stagedctl_shim_dir(
    shim_dir: &std::path::Path,
    bin: &std::path::Path,
) -> Result<(), String> {
    std::fs::create_dir_all(shim_dir).map_err(|e| format!("create {shim_dir:?}: {e}"))?;
    let link = shim_dir.join("stagedctl");
    match std::fs::remove_file(&link) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove existing {link:?}: {e}")),
    }
    std::os::unix::fs::symlink(bin, &link).map_err(|e| format!("symlink {link:?} -> {bin:?}: {e}"))
}

#[cfg(not(unix))]
fn create_stagedctl_shim_dir(
    _shim_dir: &std::path::Path,
    _bin: &std::path::Path,
) -> Result<(), String> {
    Err("stagedctl PATH shim is only supported on unix".to_string())
}

// =============================================================================
// Discovery file
// =============================================================================

fn discovery_file_path(dir: &std::path::Path, pid: u32) -> PathBuf {
    dir.join(format!("control-{pid}.json"))
}

/// Atomically write the discovery file: temp file in the same directory with
/// 0600 permissions, fsync, then rename — so a reader never sees partial
/// JSON and other users never see the port at all.
fn write_discovery_file(
    path: &std::path::Path,
    port: u16,
    pid: u32,
    generation: u64,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "port": port,
        "pid": pid,
        "generation": generation,
        "protocolVersion": PROTOCOL_VERSION,
    });
    let contents =
        serde_json::to_vec_pretty(&payload).map_err(|e| format!("serialize discovery: {e}"))?;

    let tmp = path.with_extension("json.tmp");
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|e| format!("create {tmp:?}: {e}"))?;
        file.write_all(&contents)
            .map_err(|e| format!("write {tmp:?}: {e}"))?;
        file.sync_all().map_err(|e| format!("fsync {tmp:?}: {e}"))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {tmp:?} -> {path:?}: {e}"))
}

/// Remove this instance's discovery file and PATH-shim dir. Called from the
/// app exit handler.
pub fn remove_instance_files() {
    if let Some(path) = DISCOVERY_FILE.get() {
        if let Err(e) = std::fs::remove_file(path) {
            log::warn!("[control_server] failed to remove discovery file {path:?}: {e}");
        }
    }
    if let Some(dir) = STAGEDCTL_SHIM_DIR.get() {
        if let Err(e) = std::fs::remove_dir_all(dir) {
            log::warn!("[control_server] failed to remove shim dir {dir:?}: {e}");
        }
    }
}

/// Delete `control-<pid>.json` files (plus orphaned temp files) and
/// `control-bin-<pid>` shim dirs left behind by instances that are no longer
/// running. `~/.staged` is shared by every Staged instance, so only entries
/// whose embedded pid is dead are touched.
fn sweep_stale_discovery_files(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(pid) = stale_sweep_candidate_pid(name) else {
            continue;
        };
        if pid == std::process::id() || pid_is_alive(pid) {
            continue;
        }
        let path = entry.path();
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(e) = result {
            log::warn!("[control_server] failed to sweep stale {name}: {e}");
        } else {
            log::info!("[control_server] swept stale control entry {name}");
        }
    }
}

/// Whether `pid` names a running process. The sweep runs on the app's setup
/// thread, so this is a `kill(pid, 0)` syscall — the probe `stagedctl` uses —
/// rather than `session_runner::is_process_alive`'s `kill -0` child process
/// per entry. `EPERM` still proves something runs under that pid; a value
/// that isn't a positive `pid_t` would address a process group, so it counts
/// as dead.
#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 performs only the existence and permission checks;
    // nothing is delivered.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn pid_is_alive(pid: u32) -> bool {
    crate::session_runner::is_process_alive(pid)
}

/// Parse the pid out of a `control-<pid>.json`, `control-<pid>.json.tmp`, or
/// `control-bin-<pid>` filename; `None` for anything else.
fn stale_sweep_candidate_pid(filename: &str) -> Option<u32> {
    let rest = filename.strip_prefix("control-")?;
    let pid = rest
        .strip_suffix(".json")
        .or_else(|| rest.strip_suffix(".json.tmp"))
        .or_else(|| rest.strip_prefix("bin-"))?;
    pid.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    // -------------------------------------------------------------------
    // Header rejection rules
    // -------------------------------------------------------------------

    fn loopback_headers(port: u16) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "host",
            HeaderValue::from_str(&format!("127.0.0.1:{port}")).unwrap(),
        );
        headers
    }

    #[test]
    fn accepts_plain_loopback_request() {
        assert_eq!(header_violation(&loopback_headers(4242), 4242), None);
    }

    #[test]
    fn rejects_any_origin_header() {
        for origin in ["http://evil.example", "http://127.0.0.1:4242", "null"] {
            let mut headers = loopback_headers(4242);
            headers.insert("origin", HeaderValue::from_str(origin).unwrap());
            assert!(
                header_violation(&headers, 4242).is_some(),
                "Origin {origin:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_any_sec_fetch_header() {
        for name in ["sec-fetch-mode", "sec-fetch-site", "sec-fetch-dest"] {
            let mut headers = loopback_headers(4242);
            headers.insert(name, HeaderValue::from_static("cors"));
            assert!(
                header_violation(&headers, 4242).is_some(),
                "{name} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_host_other_than_literal_loopback() {
        for host in [
            "localhost:4242", // DNS name — rebinding surface
            "127.0.0.1:9999", // wrong port
            "127.0.0.1",      // missing port
            "[::1]:4242",     // IPv6 loopback is not the literal
            "staged.example:4242",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("host", HeaderValue::from_str(host).unwrap());
            assert!(
                header_violation(&headers, 4242).is_some(),
                "Host {host:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_missing_host_header() {
        assert!(header_violation(&HeaderMap::new(), 4242).is_some());
    }

    // -------------------------------------------------------------------
    // Allowlist
    // -------------------------------------------------------------------

    /// The allowlist is the security boundary: pin it exactly so growing the
    /// CLI surface is always a deliberate, reviewed change here.
    #[test]
    fn allowlist_is_exactly_the_v1_command_table() {
        let expected = [
            "cancel_session",
            "create_image",
            "get_active_sessions",
            "get_branch_timeline",
            "get_project_timeline",
            "get_session",
            "list_branches_for_project",
            "list_project_notes",
            "list_project_repos",
            "list_projects",
            "start_branch_session",
            "start_project_session",
        ];
        let mut actual: Vec<&str> = ALLOWED_COMMANDS.to_vec();
        actual.sort_unstable();
        assert_eq!(actual, expected);
    }

    /// Every allowlisted verb must resolve to a real dispatch arm — a typo'd
    /// or removed command would otherwise surface only as a runtime
    /// "Unknown command" through the CLI.
    #[test]
    fn allowlist_commands_all_have_dispatch_arms() {
        let source = include_str!("web_server.rs");
        for command in ALLOWED_COMMANDS {
            assert!(
                source.contains(&format!("\"{command}\"")),
                "allowlisted command {command} has no dispatch arm in web_server.rs"
            );
        }
    }

    // -------------------------------------------------------------------
    // Body cap
    // -------------------------------------------------------------------

    /// Stands in for `invoke` with the same body extractor, reporting what
    /// reached it so the test can tell a parsed body from a refused one.
    async fn echo_len(Path(command): Path<String>, Json(args): Json<Value>) -> Json<Value> {
        let prompt_len = args["prompt"].as_str().map_or(0, str::len);
        Json(serde_json::json!({ "command": command, "promptLen": prompt_len }))
    }

    fn invoke_request(body: String) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/invoke/start_project_session")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(body))
            .unwrap()
    }

    /// The control surface never carries file bytes, so its cap is a fixed
    /// small number rather than axum's implicit default: a body at the cap
    /// reaches the handler, one byte over is refused before any extractor
    /// runs.
    #[tokio::test]
    async fn invoke_route_caps_the_body_well_below_the_web_upload_limit() {
        use tower::ServiceExt;
        let router: Router = invoke_router(echo_len);

        assert!(
            CONTROL_BODY_LIMIT <= 2 * 1024 * 1024,
            "the cap must not exceed axum's default"
        );

        let envelope = r#"{"prompt":""}"#.len();
        let body = format!(
            r#"{{"prompt":"{}"}}"#,
            "A".repeat(CONTROL_BODY_LIMIT - envelope)
        );
        assert_eq!(body.len(), CONTROL_BODY_LIMIT);
        let response = router.clone().oneshot(invoke_request(body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let reply: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(reply["command"], "start_project_session");
        assert_eq!(reply["promptLen"], CONTROL_BODY_LIMIT - envelope);

        let body = format!(
            r#"{{"prompt":"{}"}}"#,
            "A".repeat(CONTROL_BODY_LIMIT - envelope + 1)
        );
        assert_eq!(body.len(), CONTROL_BODY_LIMIT + 1);
        let response = router.oneshot(invoke_request(body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    // -------------------------------------------------------------------
    // Discovery file lifecycle
    // -------------------------------------------------------------------

    #[test]
    fn discovery_file_is_written_atomically_with_owner_only_perms() {
        let dir = tempfile::tempdir().unwrap();
        let path = discovery_file_path(dir.path(), 1234);

        write_discovery_file(&path, 4242, 1234, 7).unwrap();

        let contents: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(contents["port"], 4242);
        assert_eq!(contents["pid"], 1234);
        assert_eq!(contents["generation"], 7);
        assert_eq!(contents["protocolVersion"], PROTOCOL_VERSION);

        // No temp file left behind.
        assert!(!path.with_extension("json.tmp").exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "discovery file must be 0600");
        }
    }

    #[test]
    fn discovery_file_rewrite_replaces_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = discovery_file_path(dir.path(), 1234);
        write_discovery_file(&path, 4242, 1234, 1).unwrap();
        write_discovery_file(&path, 5353, 1234, 2).unwrap();

        let contents: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(contents["port"], 5353);
        assert_eq!(contents["generation"], 2);
    }

    #[test]
    fn sweep_removes_dead_pid_files_and_keeps_live_and_foreign_ones() {
        let dir = tempfile::tempdir().unwrap();

        // A pid that cannot exist (beyond macOS/Linux pid ranges) — dead.
        let dead = dir.path().join("control-999999999.json");
        std::fs::write(&dead, "{}").unwrap();
        let dead_tmp = dir.path().join("control-999999998.json.tmp");
        std::fs::write(&dead_tmp, "{").unwrap();
        let dead_shim = stagedctl_shim_dir_path(dir.path(), 999999997);
        std::fs::create_dir_all(&dead_shim).unwrap();
        std::fs::write(dead_shim.join("stagedctl"), "").unwrap();

        // Our own pid — alive.
        let alive = discovery_file_path(dir.path(), std::process::id());
        std::fs::write(&alive, "{}").unwrap();
        let alive_shim = stagedctl_shim_dir_path(dir.path(), std::process::id());
        std::fs::create_dir_all(&alive_shim).unwrap();

        // Non-discovery files are never touched.
        let foreign = dir.path().join("data.db");
        std::fs::write(&foreign, "not json").unwrap();

        sweep_stale_discovery_files(dir.path());

        assert!(!dead.exists(), "dead-pid discovery file should be swept");
        assert!(!dead_tmp.exists(), "orphaned temp file should be swept");
        assert!(!dead_shim.exists(), "dead-pid shim dir should be swept");
        assert!(alive.exists(), "live-pid discovery file must survive");
        assert!(alive_shim.exists(), "live-pid shim dir must survive");
        assert!(foreign.exists(), "unrelated files must survive");
    }

    #[test]
    #[cfg(unix)]
    fn pid_is_alive_probes_without_a_child_process() {
        assert!(pid_is_alive(std::process::id()));
        assert!(!pid_is_alive(999_999_999), "beyond any pid range");
        // Not positive `pid_t`s: kill() would address a process group.
        assert!(!pid_is_alive(0));
        assert!(!pid_is_alive(u32::MAX));
    }

    // -------------------------------------------------------------------
    // Startup readiness
    // -------------------------------------------------------------------

    /// The discovery file names a port that already accepts connections by
    /// the time the synchronous half returns — no runtime, no `serve` task —
    /// so a session started right after `start()` can reach the app.
    #[test]
    fn bind_and_write_discovery_is_ready_before_serving() {
        let dir = tempfile::tempdir().unwrap();
        let stale = discovery_file_path(dir.path(), 999_999_999);
        std::fs::write(&stale, "{}").unwrap();

        let prepared = bind_and_write_discovery(dir.path(), 1234, 3).unwrap();

        assert!(!stale.exists(), "the sweep runs before the bind");
        let contents: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(discovery_file_path(dir.path(), 1234)).unwrap(),
        )
        .unwrap();
        assert_eq!(contents["port"], prepared.port);
        assert_eq!(contents["generation"], 3);
        // The connection waits in the listen backlog until `serve` accepts.
        std::net::TcpStream::connect(("127.0.0.1", prepared.port))
            .expect("the bound listener should take connections before serve runs");
    }

    /// Startup recovery and the `KickSource::Startup` drain start local
    /// sessions with no window open, and an agent's env is fixed for its
    /// life: setup must make the control server discoverable before any of
    /// them, and `start` must do that work before it spawns anything.
    #[test]
    fn startup_prepares_the_control_server_before_recovery_and_the_queue_kick() {
        let lib = include_str!("lib.rs");
        let start_call = "control_server::start(";
        assert_eq!(
            lib.matches(start_call).count(),
            1,
            "lib.rs should start the control server exactly once"
        );
        let started_at = lib.find(start_call).unwrap();
        for later in [
            "session_runner::recover_dead_sessions(",
            "session_runner::reconcile::reconcile_all(",
            "launch_resume::detect_at_launch(",
            "session_queue::flush_follow_ups_for_project(",
            "session_queue::kick_all_with_queued_work(",
        ] {
            let at = lib
                .find(later)
                .unwrap_or_else(|| panic!("lib.rs no longer calls {later}; update this pin"));
            assert!(
                started_at < at,
                "lib.rs calls {later} before {start_call}; a session it starts would \
                 never get STAGEDCTL_* or the PATH shim"
            );
        }

        let source = include_str!("control_server.rs");
        let body_at = source.find("pub fn start(").unwrap();
        let body = &source[body_at..];
        let body = &body[..body.find("\n}\n").unwrap()];
        let prepare_at = body.find("prepare()").expect("start should call prepare()");
        let spawn_at = body.find("spawn(").expect("start should spawn serve");
        assert!(
            prepare_at < spawn_at,
            "start must prepare the discovery file and shim before spawning: {body}"
        );
    }

    #[test]
    fn stale_sweep_candidate_pid_parses_only_control_entry_names() {
        assert_eq!(stale_sweep_candidate_pid("control-123.json"), Some(123));
        assert_eq!(stale_sweep_candidate_pid("control-123.json.tmp"), Some(123));
        assert_eq!(stale_sweep_candidate_pid("control-bin-123"), Some(123));
        assert_eq!(stale_sweep_candidate_pid("control-abc.json"), None);
        assert_eq!(stale_sweep_candidate_pid("control-.json"), None);
        assert_eq!(stale_sweep_candidate_pid("control-bin-"), None);
        assert_eq!(stale_sweep_candidate_pid("preferences.json"), None);
        assert_eq!(stale_sweep_candidate_pid("data.db"), None);
    }

    // -------------------------------------------------------------------
    // stagedctl PATH shim
    // -------------------------------------------------------------------

    #[test]
    #[cfg(unix)]
    fn shim_dir_holds_a_stagedctl_symlink_to_the_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("stagedctl-sidecar");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();

        let shim_dir = stagedctl_shim_dir_path(dir.path(), 1234);
        create_stagedctl_shim_dir(&shim_dir, &bin).unwrap();

        let link = shim_dir.join("stagedctl");
        assert_eq!(std::fs::read_link(&link).unwrap(), bin);
    }

    #[test]
    #[cfg(unix)]
    fn shim_dir_refresh_replaces_an_existing_symlink() {
        // Pids are reused across app restarts of the same process image; a
        // stale symlink from an earlier start of this pid must be replaced,
        // not error the setup.
        let dir = tempfile::tempdir().unwrap();
        let old_bin = dir.path().join("old-sidecar");
        let new_bin = dir.path().join("new-sidecar");
        std::fs::write(&old_bin, "").unwrap();
        std::fs::write(&new_bin, "").unwrap();

        let shim_dir = stagedctl_shim_dir_path(dir.path(), 1234);
        create_stagedctl_shim_dir(&shim_dir, &old_bin).unwrap();
        create_stagedctl_shim_dir(&shim_dir, &new_bin).unwrap();

        let link = shim_dir.join("stagedctl");
        assert_eq!(std::fs::read_link(&link).unwrap(), new_bin);
    }
}
