//! Axum HTTPS/WebSocket server for browser-based access.
//!
//! Runs alongside the Tauri desktop app on `0.0.0.0:5175` with a provided TLS
//! certificate and key, serving:
//!
//! - `POST /api/invoke/{command}` — dispatches to the same logic as Tauri commands
//! - `GET  /api/events`           — WebSocket that broadcasts Tauri events as JSON
//! - `POST /api/auth`             — accepts bearer token and sets session cookie
//! - `GET  /*`                    — static files from `../dist` (the built Svelte frontend)
//!
//! All `/api/*` routes (except `/api/auth`) require authentication via either
//! an `Authorization: Bearer <token>` header or a valid `staged_session` cookie.

// The full implementation is preserved here but start() is currently stubbed out,
// so most items appear unused to the compiler.
#![allow(dead_code, unused_imports)]

use std::collections::HashSet;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Request, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::serve::Listener;
use axum::Router;
use axum_extra::extract::cookie::{Cookie, CookieJar};
use rand::Rng;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde_json::Value;
use subtle::ConstantTimeEq;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::session_commands;
use crate::session_runner::{self, SessionConfig, SessionRegistry};
use crate::store::{self, Store};

// =============================================================================
// Shared state
// =============================================================================

/// State shared between the Axum web server and the Tauri app.
///
/// Holds the Tauri `AppHandle` which provides access to all managed state
/// (Store, SessionRegistry, etc.) via `app_handle.state()`. This ensures
/// both Tauri commands and web endpoints operate on the exact same objects.
#[derive(Clone)]
pub struct WebAppState {
    pub app_handle: tauri::AppHandle,
    pub event_tx: broadcast::Sender<WebEvent>,
    /// Hex-encoded 256-bit token required to authenticate web clients.
    pub auth_token: String,
    /// Set of valid session IDs, one per authenticated client.
    pub sessions: Arc<Mutex<HashSet<String>>>,
}

/// A serialized event for WebSocket broadcast.
#[derive(Clone, Debug)]
pub struct WebEvent {
    pub event_name: String,
    pub payload: String,
}

// =============================================================================
// Event broadcast helper
// =============================================================================

/// Emit an event both to Tauri (for the desktop window) and to the web
/// broadcast channel (for browser clients over WebSocket).
///
/// Extracts the broadcast sender from the `AppHandle`'s managed state so
/// callers don't need to pass it explicitly. This is the preferred way to
/// emit events — it replaces direct `app_handle.emit()` calls so that web
/// browser clients connected via WebSocket also receive the event.
pub fn emit_to_all<S: serde::Serialize + Clone>(
    app_handle: &tauri::AppHandle,
    event_name: &str,
    payload: S,
) {
    use tauri::{Emitter, Manager};
    let _ = app_handle.emit(event_name, payload.clone());
    if let Some(tx) = app_handle.try_state::<broadcast::Sender<WebEvent>>() {
        if let Ok(json) = serde_json::to_string(&serde_json::json!({
            "event": event_name,
            "payload": payload,
        })) {
            let _ = tx.send(WebEvent {
                event_name: event_name.to_string(),
                payload: json,
            });
        }
    }
}

// =============================================================================
// Server startup
// =============================================================================

/// Generate a cryptographically random hex-encoded token (256-bit).
pub fn generate_token() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    hex::encode(bytes)
}

const CERT_PATH_ENV: &str = "STAGED_WEB_CERT_PATH";
const KEY_PATH_ENV: &str = "STAGED_WEB_KEY_PATH";

fn load_tls_acceptor() -> Result<TlsAcceptor, String> {
    let cert_path = std::env::var(CERT_PATH_ENV)
        .map_err(|_| format!("{CERT_PATH_ENV} must point to a PEM certificate file"))?;
    let key_path = std::env::var(KEY_PATH_ENV)
        .map_err(|_| format!("{KEY_PATH_ENV} must point to a PEM private key file"))?;

    let certs = CertificateDer::pem_file_iter(&cert_path)
        .map_err(|e| format!("failed to open TLS certificate {cert_path}: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to read TLS certificate {cert_path}: {e}"))?;
    if certs.is_empty() {
        return Err(format!(
            "TLS certificate {cert_path} did not contain any certificates"
        ));
    }

    let key = PrivateKeyDer::from_pem_file(&key_path)
        .map_err(|e| format!("failed to read TLS private key {key_path}: {e}"))?;
    let config = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::aws_lc_rs::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .map_err(|e| format!("failed to configure TLS protocol versions: {e}"))?
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .map_err(|e| format!("failed to configure TLS certificate/key pair: {e}"))?;

    Ok(TlsAcceptor::from(Arc::new(config)))
}

struct TlsListener {
    inner: TcpListener,
    acceptor: TlsAcceptor,
}

impl TlsListener {
    fn new(inner: TcpListener, acceptor: TlsAcceptor) -> Self {
        Self { inner, acceptor }
    }
}

impl Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (stream, addr) = match self.inner.accept().await {
                Ok(connection) => connection,
                Err(e) => {
                    log::error!("[web_server] accept error: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };

            match self.acceptor.accept(stream).await {
                Ok(tls_stream) => return (tls_stream, addr),
                Err(e) => log::warn!("[web_server] TLS handshake failed from {addr}: {e}"),
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

/// Start the Axum web server in a background tokio task.
///
/// Stubbed — logs a warning and returns. The full implementation (TLS listener,
/// Axum router with static file serving) is intentionally disabled in this build.
/// All route handlers, auth middleware, and the `dispatch()` match block are kept
/// compiling so they stay in sync with the rest of the codebase.
///
/// TODO(web): restore full web server startup from the `mobile-web` branch.
pub fn start(_state: WebAppState) {
    log::warn!("Web server requested but this build has the web server stubbed out");
}

// =============================================================================
// Authentication
// =============================================================================

const SESSION_COOKIE_NAME: &str = "staged_session";
const SESSION_MAX_AGE_DAYS: i64 = 7;

/// Constant-time string comparison to prevent timing side-channel attacks.
fn constant_time_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Middleware that rejects unauthenticated requests to protected routes.
///
/// Accepts either:
/// - `Authorization: Bearer <token>` header matching the server's auth token
/// - `staged_session` cookie matching the server's session ID
async fn require_auth(
    State(state): State<WebAppState>,
    jar: CookieJar,
    request: Request,
    next: Next,
) -> Response {
    // Check Authorization header (constant-time comparison to prevent timing attacks)
    if let Some(auth_header) = request.headers().get("authorization") {
        if let Ok(value) = auth_header.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                if constant_time_eq(token, &state.auth_token) {
                    return next.run(request).await;
                }
            }
        }
    }

    // Check session cookie against the set of valid sessions
    if let Some(cookie) = jar.get(SESSION_COOKIE_NAME) {
        let is_valid = {
            let sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let cookie_val = cookie.value();
            sessions.iter().any(|s| constant_time_eq(cookie_val, s))
        };
        if is_valid {
            return next.run(request).await;
        }
    }

    (StatusCode::UNAUTHORIZED, "Authentication required").into_response()
}

/// POST /api/auth — validate the bearer token and issue a session cookie.
///
/// Expects JSON body: `{ "token": "<auth_token>" }`
async fn authenticate(
    State(state): State<WebAppState>,
    jar: CookieJar,
    Json(body): Json<Value>,
) -> Response {
    let token = body
        .get("token")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    if !constant_time_eq(token, &state.auth_token) {
        return (StatusCode::UNAUTHORIZED, "Invalid token").into_response();
    }

    // Generate a unique session ID for this client and register it.
    let new_session_id = generate_token();
    state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(new_session_id.clone());

    let cookie = Cookie::build((SESSION_COOKIE_NAME, new_session_id))
        .path("/")
        .http_only(true)
        .secure(true)
        .max_age(time::Duration::days(SESSION_MAX_AGE_DAYS))
        .same_site(axum_extra::extract::cookie::SameSite::Lax)
        .build();

    (jar.add(cookie), Json(serde_json::json!({ "ok": true }))).into_response()
}

// =============================================================================
// WebSocket endpoint — /api/events
// =============================================================================

async fn ws_events(ws: WebSocketUpgrade, State(state): State<WebAppState>) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: WebAppState) {
    let mut rx = state.event_tx.subscribe();
    loop {
        tokio::select! {
            // Forward broadcast events to the WebSocket client
            msg = rx.recv() => {
                match msg {
                    Ok(event) => {
                        if socket.send(Message::Text(event.payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("[web_server] WebSocket client lagged, dropped {n} events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            // Handle incoming messages (ping/pong, close)
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(data))) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

// =============================================================================
// Command dispatch — POST /api/invoke/{command}
// =============================================================================

async fn invoke_command(
    Path(command): Path<String>,
    State(state): State<WebAppState>,
    Json(args): Json<Value>,
) -> Response {
    match dispatch(&command, args, &state).await {
        Ok(value) => Json(value).into_response(),
        Err(msg) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response(),
    }
}

/// Helper to extract a typed field from the JSON args.
fn arg<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Result<T, String> {
    args.get(key)
        .ok_or_else(|| format!("missing argument: {key}"))
        .and_then(|v| {
            serde_json::from_value(v.clone()).map_err(|e| format!("invalid argument '{key}': {e}"))
        })
}

/// Helper to extract an optional typed field.
fn opt_arg<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Result<Option<T>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|e| format!("invalid argument '{key}': {e}")),
    }
}

/// Helper to get the Store Arc from the shared mutex slot.
fn get_store(store: &Mutex<Option<Arc<Store>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Database not initialized".to_string())
}

/// The big dispatcher. Maps command names to handler logic.
///
/// Each arm extracts arguments from the JSON body and calls the same
/// underlying logic as the corresponding Tauri command. Return values
/// are serialized to `serde_json::Value`.
async fn dispatch(command: &str, args: Value, state: &WebAppState) -> Result<Value, String> {
    use tauri::Manager;

    // Convenience aliases — pull shared state from the Tauri AppHandle
    let app_handle = &state.app_handle;
    let store_mutex: &Mutex<Option<Arc<Store>>> = &app_handle.state::<Mutex<Option<Arc<Store>>>>();
    let session_registry: &Arc<SessionRegistry> = &app_handle.state::<Arc<SessionRegistry>>();
    let action_executor: &Arc<ActionExecutor> = &app_handle.state::<Arc<ActionExecutor>>();
    let action_registry: &Arc<ActionRegistry> = &app_handle.state::<Arc<ActionRegistry>>();

    match command {
        // =====================================================================
        // Store status
        // =====================================================================
        "get_store_status" => {
            // We don't have DbState in web context — return null (store ready)
            Ok(Value::Null)
        }
        "confirm_reset_store" => {
            // Not applicable in web context
            Err("confirm_reset_store is not supported in web mode".to_string())
        }

        // =====================================================================
        // Projects
        // =====================================================================
        "list_projects" => {
            let store = get_store(store_mutex)?;
            let projects = store.list_projects().map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(projects).unwrap())
        }
        "create_project" => {
            let store = get_store(store_mutex)?;
            let name: String = arg(&args, "name")?;
            let github_repo: Option<String> = opt_arg(&args, "githubRepo")?;
            let location: Option<String> = opt_arg(&args, "location")?;
            let subpath: Option<String> = opt_arg(&args, "subpath")?;
            let branch_name: Option<String> = opt_arg(&args, "branchName")?;
            let pr_number: Option<u64> = opt_arg(&args, "prNumber")?;
            let default_branch: Option<String> = opt_arg(&args, "defaultBranch")?;
            let head_repo: Option<String> = opt_arg(&args, "headRepo")?;

            let trimmed = name.trim();
            if trimmed.is_empty() {
                return Err("Project name is required".to_string());
            }

            let project_location = match location.as_deref() {
                Some("remote") => crate::store::ProjectLocation::Remote,
                _ => crate::store::ProjectLocation::Local,
            };
            let inferred_branch_name = branch_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| crate::branches::infer_branch_name(trimmed));

            let mut project = crate::store::Project::named(trimmed);
            project.location = project_location;
            if let Some(ref repo) = github_repo {
                project = project.with_primary_repo(repo);
            }
            if let Some(ref sub) = subpath {
                project = project.with_subpath(sub.clone());
            }
            store.create_project(&project).map_err(|e| e.to_string())?;

            if let Ok(project_dir) = crate::git::project_worktree_root_for(&project.id) {
                let _ = std::fs::create_dir_all(&project_dir);
            }

            if let Some(repo) = project.primary_repo() {
                store
                    .get_or_create_action_context(repo, project.subpath.as_deref())
                    .map_err(|e| e.to_string())?;
            }

            if let Some(ref repo) = github_repo {
                store
                    .record_recent_repo(repo, subpath.clone())
                    .map_err(|e| e.to_string())?;
            }

            if let Some(repo) = github_repo {
                let mut project_repo = crate::store::ProjectRepo::new(
                    &project.id,
                    &repo,
                    &inferred_branch_name,
                    subpath,
                )
                .primary();
                project_repo.head_repo = head_repo;
                store
                    .create_project_repo(&project_repo)
                    .map_err(|e| e.to_string())?;

                let effective_base = crate::git::resolve_default_branch(default_branch, &repo);

                match project.location {
                    crate::store::ProjectLocation::Local => {
                        let mut branch = crate::store::Branch::new(
                            &project.id,
                            &inferred_branch_name,
                            &effective_base,
                        )
                        .with_project_repo(&project_repo.id);
                        if let Some(pr) = pr_number {
                            branch = branch.with_pr(pr);
                        }
                        store.create_branch(&branch).map_err(|e| e.to_string())?;

                        // Fire-and-forget background setup
                        let store_bg = Arc::clone(&store);
                        let app_handle = app_handle.clone();
                        let branch_id = branch.id.clone();
                        let project_id = project.id.clone();
                        tauri::async_runtime::spawn(async move {
                            emit_to_all(&app_handle, "project-setup-progress", project_id.clone());

                            let store_clone = Arc::clone(&store_bg);
                            let branch_id_clone = branch_id.clone();
                            let app_handle_clone = app_handle.clone();
                            let worktree_result = tauri::async_runtime::spawn_blocking(move || {
                                crate::branches::setup_worktree_sync(
                                    &store_clone,
                                    &branch_id_clone,
                                    Some(&app_handle_clone),
                                )
                            })
                            .await;

                            match worktree_result {
                                Ok(Ok(path)) => {
                                    log::info!("[web_server] worktree ready at {path}");
                                    emit_to_all(&app_handle, "project-setup-progress", project_id);
                                }
                                Ok(Err(e)) => log::warn!("[web_server] worktree setup failed: {e}"),
                                Err(e) => log::warn!("[web_server] worktree task panicked: {e}"),
                            }
                        });
                    }
                    crate::store::ProjectLocation::Remote => {
                        let workspace_name =
                            crate::branches::infer_workspace_name(&inferred_branch_name);
                        let mut branch = crate::store::Branch::new_remote(
                            &project.id,
                            &inferred_branch_name,
                            &effective_base,
                            &workspace_name,
                        )
                        .with_project_repo(&project_repo.id);
                        if let Some(pr) = pr_number {
                            branch = branch.with_pr(pr);
                        }
                        store.create_branch(&branch).map_err(|e| e.to_string())?;
                    }
                };
            }

            Ok(serde_json::to_value(project).unwrap())
        }
        "list_project_repos" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let repos = store
                .list_project_repos(&project_id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(repos).unwrap())
        }
        "list_recent_repos" => {
            let store = get_store(store_mutex)?;
            let limit: Option<usize> = opt_arg(&args, "limit")?;
            let repos = store
                .list_recent_repos(limit.unwrap_or(10))
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(repos).unwrap())
        }
        "get_suggested_repos" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let limit: Option<usize> = opt_arg(&args, "limit")?;
            let repos = store
                .get_suggested_repos_for_project(&project_id, limit.unwrap_or(5))
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(repos).unwrap())
        }
        "add_project_repo" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let github_repo: String = arg(&args, "githubRepo")?;
            let branch_name: Option<String> = opt_arg(&args, "branchName")?;
            let subpath: Option<String> = opt_arg(&args, "subpath")?;
            let set_as_primary: Option<bool> = opt_arg(&args, "setAsPrimary")?;
            let pr_number: Option<u64> = opt_arg(&args, "prNumber")?;
            let default_branch: Option<String> = opt_arg(&args, "defaultBranch")?;
            let head_repo: Option<String> = opt_arg(&args, "headRepo")?;

