//! Control server for `stagedctl`.
//!
//! An HTTP listener on a Unix domain socket that lets a same-user CLI drive
//! the running app through the existing `web_server::dispatch()` logic:
//!
//! - `GET  /api/ping`              — handshake: pid, generation, protocolVersion
//! - `POST /api/invoke/{command}`  — dispatch, restricted to [`ALLOWED_COMMANDS`]
//! - `POST /api/connect`, `GET /api/connect/{id}` — class B enrolment: ask
//!   the user, long-poll for the answer (see [`enrol`])
//! - `POST /api/reconnect/challenge`, `POST /api/reconnect`, `POST /api/disconnect`
//!   — an approved client's proof-of-secret reconnect and self-revoke
//!
//! **Transport and who can connect.** The socket lives in
//! `~/.staged/control/<pid>/`, a `0700` directory the app creates and refuses
//! to use if it is a symlink or owned by someone else (see [`files`]). Other
//! local accounts therefore cannot connect at all — this is what v1's
//! loopback TCP port could not promise, and why berd abandoned the same
//! design. Every connection's peer credentials are read (`LOCAL_PEERCRED` /
//! `SO_PEERCRED`, via tokio's `peer_cred`) and a peer whose uid is not the
//! app's is refused before any request is read; the peer pid is kept on the
//! request for consent prompts and audit text, never as the decision. A
//! browser cannot open a Unix socket, so the v1 header rules — any `Origin`,
//! any `Sec-Fetch-*`, a `Host` other than the literal [`HOST_LITERAL`] — are
//! now belt and braces, and stay.
//!
//! **TCP fallback.** Where a Unix socket is unavailable — a non-unix build,
//! or a socket path over the platform limit — the server binds
//! `127.0.0.1:0` instead and mints a 256-bit per-start token into the
//! owner-only discovery file. Every request must then carry it as
//! `Authorization: Bearer`, checked in middleware before the body is read.
//! Staged ships on macOS only, so this path exists for CI and for a home
//! directory with a very long name; the Unix socket is the design. The
//! process-ancestry gate below needs the peer pid only a socket reports, so
//! it exists only on the Unix-socket transport: over the TCP fallback the
//! enrolment routes keep the credential-keyed refusal alone.
//!
//! **Who may call, and with what reach.** Every `POST /api/invoke` must
//! carry `Authorization: Bearer <token>` naming a live [`ControlGrant`] in
//! the [`ControlGrants`] registry; the grant rides into
//! `web_server::dispatch` as `DispatchCaller::Control(grant)`, where a scope
//! table confines it to its project (and, for a read-only grant, to reads)
//! ahead of the arm table. Today's only grants are class A: a local session
//! start mints a read-write grant scoped to the session's own project,
//! delivered through an owner-only per-session file (see [`grants`]), and
//! revokes it when the run ends. A same-user caller with no grant — a user
//! shell, say — is refused `authorization_required` until it enrols: class
//! B, `stagedctl connect`, asks the user by name in a consent dialog and,
//! once approved, holds a durable thirty-day grant it reconnects to with a
//! proof of its secret ([`enrol`], [`consent`]). Both classes share the
//! registry and the scope table, but never cross: every enrolment route
//! refuses a caller traceable to a running session — by its bearer, or by
//! its process ancestry reaching a pid the session runner registered as a
//! live session's agent ([`ancestry`]) — and the invoke route refuses a
//! class B bearer presented from inside a session, so a spawned agent can
//! neither ask the user for a wider grant than the one it was spawned with
//! nor borrow one the user approved for a client outside any session. The
//! ancestry check is best effort (same-uid processes are not a hard
//! security boundary): it never refuses an unrelated user process, and the
//! agent-facing docs state the rule as a rule, not as a sandbox.
//!
//! **What a caller can do.** The fixed verb table: [`ALLOWED_COMMANDS`] maps
//! the `stagedctl` surface to specific dispatch commands — every one
//! UI-visible and read-only or reversible — and nothing else is reachable.
//! That includes the project-tag commands: assigning a project's tags, and
//! the reversible catalog edits (create, rename, recolour, move, reorder,
//! the icon and sidebar flags), each of which Settings › Tags undoes with
//! one more edit. No deletes — `delete_tag` and `delete_tag_group` drop
//! assignments on every project carrying the tag, which nothing in the UI
//! undoes — no history rewrites, no `create_pr` / `push_branch`. Exposing a
//! destructive verb reopens the auth design and must come with a
//! [`PROTOCOL_VERSION`] bump.
//!
//! One verb reaches past the data into the user's view: `open_project`
//! raises a window and asks it to show a project (see
//! [`project_open`](crate::project_open)). It is the user's own gesture from
//! a terminal, so only a connected app's grant may make it — a session's
//! credential is refused in the scope table, and an agent cannot pull the
//! window away from what the user is looking at.
//!
//! **Discovery.** Once the listener is bound — synchronously at app setup,
//! before anything can start a session (see [`start`]) — the server
//! atomically writes `~/.staged/control/<pid>/discovery.json` (0600)
//! containing `{socket | port+token, pid, generation, protocolVersion}`. The
//! per-pid directory exists because multiple Staged instances share
//! `~/.staged`; stale directories from dead pids are swept on startup and our
//! own is removed on exit. Agent sessions spawned by Staged receive, in
//! `STAGEDCTL_LOCK`, the path of their per-session file — the same shape
//! plus their `token` — so the CLI finds its owning instance and its
//! credential with zero configuration.
//!
//! **Sidecar reachability.** The `stagedctl` binary ships as a Tauri sidecar
//! next to the app executable. At startup this module resolves it, exposes
//! its path to sessions as `STAGEDCTL_BIN`, and builds a PATH shim dir
//! (`~/.staged/control/<pid>/bin/stagedctl` symlink) that `session_runner`
//! prepends to every local agent session's PATH so a bare `stagedctl` just
//! works. The shim lives in the instance directory and shares its lifecycle.

mod ancestry;
pub mod commands;
mod consent;
mod enrol;
mod files;
mod grants;

