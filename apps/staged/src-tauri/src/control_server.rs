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
//! **Discovery:** on listener start the server atomically writes
//! `~/.staged/control-<pid>.json` (0600) containing
//! `{port, pid, generation, protocolVersion}`. The pid suffix exists because
//! multiple Staged instances share `~/.staged`; stale files from dead pids
//! are swept on startup and our own file is removed on exit. Agent sessions
//! spawned by Staged receive the file's path in `STAGEDCTL_LOCK` so the CLI
//! finds its owning instance with zero configuration.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use axum::extract::{Path, Request, State};
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

/// Monotonic per-process start counter, mirrored into the discovery file and
/// the ping response so the CLI can detect a restart between file-read and
/// call.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Discovery-file path, set once the listener is up. Consulted by the env
/// injection in `session_runner` and the exit cleanup; `None` means the
/// server never started (bind failure) and sessions get no `STAGEDCTL_LOCK`.
static DISCOVERY_FILE: OnceLock<PathBuf> = OnceLock::new();

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

/// Start the control server in a background task. Called unconditionally at
/// app setup; a failure to bind or to write the discovery file is logged and
/// leaves the app fully functional without a control surface.
pub fn start(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run(app_handle).await {
            log::error!("[control_server] not started: {e}");
        }
    });
}

async fn run(app_handle: tauri::AppHandle) -> Result<(), String> {
    let dir = crate::paths::data_dir().ok_or("cannot determine ~/.staged")?;
    sweep_stale_discovery_files(&dir);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("failed to bind control listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("failed to get local address: {e}"))?
        .port();

    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let pid = std::process::id();
    let path = discovery_file_path(&dir, pid);
    write_discovery_file(&path, port, pid, generation)?;
    let _ = DISCOVERY_FILE.set(path.clone());
    log::info!("[control_server] listening on 127.0.0.1:{port}, discovery file {path:?}");

    let state = ControlState {
        app_handle,
        port,
        generation,
    };
    let router = Router::new()
        .route("/api/ping", get(ping))
        .route("/api/invoke/{command}", post(invoke))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            reject_non_local,
        ))
        .with_state(state);

    axum::serve(listener, router)
        .await
        .map_err(|e| format!("control server error: {e}"))
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

    match crate::web_server::dispatch(&command, args, &state.app_handle).await {
        Ok(value) => Json(value).into_response(),
        Err(msg) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response(),
    }
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

/// Remove this instance's discovery file. Called from the app exit handler.
pub fn remove_discovery_file() {
    if let Some(path) = DISCOVERY_FILE.get() {
        if let Err(e) = std::fs::remove_file(path) {
            log::warn!("[control_server] failed to remove discovery file {path:?}: {e}");
        }
    }
}

/// Delete `control-<pid>.json` files (and orphaned temp files) left behind by
/// instances that are no longer running. `~/.staged` is shared by every
/// Staged instance, so only files whose embedded pid is dead are touched.
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
        if pid == std::process::id() || crate::session_runner::is_process_alive(pid) {
            continue;
        }
        if let Err(e) = std::fs::remove_file(entry.path()) {
            log::warn!("[control_server] failed to sweep stale {name}: {e}");
        } else {
            log::info!("[control_server] swept stale discovery file {name}");
        }
    }
}

/// Parse the pid out of a `control-<pid>.json` or `control-<pid>.json.tmp`
/// filename; `None` for anything else.
fn stale_sweep_candidate_pid(filename: &str) -> Option<u32> {
    let rest = filename.strip_prefix("control-")?;
    let pid = rest
        .strip_suffix(".json")
        .or_else(|| rest.strip_suffix(".json.tmp"))?;
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

        // Our own pid — alive.
        let alive = discovery_file_path(dir.path(), std::process::id());
        std::fs::write(&alive, "{}").unwrap();

        // Non-discovery files are never touched.
        let foreign = dir.path().join("data.db");
        std::fs::write(&foreign, "not json").unwrap();

        sweep_stale_discovery_files(dir.path());

        assert!(!dead.exists(), "dead-pid discovery file should be swept");
        assert!(!dead_tmp.exists(), "orphaned temp file should be swept");
        assert!(alive.exists(), "live-pid discovery file must survive");
        assert!(foreign.exists(), "unrelated files must survive");
    }

    #[test]
    fn stale_sweep_candidate_pid_parses_only_discovery_filenames() {
        assert_eq!(stale_sweep_candidate_pid("control-123.json"), Some(123));
        assert_eq!(stale_sweep_candidate_pid("control-123.json.tmp"), Some(123));
        assert_eq!(stale_sweep_candidate_pid("control-abc.json"), None);
        assert_eq!(stale_sweep_candidate_pid("control-.json"), None);
        assert_eq!(stale_sweep_candidate_pid("preferences.json"), None);
        assert_eq!(stale_sweep_candidate_pid("data.db"), None);
    }
}