            let repo = crate::project_commands::add_project_repo_impl(
                Arc::clone(&store),
                project_id,
                github_repo,
                branch_name,
                subpath,
                set_as_primary,
                None,
                pr_number,
                default_branch,
                head_repo,
            )
            .await?;
            Ok(serde_json::to_value(repo).unwrap())
        }
        "update_project_repo_branch_name" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let project_repo_id: String = arg(&args, "projectRepoId")?;
            let branch_name: String = arg(&args, "branchName")?;
            let trimmed = branch_name.trim();
            if trimmed.is_empty() {
                return Err("Branch name is required".to_string());
            }
            store
                .update_project_repo_branch_name(&project_id, &project_repo_id, trimmed)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "clear_project_repo_reason" => {
            let store = get_store(store_mutex)?;
            let project_repo_id: String = arg(&args, "projectRepoId")?;
            store
                .clear_project_repo_reason(&project_repo_id)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "remove_project_repo" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let project_repo_id: String = arg(&args, "projectRepoId")?;

            let removed = store
                .get_project_repo(&project_repo_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project repo not found: {project_repo_id}"))?;
            let branches = store
                .list_branches_for_project(&project_id)
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|b| b.project_repo_id.as_deref() == Some(project_repo_id.as_str()))
                .collect::<Vec<_>>();

            let branch_ids: Vec<&str> = branches.iter().map(|b| b.id.as_str()).collect();
            crate::actions::commands::stop_actions_for_branches(
                action_executor,
                action_registry,
                &branch_ids,
            );

            tauri::async_runtime::spawn_blocking({
                let store = Arc::clone(&store);
                let branches = branches.clone();
                move || -> Result<(), String> {
                    for branch in &branches {
                        crate::branches::cleanup_branch_resources(&store, branch)?;
                    }
                    Ok(())
                }
            })
            .await
            .map_err(|e| format!("Failed to clean up branch resources: {e}"))??;

            for branch in &branches {
                store.delete_branch(&branch.id).map_err(|e| e.to_string())?;
            }
            store
                .delete_project_repo(&project_repo_id)
                .map_err(|e| e.to_string())?;

            if removed.is_primary {
                let next_primary = store
                    .list_project_repos(&project_id)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .next();
                let project = store
                    .get_project(&project_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("Project not found: {project_id}"))?;
                if let Some(next) = next_primary {
                    store
                        .set_primary_project_repo(&project_id, &next.id)
                        .map_err(|e| e.to_string())?;
                    store
                        .update_project(
                            &project_id,
                            &project.name,
                            Some(&next.github_repo),
                            &project.location,
                            next.subpath.as_deref(),
                        )
                        .map_err(|e| e.to_string())?;
                } else {
                    store
                        .update_project(&project_id, &project.name, None, &project.location, None)
                        .map_err(|e| e.to_string())?;
                }
            }
            Ok(Value::Null)
        }
        "set_primary_project_repo" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let project_repo_id: String = arg(&args, "projectRepoId")?;
            let repo = store
                .get_project_repo(&project_repo_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project repo not found: {project_repo_id}"))?;
            store
                .set_primary_project_repo(&project_id, &project_repo_id)
                .map_err(|e| e.to_string())?;
            let project = store
                .get_project(&project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {project_id}"))?;
            store
                .update_project(
                    &project_id,
                    &project.name,
                    Some(&repo.github_repo),
                    &project.location,
                    repo.subpath.as_deref(),
                )
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "delete_project" => {
            let store = get_store(store_mutex)?;
            let id: String = arg(&args, "id")?;
            let branches = store
                .list_branches_for_project(&id)
                .map_err(|e| e.to_string())?;
            let branch_ids: Vec<&str> = branches.iter().map(|b| b.id.as_str()).collect();
            crate::actions::commands::stop_actions_for_branches(
                action_executor,
                action_registry,
                &branch_ids,
            );

            tauri::async_runtime::spawn_blocking({
                let store = Arc::clone(&store);
                let id = id.clone();
                let branches = branches.clone();
                move || {
                    for branch in &branches {
                        crate::branches::cleanup_branch_resources_best_effort(&store, branch);
                        let _ = store.delete_branch(&branch.id);
                    }
                    if let Ok(project_root) = crate::git::project_worktree_root_for(&id) {
                        if project_root.exists() {
                            let _ = std::fs::remove_dir_all(&project_root);
                        }
                    }
                }
            })
            .await
            .map_err(|e| format!("Failed to clean up project resources: {e}"))?;

            store.delete_project(&id).map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }

        // =====================================================================
        // Repo badges
        // =====================================================================
        "get_all_repo_badges" => {
            let store = get_store(store_mutex)?;
            let badges = store.list_repo_badges().map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(badges).unwrap())
        }
        "ensure_repo_badges" => {
            // Simplified version — just returns existing badges without AI generation
            let store = get_store(store_mutex)?;
            let repos: Vec<(String, String)> = arg(&args, "repos")?;
            let mut result = Vec::new();
            for (github_repo, subpath) in &repos {
                if let Some(badge) = store
                    .get_repo_badge(github_repo, subpath)
                    .map_err(|e| e.to_string())?
                {
                    result.push(badge);
                } else {
                    let existing_badges = store.list_repo_badges().map_err(|e| e.to_string())?;
                    let taken: Vec<String> = existing_badges
                        .iter()
                        .map(|b| b.short_name.clone())
                        .collect();
                    let short_name =
                        crate::store::fallback_short_name(github_repo, subpath, &taken);
                    let existing_hues = store.list_badge_hues().map_err(|e| e.to_string())?;
                    let hue = crate::store::next_hue(&existing_hues);
                    let badge = crate::store::RepoBadge {
                        github_repo: github_repo.clone(),
                        subpath: subpath.to_string(),
                        short_name,
                        hue,
                        created_at: crate::store::now_timestamp(),
                    };
                    let _ = store.create_repo_badge(&badge);
                    if let Some(b) = store
                        .get_repo_badge(github_repo, subpath)
                        .map_err(|e| e.to_string())?
                    {
                        result.push(b);
                    }
                }
            }
            Ok(serde_json::to_value(result).unwrap())
        }
        "update_repo_badge" => {
            let store = get_store(store_mutex)?;
            let github_repo: String = arg(&args, "githubRepo")?;
            let subpath: String = arg(&args, "subpath")?;
            let short_name: String = arg(&args, "shortName")?;
            let hue: f64 = arg(&args, "hue")?;
            store
                .update_repo_badge(&github_repo, &subpath, &short_name, hue)
                .map_err(|e| e.to_string())?;
            let badge = store
                .get_repo_badge(&github_repo, &subpath)
                .map_err(|e| e.to_string())?
                .ok_or("Badge not found after update")?;
            Ok(serde_json::to_value(badge).unwrap())
        }
        "delete_repo_badge" => {
            let store = get_store(store_mutex)?;
            let github_repo: String = arg(&args, "githubRepo")?;
            let subpath: String = arg(&args, "subpath")?;
            store
                .delete_repo_badge(&github_repo, &subpath)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }

        // =====================================================================
        // GitHub commands
        // =====================================================================
        "list_github_orgs" => {
            let orgs = crate::git::list_github_orgs().map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(orgs).unwrap())
        }
        "list_github_repos" => {
            let owner: Option<String> = opt_arg(&args, "owner")?;
            let repos =
                crate::git::list_github_repos(owner.as_deref()).map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(repos).unwrap())
        }
        "list_user_repos" => {
            let limit: Option<u32> = opt_arg(&args, "limit")?;
            let repos =
                crate::git::list_user_repos(limit.unwrap_or(30)).map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(repos).unwrap())
        }
        "get_github_repo" => {
            let owner: String = arg(&args, "owner")?;
            let repo: String = arg(&args, "repo")?;
            let result = crate::git::fetch_github_repo(&owner, &repo).map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(result).unwrap())
        }
        "search_github_repos" => {
            let query: String = arg(&args, "query")?;
            let owner: Option<String> = opt_arg(&args, "owner")?;
            let repos = crate::git::search_github_repos(&query, owner.as_deref())
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(repos).unwrap())
        }
        "check_monorepo_modules" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            let count =
                crate::git::check_monorepo_modules(&github_repo).map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(count).unwrap())
        }
        "validate_subpath" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            let subpath: String = arg(&args, "subpath")?;
            crate::git::validate_subpath_in_repo(&github_repo, &subpath)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "list_repo_directories" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            let path: String = arg(&args, "path")?;
            let dirs = crate::git::list_repo_directories(&github_repo, &path)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(dirs).unwrap())
        }
        "list_git_branches" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            let branches =
                crate::git::list_branches_for_repo(&github_repo).map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(branches).unwrap())
        }
        "detect_default_branch_cmd" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            let branch = crate::git::detect_default_branch_for_repo(&github_repo)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(branch).unwrap())
        }
        "prune_remote_refs" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            crate::git::prune_remote_for_repo(&github_repo).map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "check_existing_local_branch" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let branch_name: String = arg(&args, "branchName")?;
            let trimmed = branch_name.trim();
            if trimmed.is_empty() {
                return Ok(serde_json::to_value(false).unwrap());
            }
            let project = store
                .get_project(&project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {project_id}"))?;
            let result = project
                .primary_repo()
                .and_then(crate::paths::clone_path_for)
                .filter(|p| p.exists())
                .map(|repo_path| crate::git::branch_exists(&repo_path, trimmed).unwrap_or(false))
                .unwrap_or(false);
            Ok(serde_json::to_value(result).unwrap())
        }
        "get_pr_for_repo" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            let pr_number: u64 = arg(&args, "prNumber")?;
            let pr = tauri::async_runtime::spawn_blocking(move || {
                crate::git::github::get_pr_for_repo(&github_repo, pr_number)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
            Ok(serde_json::to_value(pr).unwrap())
        }
        "get_pr_for_branch" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            let branch_name: String = arg(&args, "branchName")?;
            let pr = tauri::async_runtime::spawn_blocking(move || {
                crate::git::github::get_pr_for_branch_for_repo(&github_repo, &branch_name)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
            Ok(serde_json::to_value(pr).unwrap())
        }
        "list_pull_requests" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            let prs = tauri::async_runtime::spawn_blocking(move || {
                crate::git::list_pull_requests_for_repo(&github_repo).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
            Ok(serde_json::to_value(prs).unwrap())
        }
        "get_parent_repo" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            let parent = tauri::async_runtime::spawn_blocking(move || {
                crate::git::github::get_parent_repo(&github_repo).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
            Ok(serde_json::to_value(parent).unwrap())
        }
        "list_issues" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            let issues = tauri::async_runtime::spawn_blocking(move || {
                crate::git::list_issues_for_repo(&github_repo).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
            Ok(serde_json::to_value(issues).unwrap())
        }
        "post_comment_to_github" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let pr_number: u64 = arg(&args, "prNumber")?;
            let comment: store::Comment = arg(&args, "comment")?;
            let result = crate::github_commands::post_comment_to_github_impl(
                store, branch_id, pr_number, comment,
            )
            .await?;
            Ok(serde_json::to_value(result).unwrap())
        }

        // =====================================================================
        // Branches
        // =====================================================================
        "get_branch" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let branch = store.get_branch(&branch_id).map_err(|e| e.to_string())?;
            let enriched = match branch {
                Some(branch) => {
                    let workdir = store
                        .get_workdir_for_branch(&branch.id)
                        .map_err(|e| e.to_string())?;
                    Some(crate::branches::to_branch_with_workdir_public(
                        branch,
                        workdir.map(|w| w.path),
                    ))
                }
                None => None,
            };
            Ok(serde_json::to_value(enriched).unwrap())
        }
        "list_branches_for_project" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let branches = store
                .list_branches_for_project(&project_id)
                .map_err(|e| e.to_string())?;
            let enriched: Vec<crate::BranchWithWorkdir> = branches
                .into_iter()
                .map(|b| {
                    let workdir = store.get_workdir_for_branch(&b.id).ok().flatten();
                    crate::branches::to_branch_with_workdir_public(b, workdir.map(|w| w.path))
                })
                .collect();
            Ok(serde_json::to_value(enriched).unwrap())
        }
        "create_branch" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let branch_name: String = arg(&args, "branchName")?;
            let base_branch: String = arg(&args, "baseBranch")?;
            let pr_number: Option<u64> = opt_arg(&args, "prNumber")?;
            let project_repo_id: Option<String> = opt_arg(&args, "projectRepoId")?;

            let mut branch = crate::store::Branch::new(&project_id, &branch_name, &base_branch);
            if let Some(pr) = pr_number {
                branch = branch.with_pr(pr);
            }
            if let Some(ref repo_id) = project_repo_id {
                branch = branch.with_project_repo(repo_id);
            }
            store.create_branch(&branch).map_err(|e| e.to_string())?;
            let workdir = store.get_workdir_for_branch(&branch.id).ok().flatten();
            let enriched =
                crate::branches::to_branch_with_workdir_public(branch, workdir.map(|w| w.path));
            Ok(serde_json::to_value(enriched).unwrap())
        }
        "delete_branch" => {
            let store = get_store(store_mutex)?;
            let id: String = arg(&args, "branchId")?;
            let branch = store
                .get_branch(&id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Branch not found: {id}"))?;
            let branch_ids = vec![id.as_str()];
            crate::actions::commands::stop_actions_for_branches(
                action_executor,
                action_registry,
                &branch_ids,
            );

            tauri::async_runtime::spawn_blocking({
                let store = Arc::clone(&store);
                let branch = branch.clone();
                move || crate::branches::cleanup_branch_resources(&store, &branch)
            })
            .await
            .map_err(|e| format!("Failed: {e}"))??;
            store.delete_branch(&id).map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "rename_branch" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let branch_name: String = arg(&args, "branchName")?;
            store
                .update_branch_name(&branch_id, &branch_name)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "get_blox_env" => Ok(serde_json::to_value(std::env::var("BLOX_ENV").ok()).unwrap()),
        // =====================================================================
        // Workspace / Blox commands (Tier 3)
        // =====================================================================
        "get_workspace_info" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let branch = store
                .get_branch(&branch_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
            let ws_name = branch
                .workspace_name
                .ok_or("Branch is not a remote workspace branch")?;
            let info = crate::branches::run_blox_blocking(move || crate::blox::ws_info(&ws_name))
                .await
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(info).unwrap())
        }
        "poll_workspace_status" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let branch = store
                .get_branch(&branch_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
            let ws_name = branch
                .workspace_name
                .as_deref()
                .ok_or("Branch is not a remote workspace branch")?;

            // Secondary clone setup: hold at Starting until setup marks it Running.
            let is_secondary_clone_setup = if branch.workspace_status
                == Some(store::WorkspaceStatus::Starting)
                && crate::branches::resolve_branch_workspace_subpath(&store, &branch)?.is_some()
            {
                if let Some(ws_name) = branch.workspace_name.as_deref() {
                    let peers = store
                        .list_branches_for_project(&branch.project_id)
                        .map_err(|e| e.to_string())?;
                    peers.into_iter().any(|peer| {
                        peer.id != branch.id
                            && peer.branch_type == store::BranchType::Remote
                            && peer.workspace_name.as_deref() == Some(ws_name)
                            && peer.workspace_status == Some(store::WorkspaceStatus::Running)
                    })
                } else {
                    false
                }
            } else {
                false
            };

            if is_secondary_clone_setup {
                return Ok(serde_json::to_value(crate::PollWorkspaceResult {
                    status: store::WorkspaceStatus::Starting.as_str().to_string(),
                    workstation_id: crate::branches::cached_workstation_id(ws_name),
                })
                .unwrap());
            }

            let info = match crate::branches::run_blox_blocking({
                let ws_name = ws_name.to_string();
                move || crate::blox::ws_info(&ws_name)
            })
            .await
            {
                Ok(info) => info,
                Err(crate::blox::BloxError::NotAuthenticated) => {
                    store
                        .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
                        .ok();
                    return Err("Not authenticated with Blox. Run: sq login".to_string());
                }
                Err(e) => {
                    let is_not_found = matches!(&e, crate::blox::BloxError::CommandFailed(msg) if msg.to_lowercase().contains("not found"));
                    if branch.workspace_status == Some(store::WorkspaceStatus::Starting) {
                        return Ok(serde_json::to_value(crate::PollWorkspaceResult {
                            status: store::WorkspaceStatus::Starting.as_str().to_string(),
                            workstation_id: crate::branches::cached_workstation_id(ws_name),
                        })
                        .unwrap());
                    }
                    if branch.workspace_status == Some(store::WorkspaceStatus::Running) {
                        if is_not_found {
                            store
                                .update_branch_workspace_status(
                                    &branch_id,
                                    &store::WorkspaceStatus::Stopped,
                                )
                                .ok();
                            return Ok(serde_json::to_value(crate::PollWorkspaceResult {
                                status: store::WorkspaceStatus::Stopped.as_str().to_string(),
                                workstation_id: crate::branches::cached_workstation_id(ws_name),
                            })
                            .unwrap());
                        }
                        return Ok(serde_json::to_value(crate::PollWorkspaceResult {
                            status: store::WorkspaceStatus::Running.as_str().to_string(),
                            workstation_id: crate::branches::cached_workstation_id(ws_name),
                        })
                        .unwrap());
                    }
                    return Err(e.to_string());
                }
            };

            let new_status = crate::branches::map_blox_status_to_workspace_status(
                info.status.as_deref(),
                branch.workspace_status.as_ref(),
            );
            if let Some(ws_id) = info.workstation_id {
                if let Some(name) = branch.workspace_name.as_deref() {
                    if let Ok(mut cache) = crate::branches::workstation_id_cache().lock() {
                        cache.insert(name.to_string(), ws_id);
                    }
                }
            }
            store
                .update_branch_workspace_status(&branch_id, &new_status)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(crate::PollWorkspaceResult {
                status: new_status.as_str().to_string(),
                workstation_id: crate::branches::cached_workstation_id(ws_name),
            })
            .unwrap())
        }
        "poll_all_workspace_statuses" => {
            let store = get_store(store_mutex)?;
            let branch_ids: Vec<String> = arg(&args, "branchIds")?;

            let entries = crate::branches::run_blox_blocking(crate::blox::ws_list)
                .await
                .map_err(|e| {
                    if matches!(e, crate::blox::BloxError::NotAuthenticated) {
                        "Not authenticated with Blox. Run: sq login".to_string()
                    } else {
                        e.to_string()
                    }
                })?;

            let ws_map: std::collections::HashMap<String, &crate::blox::WorkspaceListEntry> =
                entries.iter().map(|e| (e.name.clone(), e)).collect();

            let mut starting_workspaces: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            let mut results = std::collections::HashMap::new();

            for branch_id in &branch_ids {
                let branch = match store.get_branch(branch_id) {
                    Ok(Some(b)) => b,
                    _ => continue,
                };
                let ws_name = match branch.workspace_name.as_deref() {
                    Some(n) => n,
                    None => continue,
                };

                // Secondary clone setup check
                if branch.workspace_status == Some(store::WorkspaceStatus::Starting)
                    && crate::branches::resolve_branch_workspace_subpath(&store, &branch)
                        .unwrap_or(None)
                        .is_some()
                {
                    let is_secondary = if let Some(ws) = branch.workspace_name.as_deref() {
                        store
                            .list_branches_for_project(&branch.project_id)
                            .unwrap_or_default()
                            .into_iter()
                            .any(|peer| {
                                peer.id != *branch_id
                                    && peer.branch_type == store::BranchType::Remote
                                    && peer.workspace_name.as_deref() == Some(ws)
                                    && peer.workspace_status
                                        == Some(store::WorkspaceStatus::Running)
                            })
                    } else {
                        false
                    };
                    if is_secondary {
                        results.insert(
                            branch_id.clone(),
                            crate::PollWorkspaceResult {
                                status: store::WorkspaceStatus::Starting.as_str().to_string(),
                                workstation_id: crate::branches::cached_workstation_id(ws_name),
                            },
                        );
                        continue;
                    }
                }

                match ws_map.get(ws_name) {
                    Some(entry) => {
                        let new_status = crate::branches::map_blox_status_to_workspace_status(
                            entry.status.as_deref(),
                            branch.workspace_status.as_ref(),
                        );
                        if let Some(ws_id) = entry.workstation_id {
                            if let Ok(mut cache) = crate::branches::workstation_id_cache().lock() {
                                cache.insert(ws_name.to_string(), ws_id);
                            }
                        }
                        store
                            .update_branch_workspace_status(branch_id, &new_status)
                            .ok();
                        if new_status == store::WorkspaceStatus::Starting {
                            starting_workspaces
                                .entry(ws_name.to_string())
                                .or_default()
                                .push(branch_id.clone());
                        }
                        results.insert(
                            branch_id.clone(),
                            crate::PollWorkspaceResult {
                                status: new_status.as_str().to_string(),
                                workstation_id: crate::branches::cached_workstation_id(ws_name),
                            },
                        );
                    }
                    None => {
                        if branch.workspace_status == Some(store::WorkspaceStatus::Starting) {
                            starting_workspaces
                                .entry(ws_name.to_string())
                                .or_default()
                                .push(branch_id.clone());
                            results.insert(
                                branch_id.clone(),
                                crate::PollWorkspaceResult {
                                    status: store::WorkspaceStatus::Starting.as_str().to_string(),
                                    workstation_id: crate::branches::cached_workstation_id(ws_name),
                                },
                            );
                        } else if branch.workspace_status == Some(store::WorkspaceStatus::Running) {
                            store
                                .update_branch_workspace_status(
                                    branch_id,
                                    &store::WorkspaceStatus::Stopped,
                                )
                                .ok();
                            results.insert(
                                branch_id.clone(),
                                crate::PollWorkspaceResult {
                                    status: store::WorkspaceStatus::Stopped.as_str().to_string(),
                                    workstation_id: crate::branches::cached_workstation_id(ws_name),
                                },
                            );
                        }
                    }
                }
            }

            // Fetch bootstrap progress for starting workspaces in background.
            if !starting_workspaces.is_empty() {
                let app_handle_clone = app_handle.clone();
                tokio::task::spawn_blocking(move || {
                    for (ws_name, branch_ids) in &starting_workspaces {
                        match crate::blox::ws_commands(ws_name) {
                            Ok(cmds) => {
                                crate::branches::emit_workspace_setup_progress(
                                    &app_handle_clone,
                                    branch_ids,
                                    &cmds,
                                );
                            }
                            Err(e) => {
                                log::debug!(
                                    "[poll_all_workspace_statuses] ws_commands({}) failed: {}",
                                    ws_name,
                                    e
                                );
                            }
                        }
                    }
                });
            }

            Ok(serde_json::to_value(results).unwrap())
        }
        "setup_worktree" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let branch = store
                .get_branch(&branch_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
            let project = store
                .get_project(&branch.project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

            // Fast-path: already has a workdir
            if let Some(existing) = store
                .get_workdir_for_branch(&branch.id)
                .map_err(|e| e.to_string())?
            {
                return Ok(
                    serde_json::to_value(crate::branches::to_branch_with_workdir(
                        branch,
                        Some(existing.path),
                    ))
                    .unwrap(),
                );
            }

            let repo_slug = crate::branches::resolve_branch_repo_slug(&store, &project, &branch)?;
            let repo_path =
                crate::git::ensure_local_clone(&repo_slug).map_err(|e| e.to_string())?;
            crate::git::fetch_for_worktree(
                &repo_path,
                &repo_slug,
                &branch.branch_name,
                &branch.base_branch,
            )
            .map_err(|e| e.to_string())?;
            let desired_worktree_path = crate::git::project_worktree_path_for(
                &branch.project_id,
                &repo_slug,
                &branch.branch_name,
            )
            .map_err(|e| e.to_string())?;

            let worktree_str = crate::branches::create_and_link_worktree(
                &store,
                &branch,
                &repo_path,
                &desired_worktree_path,
            )?;

            Ok(
                serde_json::to_value(crate::branches::to_branch_with_workdir(
                    branch,
                    Some(worktree_str),
                ))
                .unwrap(),
            )
        }
        "setup_worktree_and_run_prerun" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;

            // Reuse setup_worktree logic inline
            let branch = store
                .get_branch(&branch_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
            let project = store
                .get_project(&branch.project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

            let worktree_str = if let Some(existing) = store
                .get_workdir_for_branch(&branch.id)
                .map_err(|e| e.to_string())?
            {
                existing.path
            } else {
                let repo_slug =
                    crate::branches::resolve_branch_repo_slug(&store, &project, &branch)?;
                let repo_path =
                    crate::git::ensure_local_clone(&repo_slug).map_err(|e| e.to_string())?;
                crate::git::fetch_for_worktree(
                    &repo_path,
                    &repo_slug,
                    &branch.branch_name,
                    &branch.base_branch,
                )
                .map_err(|e| e.to_string())?;
                let desired_worktree_path = crate::git::project_worktree_path_for(
                    &branch.project_id,
                    &repo_slug,
                    &branch.branch_name,
                )
                .map_err(|e| e.to_string())?;
                crate::branches::create_and_link_worktree(
                    &store,
                    &branch,
                    &repo_path,
                    &desired_worktree_path,
                )?
            };

            let result = crate::branches::to_branch_with_workdir(branch, Some(worktree_str));

            // Run prerun actions if we win the setup-complete race
            match store.mark_branch_setup_complete(&branch_id) {
                Ok(true) => {
                    match crate::branches::run_prerun_actions_for_branch(
                        &store,
                        app_handle,
                        &branch_id,
                        action_executor,
                        action_registry,
                        provider.as_deref(),
                    )
                    .await
                    {
                        Ok(count) => {
                            log::info!("[setup_worktree_and_run_prerun] ran {count} prerun actions")
                        }
                        Err(e) => {
                            log::warn!("[setup_worktree_and_run_prerun] prerun actions failed: {e}")
                        }
                    }
                }
                Ok(false) => {
                    log::info!("[setup_worktree_and_run_prerun] branch {} already setup complete, skipping prerun", branch_id);
                }
                Err(e) => {
                    log::warn!(
                        "[setup_worktree_and_run_prerun] failed to mark setup complete: {e}"
                    );
                }
            }

            Ok(serde_json::to_value(result).unwrap())
        }
        "setup_worktree_from_pr" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let pr_number: u64 = arg(&args, "prNumber")?;
            let head_ref: String = arg(&args, "headRef")?;
            let base_ref: String = arg(&args, "baseRef")?;
            let project_repo_id: Option<String> = opt_arg(&args, "projectRepoId")?;

            let target_repo = match project_repo_id {
                Some(repo_id) => store
                    .get_project_repo(&repo_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("Project repo not found: {repo_id}"))?,
                None => store
                    .get_primary_project_repo(&project_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("Project '{project_id}' has no repository attached"))?,
            };

            let repo_path = crate::git::ensure_local_clone(&target_repo.github_repo)
                .map_err(|e| e.to_string())?;
            let desired_worktree_path = crate::git::project_worktree_path_for(
                &project_id,
                &target_repo.github_repo,
                &head_ref,
            )
            .map_err(|e| e.to_string())?;

            let (worktree_path, branch_name, base_branch) =
                crate::git::create_worktree_from_pr_at_path(
                    &repo_path,
                    pr_number,
                    &head_ref,
                    &base_ref,
                    &desired_worktree_path,
                )
                .map_err(|e| e.to_string())?;

            let worktree_str = worktree_path
                .to_str()
                .ok_or("Invalid worktree path")?
                .to_string();

            let branch = store::Branch::new(&project_id, &branch_name, &base_branch)
                .with_project_repo(&target_repo.id)
                .with_pr(pr_number);
            store.create_branch(&branch).map_err(|e| e.to_string())?;

            let workdir = store::Workdir::new(&project_id, &worktree_str).with_branch(&branch.id);
            store.create_workdir(&workdir).map_err(|e| e.to_string())?;

            Ok(
                serde_json::to_value(crate::branches::to_branch_with_workdir(
                    branch,
                    Some(worktree_str),
                ))
                .unwrap(),
            )
        }
        "create_remote_branch" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let branch_name: String = arg(&args, "branchName")?;
            let base_branch: Option<String> = opt_arg(&args, "baseBranch")?;
            let workspace_name: String = arg(&args, "workspaceName")?;
            let project_repo_id: Option<String> = opt_arg(&args, "projectRepoId")?;

            let project = store
                .get_project(&project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {project_id}"))?;
            let resolved_workspace_name = crate::branches::resolve_project_workspace_name(
                &store,
                &project,
                Some(&workspace_name),
            )?;

            let target_repo = match project_repo_id {
                Some(repo_id) => store
                    .get_project_repo(&repo_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("Project repo not found: {repo_id}"))?,
                None => store
                    .get_primary_project_repo(&project_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("Project '{project_id}' has no repository attached"))?,
            };
            let effective_base = match base_branch {
                Some(b) => b,
                None => crate::git::detect_default_branch_for_repo(&target_repo.github_repo)
                    .map_err(|e| e.to_string())?,
            };
            let effective_base = if effective_base.starts_with("origin/") {
                effective_base
            } else {
                format!("origin/{effective_base}")
            };

            let branch = store::Branch::new_remote(
                &project_id,
                &branch_name,
                &effective_base,
                &resolved_workspace_name,
            )
            .with_project_repo(&target_repo.id);
            store.create_branch(&branch).map_err(|e| e.to_string())?;

            Ok(
                serde_json::to_value(crate::branches::to_branch_with_workdir(branch, None))
                    .unwrap(),
            )
        }
        "start_workspace" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;

            let branch = store
                .get_branch(&branch_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
            let is_first_start = branch.workspace_status == Some(store::WorkspaceStatus::Starting);

            let project = store
                .get_project(&branch.project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

            let ws_name = branch
                .workspace_name
                .as_deref()
                .ok_or("Branch has no workspace name")?;
            let repo_subpath = crate::branches::resolve_branch_workspace_subpath(&store, &branch)?;
            let ref_name = crate::branches::normalize_branch_ref(&branch.base_branch);
            let repo_slug = crate::branches::resolve_branch_repo_slug(&store, &project, &branch)?;

            // Pre-flight auth check
            if let Err(e) = crate::branches::run_blox_blocking(crate::blox::check_auth).await {
                store
                    .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
                    .ok();
                return Err(e.to_string());
            }

            // Secondary repo setup in an already-running shared workspace
            if let Some(repo_subpath) = repo_subpath.as_deref() {
                if let Ok(info) = crate::branches::run_blox_blocking({
                    let ws_name = ws_name.to_string();
                    move || crate::blox::ws_info(&ws_name)
                })
                .await
                {
                    let ws_status = info
                        .status
                        .as_deref()
                        .map(|s| s.to_ascii_lowercase())
                        .unwrap_or_default();
                    if let Some(ws_id) = info.workstation_id {
                        if let Ok(mut cache) = crate::branches::workstation_id_cache().lock() {
                            cache.insert(ws_name.to_string(), ws_id);
                        }
                    }
                    if ws_status == "running" {
                        crate::branches::clone_repo_into_workspace(
                            ws_name,
                            repo_subpath,
                            &repo_slug,
                            &ref_name,
                            &branch.branch_name,
                        )
                        .await?;
                        store
                            .update_branch_workspace_status(
                                &branch_id,
                                &store::WorkspaceStatus::Running,
                            )
                            .map_err(|e| e.to_string())?;
                        if is_first_start {
                            let store_bg = Arc::clone(&store);
                            let app_handle_bg = app_handle.clone();
                            let branch_id_bg = branch_id.clone();
                            tokio::spawn(async move {
                                crate::maybe_trigger_auto_review_for_new_repo(
                                    &store_bg,
                                    &app_handle_bg,
                                    &branch_id_bg,
                                    None,
                                )
                                .await;
                            });
                        }
                        return Ok(Value::Null);
                    }
                }
            }

            let resolved_source = Some(format!(
                "https://github.com/{}.git?ref={}",
                repo_slug, ref_name
            ));
            log::info!(
                "[start_workspace] branch={} workspace={} starting",
                branch_id,
                ws_name
            );
            let ws_start_started_at = std::time::Instant::now();

            match crate::branches::run_blox_blocking({
                let ws_name = ws_name.to_string();
                let source = resolved_source.clone();
                move || {
                    crate::blox::ws_start(
                        &ws_name,
                        source.as_deref(),
                        Some(crate::branches::WORKSPACE_IDLE_TIMEOUT_MINUTES),
                    )
                }
            })
            .await
            {
                Ok(_) => {
                    log::info!(
                        "[start_workspace] branch={} workspace={} ws_start completed elapsed_ms={}",
                        branch_id,
                        ws_name,
                        ws_start_started_at.elapsed().as_millis()
                    );
                    let has_remote_branch = crate::branches::run_workspace_git_async(
                        ws_name,
                        repo_subpath.as_deref(),
                        &["fetch", "origin", &branch.branch_name],
                    )
                    .await
                    .is_ok();

                    let remote_ref = format!("origin/{}", branch.branch_name);
                    let checkout_result = if has_remote_branch {
                        crate::branches::run_workspace_git_async(
                            ws_name,
                            repo_subpath.as_deref(),
                            &["checkout", "-B", &branch.branch_name, &remote_ref],
                        )
                        .await
                    } else {
                        crate::branches::run_workspace_git_async(
                            ws_name,
                            repo_subpath.as_deref(),
                            &["checkout", "-b", &branch.branch_name],
                        )
                        .await
                    };
                    if let Err(e) = checkout_result {
                        log::warn!(
                            "failed to create branch '{}' in workspace '{}': {e}",
                            branch.branch_name,
                            ws_name
                        );
                    }

                    if is_first_start {
                        let store_bg = Arc::clone(&store);
                        let app_handle_bg = app_handle.clone();
                        let branch_id_bg = branch_id.clone();
                        tokio::spawn(async move {
                            crate::maybe_trigger_auto_review_for_new_repo(
                                &store_bg,
                                &app_handle_bg,
                                &branch_id_bg,
                                None,
                            )
                            .await;
                        });
                    }
                    Ok(Value::Null)
                }
                Err(crate::blox::BloxError::NotAuthenticated) => {
                    store
                        .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
                        .ok();
                    Err("Not authenticated with Blox. Run: sq login".to_string())
                }
                Err(e) => {
                    if crate::branches::is_blox_onboarding_precondition_error(&e) {
                        store
                            .update_branch_workspace_status(
                                &branch_id,
                                &store::WorkspaceStatus::Error,
                            )
                            .ok();
                        return Err(e.to_string());
                    }
                    log::warn!(
                        "blox ws start failed for '{}', leaving status as Starting for polling to resolve: {e}",
                        ws_name
                    );
                    Ok(Value::Null)
                }
            }
        }
        "resume_workspace" => {
            let store = get_store(store_mutex)?;
            let workspace_name: String = arg(&args, "workspaceName")?;

            let branch_ids = store
                .update_workspace_status_by_workspace_name(
                    &workspace_name,
                    &store::WorkspaceStatus::Starting,
                )
                .map_err(|e| e.to_string())?;

            if branch_ids.is_empty() {
                return Err(format!("No branches found for workspace: {workspace_name}"));
            }

            let ws = workspace_name.clone();
            if let Err(e) =
                crate::branches::run_blox_blocking(move || crate::blox::ws_resume(&ws)).await
            {
                log::warn!(
                    "[resume_workspace] workspace={} resume failed: {}",
                    workspace_name,
                    e
                );
                store
                    .update_workspace_status_by_workspace_name(
                        &workspace_name,
                        &store::WorkspaceStatus::Error,
                    )
                    .ok();
                return Err(e.to_string());
            }

            Ok(serde_json::to_value(branch_ids).unwrap())
        }

        // =====================================================================
        // Actions (repo actions CRUD)
        // =====================================================================
        "list_project_actions" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let project_repo_id: Option<String> = opt_arg(&args, "projectRepoId")?;
            let context = if let Some(repo_id) = project_repo_id {
                let repo = store
                    .get_project_repo(&repo_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("Project repo not found: {repo_id}"))?;
                store
                    .get_or_create_action_context(&repo.github_repo, repo.subpath.as_deref())
                    .map_err(|e| e.to_string())?
            } else {
                let project = store
                    .get_project(&project_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("Project not found: {project_id}"))?;
                let repo = project
                    .primary_repo()
                    .ok_or("Project has no repository attached")?;
                store
                    .get_or_create_action_context(repo, project.subpath.as_deref())
                    .map_err(|e| e.to_string())?
            };
            let actions = store
                .list_repo_actions(&context.id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(actions).unwrap())
        }
        "update_project_action" => {
            let store = get_store(store_mutex)?;
            let action_id: String = arg(&args, "actionId")?;
            let name: String = arg(&args, "name")?;
            let command_str: String = arg(&args, "command")?;
            let action_type: String = arg(&args, "actionType")?;
            let sort_order: i32 = arg(&args, "sortOrder")?;
            let auto_commit: bool = arg(&args, "autoCommit")?;
            let action = store
                .get_repo_action(&action_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Action not found: {action_id}"))?;
            let updated = crate::store::models::RepoAction {
                id: action.id,
                context_id: action.context_id,
                name,
                command: command_str,
                action_type: builderbot_actions::ActionType::parse(&action_type)
                    .ok_or_else(|| format!("Invalid action type: {action_type}"))?,
                sort_order,
                auto_commit,
                run_detection_mode: action.run_detection_mode,
                created_at: action.created_at,
                updated_at: crate::store::now_timestamp(),
            };
            store
                .update_repo_action(&updated)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "delete_project_action" => {
            let store = get_store(store_mutex)?;
            let action_id: String = arg(&args, "actionId")?;
            store
                .delete_repo_action(&action_id)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "list_action_contexts" => {
            let store = get_store(store_mutex)?;
            let contexts = store.list_action_contexts().map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(contexts).unwrap())
        }
        "list_repo_actions" => {
            let store = get_store(store_mutex)?;
            let github_repo: String = arg(&args, "githubRepo")?;
            let subpath: Option<String> = opt_arg(&args, "subpath")?;
            let context = store
                .get_or_create_action_context(&github_repo, subpath.as_deref())
                .map_err(|e| e.to_string())?;
            let actions = store
                .list_repo_actions(&context.id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(actions).unwrap())
        }
        "create_repo_action" => {
            let store = get_store(store_mutex)?;
            let github_repo: String = arg(&args, "githubRepo")?;
            let subpath: Option<String> = opt_arg(&args, "subpath")?;
            let name: String = arg(&args, "name")?;
            let command_str: String = arg(&args, "command")?;
            let action_type: String = arg(&args, "actionType")?;
            let sort_order: i32 = arg(&args, "sortOrder")?;
            let auto_commit: bool = arg(&args, "autoCommit")?;
            let context = store
                .get_or_create_action_context(&github_repo, subpath.as_deref())
                .map_err(|e| e.to_string())?;
            let parsed_type = builderbot_actions::ActionType::parse(&action_type)
                .ok_or_else(|| format!("Invalid action type: {action_type}"))?;
            let action = crate::store::models::RepoAction::new(
                context.id,
                name,
                command_str,
                parsed_type,
                sort_order,
            )
            .with_auto_commit(auto_commit);
            store
                .create_repo_action(&action)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(action).unwrap())
        }
        "delete_all_repo_actions" => {
            let store = get_store(store_mutex)?;
            let context_id: String = arg(&args, "contextId")?;
            store
                .delete_all_repo_actions(&context_id)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "delete_action_context" => {
            let store = get_store(store_mutex)?;
            let context_id: String = arg(&args, "contextId")?;
            store
                .delete_action_context(&context_id)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }

        // =====================================================================
        // Action execution commands
        // =====================================================================
        "detect_repo_actions" => {
            let store = get_store(store_mutex)?;
            let github_repo: String = arg(&args, "githubRepo")?;
            let subpath: Option<String> = opt_arg(&args, "subpath")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let actions = crate::actions::commands::detect_repo_actions_impl(
                github_repo,
                subpath,
                provider,
                app_handle.clone(),
                store,
            )
            .await?;
            Ok(serde_json::to_value(actions).unwrap())
        }
        "run_branch_action" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let action_id: String = arg(&args, "actionId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let execution_id = crate::actions::commands::run_branch_action_impl(
                branch_id,
                action_id,
                provider,
                app_handle.clone(),
                store,
                Arc::clone(action_executor),
                Arc::clone(action_registry),
            )
            .await?;
            Ok(serde_json::to_value(execution_id).unwrap())
        }
        "stop_branch_action" => {
            let execution_id: String = arg(&args, "executionId")?;
            crate::actions::commands::stop_branch_action_impl(execution_id, action_executor)?;
            Ok(Value::Null)
        }
        "get_running_branch_actions" => {
            let branch_id: String = arg(&args, "branchId")?;
            let actions = crate::actions::commands::get_running_branch_actions_impl(
                branch_id,
                action_executor,
                action_registry,
            )?;
            Ok(serde_json::to_value(actions).unwrap())
        }
        "get_action_output_buffer" => {
            let execution_id: String = arg(&args, "executionId")?;
            let output = crate::actions::commands::get_action_output_buffer_impl(
                execution_id,
                action_executor,
            )?;
            Ok(serde_json::to_value(output).unwrap())
        }
        "clear_action_execution" => {
            let execution_id: String = arg(&args, "executionId")?;
            let cleared = crate::actions::commands::clear_action_execution_impl(
                execution_id,
                action_executor,
            )?;
            Ok(serde_json::to_value(cleared).unwrap())
        }
        "run_prerun_actions" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let execution_ids = crate::actions::commands::run_prerun_actions_impl(
                branch_id,
                provider,
                app_handle.clone(),
                store,
                Arc::clone(action_executor),
                Arc::clone(action_registry),
            )
            .await?;
            Ok(serde_json::to_value(execution_ids).unwrap())
        }
        "get_run_phase" => {
            let execution_id: String = arg(&args, "executionId")?;
            let phase =
                crate::actions::commands::get_run_phase_impl(action_registry, execution_id)?;
            Ok(serde_json::to_value(phase).unwrap())
        }
        "update_run_detection_mode" => {
            let store = get_store(store_mutex)?;
            let action_id: String = arg(&args, "actionId")?;
            let mode: builderbot_actions::RunDetectionMode = arg(&args, "mode")?;
            crate::actions::commands::update_run_detection_mode_impl(store, action_id, mode)?;
            Ok(Value::Null)
        }

        // =====================================================================
        // Timeline
        // =====================================================================
        "get_branch_timeline" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let timeline = tauri::async_runtime::spawn_blocking(move || {
                crate::timeline::build_branch_timeline_public(&store, &branch_id)
            })
            .await
            .map_err(|e| format!("Timeline task failed: {e}"))??;
            Ok(serde_json::to_value(timeline).unwrap())
        }

        // =====================================================================
        // Notes
        // =====================================================================
        "create_note" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let title: String = arg(&args, "title")?;
            let content: String = arg(&args, "content")?;
            let note = crate::store::models::Note::new(&branch_id, &title, &content);
            store.create_note(&note).map_err(|e| e.to_string())?;
            let item = crate::NoteTimelineItem {
                id: note.id,
                title: note.title,
                content: note.content,
                session_id: None,
                session_status: None,
                completion_reason: None,
                created_at: note.created_at,
                updated_at: note.updated_at,
                completed_at: note.completed_at,
                suggested_next_commit_step: None,
                suggested_next_note_step: None,
            };
            Ok(serde_json::to_value(item).unwrap())
        }
        "delete_note" => {
            let store = get_store(store_mutex)?;
            let note_id: String = arg(&args, "noteId")?;
            let delete_session_flag: Option<bool> = opt_arg(&args, "deleteSession")?;
            let note = store
                .get_note(&note_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Note not found: {note_id}"))?;
            store.delete_note(&note_id).map_err(|e| e.to_string())?;
            if delete_session_flag.unwrap_or(false) {
                if let Some(sid) = note.session_id {
                    let _ = store.delete_session(&sid);
                }
            }
            Ok(Value::Null)
        }
        "create_project_note" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let title: String = arg(&args, "title")?;
            let content: String = arg(&args, "content")?;
            let note = crate::store::ProjectNote::new(&project_id, &title, &content);
            store
                .create_project_note(&note)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(note).unwrap())
        }
        "list_project_notes" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let notes = store
                .list_project_notes_with_status(&project_id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(notes).unwrap())
        }
        "delete_project_note" => {
            let store = get_store(store_mutex)?;
            let note_id: String = arg(&args, "noteId")?;
            let note = store
                .get_project_note(&note_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project note not found: {note_id}"))?;
            store
                .delete_project_note(&note_id)
                .map_err(|e| e.to_string())?;
            if let Some(sid) = note.session_id {
                let _ = store.delete_session(&sid);
            }
            Ok(Value::Null)
        }

        // =====================================================================
        // Images
        // =====================================================================
        "create_image" => Err("create_image is not available in web mode".to_string()),
        "create_image_from_data" => {
            let store = get_store(store_mutex)?;
            let branch_id: Option<String> = opt_arg(&args, "branchId")?;
            let project_id: String = arg(&args, "projectId")?;
            let filename: String = arg(&args, "filename")?;
            let mime_type: String = arg(&args, "mimeType")?;
            let data: String = arg(&args, "data")?;
            let pending: Option<bool> = opt_arg(&args, "pending")?;
            let image = crate::image_commands::create_image_from_data_impl(
                store, branch_id, project_id, filename, mime_type, data, pending,
            )?;
            Ok(serde_json::to_value(image).unwrap())
        }
        "get_image_path" => {
            let store = get_store(store_mutex)?;
            let image_id: String = arg(&args, "imageId")?;
            let image = store
                .get_image(&image_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Image not found: {image_id}"))?;
            let path = crate::store::images::image_file_path(
                &image.project_id,
                &image.id,
                &image.filename,
            )
            .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(path.to_string_lossy().to_string()).unwrap())
        }
        "get_image_data" => {
            let store = get_store(store_mutex)?;
            let image_id: String = arg(&args, "imageId")?;
            let image = store
                .get_image(&image_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Image not found: {image_id}"))?;
            let path = crate::store::images::image_file_path(
                &image.project_id,
                &image.id,
                &image.filename,
            )
            .map_err(|e| e.to_string())?;
            let bytes = std::fs::read(&path).map_err(|e| format!("Failed to read image: {e}"))?;
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let data_url = format!("data:{};base64,{}", image.mime_type, encoded);
            Ok(serde_json::to_value(data_url).unwrap())
        }
        "delete_image" => {
            let store = get_store(store_mutex)?;
            let image_id: String = arg(&args, "imageId")?;
            let image = store
                .get_image(&image_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Image not found: {image_id}"))?;
            store.delete_image(&image_id).map_err(|e| e.to_string())?;
            if let Ok(path) =
                crate::store::images::image_file_path(&image.project_id, &image.id, &image.filename)
            {
                let _ = std::fs::remove_file(&path);
            }
            Ok(Value::Null)
        }
        "list_branch_images" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let images = store
                .list_images_for_branch(&branch_id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(images).unwrap())
        }

        // =====================================================================
        // Timeline delete commands
        // =====================================================================
        "delete_review" => {
            let store = get_store(store_mutex)?;
            let review_id: String = arg(&args, "reviewId")?;
            let delete_session_flag: Option<bool> = opt_arg(&args, "deleteSession")?;
            let review = store
                .get_review(&review_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Review not found: {review_id}"))?;
            store.delete_review(&review_id).map_err(|e| e.to_string())?;
            if delete_session_flag.unwrap_or(false) {
                if let Some(sid) = review.session_id {
                    let _ = store.delete_session(&sid);
                }
            }
            Ok(Value::Null)
        }
        "delete_pending_commit" => {
            let store = get_store(store_mutex)?;
            let commit_id: String = arg(&args, "commitId")?;
            let delete_session_flag: Option<bool> = opt_arg(&args, "deleteSession")?;
            let commit = store
                .get_commit(&commit_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Commit not found: {commit_id}"))?;
            if commit.sha.is_some() {
                return Err("Cannot use delete_pending_commit for commits with a SHA".to_string());
            }
            crate::timeline::cleanup_reviews_after_commit(&store, session_registry, &commit);
            store.delete_commit(&commit_id).map_err(|e| e.to_string())?;
            if delete_session_flag.unwrap_or(false) {
                if let Some(sid) = commit.session_id {
                    let _ = store.delete_session(&sid);
                }
            }
            Ok(Value::Null)
        }
        "delete_commit" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let commit_sha: String = arg(&args, "commitSha")?;
            let delete_session_flag: Option<bool> = opt_arg(&args, "deleteSession")?;
            let registry = Arc::clone(session_registry);
            crate::timeline::delete_commit_impl(
                registry,
                store,
                branch_id,
                commit_sha,
                delete_session_flag,
            )
            .await?;
            Ok(Value::Null)
        }

        // =====================================================================
        // Diff commands
        // =====================================================================
        "get_diff_files" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let commit_sha: Option<String> = opt_arg(&args, "commitSha")?;
            let scope: String = arg(&args, "scope")?;
            let response =
                crate::diff_commands::get_diff_files_impl(store, branch_id, commit_sha, scope)?;
            Ok(serde_json::to_value(response).unwrap())
        }
        "get_file_diff" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let commit_sha: String = arg(&args, "commitSha")?;
            let scope: String = arg(&args, "scope")?;
            let path: String = arg(&args, "path")?;
            let result = crate::diff_commands::get_file_diff_impl(
                store, branch_id, commit_sha, scope, path,
            )?;
            Ok(serde_json::to_value(result).unwrap())
        }
        "get_file_at_ref" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let ref_name: String = arg(&args, "refName")?;
            let path: String = arg(&args, "path")?;
            let file =
                crate::diff_commands::get_file_at_ref_impl(store, branch_id, ref_name, path)?;
            Ok(serde_json::to_value(file).unwrap())
        }

        // =====================================================================
        // Review commands
        // =====================================================================
        "ensure_review" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let commit_sha: String = arg(&args, "commitSha")?;
            let scope: String = arg(&args, "scope")?;
            let review_scope = crate::store::ReviewScope::parse(&scope)
                .ok_or_else(|| format!("Invalid scope: {scope}"))?;
            let review = store
                .ensure_review(&branch_id, &commit_sha, review_scope)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(review).unwrap())
        }
        "find_review" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let commit_sha: String = arg(&args, "commitSha")?;
            let scope: String = arg(&args, "scope")?;
            let review_scope = crate::store::ReviewScope::parse(&scope)
                .ok_or_else(|| format!("Invalid scope: {scope}"))?;
            let review = store
                .find_review(&branch_id, &commit_sha, review_scope)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(review).unwrap())
        }
        "get_review" => {
            let store = get_store(store_mutex)?;
            let review_id: String = arg(&args, "reviewId")?;
            let review = store.get_review(&review_id).map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(review).unwrap())
        }
        "mark_reviewed" => {
            let store = get_store(store_mutex)?;
            let review_id: String = arg(&args, "reviewId")?;
            let path: String = arg(&args, "path")?;
            store
                .mark_reviewed(&review_id, &path)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "unmark_reviewed" => {
            let store = get_store(store_mutex)?;
            let review_id: String = arg(&args, "reviewId")?;
            let path: String = arg(&args, "path")?;
            store
                .unmark_reviewed(&review_id, &path)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "add_comment" => {
            let store = get_store(store_mutex)?;
            let review_id: String = arg(&args, "reviewId")?;
            let path: String = arg(&args, "path")?;
            let span_start: u32 = arg(&args, "spanStart")?;
            let span_end: u32 = arg(&args, "spanEnd")?;
            let content: String = arg(&args, "content")?;
            let comment = crate::store::Comment::new(
                &path,
                crate::git::Span::new(span_start, span_end),
                &content,
            );
            store
                .add_comment(&review_id, &comment)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(comment).unwrap())
        }
        "update_comment" => {
            let store = get_store(store_mutex)?;
            let comment_id: String = arg(&args, "commentId")?;
            let content: String = arg(&args, "content")?;
            store
                .update_comment(&comment_id, &content)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "delete_comment" => {
            let store = get_store(store_mutex)?;
            let comment_id: String = arg(&args, "commentId")?;
            store
                .delete_comment(&comment_id)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "delete_all_comments" => {
            let store = get_store(store_mutex)?;
            let review_id: String = arg(&args, "reviewId")?;
            store
                .delete_all_comments(&review_id)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "restore_comment" => {
            let store = get_store(store_mutex)?;
            let comment_id: String = arg(&args, "commentId")?;
            store
                .restore_comment(&comment_id)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "get_deleted_comments" => {
            let store = get_store(store_mutex)?;
            let review_id: String = arg(&args, "reviewId")?;
            let comments = store
                .get_deleted_comments(&review_id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(comments).unwrap())
        }
        "add_reference_file" => {
            let store = get_store(store_mutex)?;
            let review_id: String = arg(&args, "reviewId")?;
            let path: String = arg(&args, "path")?;
            store
                .add_reference_file(&review_id, &path)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "remove_reference_file" => {
            let store = get_store(store_mutex)?;
            let review_id: String = arg(&args, "reviewId")?;
            let path: String = arg(&args, "path")?;
            store
                .remove_reference_file(&review_id, &path)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }

        // =====================================================================
        // Session commands
        // =====================================================================
        "discover_acp_providers" => {
            let providers = tokio::task::spawn_blocking(crate::agent::discover_providers)
                .await
                .unwrap_or_default();
            Ok(serde_json::to_value(providers).unwrap())
        }
        "get_session" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let session = store.get_session(&session_id).map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(session).unwrap())
        }
        "get_session_messages" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let messages = store
                .get_session_messages(&session_id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(messages).unwrap())
        }
        "get_session_messages_since" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let since_id: i64 = arg(&args, "sinceId")?;
            let messages = store
                .get_session_messages_since(&session_id, since_id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(messages).unwrap())
        }
        "start_session" => {
            let store = get_store(store_mutex)?;
            let prompt: String = arg(&args, "prompt")?;
            let working_dir: String = arg(&args, "workingDir")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;

            let working_dir = std::path::PathBuf::from(working_dir);
            let mut session = store::Session::new_running(&prompt, &working_dir);
            if let Some(ref p) = provider {
                session = session.with_provider(p);
            }
            store.create_session(&session).map_err(|e| e.to_string())?;

            session_runner::start_session(
                SessionConfig {
                    session_id: session.id.clone(),
                    prompt,
                    working_dir,
                    agent_session_id: None,
                    pre_head_sha: None,
                    provider,
                    workspace_name: None,
                    extra_env: vec![],
                    mcp_project_id: None,
                    action_executor: None,
                    action_registry: None,
                    remote_working_dir: None,
                    image_ids: vec![],
                    branch_id: None,
                    project_id: None,
                },
                store,
                app_handle.clone(),
                Arc::clone(session_registry),
            )?;

            Ok(serde_json::to_value(session).unwrap())
        }
        "resume_session" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let prompt: String = arg(&args, "prompt")?;
            let image_ids: Option<Vec<String>> = opt_arg(&args, "imageIds")?;
            let branch_id: Option<String> = opt_arg(&args, "branchId")?;

            let session = store
                .get_session(&session_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Session not found: {session_id}"))?;

            let provider = session.provider.clone();
            let agent_session_id = session.agent_id.clone();
            let working_dir = std::path::PathBuf::from(&session.working_dir);

            let project_note = store
                .get_project_note_by_session(&session_id)
                .ok()
                .flatten();
            let mcp_project_id = project_note.as_ref().map(|note| note.project_id.clone());
            let linked_commit = store.get_commit_by_session(&session_id).ok().flatten();
            let linked_note = store.get_note_by_session(&session_id).ok().flatten();
            let linked_review = store.get_review_by_session(&session_id).ok().flatten();

            let branch_from_id = branch_id
                .as_deref()
                .and_then(|bid| store.get_branch(bid).ok().flatten());

            let linked_branch = if branch_from_id.is_some() {
                branch_from_id.clone()
            } else if let Some(commit) = &linked_commit {
                store.get_branch(&commit.branch_id).ok().flatten()
            } else if let Some(note) = &linked_note {
                store.get_branch(&note.branch_id).ok().flatten()
            } else if let Some(review) = &linked_review {
                store.get_branch(&review.branch_id).ok().flatten()
            } else {
                None
            };

            let session_type = if project_note.is_some() {
                Some("note".to_string())
            } else if linked_commit.is_some() {
                Some("commit".to_string())
            } else if linked_note.is_some() {
                Some("note".to_string())
            } else if linked_review.is_some() {
                Some("review".to_string())
            } else {
                session_commands::infer_branch_resume_session_type(&session.prompt)
                    .map(str::to_string)
            };
            let event_branch_id = linked_branch.as_ref().map(|branch| branch.id.clone());
            let event_project_id = if let Some(note) = &project_note {
                Some(note.project_id.clone())
            } else {
                linked_branch
                    .as_ref()
                    .map(|branch| branch.project_id.clone())
            };

            let (pre_head_sha, workspace_name) = {
                if let Some(ref branch) = linked_branch {
                    let ws_name = branch.workspace_name.clone();
                    let head = if linked_commit.is_some() {
                        if let Some(ref ws) = ws_name {
                            let ws = ws.clone();
                            session_commands::run_blox_blocking(move || {
                                crate::blox::ws_exec(&ws, &["git", "rev-parse", "HEAD"])
                            })
                            .await
                            .map(|s| s.trim().to_string())
                            .ok()
                        } else {
                            crate::git::get_head_sha(&working_dir).ok()
                        }
                    } else {
                        None
                    };
                    (head, ws_name)
                } else {
                    (None, None)
                }
            };

            let remote_working_dir = if let Some(ref branch) = branch_from_id {
                if branch.workspace_name.is_some() {
                    let ws_name = branch.workspace_name.as_deref().unwrap().to_string();
                    let store_for_resolve = Arc::clone(&store);
                    let branch_for_resolve = branch.clone();
                    match tokio::task::spawn_blocking(move || {
                        crate::branches::resolve_branch_workspace_subpath(
                            &store_for_resolve,
                            &branch_for_resolve,
                        )
                        .ok()
                        .flatten()
                        .and_then(|subpath| {
                            crate::branches::resolve_workspace_repo_path(&ws_name, &subpath).ok()
                        })
                    })
                    .await
                    {
                        Ok(Some(path)) => Some(std::path::PathBuf::from(path)),
                        _ => None,
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let transitioned = store
                .transition_to_running(&session_id)
                .map_err(|e| e.to_string())?;
            if !transitioned {
                return Err("Session is already running".to_string());
            }

            let config_branch_id = event_branch_id.clone();
            let config_project_id = event_project_id.clone().or(mcp_project_id.clone());

            emit_to_all(
                app_handle,
                "session-status-changed",
                session_runner::SessionStatusEvent {
                    session_id: session_id.clone(),
                    status: "running".to_string(),
                    error_message: None,
                    completion_reason: None,
                    branch_id: event_branch_id,
                    project_id: event_project_id.or(mcp_project_id.clone()),
                    session_type,
                    is_auto_review: false,
                },
            );

            session_runner::start_session(
                SessionConfig {
                    session_id,
                    prompt,
                    working_dir,
                    agent_session_id,
                    pre_head_sha,
                    provider,
                    workspace_name,
                    extra_env: vec![],
                    mcp_project_id: mcp_project_id.clone(),
                    action_executor: if mcp_project_id.is_some() {
                        Some(Arc::clone(action_executor))
                    } else {
                        None
                    },
                    action_registry: if mcp_project_id.is_some() {
                        Some(Arc::clone(action_registry))
                    } else {
                        None
                    },
                    remote_working_dir,
                    image_ids: image_ids.unwrap_or_default(),
                    branch_id: config_branch_id,
                    project_id: config_project_id,
                },
                store,
                app_handle.clone(),
                Arc::clone(session_registry),
            )?;

            Ok(Value::Null)
        }
        "start_branch_session" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let prompt: String = arg(&args, "prompt")?;
            let session_type: session_commands::BranchSessionType = arg(&args, "sessionType")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let image_ids: Option<Vec<String>> = opt_arg(&args, "imageIds")?;
            let launch_context: Option<session_commands::BranchSessionLaunchContext> =
                opt_arg(&args, "launchContext")?;

            if matches!(
                session_type,
                session_commands::BranchSessionType::Commit
                    | session_commands::BranchSessionType::Review
            ) {
                session_commands::cancel_in_flight_auto_review_for_branch(
                    &store,
                    session_registry,
                    &branch_id,
                )?;
            }

            let branch = store
                .get_branch(&branch_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

            let project = store
                .get_project(&branch.project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

            let is_remote = branch.workspace_name.is_some();

            let (working_dir, branch_context) = if is_remote {
                let fallback_dir =
                    session_commands::resolve_branch_repo_slug(&store, &project, &branch)
                        .and_then(|repo| crate::paths::repos_dir().map(|d| d.join(repo)))
                        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
                let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
                let base_branch = branch.base_branch.clone();
                let store_for_context = Arc::clone(&store);
                let branch_id_for_context = branch_id.clone();
                let project_id_for_context = branch.project_id.clone();
                let ctx = tokio::task::spawn_blocking(move || {
                    session_commands::build_remote_branch_context(
                        &workspace_name,
                        &base_branch,
                        &store_for_context,
                        &branch_id_for_context,
                        &project_id_for_context,
                    )
                })
                .await
                .map_err(|e| format!("Failed to build remote branch context: {e}"))?;
                (fallback_dir, ctx)
            } else {
                let workdir = store
                    .get_workdir_for_branch(&branch_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

                let mut worktree_path = std::path::PathBuf::from(&workdir.path);
                let effective_subpath = if let Some(repo_id) = branch.project_repo_id.as_deref() {
                    store
                        .get_project_repo(repo_id)
                        .ok()
                        .flatten()
                        .and_then(|repo| repo.subpath)
                } else {
                    project.subpath.clone()
                };
                if let Some(ref subpath) = effective_subpath {
                    worktree_path = worktree_path.join(subpath);
                }

                let ctx = session_commands::build_branch_context(
                    &worktree_path,
                    &branch.base_branch,
                    &store,
                    &branch_id,
                    &branch.project_id,
                );
                (worktree_path, ctx)
            };

            let project_information =
                session_commands::build_project_context(&store, &project, &branch);
            let full_prompt = session_commands::build_full_prompt(
                &prompt,
                &project_information,
                &branch_context,
                &session_type,
                launch_context.as_ref(),
                Some(&branch.base_branch),
            );

            let mut session = store::Session::new_running(&full_prompt, &working_dir);
            if let Some(ref p) = provider {
                session = session.with_provider(p);
            }
            store.create_session(&session).map_err(|e| e.to_string())?;

            let session_type_str = match session_type {
                session_commands::BranchSessionType::Commit => "commit",
                session_commands::BranchSessionType::Note => "note",
                session_commands::BranchSessionType::Review => "review",
            };
            session_runner::emit_session_running(
                app_handle,
                &session.id,
                &branch_id,
                &branch.project_id,
                session_type_str,
            );

            let (artifact_id, pre_head_sha) = match session_type {
                session_commands::BranchSessionType::Note => {
                    let note = store::Note::new(&branch_id, &prompt, "").with_session(&session.id);
                    store.create_note(&note).map_err(|e| e.to_string())?;
                    (note.id, None)
                }
                session_commands::BranchSessionType::Commit => {
                    let commit = store::Commit::new_pending(&branch_id).with_session(&session.id);
                    store.create_commit(&commit).map_err(|e| e.to_string())?;
                    let head_sha = if is_remote {
                        let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
                        match session_commands::run_blox_blocking(move || {
                            crate::blox::ws_exec(&workspace_name, &["git", "rev-parse", "HEAD"])
                        })
                        .await
                        {
                            Ok(sha) => Some(sha.trim().to_string()),
                            Err(e) => {
                                log::warn!("Failed to get remote HEAD SHA via ws_exec: {e}");
                                None
                            }
                        }
                    } else {
                        Some(
                            crate::git::get_head_sha(&working_dir)
                                .map_err(|e| format!("Failed to get HEAD SHA: {e}"))?,
                        )
                    };
                    (commit.id, head_sha)
                }
                session_commands::BranchSessionType::Review => {
                    let tip_sha = if is_remote {
                        let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
                        session_commands::run_blox_blocking(move || {
                            crate::blox::ws_exec(&workspace_name, &["git", "rev-parse", "HEAD"])
                        })
                        .await
                        .map(|s| s.trim().to_string())
                        .unwrap_or_else(|_| "unknown".to_string())
                    } else {
                        crate::git::get_head_sha(&working_dir)
                            .map_err(|e| format!("Failed to get HEAD SHA: {e}"))?
                    };

                    let review =
                        store::Review::new(&branch_id, &tip_sha, store::ReviewScope::Branch)
                            .with_session(&session.id);
                    store.create_review(&review).map_err(|e| e.to_string())?;
                    (review.id, None)
                }
            };

            let effective_provider = provider;

            let remote_working_dir = if is_remote {
                let ws_name = branch.workspace_name.as_deref().unwrap().to_string();
                let store_for_resolve = Arc::clone(&store);
                let branch_for_resolve = branch.clone();
                match tokio::task::spawn_blocking(move || {
                    crate::branches::resolve_branch_workspace_subpath(
                        &store_for_resolve,
                        &branch_for_resolve,
                    )
                    .ok()
                    .flatten()
                    .and_then(|subpath| {
                        crate::branches::resolve_workspace_repo_path(&ws_name, &subpath).ok()
                    })
                })
                .await
                {
                    Ok(Some(path)) => Some(std::path::PathBuf::from(path)),
                    _ => None,
                }
            } else {
                None
            };

            session_runner::start_session(
                SessionConfig {
                    session_id: session.id.clone(),
                    prompt: full_prompt,
                    working_dir,
                    agent_session_id: None,
                    pre_head_sha,
                    provider: effective_provider,
                    workspace_name: branch.workspace_name.clone(),
                    extra_env: vec![],
                    mcp_project_id: None,
                    action_executor: None,
                    action_registry: None,
                    remote_working_dir,
                    image_ids: image_ids.unwrap_or_default(),
                    branch_id: Some(branch_id),
                    project_id: Some(branch.project_id.clone()),
                },
                store,
                app_handle.clone(),
                Arc::clone(session_registry),
            )?;

            Ok(
                serde_json::to_value(session_commands::BranchSessionResponse {
                    session_id: session.id,
                    artifact_id,
                })
                .unwrap(),
            )
        }
        "start_project_session" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let prompt: String = arg(&args, "prompt")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let image_ids: Option<Vec<String>> = opt_arg(&args, "imageIds")?;

            let project = store
                .get_project(&project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {project_id}"))?;

            let project_context =
                session_commands::build_project_session_context(&store, &project, None);

            let is_remote = project.location == store::ProjectLocation::Remote;

            let preamble = if is_remote {
                "This top-level project session runs locally and acts as a coordinator. \
For repository-specific execution, use MCP subagent tools.\n\n\
This is a remote-workspace project. Use the project MCP tools to orchestrate work:"
            } else {
                "You have access to the following tools:"
            };

            let start_repo_session_desc = if is_remote {
                "- start_repo_session: Use this to make changes or run tasks in one of the project's \
repositories. It enqueues work and returns a `repo_session_id` immediately. Use \
`expected_outcome=\"note_in_repo\"` for repo notes and `expected_outcome=\"commit\"` for \
code changes/commits. For remote branches this subagent runs on the remote workspace, where \
file access, notes, and commits must happen.\n\
- wait_for_repo_session: Use this to wait on a previously started repo session by passing the \
`repo_session_id`. It returns the queue state (`queued`, `running`, `completed`, `cancelled`, \
or `failed`) and any available artifacts.\n\
- cancel_repo_session: Use this to cancel a queued or running repo session by `repo_session_id`."
            } else {
                "- start_repo_session: Use this to make changes or run tasks in one of the project's \
repositories. It enqueues work and returns a `repo_session_id` immediately. Use \
`expected_outcome=\"note_in_repo\"` for repo notes and `expected_outcome=\"commit\"` for \
code changes/commits. Do not ask for both a note and a commit in a single start_repo_session \
request — choose one outcome per call. All reasoning specific to a repo must be done within a \
repo session rather than in this project-wide context. You MUST NOT write files directly — all \
file writes MUST go through start_repo_session with expected_outcome=\"commit\".\n\
- wait_for_repo_session: Use this to wait on a previously started repo session by passing the \
`repo_session_id`. It returns the queue state (`queued`, `running`, `completed`, `cancelled`, \
or `failed`) and any available artifacts.\n\
- cancel_repo_session: Use this to cancel a queued or running repo session by `repo_session_id`."
            };

            let coordinator_reminder = if is_remote {
                "\n\nKeep this project session focused on coordination and synthesis. Do not perform \
repository edits directly here; use `start_repo_session` for implementation work."
            } else {
                ""
            };

            let action_instructions = format!(
                "The user is requesting work at the project level. Investigate and \
fulfill the request below, then produce a project note summarizing what you found and any \
actions taken.\n\n\
{preamble}\n\n\
{start_repo_session_desc}\n\n\
- add_project_repo: Use this when the task requires a repository that isn't yet in the \
project. Pass the GitHub repo slug to add it.\n\n\
IMPORTANT: `add_project_repo` and `start_repo_session` are MCP tools, not shell commands. \
Do not run `which`/`type` for these names and do not ask the user to add repos manually \
unless the MCP tool call itself returns an error. If the tool call fails, report the exact \
error and the next action needed.\
{coordinator_reminder}\n\n\
To discover repositories that might be relevant, use `gh` to explore repos in the user's \
GitHub organizations. Only add repos from organizations the user already belongs to.\n\n\
To return the note, include a horizontal rule (---) followed by the note content. \
Begin the note with a markdown H1 heading as the title.\n\n"
            );

            let full_prompt = format!(
                "<action>\n{action_instructions}\n\nProject information:\n{project_context}\n</action>\n\n{prompt}"
            );

            let working_dir = crate::git::project_worktree_root_for(&project.id)
                .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));

            let mut session = store::Session::new_running(&full_prompt, &working_dir);
            if let Some(ref p) = provider {
                session = session.with_provider(p);
            }
            store.create_session(&session).map_err(|e| e.to_string())?;

            let note = store::ProjectNote::new(&project_id, "", "").with_session(&session.id);
            store
                .create_project_note(&note)
                .map_err(|e| e.to_string())?;
            let note_id = note.id.clone();

            session_runner::start_session(
                SessionConfig {
                    session_id: session.id.clone(),
                    prompt: full_prompt,
                    working_dir,
                    agent_session_id: None,
                    pre_head_sha: None,
                    provider,
                    workspace_name: None,
                    extra_env: vec![],
                    mcp_project_id: Some(project_id.clone()),
                    action_executor: Some(Arc::clone(action_executor)),
                    action_registry: Some(Arc::clone(action_registry)),
                    remote_working_dir: None,
                    image_ids: image_ids.unwrap_or_default(),
                    branch_id: None,
                    project_id: Some(project_id),
                },
                store,
                app_handle.clone(),
                Arc::clone(session_registry),
            )?;

            Ok(
                serde_json::to_value(session_commands::ProjectSessionResponse {
                    session_id: session.id,
                    note_id,
                })
                .unwrap(),
            )
        }
        "queue_branch_session" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let prompt: String = arg(&args, "prompt")?;
            let session_type: session_commands::BranchSessionType = arg(&args, "sessionType")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let image_ids: Option<Vec<String>> = opt_arg(&args, "imageIds")?;
            let launch_context: Option<session_commands::BranchSessionLaunchContext> =
                opt_arg(&args, "launchContext")?;

            if matches!(
                session_type,
                session_commands::BranchSessionType::Commit
                    | session_commands::BranchSessionType::Review
            ) {
                session_commands::cancel_in_flight_auto_review_for_branch(
                    &store,
                    session_registry,
                    &branch_id,
                )?;
            }

            let _branch = store
                .get_branch(&branch_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

            let queued_prompt =
                session_commands::embed_launch_context(&prompt, launch_context.as_ref())?;
            let mut session = store::Session::new_queued(&queued_prompt);
            if let Some(ref p) = provider {
                session = session.with_provider(p);
            }
            store.create_session(&session).map_err(|e| e.to_string())?;

            if let Some(ref ids) = image_ids {
                store
                    .set_images_session_id(ids, &session.id)
                    .map_err(|e| e.to_string())?;
            }

            let artifact_id = match session_type {
                session_commands::BranchSessionType::Note => {
                    let note = store::Note::new(&branch_id, &prompt, "").with_session(&session.id);
                    store.create_note(&note).map_err(|e| e.to_string())?;
                    note.id
                }
                session_commands::BranchSessionType::Commit => {
                    let commit = store::Commit::new_pending(&branch_id).with_session(&session.id);
                    store.create_commit(&commit).map_err(|e| e.to_string())?;
                    commit.id
                }
                session_commands::BranchSessionType::Review => {
                    let review = store::Review::new(&branch_id, "", store::ReviewScope::Branch)
                        .with_session(&session.id);
                    store.create_review(&review).map_err(|e| e.to_string())?;
                    review.id
                }
            };

            Ok(
                serde_json::to_value(session_commands::BranchSessionResponse {
                    session_id: session.id,
                    artifact_id,
                })
                .unwrap(),
            )
        }
        "drain_queued_sessions" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;

            let result = session_commands::drain_queued_sessions_for_branch(
                store,
                Arc::clone(session_registry),
                app_handle.clone(),
                branch_id,
                provider,
            )
            .await?;

            Ok(serde_json::to_value(result).unwrap())
        }
        "cancel_session" => {
            let session_id: String = arg(&args, "sessionId")?;
            session_registry.cancel(&session_id);
            Ok(Value::Null)
        }
        "delete_session" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            session_registry.cancel(&session_id);
            store
                .delete_session(&session_id)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "find_fresh_auto_review" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let review = tauri::async_runtime::spawn_blocking(move || {
                store
                    .find_fresh_auto_review(&branch_id, 0)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
            Ok(serde_json::to_value(review).unwrap())
        }
        "set_review_auto" => {
            let store = get_store(store_mutex)?;
            let review_id: String = arg(&args, "reviewId")?;
            let is_auto: bool = arg(&args, "isAuto")?;
            store
                .set_review_auto(&review_id, is_auto)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }

        // =====================================================================
        // PRs
        // =====================================================================
        "create_pr" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let draft: Option<bool> = opt_arg(&args, "draft")?;
            let session_id = crate::prs::start_create_pr_pipeline_for_branch(
                store,
                Arc::clone(session_registry),
                app_handle.clone(),
                branch_id,
                provider,
                draft,
            )
            .await?;
            Ok(serde_json::to_value(session_id).unwrap())
        }
        "push_branch" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let force: Option<bool> = opt_arg(&args, "force")?;
            let session_id = crate::prs::start_push_branch_pipeline_for_branch(
                store,
                Arc::clone(session_registry),
                app_handle.clone(),
                branch_id,
                provider,
                force,
            )
            .await?;
            Ok(serde_json::to_value(session_id).unwrap())
        }
        "rebase_branch" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let session_id = crate::prs::start_or_queue_commit_pipeline_for_branch(
                store,
                Arc::clone(session_registry),
                app_handle.clone(),
                branch_id,
                store::PipelineKind::Rebase,
                provider,
                None,
            )
            .await?;
            Ok(serde_json::to_value(session_id).unwrap())
        }
        "squash_commits" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let session_id = crate::prs::start_or_queue_commit_pipeline_for_branch(
                store,
                Arc::clone(session_registry),
                app_handle.clone(),
                branch_id,
                store::PipelineKind::Squash,
                provider,
                None,
            )
            .await?;
            Ok(serde_json::to_value(session_id).unwrap())
        }
        "get_pr_url" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let pr_number: u64 = arg(&args, "prNumber")?;
            let branch = store
                .get_branch(&branch_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
            let project = store
                .get_project(&branch.project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;
            let repo_slug = crate::branches::resolve_branch_repo_slug(&store, &project, &branch)?;
            let url = tauri::async_runtime::spawn_blocking(move || {
                crate::git::fetch_pr_url(&repo_slug, pr_number)
            })
            .await
            .map_err(|e| format!("Task failed: {e}"))?
            .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(url).unwrap())
        }
        "update_branch_pr" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let pr_number: Option<u64> = opt_arg(&args, "prNumber")?;
            store
                .update_branch_pr_number(&branch_id, pr_number)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "recover_branch_pr" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let pr_number = crate::prs::recover_branch_pr_impl(store, branch_id).await?;
            Ok(serde_json::to_value(pr_number).unwrap())
        }
        "refresh_pr_status" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;

            let branch = store
                .get_branch(&branch_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
            let pr_number = branch
                .pr_number
                .ok_or_else(|| "Branch does not have an associated PR".to_string())?;
            let project = store
                .get_project(&branch.project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;
            let (github_repo, _) =
                crate::prs::resolve_branch_repo_and_subpath(&store, &project, &branch)?;

            let pr_status = {
                let github_repo = github_repo.clone();
                tokio::task::spawn_blocking(move || {
                    crate::git::fetch_pr_status_for_repo(&github_repo, pr_number)
                })
                .await
                .map_err(|e| format!("refresh_pr_status task failed: {e}"))?
            };
            let pr_status = match pr_status {
                Ok(status) => status,
                Err(e) => {
                    log::error!(
                        "refresh_pr_status failed for branch_id={}, pr_number={}: {}",
                        branch_id,
                        pr_number,
                        e
                    );
                    return Err(e.to_string());
                }
            };
            let mergeable = pr_status.mergeable == "MERGEABLE";
            let pr_fetched_at = store::now_timestamp();

            store
                .update_branch_pr_status(
                    &branch_id,
                    Some(pr_status.state.clone()),
                    Some(pr_status.checks_summary.state.clone()),
                    pr_status.review_decision.clone(),
                    Some(mergeable),
                    Some(pr_status.is_draft),
                    None,
                    None,
                    pr_status.head_sha.clone(),
                )
                .map_err(|e| e.to_string())?;

            emit_to_all(
                app_handle,
                "pr-status-changed",
                crate::prs::PrStatusEvent {
                    branch_id: branch_id.clone(),
                    pr_state: pr_status.state,
                    pr_checks_status: pr_status.checks_summary.state,
                    pr_review_decision: pr_status.review_decision,
                    pr_mergeable: mergeable,
                    pr_draft: pr_status.is_draft,
                    pr_head_sha: pr_status.head_sha,
                    pr_fetched_at,
                    failed_checks: pr_status.failed_checks,
                },
            );

            Ok(Value::Null)
        }
        "refresh_all_pr_statuses" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;

            let project = store
                .get_project(&project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {project_id}"))?;
            let branches = store
                .list_branches_for_project(&project_id)
                .map_err(|e| e.to_string())?;
            let branches_with_prs: Vec<_> = branches
                .into_iter()
                .filter(|b| b.pr_number.is_some())
                .collect();

            let mut refreshed_count = 0u32;

            for branch in branches_with_prs {
                let pr_number = branch.pr_number.unwrap();
                let github_repo =
                    match crate::prs::resolve_branch_repo_and_subpath(&store, &project, &branch) {
                        Ok((repo, _)) => repo,
                        Err(e) => {
                            log::warn!(
                                "Failed to resolve repo for branch {} (PR #{}): {}",
                                branch.id,
                                pr_number,
                                e
                            );
                            continue;
                        }
                    };

                let pr_result = {
                    let github_repo = github_repo.clone();
                    tokio::task::spawn_blocking(move || {
                        crate::git::fetch_pr_status_for_repo(&github_repo, pr_number)
                    })
                    .await
                    .map_err(|e| format!("refresh_all_pr_statuses task failed: {e}"))?
                };
                match pr_result {
                    Ok(pr_status) => {
                        let mergeable = pr_status.mergeable == "MERGEABLE";
                        let pr_fetched_at = store::now_timestamp();
                        if let Err(e) = store.update_branch_pr_status(
                            &branch.id,
                            Some(pr_status.state.clone()),
                            Some(pr_status.checks_summary.state.clone()),
                            pr_status.review_decision.clone(),
                            Some(mergeable),
                            Some(pr_status.is_draft),
                            None,
                            None,
                            pr_status.head_sha.clone(),
                        ) {
                            log::warn!(
                                "Failed to update PR status for branch {}: {}",
                                branch.id,
                                e
                            );
                            continue;
                        }

                        refreshed_count += 1;

                        emit_to_all(
                            app_handle,
                            "pr-status-changed",
                            crate::prs::PrStatusEvent {
                                branch_id: branch.id.clone(),
                                pr_state: pr_status.state,
                                pr_checks_status: pr_status.checks_summary.state,
                                pr_review_decision: pr_status.review_decision,
                                pr_mergeable: mergeable,
                                pr_draft: pr_status.is_draft,
                                pr_head_sha: pr_status.head_sha,
                                pr_fetched_at,
                                failed_checks: pr_status.failed_checks,
                            },
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "Failed to fetch PR status for branch {} (PR #{}): {}",
                            branch.id,
                            pr_number,
                            e
                        );
                    }
                }
            }

            emit_to_all(app_handle, "pr-statuses-refreshed", &project_id);

            Ok(serde_json::to_value(refreshed_count).unwrap())
        }
        "has_unpushed_commits" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let result = crate::prs::has_unpushed_commits_impl(store, branch_id).await?;
            Ok(serde_json::to_value(result).unwrap())
        }
        "clear_branch_pr_status" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            store
                .update_branch_pr_status(&branch_id, None, None, None, None, None, None, None, None)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }

        // =====================================================================
        // Utilities
        // =====================================================================
        "open_url" => {
            // In web mode, the browser handles URL opening
            Err("open_url is handled by the browser in web mode".to_string())
        }
        "is_sq_available" => Ok(serde_json::to_value(crate::blox::is_sq_available()).unwrap()),
        "read_text_file" => {
            let file_path: String = arg(&args, "filePath")?;
            let path = std::path::Path::new(&file_path);
            if !path.exists() {
                return Err(format!("File does not exist: {file_path}"));
            }
            if !path.is_file() {
                return Err(format!("Not a file: {file_path}"));
            }
            let content =
                std::fs::read_to_string(path).map_err(|e| format!("Failed to read file: {e}"))?;
            Ok(serde_json::to_value(content).unwrap())
        }
        "preferences_store_path" => {
            let path = crate::preferences_store_path_buf()
                .map(|p| p.to_string_lossy().to_string())
                .ok_or("Cannot determine preferences store path")?;
            Ok(serde_json::to_value(path).unwrap())
        }
        "check_blox_auth" => {
            tauri::async_runtime::spawn_blocking(crate::blox::check_auth)
                .await
                .map_err(|e| format!("Failed to run blox auth check: {e}"))?
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "get_available_openers" => {
            // In web mode, openers are not relevant
            Ok(serde_json::to_value(Vec::<()>::new()).unwrap())
        }
        "open_in_app" => Err("open_in_app is not available in web mode".to_string()),

        // =====================================================================
        // Doctor
        // =====================================================================
        "run_doctor" => {
            let report = doctor::run_checks().await;
            Ok(serde_json::to_value(report).unwrap())
        }
        "run_doctor_fix" => {
            let check_id: String = arg(&args, "checkId")?;
            let fix_type: doctor::FixType = arg(&args, "fixType")?;
            doctor::execute_fix(check_id, fix_type).await?;
            Ok(Value::Null)
        }

        _ => Err(format!("Unknown command: {command}")),
    }
}