pub use consent::{ControlGrantSummary, AUTHORIZATION_REQUESTED_EVENT, GRANTS_CHANGED_EVENT};
pub use enrol::ControlAuthorizationRequest;
pub(crate) use files::ControlTransport;
pub use grants::ControlGrants;
pub(crate) use grants::{
    session_grant_applies, Access, Actor, AuthorizationFailure, ControlGrant, SessionGrantRequest,
    SessionLease,
};
// Named outside this module only by web_server's tests.
#[allow(unused_imports)]
pub(crate) use grants::Scope;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use axum::extract::{DefaultBodyLimit, Extension, Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::Value;
use subtle::ConstantTimeEq;

/// Version of the CLI ↔ app wire contract. Bump on any breaking reshape of
/// the discovery file, the endpoints, or the semantics of an allowlisted
/// command — the CLI refuses to talk to a mismatched app.
///
/// v2: Unix-socket transport under `~/.staged/control/<pid>/`, the literal
/// `Host`, and the bearer on the TCP fallback. A v1 CLI reads the mismatch
/// as "app updated — restart Staged".
pub const PROTOCOL_VERSION: u32 = 2;

/// Env var carrying the per-session credential file's path into spawned
/// agent sessions.
pub const STAGEDCTL_LOCK_ENV: &str = "STAGEDCTL_LOCK";

/// Env var carrying the absolute path of this instance's bundled `stagedctl`
/// sidecar into spawned agent sessions. The PATH shim dir makes the bare
/// `stagedctl` name resolve; this is the exact-path escape hatch (berd's
/// `BERDCTL_BIN` equivalent).
pub const STAGEDCTL_BIN_ENV: &str = "STAGEDCTL_BIN";

/// The `Host` every request must carry, verbatim. The CLI sets it on both
/// transports; a browser (or anything built on a URL) sends the address it
/// connected to instead, so the rule excludes them without naming a port.
pub const HOST_LITERAL: &str = "stagedctl";

/// Longest socket path the server binds: `sun_path` is 104 bytes on macOS
/// including the terminator (108 on Linux), so 103 is the portable bound.
/// `~/.staged/control/<pid>/sock` is ~45 bytes for a normal home directory;
/// a longer path falls back to loopback TCP rather than failing to start.
const MAX_SOCKET_PATH_LEN: usize = 103;

/// The dispatch commands backing the `stagedctl` command table — the only
/// verbs reachable through the control server. Every entry must be visible in
/// the UI and either read-only or reversible (berd's eligibility rule); a
/// destructive verb cannot be added here without reopening the auth design
/// (see module docs).
pub const ALLOWED_COMMANDS: &[&str] = &[
    // project list / project get
    "list_projects",
    "list_project_repos",
    "list_branches_for_project",
    // project open
    "open_project",
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
    // tag list / show, project tags / list --tag
    "get_tag_state",
    // project tag / untag
    "set_project_group_tag",
    "set_project_tag_assigned",
    // tag create / edit / move
    "create_tag",
    "update_tag",
    "move_tag",
    // tag group create / rename / set-icon / set-sidebar / reorder
    "create_tag_group",
    "rename_tag_group",
    "set_icon_tag_group",
    "set_tag_group_sidebar",
    "reorder_tag_groups",
];

/// Request body cap for `POST /api/invoke/{command}`. Control payloads are
/// prompts and ids — `create_image` takes a *path*, never file bytes — so
/// this sits far below `web_server`'s cap for base64 PDF uploads, and well
/// above any prompt an agent would reasonably send. axum's default (2 MiB)
/// would otherwise apply implicitly, and a cap that exists only by default
/// is one a refactor can lose.
const CONTROL_BODY_LIMIT: usize = 1024 * 1024;
// The cap must stay under axum's implicit default (2 MiB), or a refactor
// that drops the explicit layer would silently widen it.
const _: () = assert!(CONTROL_BODY_LIMIT <= 2 * 1024 * 1024);

/// Monotonic per-process start counter, mirrored into the discovery file and
/// the ping response so the CLI can detect a restart between file-read and
/// call.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// This instance's directory (`~/.staged/control/<pid>`), set once the
/// listener is up. Holds the socket, the discovery file, the per-session
/// files and the PATH shim; removed whole on exit.
static INSTANCE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Discovery-file path, set once the listener is up. `None` means the
/// server never started (bind failure): no session is granted a credential
/// and none gets `STAGEDCTL_LOCK`.
static DISCOVERY_FILE: OnceLock<PathBuf> = OnceLock::new();

/// The bound transport and this start's generation, set with the discovery
/// file. Per-session credential files repeat them so the CLI reads one file.
static ENDPOINT: OnceLock<(ControlTransport, u64)> = OnceLock::new();

/// Path of the bundled `stagedctl` sidecar, resolved at control-server start.
/// `None` means the sidecar wasn't found next to the app executable (e.g. a
/// bare `cargo run` outside the tauri CLI) and sessions get no
/// `STAGEDCTL_BIN` or PATH shim.
static STAGEDCTL_BIN: OnceLock<PathBuf> = OnceLock::new();

/// This instance's PATH shim dir (`~/.staged/control/<pid>/bin/`) holding a
/// `stagedctl` symlink to the sidecar, prepended to every local agent
/// session's PATH. Per instance for the same reason as the discovery file:
/// `~/.staged` is shared, and a dev instance and a production install must
/// never point each other's sessions at the wrong binary (or at a symlink
/// dangling into a cleaned dev target dir).
static STAGEDCTL_SHIM_DIR: OnceLock<PathBuf> = OnceLock::new();

#[derive(Clone)]
struct ControlState {
    app_handle: tauri::AppHandle,
    transport: Arc<ControlTransport>,
    generation: u64,
    grants: ControlGrants,
    /// The process table the session gate classifies peers against.
    ancestry: ancestry::Ancestry,
}

impl ControlState {
    /// The caller classifier the enrolment routes and `invoke` share.
    fn session_gate(&self) -> enrol::SessionGate {
        enrol::SessionGate {
            grants: self.grants.clone(),
            ancestry: self.ancestry.clone(),
        }
    }
}

/// The stagedctl env pairs for a spawned agent session holding `lease`:
/// `STAGEDCTL_LOCK` naming its per-session credential file, plus
/// `STAGEDCTL_BIN` when the sidecar resolved. A session with no lease gets
/// nothing — there is no credential to point it at, and the CLI would only
/// report `authorization_required`.
pub(crate) fn stagedctl_env(lease: Option<&SessionLease>) -> Vec<(String, String)> {
    let Some(lease) = lease else {
        return Vec::new();
    };
    let mut vars = vec![(
        STAGEDCTL_LOCK_ENV.to_string(),
        lease.file().display().to_string(),
    )];
    if let Some(bin) = STAGEDCTL_BIN.get() {
        vars.push((STAGEDCTL_BIN_ENV.to_string(), bin.display().to_string()));
    }
    vars
}

/// Mint a class A credential for a session this instance is starting: a
/// read-write grant confined to the session's project, written to
/// `~/.staged/control/<pid>/sessions/<session_id>-<epoch>.json` for
/// `STAGEDCTL_LOCK`.
/// `None` when the control server is not up or the grants registry is not
/// managed, and on a write failure (logged) — the session then simply has
/// no stagedctl.
pub(crate) fn mint_session_lease(
    app_handle: &tauri::AppHandle,
    request: SessionGrantRequest,
) -> Option<SessionLease> {
    use tauri::Manager;
    let instance_dir = INSTANCE_DIR.get()?;
    let (transport, generation) = ENDPOINT.get()?;
    let grants = app_handle.try_state::<ControlGrants>()?.inner().clone();
    let session_id = request.session_id.clone();
    match grants::mint_session_lease_at(
        &grants,
        &files::sessions_dir_path(instance_dir),
        transport,
        std::process::id(),
        *generation,
        request,
    ) {
        Ok(lease) => Some(lease),
        Err(e) => {
            log::warn!("[control_server] session {session_id} gets no stagedctl credential: {e}");
            None
        }
    }
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
    use tauri::Manager;
    // The registry the bearer middleware resolves against and session
    // starts mint into; `lib.rs` manages it ahead of this call (pinned).
    let Some(grants) = app_handle.try_state::<ControlGrants>() else {
        log::error!("[control_server] not started: ControlGrants is not managed");
        return;
    };
    let grants = grants.inner().clone();
    let prepared = match prepare() {
        Ok(prepared) => prepared,
        Err(e) => {
            log::error!("[control_server] not started: {e}");
            return;
        }
    };
    tauri::async_runtime::spawn(async move {
        if let Err(e) = serve(prepared, app_handle, grants).await {
            log::error!("[control_server] stopped: {e}");
            // Don't leave sessions and CLIs pointed at an endpoint nobody
            // answers.
            remove_instance_files();
        }
    });
}

/// A bound control listener whose discovery file is written. Built by
/// [`prepare`]; [`serve`] only has to accept on it.
struct PreparedListener {
    listener: BoundListener,
    transport: ControlTransport,
    generation: u64,
}

/// The listener [`prepare`] bound, still a std socket: `serve` hands it to
/// tokio once the runtime is running the accept loop.
enum BoundListener {
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixListener),
    Tcp(std::net::TcpListener),
}

/// The synchronous half of [`start`]: sweep, create the instance directory,
/// bind, write the discovery file, build the shim, and publish them to
/// [`stagedctl_env`] and [`stagedctl_session_ready`].
fn prepare() -> Result<PreparedListener, String> {
    let data_dir = crate::paths::data_dir().ok_or("cannot determine ~/.staged")?;
    let pid = std::process::id();
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let prepared = bind_and_write_discovery(&data_dir, pid, generation)?;
    let instance_dir = files::instance_dir_path(&files::control_root(&data_dir), pid);
    let _ = ENDPOINT.set((prepared.transport.clone(), generation));
    let _ = INSTANCE_DIR.set(instance_dir.clone());
    setup_stagedctl_shim(&instance_dir);
    // Last: `stagedctl_session_ready` keys off this, and a mint needs the
    // two statics above.
    let _ = DISCOVERY_FILE.set(files::discovery_file_path(&instance_dir));
    log::info!(
        "[control_server] listening on {}, instance dir {instance_dir:?}",
        prepared.transport.describe()
    );
    Ok(prepared)
}

/// The filesystem and socket work of [`prepare`], kept free of the
/// process-wide statics so tests can run it against a temp dir.
fn bind_and_write_discovery(
    data_dir: &std::path::Path,
    pid: u32,
    generation: u64,
) -> Result<PreparedListener, String> {
    std::fs::create_dir_all(data_dir).map_err(|e| format!("create {data_dir:?}: {e}"))?;
    let root = files::control_root(data_dir);
    files::ensure_private_dir(&root)?;
    files::sweep_legacy_v1_entries(data_dir);
    files::sweep_stale_instances(&root);

    let instance_dir = files::instance_dir_path(&root, pid);
    files::create_fresh_instance_dir(&instance_dir)?;
    let (listener, transport) = bind_listener(&instance_dir)?;
    files::write_discovery_file(
        &files::discovery_file_path(&instance_dir),
        &transport,
        pid,
        generation,
    )?;
    Ok(PreparedListener {
        listener,
        transport,
        generation,
    })
}

/// Bind the Unix socket inside the instance directory, or fall back to
/// loopback TCP with a per-start token where that is impossible.
fn bind_listener(
    instance_dir: &std::path::Path,
) -> Result<(BoundListener, ControlTransport), String> {
    #[cfg(unix)]
    {
        let socket = files::socket_path(instance_dir);
        if socket_path_fits(&socket) {
            match bind_unix(&socket) {
                Ok(listener) => {
                    return Ok((
                        BoundListener::Unix(listener),
                        ControlTransport::Unix { socket },
                    ))
                }
                Err(e) => log::warn!(
                    "[control_server] cannot bind {socket:?} ({e}); falling back to loopback TCP"
                ),
            }
        } else {
            log::warn!(
                "[control_server] socket path {socket:?} exceeds {MAX_SOCKET_PATH_LEN} bytes; \
                 falling back to loopback TCP"
            );
        }
    }
    #[cfg(not(unix))]
    let _ = instance_dir;
    bind_tcp()
}

fn socket_path_fits(socket: &std::path::Path) -> bool {
    socket.as_os_str().len() <= MAX_SOCKET_PATH_LEN
}

#[cfg(unix)]
fn bind_unix(socket: &std::path::Path) -> Result<std::os::unix::net::UnixListener, String> {
    use std::os::unix::fs::PermissionsExt;
    // `create_fresh_instance_dir` emptied the directory, so no stale socket
    // file can make the bind fail with EADDRINUSE.
    let listener = std::os::unix::net::UnixListener::bind(socket)
        .map_err(|e| format!("failed to bind control socket: {e}"))?;
    // `serve` hands the socket to tokio, which requires non-blocking mode.
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("failed to make control socket non-blocking: {e}"))?;
    // The 0700 directory already excludes other users; the socket's own mode
    // is belt and braces.
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod 0600 {socket:?}: {e}"))?;
    Ok(listener)
}

fn bind_tcp() -> Result<(BoundListener, ControlTransport), String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("failed to bind control listener: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("failed to make control listener non-blocking: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("failed to get local address: {e}"))?
        .port();
    Ok((
        BoundListener::Tcp(listener),
        ControlTransport::Tcp {
            port,
            token: crate::web_server::generate_token(),
        },
    ))
}

/// The async half of [`start`]: accept and route requests until the app exits.
async fn serve(
    prepared: PreparedListener,
    app_handle: tauri::AppHandle,
    grants: ControlGrants,
) -> Result<(), String> {
    let PreparedListener {
        listener,
        transport,
        generation,
    } = prepared;
    let state = ControlState {
        app_handle,
        transport: Arc::new(transport),
        generation,
        grants: grants.clone(),
        ancestry: ancestry::Ancestry::system(),
    };
    let router = Router::new()
        .route("/api/ping", get(ping))
        // Enrolment and reconnect need no grant — they are how one is
        // obtained — but sit behind the transport gate and header rules
        // like everything else, with the same body cap, and refuse any
        // caller traceable to a running session outright.
        .merge(enrolment_router(state.session_gate()))
        // The grant is resolved on the invoke route only (ping is the
        // unauthenticated handshake), before the body is read.
        .merge(
            invoke_router(invoke)
                .route_layer(middleware::from_fn_with_state(grants, require_grant)),
        )
        .layer(middleware::from_fn(reject_browser_shaped_requests))
        .with_state(state.clone());

    match listener {
        #[cfg(unix)]
        BoundListener::Unix(listener) => {
            let listener = tokio::net::UnixListener::from_std(listener)
                .map_err(|e| format!("failed to adopt control socket: {e}"))?;
            // Outermost, so a foreign peer is refused before any header or
            // body is looked at.
            let router = router.layer(middleware::from_fn(reject_foreign_peer));
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<UdsPeer>(),
            )
            .await
            .map_err(|e| format!("control server error: {e}"))
        }
        BoundListener::Tcp(listener) => {
            let listener = tokio::net::TcpListener::from_std(listener)
                .map_err(|e| format!("failed to adopt control listener: {e}"))?;
            // Outermost: the per-start token is the TCP fallback's stand-in
            // for the peer-uid check, so it is verified before the body is
            // read.
            let router = router.layer(middleware::from_fn_with_state(
                state,
                require_instance_token,
            ));
            axum::serve(listener, router)
                .await
                .map_err(|e| format!("control server error: {e}"))
        }
    }
}

/// The class B routes (see [`enrol`]), each under [`CONTROL_BODY_LIMIT`]
/// and behind [`enrol::reject_spawned_session`]: a caller inside a running
/// session — by credential or by process — gets nothing here.
fn enrolment_router(gate: enrol::SessionGate) -> Router<ControlState> {
    Router::new()
        .route("/api/connect", post(enrol::connect))
        .route("/api/connect/{request_id}", get(enrol::poll_connect))
        .route("/api/reconnect/challenge", post(enrol::reconnect_challenge))
        .route("/api/reconnect", post(enrol::reconnect))
        .route("/api/disconnect", post(enrol::disconnect))
        .route_layer(middleware::from_fn_with_state(
            gate,
            enrol::reject_spawned_session,
        ))
        .layer(DefaultBodyLimit::max(CONTROL_BODY_LIMIT))
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
// Peer credentials (Unix socket)
// =============================================================================

/// The connecting process, as the kernel reports it at `accept` time. The
/// peer is the `stagedctl` process itself — not the agent or terminal that
/// ran it — so the pid is informational (consent prompts, audit text) and
/// only the uid gates anything. `None` fields mean the lookup failed, which
/// the uid gate treats as a foreign peer.
#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UdsPeer {
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub pid: Option<i32>,
}

#[cfg(unix)]
impl
    axum::extract::connect_info::Connected<
        axum::serve::IncomingStream<'_, tokio::net::UnixListener>,
    > for UdsPeer
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, tokio::net::UnixListener>) -> Self {
        match stream.io().peer_cred() {
            Ok(cred) => UdsPeer {
                uid: Some(cred.uid()),
                gid: Some(cred.gid()),
                pid: cred.pid(),
            },
            Err(e) => {
                log::warn!("[control_server] cannot read peer credentials: {e}");
                UdsPeer {
                    uid: None,
                    gid: None,
                    pid: None,
                }
            }
        }
    }
}

/// Why a Unix-socket peer may not proceed, or `None` if it may: anything but
/// the app's own effective uid. Unreadable credentials fail closed.
#[cfg(unix)]
fn peer_violation(peer: &UdsPeer) -> Option<&'static str> {
    match peer.uid {
        Some(uid) if uid == files::current_uid() => None,
        Some(_) => Some("the connecting process belongs to another user"),
        None => Some("the connecting process's credentials could not be read"),
    }
}

/// Middleware refusing connections from other users on the Unix socket.
/// An admitted peer is attached to the request for the consent prompt's
/// "asked by process N" line.
#[cfg(unix)]
async fn reject_foreign_peer(
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<UdsPeer>,
    mut request: Request,
    next: Next,
) -> Response {
    if let Some(reason) = peer_violation(&peer) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": reason })),
        )
            .into_response();
    }
    request.extensions_mut().insert(peer);
    next.run(request).await
}

// =============================================================================
// Per-start token (TCP fallback)
// =============================================================================

/// The `Authorization: Bearer` value on a request, if any.
pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Why a TCP-fallback request may not proceed, or `None` if it carries this
/// start's instance token or a live grant token. A grant token is as secret
/// and as much this instance's as the per-start one, and accepting it here
/// lets a session's per-session file name a single bearer on either
/// transport.
fn instance_token_violation(
    headers: &HeaderMap,
    expected: &str,
    grants: &ControlGrants,
) -> Option<&'static str> {
    match bearer_token(headers) {
        Some(token) if constant_time_eq(token, expected) => None,
        Some(token) if grants.authorize(token).is_ok() => None,
        Some(_) => Some("the bearer token is not this Staged instance's"),
        None => Some("the loopback TCP fallback requires Authorization: Bearer <token>"),
    }
}

/// Middleware applying [`instance_token_violation`] when the transport is
/// TCP. On the Unix socket the peer-uid check is the gate and this passes
/// everything through.
async fn require_instance_token(
    State(state): State<ControlState>,
    request: Request,
    next: Next,
) -> Response {
    if let ControlTransport::Tcp { token, .. } = &*state.transport {
        if let Some(reason) = instance_token_violation(request.headers(), token, &state.grants) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": reason })),
            )
                .into_response();
        }
    }
    next.run(request).await
}

// =============================================================================
// Grants
// =============================================================================

/// Middleware on the invoke route: resolve the bearer to a live
/// [`ControlGrant`] and attach it to the request, or refuse with the
/// failure's code. Runs ahead of the `Json` extractor, so an unauthorized
/// body is never read.
async fn require_grant(
    State(grants): State<ControlGrants>,
    mut request: Request,
    next: Next,
) -> Response {
    let resolved = match bearer_token(request.headers()) {
        None => Err(AuthorizationFailure::Missing),
        Some(token) => grants.authorize(token),
    };
    match resolved {
        Ok(grant) => {
            request.extensions_mut().insert(grant);
            next.run(request).await
        }
        Err(failure) => {
            authorization_response(StatusCode::UNAUTHORIZED, failure.code(), failure.message())
        }
    }
}

/// `{ "error", "code" }` for an authorization refusal. The `code` is what
/// the CLI maps to exit 4 (report, don't retry); plain command errors carry
/// no code.
fn authorization_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": message, "code": code })),
    )
        .into_response()
}

/// How a `dispatch` error answers: a scope refusal (see
/// `web_server::SCOPE_DENIED_PREFIX`) as `403 scope_denied`, anything else
/// as a plain `400` command error.
fn dispatch_error_response(message: &str) -> Response {
    match message.strip_prefix(crate::web_server::SCOPE_DENIED_PREFIX) {
        Some(denied) => authorization_response(StatusCode::FORBIDDEN, "scope_denied", denied),
        None => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": message })),
        )
            .into_response(),
    }
}

// =============================================================================
// Request handling
// =============================================================================

/// Middleware applying the header rules on every route (see module docs).
async fn reject_browser_shaped_requests(request: Request, next: Next) -> Response {
    if let Some(reason) = header_violation(request.headers()) {
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
/// the literal [`HOST_LITERAL`].
fn header_violation(headers: &HeaderMap) -> Option<&'static str> {
    if headers.contains_key("origin") {
        return Some("requests with an Origin header are not allowed");
    }
    if headers
        .keys()
        .any(|name| name.as_str().starts_with("sec-fetch-"))
    {
        return Some("requests with Sec-Fetch-* headers are not allowed");
    }
    match headers.get("host").and_then(|v| v.to_str().ok()) {
        Some(host) if host == HOST_LITERAL => None,
        _ => Some("Host header must be the literal `stagedctl`"),
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
    Extension(grant): Extension<ControlGrant>,
    parts: axum::http::request::Parts,
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

    // A connected app's bearer presented from inside a running session's
    // process tree is the session trying to widen itself; refuse it before
    // the scope table ever sees the grant.
    #[cfg(unix)]
    let peer_pid = parts.extensions.get::<UdsPeer>().and_then(|peer| peer.pid);
    #[cfg(not(unix))]
    let peer_pid: Option<i32> = {
        let _ = &parts;
        None
    };
    if let Some(response) =
        enrol::refuse_app_bearer_from_session(&state.session_gate(), peer_pid, &grant)
    {
        return response;
    }

    // A class B grant confined to a project that has since been deleted
    // lost its row to the cascade; drop its bearers so memory and store
    // agree, instead of letting the scope table refuse it piecemeal.
    if let Some(response) = enrol::refuse_orphaned_app_grant(&state, &grant).await {
        return response;
    }
    enrol::note_app_bearer_use(&state, &grant);
    match crate::web_server::dispatch(
        &command,
        args,
        &state.app_handle,
        crate::web_server::DispatchCaller::Control(grant),
    )
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(msg) => dispatch_error_response(&msg),
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
fn setup_stagedctl_shim(instance_dir: &std::path::Path) {
    let Some(bin) = locate_stagedctl_sidecar() else {
        log::warn!(
            "[control_server] stagedctl sidecar not found next to the app executable; \
             agent sessions get STAGEDCTL_LOCK only"
        );
        return;
    };
    let shim_dir = files::shim_dir_path(instance_dir);
    match files::create_shim_dir(&shim_dir, &bin) {
        Ok(()) => {
            let _ = STAGEDCTL_SHIM_DIR.set(shim_dir);
        }
        Err(e) => log::warn!("[control_server] failed to create stagedctl PATH shim: {e}"),
    }
    let _ = STAGEDCTL_BIN.set(bin);
}

// =============================================================================
// Exit
// =============================================================================

/// Remove this instance's directory — socket, discovery file, per-session
/// files and PATH shim. Called from the app exit handler.
pub fn remove_instance_files() {
    if let Some(dir) = INSTANCE_DIR.get() {
        if let Err(e) = std::fs::remove_dir_all(dir) {
            log::warn!("[control_server] failed to remove instance dir {dir:?}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::files::tests::private_tempdir;
    use super::*;
    use axum::http::HeaderValue;

    // -------------------------------------------------------------------
    // Header rejection rules
    // -------------------------------------------------------------------

    fn literal_host_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static(HOST_LITERAL));
        headers
    }

    #[test]
    fn accepts_a_request_with_the_literal_host() {
        assert_eq!(header_violation(&literal_host_headers()), None);
    }

    #[test]
    fn rejects_any_origin_header() {
        for origin in ["http://evil.example", "http://stagedctl", "null"] {
            let mut headers = literal_host_headers();
            headers.insert("origin", HeaderValue::from_str(origin).unwrap());
            assert!(
                header_violation(&headers).is_some(),
                "Origin {origin:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_any_sec_fetch_header() {
        for name in ["sec-fetch-mode", "sec-fetch-site", "sec-fetch-dest"] {
            let mut headers = literal_host_headers();
            headers.insert(name, HeaderValue::from_static("cors"));
            assert!(
                header_violation(&headers).is_some(),
                "{name} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_host_other_than_the_literal() {
        for host in [
            "localhost:4242", // DNS name — rebinding surface
            "127.0.0.1:4242", // what a browser sends to the TCP fallback
            "127.0.0.1",      // v1's literal, minus the port
            "[::1]:4242",     // IPv6 loopback
            "stagedctl:80",   // the literal with a port is not the literal
            "Stagedctl",      // case matters: the CLI sends it verbatim
            "staged.example",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("host", HeaderValue::from_str(host).unwrap());
            assert!(
                header_violation(&headers).is_some(),
                "Host {host:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_missing_host_header() {
        assert!(header_violation(&HeaderMap::new()).is_some());
    }

    // -------------------------------------------------------------------
    // Transport gates
    // -------------------------------------------------------------------

    #[test]
    #[cfg(unix)]
    fn peer_gate_admits_only_the_apps_own_uid() {
        let ours = files::current_uid();
        let same = UdsPeer {
            uid: Some(ours),
            gid: Some(0),
            pid: Some(1),
        };
        assert_eq!(peer_violation(&same), None);
        let other = UdsPeer {
            uid: Some(ours.wrapping_add(1)),
            gid: Some(0),
            pid: Some(1),
        };
        assert!(
            peer_violation(&other).is_some(),
            "another uid must be refused"
        );
        let unknown = UdsPeer {
            uid: None,
            gid: None,
            pid: None,
        };
        assert!(
            peer_violation(&unknown).is_some(),
            "unreadable credentials must fail closed"
        );
    }

    /// `serve` stacks [`reject_foreign_peer`] outermost around every route,
    /// the enrolment ones included: a `stagedctl connect` from another user
    /// is refused before the handler (and its consent prompt) runs, and an
    /// admitted peer is attached to the request so the prompt can name the
    /// asking process.
    #[tokio::test]
    #[cfg(unix)]
    async fn enrolment_routes_refuse_a_foreign_uid_before_the_handler_runs() {
        use axum::extract::ConnectInfo;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tower::ServiceExt;

        static HANDLED: AtomicUsize = AtomicUsize::new(0);
        async fn stand_in(axum::Extension(peer): axum::Extension<UdsPeer>) -> Json<Value> {
            HANDLED.fetch_add(1, Ordering::SeqCst);
            Json(serde_json::json!({ "requestId": "r", "peerPid": peer.pid }))
        }
        let router = Router::new()
            .route("/api/connect", post(stand_in))
            .layer(middleware::from_fn(reject_foreign_peer));
        let connect = |peer: UdsPeer| {
            let mut request = axum::http::Request::builder()
                .method("POST")
                .uri("/api/connect")
                .header("Content-Type", "application/json")
                .body(axum::body::Body::from(r#"{"client":"x","readOnly":false}"#))
                .unwrap();
            request.extensions_mut().insert(ConnectInfo(peer));
            request
        };
        let ours = files::current_uid();

        let response = router
            .clone()
            .oneshot(connect(UdsPeer {
                uid: Some(ours.wrapping_add(1)),
                gid: Some(0),
                pid: Some(4242),
            }))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            json_body(response).await["error"],
            "the connecting process belongs to another user"
        );
        assert_eq!(
            HANDLED.load(Ordering::SeqCst),
            0,
            "refused before the handler"
        );

        let response = router
            .oneshot(connect(UdsPeer {
                uid: Some(ours),
                gid: Some(0),
                pid: Some(4242),
            }))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json_body(response).await["peerPid"], 4242);
        assert_eq!(HANDLED.load(Ordering::SeqCst), 1);
    }

    /// A gate whose process table is `parents` (pid → parent pid), built
    /// from [`ancestry::fake_pid`] values so no entry can be the test
    /// binary's own pid.
    fn fake_gate(
        grants: &ControlGrants,
        parents: std::collections::HashMap<i32, i32>,
    ) -> enrol::SessionGate {
        enrol::SessionGate {
            grants: grants.clone(),
            ancestry: ancestry::Ancestry::fake(parents),
        }
    }

    /// The enrolment routes refuse a session's bearer before the handler
    /// runs: nothing is parked, no prompt is shown. A caller with no bearer,
    /// an unknown one, or an approved app's bearer reaches the handler.
    #[tokio::test]
    async fn enrolment_routes_refuse_a_session_bearer_before_the_handler_runs() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tower::ServiceExt;

        static HANDLED: AtomicUsize = AtomicUsize::new(0);
        async fn stand_in(State(grants): State<ControlGrants>) -> Json<Value> {
            HANDLED.fetch_add(1, Ordering::SeqCst);
            // What `connect` would do: park a request.
            let pending = grants
                .enrolment()
                .request(
                    enrol::ConnectRequest {
                        client: "Claude Code".to_string(),
                        scope: Scope::AllProjects,
                        project_name: None,
                        access: grants::Access::ReadWrite,
                        peer_pid: None,
                        peer_parent: None,
                        peer_orphaned: false,
                    },
                    crate::store::now_timestamp(),
                )
                .unwrap();
            Json(serde_json::json!({ "requestId": pending.id }))
        }
        let grants = ControlGrants::default();
        let router = Router::new()
            .route("/api/connect", post(stand_in))
            .route("/api/reconnect/challenge", post(stand_in))
            .route_layer(middleware::from_fn_with_state(
                fake_gate(&grants, Default::default()),
                enrol::reject_spawned_session,
            ))
            .with_state(grants.clone());
        let request = |path: &str, bearer: Option<&str>| {
            let mut builder = axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("Content-Type", "application/json");
            if let Some(bearer) = bearer {
                builder = builder.header("Authorization", format!("Bearer {bearer}"));
            }
            builder
                .body(axum::body::Body::from(r#"{"client":"Claude Code"}"#))
                .unwrap()
        };

        let session = grants.mint(ControlGrant {
            actor: grants::Actor::Session {
                session_id: "s-1".to_string(),
                epoch: 1,
            },
            scope: Scope::Project("p".to_string()),
            branch_id: None,
            access: grants::Access::ReadWrite,
        });
        for path in ["/api/connect", "/api/reconnect/challenge"] {
            let response = router
                .clone()
                .oneshot(request(path, Some(&session)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
            let body = json_body(response).await;
            assert_eq!(body["code"], "scope_denied");
            assert!(
                body["error"]
                    .as_str()
                    .unwrap()
                    .contains("spawned sessions cannot request connected-app access"),
                "{body}"
            );
            assert!(
                body["error"].as_str().unwrap().contains("session s-1"),
                "{body}"
            );
        }
        assert_eq!(
            HANDLED.load(Ordering::SeqCst),
            0,
            "refused before the handler"
        );
        assert!(
            grants
                .enrolment()
                .pending(crate::store::now_timestamp())
                .is_empty(),
            "no request was parked, so no prompt could be shown"
        );

        // No bearer, an unknown bearer, and an approved app's bearer all
        // reach the handler.
        let app = grants.mint_app_bearer(
            "g-1",
            "approved",
            Scope::AllProjects,
            grants::Access::ReadWrite,
            i64::MAX,
        );
        for bearer in [None, Some("0".repeat(64)), Some(app)] {
            let response = router
                .clone()
                .oneshot(request("/api/connect", bearer.as_deref()))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{bearer:?}");
        }
        assert_eq!(HANDLED.load(Ordering::SeqCst), 3);
    }

    /// The gate is process-keyed as well as credential-keyed: a `connect`
    /// with no bearer at all (the agent unset or emptied `STAGEDCTL_LOCK`)
    /// from a process whose ancestry reaches a live session's agent is
    /// refused before the handler, with nothing parked and no prompt. The
    /// same request from a process outside every session's tree, or whose
    /// ancestry cannot be read, reaches the handler.
    #[tokio::test]
    #[cfg(unix)]
    async fn enrolment_routes_refuse_a_peer_inside_a_running_session_without_a_credential() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tower::ServiceExt;

        static HANDLED: AtomicUsize = AtomicUsize::new(0);
        async fn stand_in(State(grants): State<ControlGrants>) -> Json<Value> {
            HANDLED.fetch_add(1, Ordering::SeqCst);
            let pending = grants
                .enrolment()
                .request(
                    enrol::ConnectRequest {
                        client: "Claude Code".to_string(),
                        scope: Scope::AllProjects,
                        project_name: None,
                        access: grants::Access::ReadWrite,
                        peer_pid: None,
                        peer_parent: None,
                        peer_orphaned: false,
                    },
                    crate::store::now_timestamp(),
                )
                .unwrap();
            Json(serde_json::json!({ "requestId": pending.id }))
        }
        let grants = ControlGrants::default();
        let fp = ancestry::fake_pid;
        // Session s-1's agent is pid 500; stagedctl 503 ← sh 502 ← tool 501
        // ← agent 500. Pid 900 is the user's own shell under launchd.
        grants.register_agent_process(
            fp(500) as u32,
            grants::SpawnedSession {
                session_id: "s-1".to_string(),
                epoch: 1,
                project_id: "p".to_string(),
            },
        );
        let parents = std::collections::HashMap::from([
            (fp(503), fp(502)),
            (fp(502), fp(501)),
            (fp(501), fp(500)),
            (fp(500), std::process::id() as i32),
            (fp(900), fp(800)),
            (fp(800), 1),
        ]);
        let router = Router::new()
            .route("/api/connect", post(stand_in))
            .route("/api/reconnect/challenge", post(stand_in))
            .route("/api/reconnect", post(stand_in))
            .route("/api/disconnect", post(stand_in))
            .route_layer(middleware::from_fn_with_state(
                fake_gate(&grants, parents),
                enrol::reject_spawned_session,
            ))
            .with_state(grants.clone());
        let ours = files::current_uid();
        let request = |path: &str, pid: Option<i32>| {
            let mut request = axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("Content-Type", "application/json")
                .body(axum::body::Body::from(r#"{"client":"Claude Code"}"#))
                .unwrap();
            // What `reject_foreign_peer` attaches for an admitted peer.
            request.extensions_mut().insert(UdsPeer {
                uid: Some(ours),
                gid: Some(0),
                pid,
            });
            request
        };

        for path in [
            "/api/connect",
            "/api/reconnect/challenge",
            "/api/reconnect",
            "/api/disconnect",
        ] {
            let response = router
                .clone()
                .oneshot(request(path, Some(fp(503))))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
            let body = json_body(response).await;
            assert_eq!(body["code"], "scope_denied", "{path}");
            let error = body["error"].as_str().unwrap();
            assert!(
                error.starts_with("spawned sessions cannot request connected-app access"),
                "{error}"
            );
            assert!(error.contains("session s-1"), "{error}");
        }
        assert_eq!(
            HANDLED.load(Ordering::SeqCst),
            0,
            "refused before the handler"
        );
        assert!(
            grants
                .enrolment()
                .pending(crate::store::now_timestamp())
                .is_empty(),
            "no request was parked, so no prompt could be shown"
        );

        // Unrelated ancestry, an unreadable one (pid absent from the table)
        // and no pid at all pass through.
        for (pid, what) in [
            (Some(fp(900)), "the user's own shell"),
            (Some(fp(31337)), "a pid whose parent cannot be read"),
            (None, "no peer pid reported"),
        ] {
            let response = router
                .clone()
                .oneshot(request("/api/connect", pid))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{what}");
        }
        assert_eq!(HANDLED.load(Ordering::SeqCst), 3);
    }

    /// On the invoke route the same classification refuses a connected
    /// app's bearer presented from inside a session's process tree, so an
    /// agent that drops its own credential cannot borrow the user's grant;
    /// the session's own bearer, and the app bearer from outside any
    /// session, pass.
    #[test]
    fn a_connected_app_bearer_from_inside_a_session_is_refused_on_invoke() {
        let grants = ControlGrants::default();
        let fp = ancestry::fake_pid;
        grants.register_agent_process(
            fp(500) as u32,
            grants::SpawnedSession {
                session_id: "s-1".to_string(),
                epoch: 1,
                project_id: "p".to_string(),
            },
        );
        let gate = fake_gate(
            &grants,
            std::collections::HashMap::from([(fp(503), fp(502)), (fp(502), fp(500)), (fp(900), 1)]),
        );
        let app_token = grants.mint_app_bearer(
            "g-1",
            "Claude Code",
            Scope::AllProjects,
            grants::Access::ReadWrite,
            i64::MAX,
        );
        let app_grant = grants.authorize(&app_token).unwrap();
        let session_grant = ControlGrant {
            actor: Actor::Session {
                session_id: "s-1".to_string(),
                epoch: 1,
            },
            scope: Scope::Project("p".to_string()),
            branch_id: None,
            access: Access::ReadWrite,
        };

        let refused = enrol::refuse_app_bearer_from_session(&gate, Some(fp(503)), &app_grant)
            .expect("a class B bearer from inside session s-1 is refused");
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let body = tauri::async_runtime::block_on(json_body(refused));
        assert_eq!(body["code"], "scope_denied");
        let error = body["error"].as_str().unwrap();
        assert!(error.contains("session s-1"), "{error}");
        assert!(error.contains("\"Claude Code\""), "{error}");
        assert!(
            grants.authorize(&app_token).is_ok(),
            "the user's grant itself stays live; only this caller is refused"
        );

        assert!(
            enrol::refuse_app_bearer_from_session(&gate, Some(fp(503)), &session_grant).is_none(),
            "the session's own credential is what it should use"
        );
        assert!(
            enrol::refuse_app_bearer_from_session(&gate, Some(fp(900)), &app_grant).is_none(),
            "the connected app from the user's shell"
        );
        assert!(
            enrol::refuse_app_bearer_from_session(&gate, None, &app_grant).is_none(),
            "no peer pid: fail open"
        );
    }

    #[test]
    fn instance_token_gate_requires_this_starts_bearer_or_a_live_grant() {
        let grants = ControlGrants::default();
        let token = "ab".repeat(32);
        let mut headers = HeaderMap::new();
        assert!(
            instance_token_violation(&headers, &token, &grants).is_some(),
            "missing bearer must be refused"
        );
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", "cd".repeat(32))).unwrap(),
        );
        assert!(
            instance_token_violation(&headers, &token, &grants).is_some(),
            "another token must be refused"
        );
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        assert_eq!(instance_token_violation(&headers, &token, &grants), None);
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Basic {token}")).unwrap(),
        );
        assert!(
            instance_token_violation(&headers, &token, &grants).is_some(),
            "only the Bearer scheme is a bearer"
        );

        // A session's grant token passes the transport gate too, so the
        // per-session file names one bearer on either transport.
        let grant_token = grants.mint(ControlGrant::test_all_projects());
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {grant_token}")).unwrap(),
        );
        assert_eq!(instance_token_violation(&headers, &token, &grants), None);
        grants.revoke(&grant_token);
        assert!(
            instance_token_violation(&headers, &token, &grants).is_some(),
            "a revoked grant token no longer passes"
        );
    }

    // -------------------------------------------------------------------
    // Grants on the invoke route
    // -------------------------------------------------------------------

    /// Stands in for `invoke`: reports the grant the middleware attached.
    async fn echo_grant(Extension(grant): Extension<ControlGrant>) -> Json<Value> {
        Json(serde_json::json!({ "scope": grant.project_scope() }))
    }

    fn grant_router(grants: &ControlGrants) -> Router {
        invoke_router(echo_grant).route_layer(middleware::from_fn_with_state(
            grants.clone(),
            require_grant,
        ))
    }

    fn invoke_request_with_bearer(bearer: Option<&str>) -> axum::http::Request<axum::body::Body> {
        let mut builder = axum::http::Request::builder()
            .method("POST")
            .uri("/api/invoke/list_projects")
            .header("Content-Type", "application/json");
        if let Some(bearer) = bearer {
            builder = builder.header("Authorization", format!("Bearer {bearer}"));
        }
        builder.body(axum::body::Body::from("{}")).unwrap()
    }

    async fn json_body(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// The invoke route resolves the bearer to a grant before the handler
    /// (and its body extractor) runs; each failure carries the code the CLI
    /// maps to exit 4.
    #[tokio::test]
    async fn invoke_route_requires_a_live_grant_and_codes_each_refusal() {
        use tower::ServiceExt;
        let grants = ControlGrants::default();
        let router = grant_router(&grants);

        let response = router
            .clone()
            .oneshot(invoke_request_with_bearer(None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(response).await["code"],
            "authorization_required",
            "no bearer"
        );

        let response = router
            .clone()
            .oneshot(invoke_request_with_bearer(Some(&"0".repeat(64))))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(response).await["code"],
            "authorization_required",
            "a bearer this instance never minted"
        );

        let token = grants.mint(ControlGrant {
            actor: Actor::Session {
                session_id: "s1".to_string(),
                epoch: 1,
            },
            scope: Scope::Project("p1".to_string()),
            branch_id: None,
            access: Access::ReadWrite,
        });
        let response = router
            .clone()
            .oneshot(invoke_request_with_bearer(Some(&token)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_body(response).await["scope"],
            "p1",
            "the handler sees the resolved grant"
        );

        grants.revoke(&token);
        let response = router
            .oneshot(invoke_request_with_bearer(Some(&token)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(response).await["code"],
            "authorization_revoked",
            "a revoked bearer"
        );
    }

    /// A scope refusal from `dispatch` answers `403 scope_denied` with the
    /// prefix stripped; any other dispatch error stays a plain `400`.
    #[tokio::test]
    async fn dispatch_errors_map_scope_refusals_to_403_with_a_code() {
        let denied = format!(
            "{}the branch belongs to another project",
            crate::web_server::SCOPE_DENIED_PREFIX
        );
        let response = dispatch_error_response(&denied);
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = json_body(response).await;
        assert_eq!(body["code"], "scope_denied");
        assert_eq!(body["error"], "the branch belongs to another project");

        let response = dispatch_error_response("Branch not found: b1");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = json_body(response).await;
        assert!(body.get("code").is_none(), "command errors carry no code");
        assert_eq!(body["error"], "Branch not found: b1");
    }

    /// Every allowlisted command has a scope row — and only those — so a
    /// new verb cannot reach `dispatch` without saying how its project is
    /// resolved and whether it writes.
    #[test]
    fn allowlist_and_control_scope_table_name_the_same_commands() {
        let mut scoped: Vec<&str> = crate::web_server::control_scope::CONTROL_SCOPE
            .iter()
            .map(|(name, ..)| *name)
            .collect();
        scoped.sort_unstable();
        let mut allowed: Vec<&str> = ALLOWED_COMMANDS.to_vec();
        allowed.sort_unstable();
        assert_eq!(
            scoped, allowed,
            "CONTROL_SCOPE and ALLOWED_COMMANDS must list exactly the same commands"
        );
    }

    // -------------------------------------------------------------------
    // Allowlist
    // -------------------------------------------------------------------

    /// The allowlist is the security boundary: pin it exactly so growing the
    /// CLI surface is always a deliberate, reviewed change here.
    #[test]
    fn allowlist_is_exactly_the_command_table() {
        let expected = [
            "cancel_session",
            "create_image",
            "create_tag",
            "create_tag_group",
            "get_active_sessions",
            "get_branch_timeline",
            "get_project_timeline",
            "get_session",
            "get_tag_state",
            "list_branches_for_project",
            "list_project_notes",
            "list_project_repos",
            "list_projects",
            "move_tag",
            "open_project",
            "rename_tag_group",
            "reorder_tag_groups",
            "set_icon_tag_group",
            "set_project_group_tag",
            "set_project_tag_assigned",
            "set_tag_group_sidebar",
            "start_branch_session",
            "start_project_session",
            "update_tag",
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
    // Startup readiness
    // -------------------------------------------------------------------

    /// The discovery file names an endpoint that already accepts connections
    /// by the time the synchronous half returns — no runtime, no `serve`
    /// task — so a session started right after `start()` can reach the app.
    #[test]
    fn bind_and_write_discovery_is_ready_before_serving() {
        let data_dir = tempfile::tempdir().unwrap();
        let root = files::control_root(data_dir.path());
        // Leftovers of a dead instance in the v2 layout and of a v1 build.
        let stale = files::instance_dir_path(&root, 999_999_999);
        std::fs::create_dir_all(&stale).unwrap();
        let legacy = data_dir.path().join("control-999999998.json");
        std::fs::write(&legacy, "{}").unwrap();

        let prepared = bind_and_write_discovery(data_dir.path(), 1234, 3).unwrap();

        assert!(!stale.exists(), "the sweep runs before the bind");
        assert!(!legacy.exists(), "v1 leftovers are swept too");
        let instance_dir = files::instance_dir_path(&root, 1234);
        assert!(files::sessions_dir_path(&instance_dir).is_dir());
        let contents: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(files::discovery_file_path(&instance_dir)).unwrap(),
        )
        .unwrap();
        assert_eq!(contents["generation"], 3);
        assert_eq!(contents["pid"], 1234);

        // The connection waits in the listen backlog until `serve` accepts.
        match (&prepared.listener, &prepared.transport) {
            #[cfg(unix)]
            (BoundListener::Unix(_), ControlTransport::Unix { socket }) => {
                assert_eq!(socket, &files::socket_path(&instance_dir));
                assert_eq!(contents["socket"], socket.display().to_string());
                std::os::unix::net::UnixStream::connect(socket)
                    .expect("the bound socket should take connections before serve runs");
            }
            (BoundListener::Tcp(_), ControlTransport::Tcp { port, token }) => {
                assert_eq!(contents["port"], *port);
                assert_eq!(contents["token"], token.as_str());
                std::net::TcpStream::connect(("127.0.0.1", *port))
                    .expect("the bound listener should take connections before serve runs");
            }
            #[allow(unreachable_patterns)]
            _ => panic!("listener and transport disagree"),
        }
        #[cfg(unix)]
        assert!(
            matches!(prepared.transport, ControlTransport::Unix { .. }),
            "a short socket path must bind the Unix socket, not fall back"
        );
    }

    /// `sun_path` is finite; an instance directory whose socket path would
    /// overflow it falls back to loopback TCP with a per-start token rather
    /// than failing to start.
    #[test]
    fn over_long_socket_path_falls_back_to_tcp_with_a_token() {
        let root = private_tempdir();
        let long = root.path().join("x".repeat(MAX_SOCKET_PATH_LEN));
        std::fs::create_dir_all(&long).unwrap();
        assert!(!socket_path_fits(&files::socket_path(&long)));

        let (listener, transport) = bind_listener(&long).unwrap();
        assert!(matches!(listener, BoundListener::Tcp(_)));
        let ControlTransport::Tcp { port, token } = transport else {
            panic!("expected the TCP fallback, got {transport:?}");
        };
        assert_ne!(port, 0);
        assert_eq!(token.len(), 64, "a 256-bit hex token");
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(
            !files::socket_path(&long).exists(),
            "no socket file is left behind"
        );
    }

    #[test]
    fn socket_path_fits_bounds_at_the_portable_sun_path_limit() {
        let fits = PathBuf::from("/".to_string() + &"a".repeat(MAX_SOCKET_PATH_LEN - 1));
        assert_eq!(fits.as_os_str().len(), MAX_SOCKET_PATH_LEN);
        assert!(socket_path_fits(&fits));
        let over = PathBuf::from("/".to_string() + &"a".repeat(MAX_SOCKET_PATH_LEN));
        assert!(!socket_path_fits(&over));
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
        // The grants registry is state `start` reads and the first session
        // start mints into, so it is managed ahead of both.
        let manage_call = "app.manage(control_server::ControlGrants::default());";
        let managed_at = lib
            .find(manage_call)
            .expect("lib.rs should manage control_server::ControlGrants");
        assert!(
            managed_at < started_at,
            "lib.rs must manage ControlGrants before control_server::start"
        );
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

    // -------------------------------------------------------------------
    // Unix socket end to end
    // -------------------------------------------------------------------

    /// One raw HTTP/1.1 request over the socket, returning the response body.
    #[cfg(unix)]
    async fn unix_http_get(socket: &std::path::Path, path: &str, host: &str) -> (u16, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::UnixStream::connect(socket).await.unwrap();
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.unwrap();
        let text = String::from_utf8(raw).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap();
        (status, body.to_string())
    }

    /// axum serves the tokio `UnixListener` as is, the connect-info carries
    /// the kernel's peer credentials for the connecting process (us), and the
    /// uid gate plus the header rules sit in front of the handler exactly as
    /// `serve` stacks them.
    #[tokio::test]
    #[cfg(unix)]
    async fn unix_socket_serves_requests_with_the_peers_credentials() {
        use axum::extract::ConnectInfo;
        let root = private_tempdir();
        let instance_dir = files::instance_dir_path(root.path(), 1234);
        files::create_fresh_instance_dir(&instance_dir).unwrap();
        let socket = files::socket_path(&instance_dir);
        let listener = tokio::net::UnixListener::from_std(bind_unix(&socket).unwrap()).unwrap();

        async fn echo_peer(ConnectInfo(peer): ConnectInfo<UdsPeer>) -> Json<Value> {
            Json(serde_json::json!({ "uid": peer.uid, "pid": peer.pid }))
        }
        let router = Router::new()
            .route("/peer", get(echo_peer))
            .layer(middleware::from_fn(reject_browser_shaped_requests))
            .layer(middleware::from_fn(reject_foreign_peer));
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<UdsPeer>(),
            )
            .await
            .unwrap();
        });

        let (status, body) = unix_http_get(&socket, "/peer", HOST_LITERAL).await;
        assert_eq!(status, 200, "body: {body}");
        let peer: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(peer["uid"], files::current_uid());
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        assert_eq!(
            peer["pid"],
            std::process::id(),
            "LOCAL_PEEREPID / SO_PEERCRED report the connecting process"
        );

        // The header rules still apply on the socket (belt and braces).
        let (status, body) = unix_http_get(&socket, "/peer", "127.0.0.1:80").await;
        assert_eq!(status, 403, "body: {body}");

        server.abort();
    }
}
