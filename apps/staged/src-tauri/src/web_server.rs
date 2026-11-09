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
//!
//! # Handler futures must not own queue state
//!
//! `invoke_command` awaits [`dispatch`] inline, and hyper's HTTP/1 default
//! (`http1_half_close = false`) drops the in-flight handler future at its next
//! `.await` the moment the client closes the connection — a closed tab, mobile
//! Safari backgrounding. An arm that writes a `running` session row or a
//! workspace status and then awaits would leave that write with nothing to
//! release it: the row is the branch's busy marker, so everything queued
//! behind it parks. Such arms run their work through [`detached`], which
//! spawns it and only awaits the handle; the source-text test
//! `web_arms_that_write_running_rows_are_detached` keeps new arms honest.

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
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::serve::Listener;
use axum::Router;
use axum_extra::extract::cookie::{Cookie, CookieJar};
use rand::RngExt;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde_json::Value;
use subtle::ConstantTimeEq;
use tauri::Manager;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;
use tower_http::compression::predicate::{DefaultPredicate, NotForContentType, Predicate};
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::pr_poll_scheduler::PrPollScheduler;
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

const EVENT_GAP_EVENT: &str = "transport:event-gap";

fn serialize_web_event<S: serde::Serialize>(
    event_name: &str,
    payload: S,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&serde_json::json!({
        "event": event_name,
        "payload": payload,
    }))
}

fn event_gap_payload() -> String {
    // serde_json::Value cannot fail serialization.
    serialize_web_event(EVENT_GAP_EVENT, Value::Null)
        .expect("event-gap payload serialization should be infallible")
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
///
/// Generic over the runtime so command bodies that emit can be driven by
/// `MockRuntime` under test; production callers pass the concrete `Wry`
/// handle and infer `R`.
pub fn emit_to_all<R: tauri::Runtime, S: serde::Serialize + Clone>(
    app_handle: &tauri::AppHandle<R>,
    event_name: &str,
    payload: S,
) {
    use tauri::{Emitter, Manager};
    let _ = app_handle.emit(event_name, &payload);
    if let Some(tx) = app_handle.try_state::<broadcast::Sender<WebEvent>>() {
        broadcast_web_event(&tx, event_name, &payload);
    }
}

/// Send an event to the browser clients connected over WebSocket. Returns
/// whether there was anyone to send it to.
///
/// With no client connected the payload is not serialized at all. Most
/// installs never have one, and events such as action output are frequent.
/// A client that connects a moment later misses the event either way: the
/// channel only delivers what is sent after a client subscribes.
fn broadcast_web_event<S: serde::Serialize>(
    tx: &broadcast::Sender<WebEvent>,
    event_name: &str,
    payload: &S,
) -> bool {
    if tx.receiver_count() == 0 {
        return false;
    }
    if let Ok(json) = serialize_web_event(event_name, payload) {
        let _ = tx.send(WebEvent {
            event_name: event_name.to_string(),
            payload: json,
        });
    }
    true
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
    let mut config = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::aws_lc_rs::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .map_err(|e| format!("failed to configure TLS protocol versions: {e}"))?
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .map_err(|e| format!("failed to configure TLS certificate/key pair: {e}"))?;
    // Browsers only speak HTTP/2 when the server offers it over ALPN; without
    // this they stay on HTTP/1.1 and its six-connection cap, which the 500 ms
    // session poll (four requests per tick) shares with multi-MB transcript
    // bodies. `axum::serve` already runs hyper's auto h1/h2 builder.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

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
/// That restore should serve [`app_router`], which already composes the API
/// routes, [`media_router`] (the frontend's `resolveStagedMediaUrl` emits
/// `/api/media/...` in non-Tauri builds) and the static bundle.
pub fn start(_state: WebAppState) {
    log::warn!("Web server requested but this build has the web server stubbed out");
}

/// The composed app: API routes, note media, and the built frontend from
/// `dist_dir`. Responses are compressed (brotli, then gzip, by client
/// preference): a session transcript is 10–25 MB of JSON uncompressed and
/// brotli at tower-http's default quality brings a text-heavy one down ~10×
/// and a base64-image-heavy one ~3×. The default predicate skips images and
/// bodies under 32 bytes, so WebSocket upgrades and media are untouched.
pub(crate) fn app_router(state: WebAppState, dist_dir: &std::path::Path) -> Router {
    let media_routes = match media_router(state.clone()) {
        Ok(router) => router,
        Err(e) => {
            log::warn!("[web_server] note media unavailable: {e}");
            Router::new()
        }
    };

    invoke_router(invoke_command)
        .route("/api/events", get(ws_events))
        .with_state(state)
        .merge(media_routes)
        .fallback_service(ServeDir::new(dist_dir).append_index_html_on_directories(true))
        .layer(compression_layer())
        .layer(CorsLayer::permissive())
}

/// Response compression for every route. tower-http's default predicate
/// already skips small bodies, `image/*`, gRPC, and event streams; video and
/// PDF are added to that list because their bytes are already codec-compressed
/// (a 100 MiB MP4 gains nothing from brotli) and because compressing a
/// response drops its `Content-Length` and `Accept-Ranges`. `<video>`
/// playback already sends `Accept-Encoding: identity` and range responses
/// are exempt, so this matters for the paths that negotiate br/gzip: a HEAD,
/// a `fetch()`, "open in new tab", and the download/share sheet, which
/// otherwise lose progress and resumability.
fn compression_layer() -> CompressionLayer<impl Predicate> {
    CompressionLayer::new().compress_when(
        DefaultPredicate::new()
            .and(NotForContentType::new("video/"))
            .and(NotForContentType::new("application/pdf")),
    )
}

/// Request body cap for `/api/invoke/{command}`, the only route that takes a
/// large body. axum's default is 2 MiB, which would refuse a base64 PDF
/// attachment before `create_image_from_data`'s own
/// [`MAX_PDF_INPUT_SIZE`](crate::note_media::files::MAX_PDF_INPUT_SIZE) check
/// ever sees it, so this is the base64 of a cap-sized payload plus a margin
/// for the JSON envelope (the other arguments are short strings).
const INVOKE_BODY_LIMIT: usize =
    (crate::note_media::files::MAX_PDF_INPUT_SIZE.div_ceil(3) * 4) as usize + 64 * 1024;

/// The invoke route with its body limit applied to that route alone. Generic
/// over the handler so a test can mount a stand-in without a Tauri
/// `AppHandle` and exercise the exact layering [`app_router`] uses.
fn invoke_router<S, H, T>(handler: H) -> Router<S>
where
    H: axum::handler::Handler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    Router::new().route(
        "/api/invoke/{command}",
        post(handler).layer(DefaultBodyLimit::max(INVOKE_BODY_LIMIT)),
    )
}

/// Kept ready for the mobile-web startup restore. The route and auth layer are
/// exercised together by oneshot tests, without starting a listener.
pub(crate) fn media_router(state: WebAppState) -> Result<Router, String> {
    let store = get_store(
        state
            .app_handle
            .state::<Mutex<Option<Arc<Store>>>>()
            .inner(),
    )?;
    Ok(media_router_for_store(
        store,
        state.auth_token,
        state.sessions,
    ))
}

/// Tool and note media are user/agent-supplied bytes served from the app
/// origin. Viewed top-level (e.g. "Open image in new tab") an SVG would run
/// script with the session cookie in scope; `sandbox` puts it in an opaque
/// origin with script disabled and `default-src 'none'` blocks its subresource
/// loads, while `style-src 'unsafe-inline'` keeps its own `<style>` working.
/// `<img>` rendering ignores response CSP, so this costs nothing there.
const UNTRUSTED_MEDIA_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";

/// A PDF attachment is shown in an `<iframe>` by the browser's own viewer,
/// which WebKit treats as a plugin and refuses in a sandboxed document (the
/// frame stays blank). The viewer runs no page-origin script, so only
/// `sandbox` is dropped for it.
const UNTRUSTED_PDF_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'";

fn media_router_for_store(
    store: Arc<Store>,
    token: String,
    sessions: Arc<Mutex<HashSet<String>>>,
) -> Router {
    Router::new()
        .route("/api/media/{file}", get(serve_media))
        .route(
            "/api/tool-images/{message_id}/{field}/{index}",
            get(serve_tool_image),
        )
        .route("/api/tool-bodies/{message_id}", get(serve_tool_call_body))
        .with_state(store)
        .route_layer(middleware::from_fn(
            move |jar: CookieJar, request: Request, next: Next| {
                let token = token.clone();
                let sessions = sessions.clone();
                async move {
                    if is_authenticated(&token, &sessions, &jar, request.headers()) {
                        next.run(request).await
                    } else {
                        (StatusCode::UNAUTHORIZED, "Authentication required").into_response()
                    }
                }
            },
        ))
}

async fn serve_media(
    State(store): State<Arc<Store>>,
    Path(file): Path<String>,
    request: Request,
) -> Response {
    let (path, mime) = match crate::note_media::serving::lookup(&store, &file) {
        Ok(value) => value,
        Err(status) => return status.into_response(),
    };
    // A multi-range request gets the whole file as a 200 instead of a 416:
    // Chrome's PDF viewer sends them, and browsers accept the full body.
    let mut service = tower_http::services::ServeFile::new_with_mime(
        path,
        &mime.parse().expect("validated MIME"),
    )
    .ignore_multi_range_requests(true);
    match service.try_call(request).await {
        Ok(response) => {
            let mut response = response.into_response();
            response.headers_mut().insert(
                "X-Content-Type-Options",
                axum::http::HeaderValue::from_static("nosniff"),
            );
            // Long-lived for the same reason as the tool-image route: an id
            // is a fresh UUID per write and nothing rewrites bytes under an
            // existing one (a cross-project move renames the file, keeping
            // its contents), so the URL is a stable identity for the bytes.
            // Applies to every 2xx (200, 206, HEAD) and to 304, so a reopened
            // note costs no conditional round trips and the browser stops
            // revalidating a partially cached video before seeking. `Ok`
            // also carries `ServeFile`'s 412 and 416, which describe the
            // request rather than the bytes and must not be stored.
            let status = response.status();
            if status.is_success() || status == StatusCode::NOT_MODIFIED {
                response.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("private, max-age=31536000, immutable"),
                );
            }
            // `mime_for_extension` has no SVG entry today; this keeps a future
            // one from reopening the top-level script hole.
            let csp = if mime == "application/pdf" {
                UNTRUSTED_PDF_CSP
            } else {
                UNTRUSTED_MEDIA_CSP
            };
            response.headers_mut().insert(
                axum::http::header::CONTENT_SECURITY_POLICY,
                axum::http::HeaderValue::from_static(csp),
            );
            response
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Serve the bytes behind a tool-result image reference. The transcript
/// queries replace base64 image blocks with
/// `{"type":"image","ref":"<message_id>/<output|content>/<index>",…}` (see
/// `store/tool_images.rs`); the `<img>` then requests this route lazily.
///
/// Anything that does not resolve to a base64 image block — unknown row,
/// bad field, index past the end, a text block, a row still in flight — is a
/// 404, including unparseable ids, so a stale ref never surfaces as a 400 in
/// the console. `Cache-Control` is long-lived: the store only projects and
/// resolves refs for settled tool rows, whose block lists never change again,
/// so the id/index pair is a stable identity for the block. The compression
/// layer's default predicate skips `image/*` bodies, so these go out as-is.
/// Every response carries [`UNTRUSTED_MEDIA_CSP`]: `is_image_mime_type`
/// admits `image/svg+xml`, and an SVG opened as a document must not run on
/// this origin.
async fn serve_tool_image(
    State(store): State<Arc<Store>>,
    Path((message_id, field, index)): Path<(String, String, String)>,
) -> Response {
    let (Ok(message_id), Some(field), Ok(index)) = (
        message_id.parse::<i64>(),
        store::ToolImageField::parse(&field),
        index.parse::<usize>(),
    ) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let image = tokio::task::spawn_blocking(move || store.get_tool_image(message_id, field, index))
        .await
        .map_err(|e| e.to_string())
        .and_then(|result| result.map_err(|e| e.to_string()));
    let image = match image {
        Ok(Some(image)) => image,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            log::warn!("[web_server] tool image {message_id}/{field}/{index} unreadable: {e}");
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    let Ok(content_type) = axum::http::HeaderValue::from_str(&image.mime_type) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (axum::http::header::CONTENT_TYPE, content_type),
            (
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("private, max-age=31536000, immutable"),
            ),
            (
                axum::http::header::HeaderName::from_static("x-content-type-options"),
                axum::http::HeaderValue::from_static("nosniff"),
            ),
            (
                axum::http::header::CONTENT_SECURITY_POLICY,
                axum::http::HeaderValue::from_static(UNTRUSTED_MEDIA_CSP),
            ),
        ],
        image.bytes,
    )
        .into_response()
}

/// Serve the bodies behind a tool row's `$deferred` markers. The transcript
/// queries replace the large input/output/content of settled tool rows with
/// `{"$deferred":{"messageId":…}}` (see `store/tool_bodies.rs`); the card
/// requests this route when it is expanded.
///
/// An unknown or unparseable id is a 404, as for tool images. The bodies are
/// JSON, so the compression layer applies, and `Cache-Control` is long-lived
/// for the same reason as the image route: only settled rows are deferred,
/// and a settled row's bodies never change.
async fn serve_tool_call_body(
    State(store): State<Arc<Store>>,
    Path(message_id): Path<String>,
) -> Response {
    let Ok(message_id) = message_id.parse::<i64>() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let body = tokio::task::spawn_blocking(move || store.get_tool_call_body(message_id))
        .await
        .map_err(|e| e.to_string())
        .and_then(|result| result.map_err(|e| e.to_string()));
    let body = match body {
        Ok(Some(body)) => body,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            log::warn!("[web_server] tool call body {message_id} unreadable: {e}");
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    (
        [
            (
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("private, max-age=31536000, immutable"),
            ),
            (
                axum::http::header::HeaderName::from_static("x-content-type-options"),
                axum::http::HeaderValue::from_static("nosniff"),
            ),
        ],
        Json(body),
    )
        .into_response()
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
    if is_authenticated(&state.auth_token, &state.sessions, &jar, request.headers()) {
        next.run(request).await
    } else {
        (StatusCode::UNAUTHORIZED, "Authentication required").into_response()
    }
}

fn is_authenticated(
    token: &str,
    sessions: &Mutex<HashSet<String>>,
    jar: &CookieJar,
    headers: &axum::http::HeaderMap,
) -> bool {
    if headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .is_some_and(|value| constant_time_eq(value, token))
    {
        return true;
    }
    jar.get(SESSION_COOKIE_NAME).is_some_and(|cookie| {
        sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|s| constant_time_eq(cookie.value(), s))
    })
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

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsQuery {
    client_id: Option<String>,
}

async fn ws_events(
    ws: WebSocketUpgrade,
    Query(query): Query<EventsQuery>,
    State(state): State<WebAppState>,
) -> Response {
    let client_id = normalize_ws_client_id(query.client_id);
    ws.on_upgrade(move |socket| handle_ws(socket, state, client_id))
}

/// The scheduler client id a WS connection may claim, or `None` for "don't track
/// this socket in the PR-poll scheduler" — the socket still delivers events.
///
/// Drops blank ids, and ids in the reserved native-window namespace (see
/// [`crate::pr_poll_scheduler::is_reserved_client_id`]): those are TTL-exempt, so
/// a web client holding one would pin its interest forever on a dirty drop, and
/// could spoof or tear down a real window's entry. Stripping rather than failing
/// the upgrade is deliberate — this socket also carries the change feed and
/// session events, so killing event delivery over a bad `clientId` would be the
/// worse failure mode. The warning is what makes a buggy client visible.
fn normalize_ws_client_id(client_id: Option<String>) -> Option<String> {
    client_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .filter(|id| {
            let reserved = crate::pr_poll_scheduler::is_reserved_client_id(id);
            if reserved {
                log::warn!(
                    "[web_server] ignoring WS clientId {id:?}: the 'tauri-' prefix is reserved for native windows"
                );
            }
            !reserved
        })
}

// clippy's suggested fix (collapsing the inner `if` into a match guard) doesn't
// compile because `data: Bytes` can't be moved out of the pattern binding into
// the guard expression.
#[allow(clippy::collapsible_match)]
async fn handle_ws(mut socket: WebSocket, state: WebAppState, client_id: Option<String>) {
    let pr_scheduler = client_id.as_ref().map(|client_id| {
        use tauri::Manager;
        let scheduler = Arc::clone(&state.app_handle.state::<Arc<PrPollScheduler>>());
        scheduler.touch(client_id.clone());
        scheduler
    });

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
                        if socket
                            .send(Message::Text(event_gap_payload().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            // Handle incoming messages (heartbeat, ping/pong, close)
            msg = socket.recv() => {
                let pong_data = match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Text(text))) => {
                        if is_heartbeat_message(text.as_str()) {
                            if let (Some(scheduler), Some(client_id)) = (&pr_scheduler, &client_id) {
                                scheduler.touch(client_id.clone());
                            }
                        }
                        continue;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if let (Some(scheduler), Some(client_id)) = (&pr_scheduler, &client_id) {
                            scheduler.touch(client_id.clone());
                        }
                        data
                    }
                    Some(Ok(Message::Pong(_))) => {
                        if let (Some(scheduler), Some(client_id)) = (&pr_scheduler, &client_id) {
                            scheduler.touch(client_id.clone());
                        }
                        continue;
                    }
                    _ => continue,
                };
                if socket.send(Message::Pong(pong_data)).await.is_err() {
                    break;
                }
            }
        }
    }

    if let (Some(scheduler), Some(client_id)) = (pr_scheduler, client_id) {
        scheduler.disconnect_client(client_id);
    }
}

fn is_heartbeat_message(text: &str) -> bool {
    if text == "heartbeat" {
        return true;
    }

    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(Value::as_str)
                .map(|message_type| message_type == "heartbeat")
        })
        .unwrap_or(false)
}

// =============================================================================
// Command dispatch — POST /api/invoke/{command}
// =============================================================================

async fn invoke_command(
    Path(command): Path<String>,
    State(state): State<WebAppState>,
    Json(args): Json<Value>,
) -> Response {
    match dispatch(&command, args, &state.app_handle, DispatchCaller::Web).await {
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

/// The `clientId` for the PR-poll dispatch verbs, rejecting the reserved
/// native-window namespace (see [`crate::pr_poll_scheduler::is_reserved_client_id`]).
///
/// A web caller must not be able to create a TTL-exempt entry that no window
/// teardown will ever reclaim, nor mutate a real window's interest. Erroring
/// (rather than silently ignoring) is right here — the id *is* the point of these
/// verbs, the frontend already logs dispatch failures, and a legitimate web client
/// uses a UUID so it can never trip this.
fn web_client_id(args: &Value) -> Result<String, String> {
    let client_id: String = arg(args, "clientId")?;
    if crate::pr_poll_scheduler::is_reserved_client_id(&client_id) {
        return Err(format!(
            "clientId '{client_id}' uses the reserved native-window prefix 'tauri-'"
        ));
    }
    Ok(client_id)
}

/// Run a store read and its JSON serialization on the blocking pool. The
/// session-transcript reads produce tens of MB of JSON; doing that inline
/// would stall the tokio worker that is also servicing `/api/events`.
async fn blocking_store_json<T>(
    read: impl FnOnce() -> Result<T, store::StoreError> + Send + 'static,
) -> Result<Value, String>
where
    T: serde::Serialize + Send + 'static,
{
    blocking_json(read).await
}

/// Run a blocking call and its JSON serialization on the blocking pool. For
/// arms that wait on `gh`, git or another child process: run inline, the
/// call holds its tokio worker for its whole duration, and with it the timers
/// and sockets every other request and `/api/events` depend on.
async fn blocking_json<T, E>(
    call: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> Result<Value, String>
where
    T: serde::Serialize + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let value = call().map_err(|e| e.to_string())?;
        Ok(serde_json::to_value(value).unwrap())
    })
    .await
    .map_err(|e| format!("task failed: {e}"))?
}

/// Helper to get the Store Arc from the shared mutex slot.
/// Run `fut` on its own task and wait for its result.
///
/// For arms that write a `running` session row or a workspace status and then
/// await. The request future may be dropped at any `.await` (see the module
/// docs); dropping a `JoinHandle` detaches the task rather than cancelling it,
/// so the write is always followed by its release and the client merely
/// misses the response. A task that panics surfaces as a plain `Err`, like
/// any other arm failure.
async fn detached<T, F>(fut: F) -> Result<T, String>
where
    T: Send + 'static,
    F: std::future::Future<Output = Result<T, String>> + Send + 'static,
{
    tauri::async_runtime::spawn(fut)
        .await
        .unwrap_or_else(|e| Err(format!("Detached task failed: {e}")))
}

fn get_store(store: &Mutex<Option<Arc<Store>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Database not initialized".to_string())
}

/// Open the shared preferences store (`~/.staged/preferences.json`) through
/// the store plugin. Returns the desktop frontend's already-loaded instance
/// when one exists, so web writes can't be clobbered by a stale in-memory
/// copy; the backend paths that read the file directly (e.g. `branch-prefix`
/// in `branches.rs`) see web writes because every mutation is saved to disk.
pub(crate) fn preferences_store(
    app_handle: &tauri::AppHandle,
) -> Result<Arc<tauri_plugin_store::Store<tauri::Wry>>, String> {
    use tauri_plugin_store::StoreExt;
    let path =
        crate::preferences_store_path_buf().ok_or("Cannot determine preferences store path")?;
    app_handle
        .store(path)
        .map_err(|e| format!("Failed to load preferences store: {e}"))
}

pub(crate) mod control_scope;
pub(crate) use control_scope::SCOPE_DENIED_PREFIX;

/// Who is calling [`dispatch`]. A handful of commands encode same-host-only
/// semantics — they take local file paths or launch local apps — which are
/// valid from the control server but a hole from a remote browser. Making
/// the caller a parameter closes those arms by construction (see
/// [`web_caller_rejection`]) instead of relying on `start()` staying stubbed,
/// and carries a control caller's grant so its scope can be enforced ahead
/// of the arm table (see [`control_scope`]).
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum DispatchCaller {
    /// The `stagedctl` control server (`control_server.rs`): a same-user
    /// process on the app's machine, holding this grant.
    Control(crate::control_server::ControlGrant),
    /// The browser-facing web server: the caller may be a remote browser.
    /// (Its `start()` is currently stubbed, but rejection of same-host-only
    /// commands must not depend on that.)
    Web,
}

impl DispatchCaller {
    /// What a session this caller starts records in `sessions.started_by`:
    /// the control grant's actor (`session:<id>` / `app:<grant-id>`), or
    /// `None` for a web caller, which — like the desktop UI — is the user.
    pub(crate) fn started_by(&self) -> Option<String> {
        match self {
            Self::Control(grant) => Some(grant.actor.started_by()),
            Self::Web => None,
        }
    }
}

/// Why a web (remote-browser) caller may not run `command`, or `None` if it
/// may. From a remote browser, `create_image` would mean "read any file on
/// the app's machine into a viewable timeline" and `open_in_app` would mean
/// "open an arbitrary path in an arbitrary local app" — so both are rejected
/// before any dispatch arm runs, and restoring the web listener cannot
/// silently inherit them.
fn web_caller_rejection(cmd: &str, caller: &DispatchCaller) -> Option<&'static str> {
    if !matches!(caller, DispatchCaller::Web) {
        return None;
    }
    // `cmd`, not `command`: `web_dispatch_covers_tauri_commands` locates
    // dispatch's arm table by searching for a match over `command`, so that
    // spelling must stay unique to dispatch().
    match cmd {
        "create_image" => Some(
            "create_image is not available in web mode: it reads a file path \
             on the app's machine (use create_image_from_data)",
        ),
        "open_in_app" => Some(
            "open_in_app is not available in web mode: it launches an app on \
             the app's machine",
        ),
        _ => None,
    }
}

/// The tag catalog as a control caller may see it: the shared groups and
/// tags whole, and the per-project `assignments` confined to the caller's
/// project when its credential is scoped to one, so a session learns which
/// tags exist but not which other projects carry them. Unconfined callers
/// get the state unchanged.
fn confine_tag_state(mut state: store::TagState, caller: &DispatchCaller) -> store::TagState {
    if let Some(project_id) = control_scope::project_filter(caller) {
        state
            .assignments
            .retain(|assignment| assignment.project_id == project_id);
    }
    state
}

/// The `create_image` args the control surface accepts, enforcing that the
/// created image is UI-visible: `pending: true` (hidden until a session
/// attaches it) and a missing `branchId` (project-scoped images are
/// write-only — nothing lists or renders them) are both rejected, so no
/// caller — the well-behaved CLI or a raw POST to the control port — can
/// create an artifact the UI never shows. Returns
/// `(branchId, projectId, filePath)`.
fn control_create_image_args(args: &Value) -> Result<(String, String, String), String> {
    if opt_arg::<bool>(args, "pending")?.unwrap_or(false) {
        return Err(
            "create_image with pending=true is not available through the control surface: \
             a pending image is invisible until a session attaches it"
                .to_string(),
        );
    }
    let Some(branch_id) = opt_arg::<String>(args, "branchId")? else {
        return Err(
            "create_image requires a branchId through the control surface: \
             a project-scoped image is invisible in the UI"
                .to_string(),
        );
    };
    let file_path: String = arg(args, "filePath")?;
    // Nothing downstream (`create_image_impl`, `read_local_attachment`)
    // canonicalizes or checks this, so a relative path would resolve against
    // the app's own working directory — not the caller's. The CLI always
    // sends an absolute path; a raw POST must too.
    if !std::path::Path::new(&file_path).is_absolute() {
        return Err(format!(
            "create_image requires an absolute filePath through the control surface \
             (got {file_path:?}): a relative path would resolve against the app's \
             working directory, not the caller's"
        ));
    }
    Ok((branch_id, arg(args, "projectId")?, file_path))
}

/// The big dispatcher. Maps command names to handler logic.
///
/// Each arm extracts arguments from the JSON body and calls the same
/// underlying logic as the corresponding Tauri command. Return values
/// are serialized to `serde_json::Value`. Shared by the (stubbed) web
/// server and the `stagedctl` control server (`control_server.rs`);
/// `caller` says which, so same-host-only arms can be closed to web
/// callers by construction.
pub(crate) async fn dispatch(
    command: &str,
    args: Value,
    app_handle: &tauri::AppHandle,
    caller: DispatchCaller,
) -> Result<Value, String> {
    use tauri::Manager;

    if let Some(reason) = web_caller_rejection(command, &caller) {
        return Err(reason.to_string());
    }

    // Convenience aliases — pull shared state from the Tauri AppHandle
    let store_mutex: &Mutex<Option<Arc<Store>>> = &app_handle.state::<Mutex<Option<Arc<Store>>>>();
    let session_registry: &Arc<SessionRegistry> = &app_handle.state::<Arc<SessionRegistry>>();
    let action_executor: &Arc<ActionExecutor> = &app_handle.state::<Arc<ActionExecutor>>();
    let action_registry: &Arc<ActionRegistry> = &app_handle.state::<Arc<ActionRegistry>>();
    let pr_scheduler: &Arc<PrPollScheduler> = &app_handle.state::<Arc<PrPollScheduler>>();

    // A control caller's grant confines it to one project (and maybe to
    // reads); refused here, before any arm, so no arm has to remember to.
    if let DispatchCaller::Control(grant) = &caller {
        control_scope::check(command, &args, grant, store_mutex)?;
    }

    match command {
        // =====================================================================
        // Store status
        // =====================================================================
        "get_store_status" => {
            // Same-host clients (stagedctl) need the real status so a
            // needs-reset prompt surfaces as data instead of opaque
            // "Database not initialized" errors on every other command.
            let db_state = app_handle.state::<crate::DbState>();
            let status = db_state.needs_reset.lock().unwrap().clone();
            Ok(serde_json::to_value(status).unwrap())
        }
        "confirm_reset_store" => {
            // Not applicable in web context
            Err("confirm_reset_store is not supported in web mode".to_string())
        }

        // =====================================================================
        // Windows (native only — a browser client opens its own tab instead)
        // =====================================================================
        "new_window" => Err("new_window is not supported in web mode".to_string()),
        "take_window_seed" => {
            // Web clients are never opener-seeded; report "no seed".
            Ok(Value::Null)
        }
        "claim_updater_ownership" => {
            Err("claim_updater_ownership is not supported in web mode".to_string())
        }

        // =====================================================================
        // App lifecycle
        // =====================================================================
        // Refused rather than proxied: a browser client must not be able to
        // terminate the desktop host every other client is connected to. The
        // confirmation this gates on is a native alert on the host anyway, with
        // no browser client to answer it.
        "quit_app" => Err(format!("{command} is not available in web mode")),

        // =====================================================================
        // Launch resume (desktop only)
        // =====================================================================
        // The interrupted set is computed once per process and offered to the
        // desktop main window; a browser client must not take it or trigger a
        // second resume of the same rows.
        "get_interrupted_sessions" | "resume_interrupted_sessions" => {
            Err(format!("{command} is not available in web mode"))
        }

        // =====================================================================
        // stagedctl connected apps (desktop only)
        // =====================================================================
        // Consent is the user's to give in the app window, and the grant
        // list is theirs to prune there; a browser client (or a stagedctl
        // caller, which never reaches these — they are outside
        // `ALLOWED_COMMANDS`) must not approve a request or revoke a grant.
        // One arm per line: the parity test reads an arm's patterns off the
        // line that holds its `=>`.
        "list_pending_control_authorizations" => {
            Err(format!("{command} is not available in web mode"))
        }
        "answer_control_authorization" => Err(format!("{command} is not available in web mode")),
        "list_control_grants" => Err(format!("{command} is not available in web mode")),
        "revoke_control_grant" => Err(format!("{command} is not available in web mode")),

        // =====================================================================
        // Projects
        // =====================================================================
        "list_projects" => {
            let store = get_store(store_mutex)?;
            let mut projects = store.list_projects().map_err(|e| e.to_string())?;
            // A project-scoped control credential sees only its own project.
            if let Some(project_id) = control_scope::project_filter(&caller) {
                projects.retain(|project| project.id == project_id);
            }
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
                .unwrap_or_else(|| crate::branches::infer_prefixed_branch_name(trimmed));

            let mut project = crate::store::Project::named(trimmed);
            project.location = project_location;
            if let Some(ref repo) = github_repo {
                project = project.with_primary_repo(repo);
            }
            if let Some(ref sub) = subpath {
                project = project.with_subpath(sub.clone());
            }

            // Without a prefetched default branch this asks `gh`, so it runs on
            // the blocking pool, and before the first write so no await falls
            // between the writes below (see the module docs).
            let effective_base = match github_repo.clone() {
                Some(repo) => Some(
                    tauri::async_runtime::spawn_blocking(move || {
                        crate::git::resolve_default_branch(default_branch, &repo)
                    })
                    .await
                    .map_err(|e| e.to_string())?,
                ),
                None => None,
            };

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

            if let (Some(repo), Some(effective_base)) = (github_repo, effective_base) {
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
                                    &app_handle_clone,
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
                        let workspace_name = crate::branches::infer_workspace_name(
                            &project.id,
                            &inferred_branch_name,
                        );
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
            .await?
            .repo;
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
            let project_id: String = arg(&args, "projectId")?;
            let project_repo_id: String = arg(&args, "projectRepoId")?;
            let services = crate::branch_teardown::TeardownServices {
                store: get_store(store_mutex)?,
                action_executor: Arc::clone(action_executor),
                action_registry: Arc::clone(action_registry),
                session_registry: Arc::clone(session_registry),
                app_handle: app_handle.clone(),
            };
            detached(crate::branch_teardown::remove_project_repo_impl(
                services,
                project_id,
                project_repo_id,
            ))
            .await?;
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
            let id: String = arg(&args, "id")?;
            let services = crate::branch_teardown::TeardownServices {
                store: get_store(store_mutex)?,
                action_executor: Arc::clone(action_executor),
                action_registry: Arc::clone(action_registry),
                session_registry: Arc::clone(session_registry),
                app_handle: app_handle.clone(),
            };
            detached(crate::branch_teardown::delete_project_impl(services, id)).await?;
            Ok(Value::Null)
        }

        // =====================================================================
        // Tags (mirrors `tag_commands`)
        // =====================================================================
        "get_tag_state" => {
            let store = get_store(store_mutex)?;
            let state = store.get_tag_state().map_err(|e| e.to_string())?;
            let state = confine_tag_state(state, &caller);
            Ok(serde_json::to_value(state).unwrap())
        }
        "create_tag_group" => {
            let store = get_store(store_mutex)?;
            let name: String = arg(&args, "name")?;
            let tags: Vec<store::NewTag> = arg(&args, "tags")?;
            let group = store
                .create_tag_group(&name, &tags)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(group).unwrap())
        }
        "rename_tag_group" => {
            let store = get_store(store_mutex)?;
            let group_id: String = arg(&args, "groupId")?;
            let name: String = arg(&args, "name")?;
            store
                .rename_tag_group(&group_id, &name)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "set_icon_tag_group" => {
            let store = get_store(store_mutex)?;
            let group_id: Option<String> = opt_arg(&args, "groupId")?;
            store
                .set_icon_tag_group(group_id.as_deref())
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "set_tag_group_sidebar" => {
            let store = get_store(store_mutex)?;
            let group_id: String = arg(&args, "groupId")?;
            let shown: bool = arg(&args, "shown")?;
            store
                .set_tag_group_sidebar(&group_id, shown)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "reorder_tag_groups" => {
            let store = get_store(store_mutex)?;
            let group_ids: Vec<String> = arg(&args, "groupIds")?;
            store
                .reorder_tag_groups(&group_ids)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "delete_tag_group" => {
            let store = get_store(store_mutex)?;
            let group_id: String = arg(&args, "groupId")?;
            let mode: store::DeleteTagGroupMode = arg(&args, "mode")?;
            store
                .delete_tag_group(&group_id, mode)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "create_tag" => {
            let store = get_store(store_mutex)?;
            let new = store::NewTag {
                name: arg(&args, "name")?,
                color: opt_arg(&args, "color")?,
                icon: opt_arg(&args, "icon")?,
            };
            let group_id: Option<String> = opt_arg(&args, "groupId")?;
            let assign_project_id: Option<String> = opt_arg(&args, "assignProjectId")?;
            let tag = store
                .create_tag(&new, group_id.as_deref(), assign_project_id.as_deref())
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(tag).unwrap())
        }
        "update_tag" => {
            let store = get_store(store_mutex)?;
            let tag_id: String = arg(&args, "tagId")?;
            let name: String = arg(&args, "name")?;
            let color: store::TagColor = arg(&args, "color")?;
            let icon: Option<String> = opt_arg(&args, "icon")?;
            store
                .update_tag(&tag_id, &name, color, icon.as_deref())
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "move_tag" => {
            let store = get_store(store_mutex)?;
            let tag_id: String = arg(&args, "tagId")?;
            let group_id: Option<String> = opt_arg(&args, "groupId")?;
            let index: usize = arg(&args, "index")?;
            store
                .move_tag(&tag_id, group_id.as_deref(), index)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "delete_tag" => {
            let store = get_store(store_mutex)?;
            let tag_id: String = arg(&args, "tagId")?;
            store.delete_tag(&tag_id).map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "set_project_group_tag" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let group_id: String = arg(&args, "groupId")?;
            let tag_id: Option<String> = opt_arg(&args, "tagId")?;
            store
                .set_project_group_tag(&project_id, &group_id, tag_id.as_deref())
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "set_project_tag_assigned" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let tag_id: String = arg(&args, "tagId")?;
            let assigned: bool = arg(&args, "assigned")?;
            store
                .set_project_tag_assigned(&project_id, &tag_id, assigned)
                .map_err(|e| e.to_string())?;
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
                        pinned: false,
                        pin_sort_order: None,
                        default_branch: None,
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
        // Pinned repos
        // =====================================================================
        "pin_repo" => {
            let store = get_store(store_mutex)?;
            let github_repo: String = arg(&args, "githubRepo")?;
            let subpath: String = arg(&args, "subpath")?;
            // The backfill may ask `gh` or git for the default branch, so the
            // whole arm runs on the blocking pool.
            blocking_json(move || {
                store
                    .pin_repo(&github_repo, &subpath)
                    .map_err(|e| e.to_string())?;
                // Backfill default_branch if not yet detected
                if let Ok(Some(badge)) = store.get_repo_badge(&github_repo, &subpath) {
                    if badge.default_branch.is_none() {
                        if let Err(e) =
                            crate::detect_and_store_default_branch(&store, &github_repo, &subpath)
                        {
                            log::warn!("[pin_repo] failed to backfill default_branch: {e}");
                        }
                    }
                }
                Ok::<_, String>(())
            })
            .await
        }
        "unpin_repo" => {
            let store = get_store(store_mutex)?;
            let github_repo: String = arg(&args, "githubRepo")?;
            let subpath: String = arg(&args, "subpath")?;
            store
                .unpin_repo(&github_repo, &subpath)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "reorder_pinned_repos" => {
            let store = get_store(store_mutex)?;
            let ordered_keys: Vec<(String, String)> = arg(&args, "orderedKeys")?;
            store
                .reorder_pinned_repos(&ordered_keys)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "list_repos_for_home" => {
            let store = get_store(store_mutex)?;
            let badges = store.list_repos_for_home().map_err(|e| e.to_string())?;
            let items: Vec<crate::RepoHomeItem> = badges
                .into_iter()
                .map(|badge| {
                    let has_local_clone = crate::paths::clone_path_for(&badge.github_repo)
                        .map(|p| p.join(".git").exists())
                        .unwrap_or(false);
                    crate::RepoHomeItem {
                        badge,
                        has_local_clone,
                    }
                })
                .collect();
            Ok(serde_json::to_value(items).unwrap())
        }
        "set_repo_default_branch" => {
            let store = get_store(store_mutex)?;
            let github_repo: String = arg(&args, "githubRepo")?;
            let subpath: String = arg(&args, "subpath")?;
            let default_branch: String = arg(&args, "defaultBranch")?;
            store
                .set_default_branch(&github_repo, &subpath, &default_branch)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "detect_default_branch" => {
            let store = get_store(store_mutex)?;
            let github_repo: String = arg(&args, "githubRepo")?;
            let subpath: String = arg(&args, "subpath")?;
            blocking_json(move || {
                crate::detect_and_store_default_branch(&store, &github_repo, &subpath)
            })
            .await
        }
        "get_repo_default_branch_timeline" => {
            let store = get_store(store_mutex)?;
            let github_repo: String = arg(&args, "githubRepo")?;
            let subpath: String = arg(&args, "subpath")?;
            let limit: Option<usize> = opt_arg(&args, "limit")?;
            let result = tokio::task::spawn_blocking(move || {
                let default_branch =
                    crate::detect_and_store_default_branch(&store, &github_repo, &subpath)?;
                let clone_path = crate::paths::clone_path_for(&github_repo)
                    .ok_or_else(|| "Cannot determine clone path".to_string())?;
                if !clone_path.join(".git").exists() {
                    return Err(format!(
                        "No local clone found for {github_repo}. Clone the repo first."
                    ));
                }
                let max_count = limit.unwrap_or(50);
                let origin_ref = format!("origin/{default_branch}");
                let limit_arg = format!("-{max_count}");
                let output = crate::git::cli_run(
                    &clone_path,
                    &[
                        "log",
                        &limit_arg,
                        crate::git::BRANCH_COMMIT_LOG_FORMAT,
                        &origin_ref,
                    ],
                )
                .map_err(|e| format!("Failed to get commits: {e}"))?;
                let commits: Vec<crate::CommitTimelineItem> = output
                    .lines()
                    .enumerate()
                    .filter_map(|(i, line)| {
                        let fields = crate::git::parse_branch_commit_line(line)?;
                        Some(crate::CommitTimelineItem {
                            id: None,
                            sha: fields.sha.to_string(),
                            short_sha: fields.short_sha.to_string(),
                            subject: fields.subject.to_string(),
                            author: fields.author.to_string(),
                            author_email: fields.author_email.to_string(),
                            // Committer time, as in the Tauri command this mirrors.
                            timestamp: fields.committer_timestamp,
                            sort_timestamp: fields.committer_timestamp,
                            order: (max_count - 1 - i) as i64,
                            session_id: None,
                            session_status: None,
                            completion_reason: None,
                            session_error_message: None,
                            pipeline_kind: None,
                            rate_limit_resets_at: None,
                            rate_limit_kind: None,
                            is_own_commit: false,
                        })
                    })
                    .collect();
                Ok(crate::RepoDefaultBranchTimeline {
                    commits,
                    default_branch,
                })
            })
            .await
            .map_err(|e| format!("task failed: {e}"))??;
            Ok(serde_json::to_value(result).unwrap())
        }
        "clone_repo_locally" => {
            let store = get_store(store_mutex)?;
            let github_repo: String = arg(&args, "githubRepo")?;
            let github_repo_clone = github_repo.clone();
            let clone_path = tokio::task::spawn_blocking(move || {
                crate::git::ensure_local_clone(&github_repo_clone).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| format!("Clone task failed: {e}"))??;
            // Detect default branch
            let store_clone = std::sync::Arc::clone(&store);
            let github_repo_ref = github_repo.clone();
            let default_branch = tokio::task::spawn_blocking(move || {
                let branch = crate::git::detect_default_branch_from_remote(&clone_path)
                    .unwrap_or_else(|_| "main".to_string());
                let badges = store_clone.list_repo_badges().unwrap_or_default();
                for badge in &badges {
                    if badge.github_repo == github_repo_ref && badge.default_branch.is_none() {
                        let _ = store_clone.set_default_branch(
                            &github_repo_ref,
                            &badge.subpath,
                            &branch,
                        );
                    }
                }
                branch
            })
            .await
            .map_err(|e| format!("Default branch detection failed: {e}"))?;
            emit_to_all(app_handle, "repo-cloned", &github_repo);
            Ok(serde_json::to_value(default_branch).unwrap())
        }
        "get_repo_clone_path" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            let path = crate::paths::clone_path_for(&github_repo)
                .ok_or_else(|| "Cannot determine clone path (no home directory)".to_string())?;
            Ok(serde_json::to_value(path.to_string_lossy().into_owned()).unwrap())
        }

        // =====================================================================
        // GitHub commands
        // =====================================================================
        "list_github_orgs" => blocking_json(crate::git::list_github_orgs).await,
        "list_github_repos" => {
            let owner: Option<String> = opt_arg(&args, "owner")?;
            blocking_json(move || crate::git::list_github_repos(owner.as_deref())).await
        }
        "list_user_repos" => {
            let limit: Option<u32> = opt_arg(&args, "limit")?;
            blocking_json(move || crate::git::list_user_repos(limit.unwrap_or(30))).await
        }
        "get_github_repo" => {
            let owner: String = arg(&args, "owner")?;
            let repo: String = arg(&args, "repo")?;
            blocking_json(move || crate::git::fetch_github_repo(&owner, &repo)).await
        }
        "search_github_repos" => {
            let query: String = arg(&args, "query")?;
            let owner: Option<String> = opt_arg(&args, "owner")?;
            blocking_json(move || crate::git::search_github_repos(&query, owner.as_deref())).await
        }
        "check_monorepo_modules" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            blocking_json(move || crate::git::check_monorepo_modules(&github_repo)).await
        }
        "validate_subpath" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            let subpath: String = arg(&args, "subpath")?;
            blocking_json(move || crate::git::validate_subpath_in_repo(&github_repo, &subpath))
                .await
        }
        "list_repo_directories" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            let path: String = arg(&args, "path")?;
            blocking_json(move || crate::git::list_repo_directories(&github_repo, &path)).await
        }
        "list_git_branches" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            blocking_json(move || crate::git::list_branches_for_repo(&github_repo)).await
        }
        "detect_default_branch_cmd" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            blocking_json(move || crate::git::detect_default_branch_for_repo(&github_repo)).await
        }
        "prune_remote_refs" => {
            let github_repo: String = arg(&args, "githubRepo")?;
            blocking_json(move || crate::git::prune_remote_for_repo(&github_repo)).await
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
            let repo_path = project
                .primary_repo()
                .and_then(crate::paths::clone_path_for)
                .filter(|p| p.exists());
            let branch_name = trimmed.to_owned();
            let result = tauri::async_runtime::spawn_blocking(move || {
                repo_path
                    .map(|repo_path| {
                        crate::git::branch_exists(&repo_path, &branch_name).unwrap_or(false)
                    })
                    .unwrap_or(false)
            })
            .await
            .map_err(|e| e.to_string())?;
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
            crate::project_transfer::ensure_project_not_transferring(&project_id)?;

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
            let id: String = arg(&args, "branchId")?;
            let services = crate::branch_teardown::TeardownServices {
                store: get_store(store_mutex)?,
                action_executor: Arc::clone(action_executor),
                action_registry: Arc::clone(action_registry),
                session_registry: Arc::clone(session_registry),
                app_handle: app_handle.clone(),
            };
            detached(crate::branch_teardown::delete_branch_impl(services, id)).await?;
            Ok(Value::Null)
        }
        "rename_branch" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let branch_name: String = arg(&args, "branchName")?;
            let branch = store
                .get_branch(&branch_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
            // The web server never touches git, so the checkout's upstream —
            // where the branch pushes — is untouched by construction; only the
            // stored name (and the project-repo row) moves.
            crate::branches::record_branch_rename(&store, &branch, &branch_name)?;
            Ok(Value::Null)
        }
        "move_branch" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let target_project_id: String = arg(&args, "targetProjectId")?;
            let moved = crate::branches::move_branch_impl(
                &store,
                action_executor,
                action_registry,
                &branch_id,
                &target_project_id,
            )
            .await?;
            Ok(serde_json::to_value(moved).unwrap())
        }
        "get_project_transfer_preflight" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let preflight =
                crate::project_transfer::get_project_transfer_preflight_impl(store, project_id)
                    .await?;
            Ok(serde_json::to_value(preflight).unwrap())
        }
        "start_project_transfer_preflight" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let scan_id: String = arg(&args, "scanId")?;
            crate::project_transfer::start_project_transfer_preflight_impl(
                store,
                app_handle.clone(),
                project_id,
                scan_id,
            )
            .await?;
            Ok(serde_json::Value::Null)
        }
        "cancel_project_transfer_preflight" => {
            let scan_id: String = arg(&args, "scanId")?;
            Ok(serde_json::Value::Bool(
                crate::project_transfer::cancel_project_transfer_preflight_impl(&scan_id),
            ))
        }
        "transfer_project" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            // The way the client's preflight showed the project going.
            let direction: crate::project_transfer::TransferDirection = arg(&args, "direction")?;
            let include_uncommitted: bool = arg(&args, "includeUncommitted")?;
            // Optional, so a client from before ignored files could move
            // still transfers (without them).
            let include_ignored: bool = opt_arg(&args, "includeIgnored")?.unwrap_or(false);
            // Detached: the transfer writes workspace statuses and the
            // project's location, and must finish (or roll back) whether or
            // not the client is still waiting.
            let result = detached(crate::project_transfer::transfer_project_impl(
                store,
                app_handle.clone(),
                Arc::clone(action_executor),
                Arc::clone(action_registry),
                project_id,
                direction,
                crate::project_transfer::TransferOptions {
                    include_uncommitted,
                    include_ignored,
                },
            ))
            .await?;
            Ok(serde_json::to_value(result).unwrap())
        }
        "cancel_project_transfer" => {
            let project_id: String = arg(&args, "projectId")?;
            Ok(
                serde_json::to_value(crate::project_transfer::cancel_project_transfer(project_id))
                    .unwrap(),
            )
        }
        "list_transferring_projects" => Ok(serde_json::to_value(
            crate::project_transfer::list_transferring_projects(),
        )
        .unwrap()),
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
            let changed = store
                .update_branch_workspace_status(&branch_id, &new_status)
                .map_err(|e| e.to_string())?;
            crate::branches::workspace_status_written(app_handle, &branch_id, changed, &new_status);
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
                        if let Ok(changed) =
                            store.update_branch_workspace_status(branch_id, &new_status)
                        {
                            crate::branches::workspace_status_written(
                                app_handle,
                                branch_id,
                                changed,
                                &new_status,
                            );
                        }
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
            let result =
                crate::branches::setup_worktree_impl(store, app_handle.clone(), branch_id).await?;
            Ok(serde_json::to_value(result).unwrap())
        }
        "setup_worktree_and_run_prerun" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let app_handle = app_handle.clone();

            // Detached so prerun is still kicked off when the client goes away
            // while the worktree is being set up.
            let result = detached(async move {
                let result = crate::branches::setup_worktree_impl(
                    Arc::clone(&store),
                    app_handle.clone(),
                    branch_id.clone(),
                )
                .await?;

                // Prerun runs detached: this is an HTTP request with no timeout
                // layer in front of it, and prerun can take minutes.
                crate::branches::spawn_prerun_actions(
                    store,
                    app_handle,
                    branch_id,
                    provider,
                    "setup_worktree_and_run_prerun",
                );
                Ok(result)
            })
            .await?;

            Ok(serde_json::to_value(result).unwrap())
        }
        "setup_worktree_from_pr" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let pr_number: u64 = arg(&args, "prNumber")?;
            let head_ref: String = arg(&args, "headRef")?;
            let base_ref: String = arg(&args, "baseRef")?;
            let project_repo_id: Option<String> = opt_arg(&args, "projectRepoId")?;
            let result = crate::branches::setup_worktree_from_pr_impl(
                store,
                project_id,
                pr_number,
                head_ref,
                base_ref,
                project_repo_id,
            )
            .await?;
            Ok(serde_json::to_value(result).unwrap())
        }
        "create_remote_branch" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let branch_name: String = arg(&args, "branchName")?;
            let base_branch: Option<String> = opt_arg(&args, "baseBranch")?;
            let workspace_name: String = arg(&args, "workspaceName")?;
            let project_repo_id: Option<String> = opt_arg(&args, "projectRepoId")?;
            let result = crate::branches::create_remote_branch_impl(
                store,
                project_id,
                branch_name,
                base_branch,
                workspace_name,
                project_repo_id,
            )
            .await?;
            Ok(serde_json::to_value(result).unwrap())
        }
        "start_workspace" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            crate::project_transfer::ensure_branch_not_transferring(&branch_id)?;
            let app_handle = app_handle.clone();
            detached(async move {
                let branch = store
                    .get_branch(&branch_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

                let project = store
                    .get_project(&branch.project_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

                let ws_name = branch
                    .workspace_name
                    .as_deref()
                    .ok_or("Branch has no workspace name")?;
                let repo_subpath = crate::branches::resolve_branch_workspace_subpath(&store, &branch)?;
                let clone_dir = crate::branches::resolve_branch_clone_dir(&store, &branch)?;
                let ref_name = crate::branches::normalize_branch_ref(&branch.base_branch);
                let repo_slug = crate::branches::resolve_branch_repo_slug(&store, &project, &branch)?;

                // Pre-flight auth check
                if let Err(e) = crate::branches::run_blox_blocking(crate::blox::check_auth).await {
                    store
                        .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
                        .ok();
                    return Err(e.to_string());
                }

                // Secondary repo setup in an already-running shared workspace —
                // or a branch re-entering `Starting` on a workstation that
                // already has this clone.
                if let Some(clone_dir) = clone_dir.as_deref() {
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
                                clone_dir,
                                &repo_slug,
                                &ref_name,
                                &branch.branch_name,
                            )
                            .await?;
                            let changed = store
                                .update_branch_workspace_status(
                                    &branch_id,
                                    &store::WorkspaceStatus::Running,
                                )
                                .map_err(|e| e.to_string())?;
                            crate::branches::workspace_status_written(
                                &app_handle,
                                &branch_id,
                                changed,
                                &store::WorkspaceStatus::Running,
                            );
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
                        // Tracked: a transient fetch killed at 60 s would read
                        // as "no remote branch" and silently branch from base.
                        let has_remote_branch = crate::branches::run_workspace_git_async(
                            ws_name,
                            repo_subpath.as_deref(),
                            crate::blox::ExecMode::Tracked,
                            &["fetch", "origin", &branch.branch_name],
                        )
                        .await
                        .is_ok();

                        let remote_ref = format!("origin/{}", branch.branch_name);
                        let checkout_args: &[&str] = if has_remote_branch {
                            &["checkout", "-B", &branch.branch_name, &remote_ref]
                        } else {
                            &["checkout", "-b", &branch.branch_name]
                        };
                        // Tracked: a checkout SIGKILLed at the transient 60 s
                        // limit leaves `.git/index.lock` behind.
                        let checkout_result = crate::branches::run_workspace_git_async(
                            ws_name,
                            repo_subpath.as_deref(),
                            crate::blox::ExecMode::Tracked,
                            checkout_args,
                        )
                        .await;
                        if let Err(e) = checkout_result {
                            log::warn!(
                                "failed to create branch '{}' in workspace '{}': {e}",
                                branch.branch_name,
                                ws_name
                            );
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
            })
            .await
        }
        "resume_workspace" => {
            let store = get_store(store_mutex)?;
            let workspace_name: String = arg(&args, "workspaceName")?;
            crate::project_transfer::ensure_workspace_not_transferring(&workspace_name)?;
            detached(async move {
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
            })
            .await
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
            let pinned: bool = arg(&args, "pinned")?;
            let icon: Option<String> = opt_arg(&args, "icon")?;
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
                run_detection_mode: action.run_detection_mode,
                pinned,
                icon,
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
        "list_all_repo_actions" => {
            let store = get_store(store_mutex)?;
            let contexts = store.list_all_repo_actions().map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(contexts).unwrap())
        }
        "create_repo_action" => {
            let store = get_store(store_mutex)?;
            let github_repo: String = arg(&args, "githubRepo")?;
            let subpath: Option<String> = opt_arg(&args, "subpath")?;
            let name: String = arg(&args, "name")?;
            let command_str: String = arg(&args, "command")?;
            let action_type: String = arg(&args, "actionType")?;
            let sort_order: i32 = arg(&args, "sortOrder")?;
            let pinned: bool = arg(&args, "pinned")?;
            let icon: Option<String> = opt_arg(&args, "icon")?;
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
            .with_pinned(pinned)
            .with_icon(icon);
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
        "run_repo_action" => {
            let store = get_store(store_mutex)?;
            let github_repo: String = arg(&args, "githubRepo")?;
            let subpath: Option<String> = opt_arg(&args, "subpath")?;
            let action_id: String = arg(&args, "actionId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let execution_id = crate::actions::commands::run_repo_action_impl(
                github_repo,
                subpath,
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
        "get_all_running_actions" => {
            let actions = crate::actions::commands::get_all_running_actions_impl(
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
        "get_project_timeline" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let timeline = tauri::async_runtime::spawn_blocking(move || {
                crate::timeline::build_project_timeline(&store, &project_id)
            })
            .await
            .map_err(|e| format!("Project timeline task failed: {e}"))??;
            Ok(serde_json::to_value(timeline).unwrap())
        }
        "refresh_branch_git_state" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let force: Option<bool> = opt_arg(&args, "force")?;
            let app = app_handle.clone();
            tauri::async_runtime::spawn_blocking(move || {
                crate::timeline::refresh_branch_git_state_impl(&app, &store, &branch_id, force)
            })
            .await
            .map_err(|e| format!("Git state refresh task failed: {e}"))??;
            Ok(Value::Null)
        }
        "list_parent_branch_commits" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let commits = tauri::async_runtime::spawn_blocking(move || {
                crate::timeline::list_parent_branch_commits_impl(&store, &branch_id)
            })
            .await
            .map_err(|e| format!("List parent branch commits task failed: {e}"))??;
            Ok(serde_json::to_value(commits).unwrap())
        }
        "pull_or_queue_branch" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            // Returns the queued session id, or null when the pull ran now.
            let queued_session_id = detached(crate::prs::pull_or_queue_branch_for_branch(
                store,
                Arc::clone(session_registry),
                app_handle.clone(),
                branch_id,
            ))
            .await?;
            Ok(serde_json::to_value(queued_session_id).unwrap())
        }
        "reset_branch_to_remote" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            tauri::async_runtime::spawn_blocking(move || {
                crate::timeline::reset_branch_to_remote_impl(&store, &branch_id)
            })
            .await
            .map_err(|e| format!("Reset task failed: {e}"))??;
            Ok(Value::Null)
        }
        "get_worktree_changes_preview" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let preview = tauri::async_runtime::spawn_blocking(move || {
                crate::timeline::get_worktree_changes_preview_impl(&store, &branch_id)
            })
            .await
            .map_err(|e| format!("Worktree preview task failed: {e}"))??;
            Ok(serde_json::to_value(preview).unwrap())
        }
        "discard_worktree_changes" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let expected_preview: Option<crate::timeline::WorktreeChangesPreview> =
                opt_arg(&args, "expectedPreview")?;
            tauri::async_runtime::spawn_blocking(move || {
                crate::timeline::discard_worktree_changes_impl(&store, &branch_id, expected_preview)
            })
            .await
            .map_err(|e| format!("Discard task failed: {e}"))??;
            Ok(Value::Null)
        }

        // =====================================================================
        // Notes
        // =====================================================================
        "create_note" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let title: String = arg(&args, "title")?;
            let content: String = arg(&args, "content")?;
            let subtype: Option<String> = opt_arg(&args, "subtype")?;
            let item = crate::note_commands::create_standalone_note(
                &store, &branch_id, &title, &content, subtype,
            )?;
            Ok(serde_json::to_value(item).unwrap())
        }
        "update_note" => {
            let store = get_store(store_mutex)?;
            let note_id: String = arg(&args, "noteId")?;
            let title: String = arg(&args, "title")?;
            let content: String = arg(&args, "content")?;
            let item = crate::note_commands::update_note_impl(&store, &note_id, &title, &content)?;
            Ok(serde_json::to_value(item).unwrap())
        }
        "delete_note" => {
            let store = get_store(store_mutex)?;
            let note_id: String = arg(&args, "noteId")?;
            let delete_session_flag: Option<bool> = opt_arg(&args, "deleteSession")?;
            let app = app_handle.clone();
            tauri::async_runtime::spawn_blocking(move || {
                crate::note_commands::delete_note_impl(
                    &store,
                    &app,
                    &note_id,
                    delete_session_flag.unwrap_or(false),
                )
            })
            .await
            .map_err(|e| format!("Note delete task failed: {e}"))??;
            Ok(Value::Null)
        }
        "get_note" => {
            let store = get_store(store_mutex)?;
            let note_id: String = arg(&args, "noteId")?;
            let note = store.get_note(&note_id).map_err(|e| e.to_string())?;
            let item = note.map(|n| crate::note_commands::note_to_timeline_item(&store, n));
            Ok(serde_json::to_value(item).unwrap())
        }
        "set_note_read" => {
            let store = get_store(store_mutex)?;
            let note_id: String = arg(&args, "noteId")?;
            let read: bool = arg(&args, "read")?;
            let note = store
                .set_note_read(&note_id, read)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(note).unwrap())
        }
        "list_child_notes" => {
            let store = get_store(store_mutex)?;
            let parent_project_note_id: String = arg(&args, "parentProjectNoteId")?;
            let notes = store
                .list_child_notes(&parent_project_note_id)
                .map_err(|e| e.to_string())?;
            let items: Vec<crate::NoteTimelineItem> = notes
                .into_iter()
                .map(|n| crate::note_commands::note_to_timeline_item(&store, n))
                .collect();
            Ok(serde_json::to_value(items).unwrap())
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
        "get_project_search_index" => {
            let store = get_store(store_mutex)?;
            let items = store
                .list_project_search_items()
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(items).unwrap())
        }
        "get_project_note" => {
            let store = get_store(store_mutex)?;
            let project_note_id: String = arg(&args, "projectNoteId")?;
            let note = store
                .get_project_note_with_status(&project_note_id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(note).unwrap())
        }
        "get_project_note_by_session" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let note = store
                .get_project_note_by_session_with_status(&session_id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(note).unwrap())
        }
        "get_branch_note_by_session" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let note = store
                .get_note_by_session(&session_id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(note).unwrap())
        }
        "set_project_note_read" => {
            let store = get_store(store_mutex)?;
            let project_note_id: String = arg(&args, "projectNoteId")?;
            let read: bool = arg(&args, "read")?;
            let note = store
                .set_project_note_read(&project_note_id, read)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(note).unwrap())
        }
        "delete_project_note" => {
            let store = get_store(store_mutex)?;
            let note_id: String = arg(&args, "noteId")?;
            let registry = Arc::clone(session_registry);
            tauri::async_runtime::spawn_blocking(move || {
                crate::note_commands::delete_project_note_impl(&store, &registry, &note_id)
            })
            .await
            .map_err(|e| format!("Project note delete task failed: {e}"))??;
            Ok(Value::Null)
        }

        "get_note_repo" => {
            let store = get_store(store_mutex)?;
            let note_id: String = arg(&args, "noteId")?;
            let repo = crate::note_transfer::branch_note_repo(&store, &note_id)?;
            Ok(serde_json::to_value(repo).unwrap())
        }
        "move_note_to_project" | "copy_note_to_project" => {
            let store = get_store(store_mutex)?;
            let note_id: String = arg(&args, "noteId")?;
            let target_project_id: String = arg(&args, "targetProjectId")?;
            let mode = if command == "move_note_to_project" {
                crate::note_transfer::TransferMode::Move
            } else {
                crate::note_transfer::TransferMode::Copy
            };
            let note = crate::note_transfer::transfer_branch_note(
                &store,
                &note_id,
                &target_project_id,
                mode,
                |key| {
                    crate::note_transfer::add_repo_to_project(
                        app_handle.clone(),
                        Arc::clone(&store),
                        target_project_id.clone(),
                        key,
                    )
                },
            )
            .await?;
            let item = match mode {
                crate::note_transfer::TransferMode::Move => {
                    crate::note_commands::note_to_timeline_item(&store, note)
                }
                crate::note_transfer::TransferMode::Copy => {
                    crate::note_commands::standalone_note_to_timeline_item(note)
                }
            };
            Ok(serde_json::to_value(item).unwrap())
        }
        "move_project_note_to_project" | "copy_project_note_to_project" => {
            let store = get_store(store_mutex)?;
            let project_note_id: String = arg(&args, "projectNoteId")?;
            let target_project_id: String = arg(&args, "targetProjectId")?;
            let mode = if command == "move_project_note_to_project" {
                crate::note_transfer::TransferMode::Move
            } else {
                crate::note_transfer::TransferMode::Copy
            };
            let note = crate::note_transfer::transfer_project_note(
                &store,
                &project_note_id,
                &target_project_id,
                mode,
                |key| {
                    crate::note_transfer::add_repo_to_project(
                        app_handle.clone(),
                        Arc::clone(&store),
                        target_project_id.clone(),
                        key,
                    )
                },
            )
            .await?;
            Ok(serde_json::to_value(note).unwrap())
        }

        // =====================================================================
        // Images
        // =====================================================================
        "create_image" => {
            // Web callers never reach this arm (`web_caller_rejection` — a
            // filesystem path is only valid same-host); what remains is the
            // control surface, whose args are pinned so it can never create
            // a UI-invisible image.
            let (branch_id, project_id, file_path) = control_create_image_args(&args)?;
            let store = get_store(store_mutex)?;
            let app_handle = app_handle.clone();
            // Reading and validating the file (a 50 MB PDF, say) is not
            // runtime-worker work; the event goes out from the same closure
            // so it follows the write it announces.
            blocking_json(move || {
                let image = crate::image_commands::create_image_impl(
                    store,
                    Some(branch_id),
                    project_id,
                    file_path,
                    Some(false),
                )?;
                crate::image_commands::emit_image_created(&app_handle, &image);
                Ok::<_, String>(image)
            })
            .await
        }
        "create_image_from_data" => {
            let store = get_store(store_mutex)?;
            let branch_id: Option<String> = opt_arg(&args, "branchId")?;
            let project_id: String = arg(&args, "projectId")?;
            let filename: String = arg(&args, "filename")?;
            let mime_type: String = arg(&args, "mimeType")?;
            let data: String = arg(&args, "data")?;
            let pending: Option<bool> = opt_arg(&args, "pending")?;
            let image = tauri::async_runtime::spawn_blocking(move || {
                crate::image_commands::create_image_from_data_impl(
                    store, branch_id, project_id, filename, mime_type, data, pending,
                )
            })
            .await
            .map_err(|e| format!("Image task failed: {e}"))??;
            crate::image_commands::emit_image_created(app_handle, &image);
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
        "read_image_file" => {
            let file_path: String = arg(&args, "filePath")?;
            crate::image_commands::read_image_file(file_path).map(Value::String)
        }
        "delete_image" => {
            let store = get_store(store_mutex)?;
            let image_id: String = arg(&args, "imageId")?;
            let image = store
                .get_image(&image_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Image not found: {image_id}"))?;
            store.delete_image(&image_id).map_err(|e| e.to_string())?;
            crate::store::images::remove_image_files(&image.project_id, &image.id, &image.filename);
            Ok(Value::Null)
        }
        "get_attachment_thumbnail" => {
            let store = get_store(store_mutex)?;
            let image_id: String = arg(&args, "imageId")?;
            let thumbnail = tauri::async_runtime::spawn_blocking(move || {
                crate::image_commands::attachment_thumbnail_impl(&store, &image_id)
            })
            .await
            .map_err(|e| format!("Thumbnail task failed: {e}"))??;
            Ok(serde_json::to_value(thumbnail).unwrap())
        }
        "list_branch_images" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let images = store
                .list_images_for_branch(&branch_id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(images).unwrap())
        }
        "get_media_dimensions" => {
            let store = get_store(store_mutex)?;
            let image_ids: Vec<String> = arg(&args, "imageIds")?;
            let dimensions = tauri::async_runtime::spawn_blocking(move || {
                crate::image_commands::media_dimensions_impl(&store, &image_ids)
            })
            .await
            .map_err(|e| format!("Dimensions task failed: {e}"))?;
            Ok(serde_json::to_value(dimensions).unwrap())
        }

        // =====================================================================
        // Timeline delete commands
        // =====================================================================
        "delete_review" => {
            let store = get_store(store_mutex)?;
            let review_id: String = arg(&args, "reviewId")?;
            let delete_session_flag: Option<bool> = opt_arg(&args, "deleteSession")?;
            crate::timeline::delete_review_impl(
                &store,
                app_handle,
                &review_id,
                delete_session_flag.unwrap_or(false),
            )?;
            Ok(Value::Null)
        }
        "delete_pending_commit" => {
            let store = get_store(store_mutex)?;
            let commit_id: String = arg(&args, "commitId")?;
            let delete_session_flag: Option<bool> = opt_arg(&args, "deleteSession")?;
            crate::timeline::delete_pending_commit_impl(
                &store,
                session_registry,
                app_handle,
                &commit_id,
                delete_session_flag.unwrap_or(false),
            )?;
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
            blocking_json(move || {
                crate::diff_commands::get_diff_files_impl(store, branch_id, commit_sha, scope)
            })
            .await
        }
        "get_file_diff" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let commit_sha: String = arg(&args, "commitSha")?;
            let scope: String = arg(&args, "scope")?;
            let path: String = arg(&args, "path")?;
            blocking_json(move || {
                crate::diff_commands::get_file_diff_impl(store, branch_id, commit_sha, scope, path)
            })
            .await
        }
        "get_file_at_ref" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let ref_name: String = arg(&args, "refName")?;
            let path: String = arg(&args, "path")?;
            blocking_json(move || {
                crate::diff_commands::get_file_at_ref_impl(store, branch_id, ref_name, path)
            })
            .await
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
        "link_comment_session" => {
            let store = get_store(store_mutex)?;
            let comment_id: String = arg(&args, "commentId")?;
            let session_id: String = arg(&args, "sessionId")?;
            let session_type: String = arg(&args, "sessionType")?;
            store
                .set_comment_session(&comment_id, &session_type, &session_id)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "get_branch_commit_by_session" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let commit = store
                .get_commit_by_session(&session_id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(commit.and_then(|c| c.sha)).unwrap())
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
        "discover_acp_config" => {
            let provider_id: String = arg(&args, "providerId")?;
            let working_dir: Option<String> = opt_arg(&args, "workingDir")?;
            let force: Option<bool> = opt_arg(&args, "force")?;
            let selected_model_value: Option<String> = opt_arg(&args, "selectedModelValue")?;
            let config = crate::session_commands::discover_acp_config(
                provider_id,
                working_dir,
                force,
                selected_model_value,
            )
            .await?;
            Ok(serde_json::to_value(config).unwrap())
        }
        "get_active_sessions" => {
            let store = get_store(store_mutex)?;
            let mut sessions =
                crate::session_commands::get_active_sessions_impl(&store, session_registry)?;
            // A project-scoped control credential sees only its own
            // project's sessions.
            if let Some(project_id) = control_scope::project_filter(&caller) {
                sessions.retain(|session| session.project_id.as_deref() == Some(project_id));
            }
            Ok(serde_json::to_value(sessions).unwrap())
        }
        "list_child_sessions" => {
            let store = get_store(store_mutex)?;
            let parent_session_id: String = arg(&args, "parentSessionId")?;
            let children = store
                .list_active_child_sessions(&parent_session_id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(children).unwrap())
        }
        "get_session" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            blocking_store_json(move || store.get_session(&session_id)).await
        }
        "get_session_messages" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            // Both optional: absent means the whole transcript, which is what
            // every caller other than the windowed session pane still asks for.
            let window = store::TranscriptWindow {
                limit: opt_arg(&args, "limit")?,
                before_id: opt_arg(&args, "beforeId")?,
            };
            blocking_store_json(move || store.get_session_messages_window(&session_id, window))
                .await
        }
        "get_session_messages_since" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let since_id: i64 = arg(&args, "sinceId")?;
            // Optional so an older frontend bundle keeps working against a
            // newer server.
            let refetch_ids: Vec<i64> = opt_arg(&args, "refetchIds")?.unwrap_or_default();
            blocking_store_json(move || {
                store.get_session_messages_since(&session_id, since_id, &refetch_ids)
            })
            .await
        }
        "count_assistant_messages_after" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let after_timestamp: i64 = arg(&args, "afterTimestamp")?;
            let count = store
                .count_assistant_messages_after(&session_id, after_timestamp)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(count).unwrap())
        }
        "mark_session_chat_read" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            store
                .mark_session_chat_read(&session_id)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "get_session_acp_metadata_messages" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            blocking_store_json(move || store.get_session_acp_metadata_messages(&session_id)).await
        }
        "get_session_acp_metadata_messages_since" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let since_id: i64 = arg(&args, "sinceId")?;
            let refetch_ids: Vec<i64> = arg(&args, "refetchIds")?;
            blocking_store_json(move || {
                store.get_session_acp_metadata_messages_since(&session_id, since_id, &refetch_ids)
            })
            .await
        }
        // The session pane's whole tick in one request. The standalone arms
        // above stay for the timeline's live hints and other callers.
        "poll_session" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let request: store::SessionPollRequest = arg(&args, "request")?;
            blocking_store_json(move || store.poll_session(&session_id, &request)).await
        }
        "get_session_acp_initialization" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let metadata = store
                .get_session_acp_initialization(&session_id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(metadata).unwrap())
        }
        // The browser normally loads `/api/tool-images/…` straight into an
        // `<img>`; this arm keeps the command callable from both transports.
        "get_tool_image" => {
            let store = get_store(store_mutex)?;
            let message_id: i64 = arg(&args, "messageId")?;
            let field: String = arg(&args, "field")?;
            let index: usize = arg(&args, "index")?;
            tokio::task::spawn_blocking(move || {
                session_commands::tool_image_data_url(&store, message_id, &field, index)
                    .map(Value::String)
            })
            .await
            .map_err(|e| format!("task failed: {e}"))?
        }
        // The browser normally fetches `/api/tool-bodies/{id}` directly; this
        // arm keeps the command callable from both transports. A body can be
        // hundreds of KB, so it is read and serialized off the runtime thread.
        "get_tool_call_body" => {
            let store = get_store(store_mutex)?;
            let message_id: i64 = arg(&args, "messageId")?;
            blocking_store_json(move || session_commands::tool_call_body(&store, message_id)).await
        }
        "start_session" => {
            let store = get_store(store_mutex)?;
            let prompt: String = arg(&args, "prompt")?;
            let working_dir: String = arg(&args, "workingDir")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let acp_config_selection: Option<store::AcpConfigSelection> =
                opt_arg(&args, "acpConfigSelection")?;

            let working_dir = std::path::PathBuf::from(working_dir);
            let mut session = store::Session::new_running(&prompt, &working_dir);
            if let Some(selection) = acp_config_selection.clone() {
                session = session.with_acp_config_selection(selection);
            }
            if let Some(ref p) = provider {
                session = session.with_provider(p);
            }
            // Live from before the `running` write until the runner has
            // registered (see `SessionRegistry::hold`).
            let _liveness = session_registry.hold(&session.id);
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
                    queued_message_id: None,
                    acp_config_selection,
                    branch_id: None,
                    project_id: None,
                    expose_pikchr_tools: false,
                    parent_project_note_id: None,
                    background_hold: session_commands::default_background_hold(),
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
            let acp_config_selection: Option<store::AcpConfigSelection> =
                opt_arg(&args, "acpConfigSelection")?;
            let app_handle = app_handle.clone();
            let session_registry = Arc::clone(session_registry);
            let action_executor = Arc::clone(action_executor);
            let action_registry = Arc::clone(action_registry);
            // The desktop command's path, `prepare_resume` included: a
            // resume waits out a Stop's backfill, and refuses a
            // `disconnected` row. Detached, so a client that goes mid-resume
            // can't leave the row claimed with no run behind it.
            detached(async move {
                let outcome = session_commands::resume_or_queue_session_for_store(
                    store,
                    session_registry,
                    action_executor,
                    action_registry,
                    app_handle,
                    session_id,
                    prompt,
                    image_ids,
                    branch_id,
                    acp_config_selection,
                )
                .await?;
                Ok(serde_json::to_value(outcome).unwrap())
            })
            .await
        }
        "queue_session_message" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let content: String = arg(&args, "content")?;
            let image_ids: Option<Vec<String>> = opt_arg(&args, "imageIds")?;
            let branch_id: Option<String> = opt_arg(&args, "branchId")?;
            session_commands::ensure_session_chat_writable(&store, &session_id)?;
            let message = store
                .add_queued_session_message(
                    &session_id,
                    &content,
                    &image_ids.unwrap_or_default(),
                    branch_id.as_deref(),
                )
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(message).unwrap())
        }
        "list_queued_session_messages" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            blocking_store_json(move || store.list_queued_session_messages(&session_id)).await
        }
        "delete_queued_session_message" => {
            let store = get_store(store_mutex)?;
            let id: String = arg(&args, "id")?;
            let deleted = store
                .delete_queued_session_message(&id)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(deleted).unwrap())
        }
        "send_queued_session_message" => {
            let store = get_store(store_mutex)?;
            let id: String = arg(&args, "id")?;
            session_commands::send_queued_session_message_for_store(
                store,
                Arc::clone(session_registry),
                Arc::clone(action_executor),
                Arc::clone(action_registry),
                app_handle.clone(),
                id,
            )
            .await?;
            Ok(Value::Null)
        }
        "send_session_message_now" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let content: String = arg(&args, "content")?;
            let image_ids: Option<Vec<String>> = opt_arg(&args, "imageIds")?;
            let session_registry = Arc::clone(session_registry);
            // Detached: interrupt-and-reprompt is two agent round trips, and
            // a client that disconnects between them must not leave the turn
            // cancelled with the re-prompt never sent.
            let delivery = detached(async move {
                let outcome = session_commands::send_session_message_now_for_store(
                    &store,
                    &session_registry,
                    &session_id,
                    &content,
                    &image_ids.unwrap_or_default(),
                )
                .await?;
                Ok(session_commands::SessionMessageDelivery::from(&outcome))
            })
            .await?;
            Ok(serde_json::to_value(delivery).unwrap())
        }
        "get_session_live_delivery" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let capability =
                session_commands::live_delivery_for_session(&store, session_registry, &session_id)?;
            Ok(serde_json::to_value(capability).unwrap())
        }
        "build_note_followup_message" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            let branch_id: Option<String> = opt_arg(&args, "branchId")?;
            let has_parsed_note: bool = arg(&args, "hasParsedNote")?;
            let app = app_handle.clone();
            let message = tauri::async_runtime::spawn_blocking(move || {
                session_commands::build_note_followup_message_impl(
                    &store,
                    &app,
                    &session_id,
                    branch_id.as_deref(),
                    has_parsed_note,
                )
            })
            .await
            .map_err(|e| format!("Failed to build note follow-up message: {e}"))??;
            Ok(serde_json::to_value(message).unwrap())
        }
        "start_branch_session" | "start_or_queue_branch_session" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let prompt: String = arg(&args, "prompt")?;
            let session_type: session_commands::BranchSessionType = arg(&args, "sessionType")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let image_ids: Option<Vec<String>> = opt_arg(&args, "imageIds")?;
            let launch_context: Option<session_commands::BranchSessionLaunchContext> =
                opt_arg(&args, "launchContext")?;
            let acp_config_selection: Option<store::AcpConfigSelection> =
                opt_arg(&args, "acpConfigSelection")?;

            let result = session_commands::start_or_queue_branch_session_for_store(
                store,
                Arc::clone(session_registry),
                app_handle.clone(),
                branch_id,
                prompt,
                session_type,
                provider,
                image_ids,
                launch_context,
                acp_config_selection,
                caller.started_by(),
            )
            .await?;

            Ok(serde_json::to_value(result).unwrap())
        }
        "start_project_session" => {
            let store = get_store(store_mutex)?;
            let project_id: String = arg(&args, "projectId")?;
            let prompt: String = arg(&args, "prompt")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let image_ids: Option<Vec<String>> = opt_arg(&args, "imageIds")?;
            let acp_config_selection: Option<store::AcpConfigSelection> =
                opt_arg(&args, "acpConfigSelection")?;
            let app_handle = app_handle.clone();
            let session_registry = Arc::clone(session_registry);
            let action_executor = Arc::clone(action_executor);
            let action_registry = Arc::clone(action_registry);
            let started_by = caller.started_by();
            // The desktop command's body. It writes the `running` row and
            // then runs the rest of the start (note stub, runner, failure
            // recording); detached, so a client that goes mid-start can't
            // drop it between the write and its release — whatever that
            // body's await points become.
            detached(async move {
                let result = session_commands::start_project_session_for_store(
                    store,
                    session_registry,
                    action_executor,
                    action_registry,
                    app_handle,
                    project_id,
                    prompt,
                    provider,
                    image_ids,
                    acp_config_selection,
                    started_by,
                )
                .await?;
                Ok(serde_json::to_value(result).unwrap())
            })
            .await
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
            let acp_config_selection: Option<store::AcpConfigSelection> =
                opt_arg(&args, "acpConfigSelection")?;

            // A review without a provider discovers the installed agents.
            let app_handle = app_handle.clone();
            blocking_json(move || {
                session_commands::queue_branch_session_for_store(
                    store,
                    Some(&app_handle),
                    branch_id,
                    prompt,
                    session_type,
                    provider,
                    image_ids,
                    launch_context,
                    acp_config_selection,
                )
            })
            .await
        }
        "start_queued_session_now" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let session_id: String = arg(&args, "sessionId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;

            let result = detached(session_commands::start_queued_session_now_for_branch(
                store,
                Arc::clone(session_registry),
                app_handle.clone(),
                branch_id,
                session_id,
                provider,
            ))
            .await?;

            Ok(serde_json::to_value(result).unwrap())
        }
        "cancel_session" => {
            let session_id: String = arg(&args, "sessionId")?;
            // Shared with the Tauri command so a browser Stop also takes the
            // store-write fallback when the session is no longer registered.
            session_commands::cancel_session_impl(
                session_registry,
                store_mutex,
                app_handle,
                &session_id,
            )?;
            Ok(Value::Null)
        }
        "stop_session_async_task" => {
            let session_id: String = arg(&args, "sessionId")?;
            let task_id: String = arg(&args, "taskId")?;
            let handle = session_registry
                .async_task_stop_handle(&session_id)
                .ok_or_else(|| "Session is not running".to_string())?;
            let stopped = handle.stop(&task_id).await?;
            Ok(serde_json::to_value(stopped).unwrap())
        }
        "reconcile_session" => {
            let session_id: String = arg(&args, "sessionId")?;
            session_runner::reconcile::reconcile_session(session_id)?;
            Ok(Value::Null)
        }
        "get_session_reconcile_states" => {
            let states = session_runner::reconcile::get_session_reconcile_states();
            Ok(serde_json::to_value(states).unwrap())
        }
        "get_session_background_hold" => {
            let session_id: String = arg(&args, "sessionId")?;
            let snapshot: session_runner::SessionBackgroundHoldSnapshot =
                session_registry.background_hold(&session_id).into();
            Ok(serde_json::to_value(snapshot).unwrap())
        }
        "delete_session" => {
            let store = get_store(store_mutex)?;
            let session_id: String = arg(&args, "sessionId")?;
            session_commands::delete_session_impl(
                session_registry,
                &store,
                app_handle,
                &session_id,
            )?;
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
            let session_id = detached(crate::prs::start_create_pr_pipeline_for_branch(
                store,
                Arc::clone(session_registry),
                app_handle.clone(),
                branch_id,
                provider,
                draft,
            ))
            .await?;
            Ok(serde_json::to_value(session_id).unwrap())
        }
        "push_branch" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let force: Option<bool> = opt_arg(&args, "force")?;
            let no_verify: Option<bool> = opt_arg(&args, "noVerify")?;
            let response = detached(crate::prs::start_or_queue_push_pipeline_for_branch(
                store,
                Arc::clone(session_registry),
                app_handle.clone(),
                branch_id,
                provider,
                crate::prs::PushVariant::from_request(force, no_verify),
            ))
            .await?;
            Ok(serde_json::to_value(response).unwrap())
        }
        "defer_branch_push" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let force: Option<bool> = opt_arg(&args, "force")?;
            let no_verify: Option<bool> = opt_arg(&args, "noVerify")?;
            let response = detached(crate::prs::defer_branch_push_for_branch(
                store,
                Arc::clone(session_registry),
                app_handle.clone(),
                branch_id,
                provider,
                crate::prs::PushVariant::from_request(force, no_verify),
            ))
            .await?;
            Ok(serde_json::to_value(response).unwrap())
        }
        "cancel_deferred_push" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            crate::prs::cancel_deferred_push_for_branch(&store, &branch_id)?;
            Ok(Value::Null)
        }
        "rebase_branch" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            // Forwarded so web mode honours "Rebase onto Origin" instead of
            // silently downgrading it to a base rebase.
            let target: Option<String> = opt_arg(&args, "target")?;
            let then_force_push: Option<bool> = opt_arg(&args, "thenForcePush")?;
            let response = detached(crate::prs::start_or_queue_commit_pipeline_for_branch(
                store,
                Arc::clone(session_registry),
                app_handle.clone(),
                branch_id,
                store::PipelineKind::Rebase,
                provider,
                target,
                then_force_push.unwrap_or(false),
            ))
            .await?;
            Ok(serde_json::to_value(response).unwrap())
        }
        "squash_commits" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let provider: Option<String> = opt_arg(&args, "provider")?;
            let response = detached(crate::prs::start_or_queue_commit_pipeline_for_branch(
                store,
                Arc::clone(session_registry),
                app_handle.clone(),
                branch_id,
                store::PipelineKind::Squash,
                provider,
                None,
                false,
            ))
            .await?;
            Ok(serde_json::to_value(response).unwrap())
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
                .update_branch_pr_number(&branch_id, pr_number, None)
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "recover_branch_pr" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let pr_number = crate::pr_detection::recover_branch_pr_impl(store, branch_id).await?;
            Ok(serde_json::to_value(pr_number).unwrap())
        }
        "detect_branch_pr" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let result = crate::pr_detection::detect_branch_pr_impl(store, branch_id).await?;
            Ok(serde_json::to_value(result).unwrap())
        }
        "link_branch_pr" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            let pr_number: u64 = arg(&args, "prNumber")?;
            let linked =
                crate::pr_detection::link_branch_pr_impl(store, branch_id, pr_number).await?;
            Ok(serde_json::to_value(linked).unwrap())
        }
        "refresh_pr_status" => {
            let store = get_store(store_mutex)?;
            let branch_id: String = arg(&args, "branchId")?;
            crate::prs::refresh_pr_status_impl(store, app_handle.clone(), branch_id).await?;
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
                        match store.update_branch_pr_status(
                            &branch.id,
                            Some(pr_number),
                            Some(pr_status.state.clone()),
                            Some(pr_status.checks_summary.state.clone()),
                            pr_status.review_decision.clone(),
                            Some(mergeable),
                            Some(pr_status.is_draft),
                            None,
                            None,
                            pr_status.head_sha.clone(),
                            pr_status.title.clone(),
                        ) {
                            Ok(true) => {}
                            // Relinked or unlinked during the fetch: this
                            // status is another PR's, so don't emit or count
                            // it — neutral, as `BranchRefresh::Skipped` is in
                            // `prs::refresh_project_pr_statuses`.
                            Ok(false) => continue,
                            Err(e) => {
                                log::warn!(
                                    "Failed to update PR status for branch {}: {}",
                                    branch.id,
                                    e
                                );
                                continue;
                            }
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
            crate::prs::clear_branch_pr_status_impl(&store, app_handle, &branch_id)?;
            Ok(Value::Null)
        }

        // =====================================================================
        // PR poll scheduler
        // =====================================================================
        "set_foreground_project" => {
            let client_id = web_client_id(&args)?;
            let project_id: Option<String> = opt_arg(&args, "projectId")?;
            pr_scheduler.set_foreground(client_id, project_id);
            Ok(Value::Null)
        }
        "set_focus" => {
            let client_id = web_client_id(&args)?;
            let focused: bool = arg(&args, "focused")?;
            pr_scheduler.set_focus(client_id, focused);
            Ok(Value::Null)
        }
        "set_branch_pending" => {
            let client_id = web_client_id(&args)?;
            let branch_id: String = arg(&args, "branchId")?;
            let project_id: String = arg(&args, "projectId")?;
            let pending: bool = arg(&args, "pending")?;
            pr_scheduler.set_branch_pending(client_id, branch_id, project_id, pending);
            Ok(Value::Null)
        }
        "refresh_now" => {
            // Validated before either scheduler call: a rejected id must not
            // half-apply (no `touch`, and no `force` either).
            let client_id = web_client_id(&args)?;
            let project_id: String = arg(&args, "projectId")?;
            pr_scheduler.touch(client_id);
            pr_scheduler.force(project_id);
            Ok(Value::Null)
        }
        "disconnect_client" => {
            // Guarded too: this is the reverse-spoofing hole — a web caller
            // tearing down a real native window's interest.
            let client_id = web_client_id(&args)?;
            pr_scheduler.disconnect_client(client_id);
            Ok(Value::Null)
        }

        // =====================================================================
        // Utilities
        // =====================================================================
        "open_url" => {
            // In web mode, the browser handles URL opening
            Err("open_url is handled by the browser in web mode".to_string())
        }
        "is_sq_available" => {
            // The first call looks `sq` up through a login shell.
            let available = tauri::async_runtime::spawn_blocking(crate::blox::is_sq_available)
                .await
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(available).unwrap())
        }
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
        "resolve_path_aliases" => {
            let paths: Vec<String> = arg(&args, "paths")?;
            Ok(
                serde_json::to_value(crate::util_commands::resolve_path_aliases_impl(paths))
                    .unwrap(),
            )
        }
        "preferences_store_path" => {
            let path = crate::preferences_store_path_buf()
                .map(|p| p.to_string_lossy().to_string())
                .ok_or("Cannot determine preferences store path")?;
            Ok(serde_json::to_value(path).unwrap())
        }
        "get_preference" => {
            let key: String = arg(&args, "key")?;
            let store = preferences_store(app_handle)?;
            Ok(store.get(&key).unwrap_or(Value::Null))
        }
        "set_preference" => {
            let key: String = arg(&args, "key")?;
            let value = args
                .get("value")
                .cloned()
                .ok_or("missing argument: value")?;
            let store = preferences_store(app_handle)?;
            store.set(key, value);
            store
                .save()
                .map_err(|e| format!("Failed to save preferences: {e}"))?;
            Ok(Value::Null)
        }
        "list_preference_keys" => {
            let store = preferences_store(app_handle)?;
            serde_json::to_value(store.keys())
                .map_err(|e| format!("Failed to list preference keys: {e}"))
        }
        "delete_preference" => {
            let key: String = arg(&args, "key")?;
            let store = preferences_store(app_handle)?;
            store.delete(&key);
            store
                .save()
                .map_err(|e| format!("Failed to save preferences: {e}"))?;
            Ok(Value::Null)
        }
        "check_blox_auth" => {
            tauri::async_runtime::spawn_blocking(crate::blox::check_auth)
                .await
                .map_err(|e| format!("Failed to run blox auth check: {e}"))?
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "get_available_openers" => {
            let openers = crate::util_commands::get_available_openers().await?;
            Ok(serde_json::to_value(openers).unwrap())
        }
        "open_in_app" => {
            // Web callers never reach this arm (`web_caller_rejection`);
            // for the control surface the app to open in runs on the same
            // machine as the caller.
            let path: String = arg(&args, "path")?;
            let app_id: String = arg(&args, "appId")?;
            crate::util_commands::open_in_app(path, app_id).await?;
            Ok(Value::Null)
        }

        // =====================================================================
        // Doctor
        // =====================================================================
        "run_doctor" => {
            let report = crate::doctor::run_doctor(app_handle.clone()).await;
            Ok(serde_json::to_value(report).unwrap())
        }
        "run_doctor_freshness" => {
            let report = crate::doctor::run_doctor_freshness(app_handle.clone()).await;
            Ok(serde_json::to_value(report).unwrap())
        }
        "start_doctor_login" => {
            let check_id: String = arg(&args, "checkId")?;
            let start = crate::doctor::start_doctor_login(app_handle.clone(), check_id).await?;
            Ok(serde_json::to_value(start).unwrap())
        }
        "cancel_doctor_login" => {
            let check_id: String = arg(&args, "checkId")?;
            let run_id: String = arg(&args, "runId")?;
            let cancelled = crate::doctor::cancel_doctor_login(check_id, run_id).await;
            Ok(Value::Bool(cancelled))
        }
        "doctor_login_status" => {
            let check_id: String = arg(&args, "checkId")?;
            let status = crate::doctor::doctor_login_status(check_id).await;
            Ok(serde_json::to_value(status).unwrap())
        }
        "send_doctor_login_code" => {
            let check_id: String = arg(&args, "checkId")?;
            let run_id: String = arg(&args, "runId")?;
            let code: String = arg(&args, "code")?;
            crate::doctor::send_doctor_login_code(check_id, run_id, code).await?;
            Ok(Value::Null)
        }
        "run_doctor_fix" => {
            let check_id: String = arg(&args, "checkId")?;
            let fix_type: doctor::FixType = arg(&args, "fixType")?;
            crate::doctor::run_doctor_fix(app_handle.clone(), check_id, fix_type).await?;
            Ok(Value::Null)
        }
        "run_doctor_update" => {
            let check_id: String = arg(&args, "checkId")?;
            let fix_type: doctor::FixType = arg(&args, "fixType")?;
            let command: String = arg(&args, "command")?;
            crate::doctor::run_doctor_update(check_id, fix_type, command).await?;
            Ok(Value::Null)
        }

        _ => Err(format!("Unknown command: {command}")),
    }
}

#[cfg(test)]
#[path = "web_server/blocking_guard_tests.rs"]
mod blocking_guard_tests;

#[cfg(test)]
mod tests {
    use super::{
        broadcast_web_event, control_create_image_args, emit_to_all, event_gap_payload,
        normalize_ws_client_id, web_caller_rejection, web_client_id, DispatchCaller, WebEvent,
    };
    use serde_json::{json, Value};
    use std::collections::{BTreeMap, BTreeSet};

    // -------------------------------------------------------------------
    // Caller identity: same-host-only commands are closed to web callers
    // -------------------------------------------------------------------

    /// A control caller's grant names who is starting a session, and both
    /// start arms hand that name to the shared bodies so the row records it;
    /// a web caller is the user and records nothing.
    #[test]
    fn control_callers_record_their_actor_as_started_by_on_both_start_arms() {
        use crate::control_server::{Access, Actor, ControlGrant, Scope};
        let session_caller = DispatchCaller::Control(ControlGrant {
            actor: Actor::Session {
                session_id: "s1".to_string(),
                epoch: 1,
            },
            scope: Scope::Project("p1".to_string()),
            branch_id: None,
            access: Access::ReadWrite,
        });
        assert_eq!(session_caller.started_by().as_deref(), Some("session:s1"));
        let app_caller = DispatchCaller::Control(ControlGrant {
            actor: Actor::App {
                grant_id: "g1".to_string(),
                client: "agent".to_string(),
            },
            scope: Scope::AllProjects,
            branch_id: None,
            access: Access::ReadWrite,
        });
        assert_eq!(app_caller.started_by().as_deref(), Some("app:g1"));
        assert_eq!(DispatchCaller::Web.started_by(), None);

        // Pin the arms: each must pass the caller's actor into the shared
        // start body rather than the desktop command's `None`.
        let source = include_str!("web_server.rs");
        let (_, branch_arm) = dispatch_arms(source)
            .into_iter()
            .find(|(patterns, _)| patterns.iter().any(|p| p == "start_branch_session"))
            .expect("dispatch should have a `start_branch_session` arm");
        assert!(
            branch_arm.contains("caller.started_by()"),
            "start_branch_session must pass caller.started_by(): {branch_arm}"
        );
        let project_arm = dispatch_arm_body(source, "start_project_session");
        assert!(
            project_arm.contains("let started_by = caller.started_by();")
                && project_arm.contains("started_by,\n"),
            "start_project_session must pass caller.started_by(): {project_arm}"
        );
    }

    #[test]
    fn web_callers_cannot_reach_same_host_only_commands() {
        let control =
            DispatchCaller::Control(crate::control_server::ControlGrant::test_all_projects());
        for command in ["create_image", "open_in_app"] {
            assert!(
                web_caller_rejection(command, &DispatchCaller::Web).is_some(),
                "{command} takes local paths / launches local apps and must be \
                 rejected for web callers"
            );
            assert!(
                web_caller_rejection(command, &control).is_none(),
                "{command} must stay reachable from the same-host control server"
            );
        }
        // Everything else is unaffected by caller identity.
        assert_eq!(
            web_caller_rejection("list_projects", &DispatchCaller::Web),
            None
        );

        // The check is only structural if dispatch() actually consults it
        // before running any arm: pin the call site. The needle is assembled
        // at runtime so this assertion's own source can't satisfy it.
        let call_site = format!("web_caller_rejection({}, &caller)", "command");
        assert!(
            include_str!("web_server.rs").contains(&call_site),
            "dispatch() must gate on web_caller_rejection before its match"
        );
    }

    // -------------------------------------------------------------------
    // get_tag_state through the control surface shows one project's tags
    // -------------------------------------------------------------------

    /// A project-scoped control caller gets the whole catalog but only its
    /// own project's assignments; an unconfined caller, and the web, get
    /// everything. Pins that the arm goes through the helper, since
    /// `dispatch` itself can't run in a unit test.
    #[test]
    fn get_tag_state_filters_assignments_for_a_scoped_caller() {
        use super::confine_tag_state;
        use crate::control_server::{Access, Actor, ControlGrant, Scope};
        use crate::store::{self, Store};

        let store = Store::in_memory().unwrap();
        let own = store::Project::named("alpha");
        let other = store::Project::named("beta");
        store.create_project(&own).unwrap();
        store.create_project(&other).unwrap();
        let group = store
            .create_tag_group(
                "Status",
                &[store::NewTag {
                    name: "Done".to_string(),
                    color: None,
                    icon: None,
                }],
            )
            .unwrap();
        let done = store.get_tag_state().unwrap().tags[0].id.clone();
        let q4 = store
            .create_tag(
                &store::NewTag {
                    name: "q4".to_string(),
                    color: None,
                    icon: None,
                },
                None,
                Some(&other.id),
            )
            .unwrap();
        store
            .set_project_group_tag(&own.id, &group.id, Some(&done))
            .unwrap();
        store
            .set_project_group_tag(&other.id, &group.id, Some(&done))
            .unwrap();
        let state = store.get_tag_state().unwrap();
        assert_eq!(state.assignments.len(), 3, "two Done, one q4");

        let scoped = DispatchCaller::Control(ControlGrant {
            actor: Actor::Session {
                session_id: "s1".to_string(),
                epoch: 1,
            },
            scope: Scope::Project(own.id.clone()),
            branch_id: None,
            access: Access::ReadWrite,
        });
        let confined = confine_tag_state(state.clone(), &scoped);
        assert_eq!(confined.groups.len(), 1);
        assert!(
            confined.tags.iter().any(|tag| tag.id == q4.id),
            "the whole catalog, another project's q4 included"
        );
        assert_eq!(
            confined
                .assignments
                .iter()
                .map(|a| (a.project_id.as_str(), a.tag_id.as_str()))
                .collect::<Vec<_>>(),
            vec![(own.id.as_str(), done.as_str())]
        );

        let all = DispatchCaller::Control(ControlGrant::test_all_projects());
        assert_eq!(confine_tag_state(state.clone(), &all).assignments.len(), 3);
        assert_eq!(
            confine_tag_state(state.clone(), &DispatchCaller::Web)
                .assignments
                .len(),
            3
        );

        let arm = dispatch_arm_body(include_str!("web_server.rs"), "get_tag_state");
        assert!(
            arm.contains("confine_tag_state(state, &caller)"),
            "the get_tag_state arm must confine assignments for control callers: {arm}"
        );
    }

    // -------------------------------------------------------------------
    // create_image through the control surface can't make invisible artifacts
    // -------------------------------------------------------------------

    #[test]
    fn control_create_image_rejects_pending_images() {
        // A pending image is hidden until a session attaches it — invisible
        // through the control surface, so rejected regardless of the CLI
        // behaving (a raw POST to the control port hits this too).
        let err = control_create_image_args(&json!({
            "branchId": "b1", "projectId": "p1",
            "filePath": "/tmp/x.png", "pending": true,
        }))
        .unwrap_err();
        assert!(err.contains("pending"), "got: {err}");
    }

    #[test]
    fn control_create_image_rejects_missing_branch_id() {
        // Project-scoped images are write-only — nothing lists or renders
        // them — so a branch is required.
        for args in [
            json!({ "projectId": "p1", "filePath": "/tmp/x.png" }),
            json!({ "branchId": null, "projectId": "p1", "filePath": "/tmp/x.png" }),
        ] {
            let err = control_create_image_args(&args).unwrap_err();
            assert!(err.contains("branchId"), "got: {err}");
        }
    }

    #[test]
    fn control_create_image_rejects_relative_file_paths() {
        // The path is opened by the app process, so a relative one would
        // resolve against the app's cwd rather than the caller's; the CLI
        // sends absolute paths and a raw POST gets the same rule.
        for file_path in ["rel/x.png", "./x.png", "x.png", "../x.png", ""] {
            let err = control_create_image_args(&json!({
                "branchId": "b1", "projectId": "p1", "filePath": file_path,
            }))
            .unwrap_err();
            assert!(
                err.contains("absolute"),
                "{file_path:?} should be rejected as non-absolute, got: {err}"
            );
        }
    }

    #[test]
    fn control_create_image_accepts_branch_scoped_non_pending_args() {
        // pending omitted and pending:false are equivalent — the dispatch
        // arm pins the impl call to pending=Some(false) either way.
        for args in [
            json!({ "branchId": "b1", "projectId": "p1", "filePath": "/tmp/x.png" }),
            json!({
                "branchId": "b1", "projectId": "p1",
                "filePath": "/tmp/x.png", "pending": false,
            }),
        ] {
            let (branch_id, project_id, file_path) = control_create_image_args(&args).unwrap();
            assert_eq!(branch_id, "b1");
            assert_eq!(project_id, "p1");
            assert_eq!(file_path, "/tmp/x.png");
        }
    }

    #[test]
    fn ws_client_id_drops_blank_and_reserved_ids() {
        // A normal web id (a UUID in practice) is trimmed and kept.
        assert_eq!(
            normalize_ws_client_id(Some(" 3f1a-uuid ".into())),
            Some("3f1a-uuid".into())
        );
        assert_eq!(normalize_ws_client_id(None), None);
        assert_eq!(normalize_ws_client_id(Some("   ".into())), None);

        // The native-window namespace is reserved: dropped rather than tracked,
        // so no TTL-exempt entry is minted and the close path can't aim
        // `disconnect_client` at a real window.
        assert_eq!(normalize_ws_client_id(Some("tauri-main".into())), None);
        assert_eq!(normalize_ws_client_id(Some("tauri-win-2".into())), None);
        // Trimming happens first, so padding can't smuggle one through.
        assert_eq!(normalize_ws_client_id(Some(" tauri-main ".into())), None);
    }

    #[test]
    fn web_client_id_rejects_the_reserved_namespace() {
        assert_eq!(
            web_client_id(&json!({ "clientId": "3f1a-uuid" })),
            Ok("3f1a-uuid".to_string())
        );

        for reserved in ["tauri-main", "tauri-win-2", "tauri-"] {
            let err = web_client_id(&json!({ "clientId": reserved }))
                .expect_err("reserved client ids must be rejected");
            assert!(err.contains(reserved), "unexpected error: {err}");
            assert!(err.contains("reserved"), "unexpected error: {err}");
        }

        // Non-reserved lookalikes still pass; the missing/invalid cases keep
        // `arg`'s errors.
        assert!(web_client_id(&json!({ "clientId": "TAURI-main" })).is_ok());
        assert!(web_client_id(&json!({})).is_err());
        assert!(web_client_id(&json!({ "clientId": 7 })).is_err());
    }

    /// A payload that counts how many times it is serialized.
    #[derive(Clone)]
    struct CountedPayload<'a>(&'a std::sync::atomic::AtomicUsize);

    impl serde::Serialize for CountedPayload<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            serializer.serialize_str("payload")
        }
    }

    #[test]
    fn web_events_are_serialized_only_while_a_client_is_connected() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let serialized = AtomicUsize::new(0);
        let payload = CountedPayload(&serialized);
        let (tx, _) = tokio::sync::broadcast::channel::<WebEvent>(4);

        // Nobody connected: nothing is serialized and nothing is queued.
        assert!(!broadcast_web_event(&tx, "action_output", &payload));
        assert_eq!(serialized.load(Ordering::SeqCst), 0);
        assert!(tx.is_empty());

        // A connected client gets the events sent from then on.
        let mut client = tx.subscribe();
        assert!(broadcast_web_event(&tx, "action_output", &payload));
        assert_eq!(serialized.load(Ordering::SeqCst), 1);
        let event = client.try_recv().unwrap();
        assert_eq!(event.event_name, "action_output");
        assert_eq!(
            serde_json::from_str::<Value>(&event.payload).unwrap(),
            json!({ "event": "action_output", "payload": "payload" })
        );
        assert!(client.try_recv().is_err());

        // Once it disconnects, events are skipped again.
        drop(client);
        assert!(!broadcast_web_event(&tx, "action_output", &payload));
        assert_eq!(serialized.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn emit_to_all_reaches_the_window_and_only_connected_web_clients() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tauri::{Listener, Manager};

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app");
        let (tx, _) = tokio::sync::broadcast::channel::<WebEvent>(4);
        app.manage(tx.clone());
        let (heard_tx, heard) = std::sync::mpsc::channel();
        app.listen("action_output", move |event| {
            let _ = heard_tx.send(event.payload().to_string());
        });
        let serialized = AtomicUsize::new(0);
        let payload = CountedPayload(&serialized);

        // No web client: the Tauri side still gets the event.
        emit_to_all(app.handle(), "action_output", payload.clone());
        assert_eq!(heard.try_recv().unwrap(), "\"payload\"");
        let for_tauri_alone = serialized.swap(0, Ordering::SeqCst);

        // A web client costs exactly one more serialization, and gets it.
        let mut client = tx.subscribe();
        emit_to_all(app.handle(), "action_output", payload.clone());
        assert_eq!(heard.try_recv().unwrap(), "\"payload\"");
        assert_eq!(serialized.load(Ordering::SeqCst), for_tauri_alone + 1);
        assert_eq!(client.try_recv().unwrap().event_name, "action_output");
        assert!(client.try_recv().is_err());
    }

    #[test]
    fn event_gap_payload_requests_full_client_recovery() {
        assert_eq!(
            serde_json::from_str::<Value>(&event_gap_payload()).unwrap(),
            json!({
                "event": "transport:event-gap",
                "payload": null,
            })
        );
    }

    #[test]
    fn web_dispatch_covers_tauri_commands() {
        let tauri_commands = extract_generate_handler_commands(include_str!("lib.rs"));
        let dispatch_commands = extract_dispatch_commands(include_str!("web_server.rs"));

        let missing = tauri_commands
            .difference(&dispatch_commands)
            .cloned()
            .collect::<Vec<_>>();

        assert!(
            missing.is_empty(),
            "web_server::dispatch is missing command arms for: {missing:#?}"
        );
    }

    /// An expression arm whose call rustfmt wraps has no block, so slicing by
    /// braces alone ended it on its first line, with a `(` still open. The
    /// blocking guard's scan then panicked on `list_transferring_projects`.
    #[test]
    fn dispatch_arms_keep_wrapped_expression_arms_whole() {
        let source = r#"
    match command {
        "guard_wrapped_call" => Ok(serde_json::to_value(
            crate::module::listing(),
        )
        .unwrap()),
        "guard_wrapped_list" => Ok(json!([
            "{ (",
        ])),
        "guard_one_line" | "guard_alias" => Ok(Value::Null),
        "guard_block" => {
            let value = compute(
                "a {",
            );
            Ok(value)
        }
        _ => Err(String::new()),
    }
"#;
        let arms: BTreeMap<String, String> = dispatch_arms(source)
            .into_iter()
            .map(|(commands, body)| (commands.join(" | "), body))
            .collect();

        let names: Vec<&str> = arms.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [
                "guard_block",
                "guard_one_line | guard_alias",
                "guard_wrapped_call",
                "guard_wrapped_list",
            ]
        );
        assert!(
            arms["guard_wrapped_call"]
                .ends_with("crate::module::listing(),\n        )\n        .unwrap()),\n"),
            "{}",
            arms["guard_wrapped_call"]
        );
        assert!(arms["guard_wrapped_list"].ends_with("])),\n"));
        assert!(arms["guard_block"].ends_with("Ok(value)\n        }\n"));
        assert_eq!(
            arms["guard_one_line | guard_alias"].lines().count(),
            1,
            "{}",
            arms["guard_one_line | guard_alias"]
        );
    }

    /// Having an arm is not the same as having the *same* arm — which is the
    /// gap `web_dispatch_covers_tauri_commands` can't see. This one had drifted:
    /// it cancelled through `SessionRegistry::cancel` alone, so a browser Stop
    /// on a session no longer registered — deregistered before its
    /// post-completion hooks ran — would flip no token and write no status, and
    /// `session_runner`'s queued follow-up would drain anyway.
    ///
    /// Latent rather than live, since [`start`] is stubbed and nothing serves
    /// this dispatcher in this build. That is precisely why it needs a test:
    /// the arms are kept compiling to stay in sync for the day the server is
    /// restored, and a dormant arm has no user to notice it diverging.
    ///
    /// Both transports now share one body; the behaviour it must have is pinned
    /// by
    /// `session_commands::tests::cancelling_an_unregistered_session_records_the_stop_in_the_store`,
    /// and this keeps the web arm reaching it.
    #[test]
    fn web_cancel_session_takes_the_shared_store_write_fallback() {
        let arm = dispatch_arm_body(include_str!("web_server.rs"), "cancel_session");

        assert!(
            arm.contains("session_commands::cancel_session_impl"),
            "the web `cancel_session` arm must call the shared body, not cancel \
             through the registry alone: {arm}"
        );
    }

    /// The delete arms must reach the bodies the Tauri commands use. The
    /// artifact deletes kick the branch queue after the delete — a queued
    /// commit is the barrier in front of everything queued behind it — and the
    /// branch deletes stop the branch's sessions before the cascade takes
    /// their rows; an arm that re-implemented either inline would drop that
    /// without any compile error.
    #[test]
    fn web_delete_arms_share_the_tauri_bodies() {
        let source = include_str!("web_server.rs");
        for (command, body) in [
            ("delete_note", "note_commands::delete_note_impl("),
            (
                "delete_project_note",
                "note_commands::delete_project_note_impl(",
            ),
            ("delete_review", "timeline::delete_review_impl("),
            (
                "delete_pending_commit",
                "timeline::delete_pending_commit_impl(",
            ),
            ("delete_session", "session_commands::delete_session_impl("),
            ("delete_branch", "branch_teardown::delete_branch_impl("),
            (
                "remove_project_repo",
                "branch_teardown::remove_project_repo_impl(",
            ),
            ("delete_project", "branch_teardown::delete_project_impl("),
        ] {
            let arm = dispatch_arm_body(source, command);
            assert!(
                arm.contains(body),
                "the web `{command}` arm must call `{body}`, not delete inline: {arm}"
            );
        }
    }

    /// A note delete reads every note body that embeds media and every chat
    /// attachment list, and cascades a session's messages, on the one store
    /// connection. Inline, that held the tokio worker that also services
    /// `/api/events`; the desktop commands moved to the blocking pool for the
    /// same scans. The project-note arm also used to carry its own copy of the
    /// delete, which is what the second half rules out.
    #[test]
    fn web_note_delete_arms_run_under_spawn_blocking() {
        let source = include_str!("web_server.rs");
        for command in ["delete_note", "delete_project_note"] {
            let arm = dispatch_arm_body(source, command);
            assert!(
                arm.contains("spawn_blocking("),
                "the web `{command}` arm must run its body on the blocking pool: {arm}"
            );
        }

        let arm = dispatch_arm_body(source, "delete_project_note");
        for inline in [
            ".delete_project_note(",
            "cleanup_unreferenced(",
            ".delete_session(",
        ] {
            assert!(
                !arm.contains(inline),
                "the web `delete_project_note` arm must not delete around the shared \
                 body (`{inline}`): {arm}"
            );
        }
    }

    /// The web editor saves through the same body as the desktop one, so the
    /// two can't drift apart; in particular, neither collects the media an
    /// edit removed (see `note_commands::update_note_impl`).
    #[test]
    fn web_update_note_shares_the_tauri_body() {
        let arm = dispatch_arm_body(include_str!("web_server.rs"), "update_note");
        assert!(
            arm.contains("note_commands::update_note_impl("),
            "the web `update_note` arm must call the shared body: {arm}"
        );
        assert!(
            !arm.contains("update_written_note("),
            "the web `update_note` arm must not save around the shared body: {arm}"
        );
    }

    /// Queueing a chat message for a hidden child note's session is refused
    /// on the web path as on the desktop one: the arm runs the shared gate
    /// before the insert.
    #[test]
    fn web_queue_session_message_gates_hidden_child_note_chats() {
        let arm = dispatch_arm_body(include_str!("web_server.rs"), "queue_session_message");
        let gate = arm
            .find("session_commands::ensure_session_chat_writable(")
            .unwrap_or_else(|| {
                panic!("the web `queue_session_message` arm must run the chat gate: {arm}")
            });
        let insert = arm
            .find("add_queued_session_message(")
            .unwrap_or_else(|| panic!("the web `queue_session_message` arm must queue: {arm}"));
        assert!(gate < insert, "the gate must run before the insert: {arm}");
    }

    /// A web resume goes through the desktop command's body, `prepare_resume`
    /// included, so it waits out a Stop's backfill and refuses a
    /// `disconnected` row as the desktop one does, instead of a copy that
    /// drifts from it.
    #[test]
    fn web_resume_session_shares_the_tauri_body() {
        let arm = dispatch_arm_body(include_str!("web_server.rs"), "resume_session");
        assert!(
            arm.contains("session_commands::resume_or_queue_session_for_store("),
            "the web `resume_session` arm must call the shared body: {arm}"
        );
        assert!(
            !arm.contains("start_session("),
            "the web `resume_session` arm must not start the run around the shared body: {arm}"
        );
    }

    /// Arms that write a `running` session row or a workspace status and then
    /// await must run that work through [`detached`]: the request future is
    /// dropped at its next `.await` when the client disconnects (see the module
    /// docs), and a dropped arm would leave its write with nothing to release
    /// it. Keyed on explicit lists so a new arm that writes-then-awaits fails
    /// here until it is either detached or exempted with a reason.
    #[test]
    fn web_arms_that_write_running_rows_are_detached() {
        /// Calls that write a `running` row or a workspace status.
        const WRITE_MARKERS: &[&str] = &[
            "Session::new_running(",
            "transition_to_running",
            "update_branch_workspace_status(",
            "update_workspace_status_by_workspace_name(",
            "_pipeline_for_branch(",
            "pull_or_queue_branch_for_branch(",
            "start_queued_session_now_for_branch(",
            "resume_session_for_store(",
            "resume_or_queue_session_for_store(",
            "start_project_session_for_store(",
        ];
        /// Arms whose bodies run through `detached`.
        const DETACHED: &[&str] = &[
            "start_workspace",
            "resume_workspace",
            "pull_or_queue_branch",
            "resume_session",
            "start_queued_session_now",
            "start_project_session",
            "push_branch",
            "rebase_branch",
            "squash_commits",
            "create_pr",
        ];
        /// Arms that write only after their last await, with the kick in the
        /// same synchronous stretch: a drop before the write has written
        /// nothing (the polls redo it next tick), and nothing is left
        /// half-written.
        const WRITES_AFTER_LAST_AWAIT: &[&str] =
            &["poll_workspace_status", "poll_all_workspace_statuses"];

        let source = include_str!("web_server.rs");
        for (commands, arm) in dispatch_arms(source) {
            let command = commands.join(" | ");
            let writes = WRITE_MARKERS.iter().any(|marker| arm.contains(marker));
            let awaits = arm.contains(".await");
            if commands.iter().any(|c| DETACHED.contains(&c.as_str())) {
                assert!(
                    writes,
                    "the web `{command}` arm no longer writes a running row or workspace \
                     status; drop it from DETACHED"
                );
                assert!(
                    arm.contains("detached("),
                    "the web `{command}` arm writes a running row or workspace status and \
                     awaits, so it must run through `detached(...)`: {arm}"
                );
            } else if commands
                .iter()
                .any(|c| WRITES_AFTER_LAST_AWAIT.contains(&c.as_str()))
            {
                let last_await = arm.rfind(".await").expect("exempted arm should await");
                let first_write = WRITE_MARKERS
                    .iter()
                    .filter_map(|marker| arm.find(marker))
                    .min()
                    .expect("exempted arm should write");
                assert!(
                    first_write > last_await,
                    "the web `{command}` arm writes before its last await; it no longer \
                     qualifies for WRITES_AFTER_LAST_AWAIT and must run through \
                     `detached(...)`"
                );
            } else {
                assert!(
                    !(writes && awaits),
                    "the web `{command}` arm writes a running row or workspace status and \
                     then awaits; run that work through `detached(...)` and add it to \
                     DETACHED, or add it to WRITES_AFTER_LAST_AWAIT if every write follows \
                     its last await: {arm}"
                );
            }
        }
    }

    /// The windowed transcript read must accept the same optional window the
    /// Tauri command does, and keep reading off the runtime thread: this is the
    /// largest payload the server produces, and the client pages through it
    /// while the same worker services `/api/events`.
    #[test]
    fn web_get_session_messages_windows_under_spawn_blocking() {
        let arm = dispatch_arm_body(include_str!("web_server.rs"), "get_session_messages");

        for needle in [
            "opt_arg(&args, \"limit\")",
            "opt_arg(&args, \"beforeId\")",
            "get_session_messages_window(",
            "blocking_store_json(",
        ] {
            assert!(
                arm.contains(needle),
                "the web `get_session_messages` arm must contain `{needle}`: {arm}"
            );
        }
    }

    /// A deferred body is read on expand while the same worker services
    /// `/api/events`, and can be hundreds of KB; keep it off the runtime
    /// thread like the transcript reads.
    #[test]
    fn web_get_tool_call_body_reads_under_spawn_blocking() {
        let arm = dispatch_arm_body(include_str!("web_server.rs"), "get_tool_call_body");
        for needle in ["session_commands::tool_call_body(", "blocking_store_json("] {
            assert!(
                arm.contains(needle),
                "the web `get_tool_call_body` arm must contain `{needle}`: {arm}"
            );
        }
    }

    /// The poll runs twice a second per open running pane and carries the
    /// transcript tail; keep it off the runtime thread like the reads it
    /// replaced.
    #[test]
    fn web_poll_session_reads_under_spawn_blocking() {
        let arm = dispatch_arm_body(include_str!("web_server.rs"), "poll_session");
        for needle in [
            "arg(&args, \"request\")",
            ".poll_session(",
            "blocking_store_json(",
        ] {
            assert!(
                arm.contains(needle),
                "the web `poll_session` arm must contain `{needle}`: {arm}"
            );
        }
    }

    /// The web rename arm has no git to run, so the checkout's upstream —
    /// and with it where the branch pushes — is untouched by construction.
    /// What it must still do is record the name through the shared helper,
    /// which also renames the project-repo row the old bare
    /// `update_branch_name` write skipped. Source-level like the other arm
    /// checks (see `dispatch_arm_body`).
    #[test]
    fn web_rename_branch_records_the_name_through_the_shared_helper() {
        let arm = dispatch_arm_body(include_str!("web_server.rs"), "rename_branch");
        assert!(
            arm.contains("record_branch_rename("),
            "the web `rename_branch` arm must go through `record_branch_rename`: {arm}"
        );
        assert!(
            !arm.contains("update_branch_name("),
            "the web `rename_branch` arm must not bypass `record_branch_rename`: {arm}"
        );
    }

    /// The store sequence the web arm runs: the branch and the project-repo
    /// row the old arm never updated are renamed together.
    #[test]
    fn web_rename_sequence_renames_the_branch_and_its_repo_row() {
        use crate::branches::record_branch_rename;
        use crate::store::{self, Store};
        use std::sync::Arc;

        let store = Arc::new(Store::in_memory().unwrap());
        let project = store::Project::new("acme/widgets");
        store.create_project(&project).unwrap();
        let repo = store::ProjectRepo::new(&project.id, "acme/widgets", "feature-x", None);
        store.create_project_repo(&repo).unwrap();
        let branch = store::Branch::new(&project.id, "feature-x", "main")
            .with_pr(7)
            .with_project_repo(&repo.id);
        store.create_branch(&branch).unwrap();

        record_branch_rename(&store, &branch, "feature-y").unwrap();

        let stored = store.get_branch(&branch.id).unwrap().unwrap();
        assert_eq!(stored.branch_name, "feature-y");
        let repo = store.get_project_repo(&repo.id).unwrap().unwrap();
        assert_eq!(repo.branch_name, "feature-y");
    }

    /// The source of one `dispatch` arm, from its `"command" =>` pattern to the
    /// end of its block. Source-level for the same reason as
    /// `web_dispatch_covers_tauri_commands`: `dispatch` needs a `WebAppState`
    /// with a real `AppHandle`, which a unit test has no way to build.
    fn dispatch_arm_body(source: &str, command: &str) -> String {
        let marker = format!("\"{command}\" =>");
        let start = source
            .find(&marker)
            .unwrap_or_else(|| panic!("dispatch should have a `{command}` arm"))
            + marker.len();

        let mut depth = 0isize;
        let mut end = start;
        for line in source[start..].lines() {
            end += line.len() + 1;
            depth += brace_delta_ignoring_strings(line);
            if depth == 0 {
                break;
            }
        }

        source[start..end.min(source.len())].to_string()
    }

    /// Every arm of the dispatch match as (its command patterns, its body),
    /// including the `"a" | "b" =>` arms `dispatch_arm_body` can't address.
    /// An arm runs until its parens and brackets close as well as its braces,
    /// so an expression arm whose call rustfmt wrapped (`=> Ok(f(` … `)),`)
    /// stays whole. A chain whose first line balances (`=> f(x)` then
    /// `.g()`) still ends on that line.
    pub(super) fn dispatch_arms(source: &str) -> Vec<(Vec<String>, String)> {
        let marker = "match command {";
        let start = source
            .find(marker)
            .expect("dispatch command match should exist")
            + marker.len();
        let mut depth = 1isize;
        let mut arms = Vec::new();
        let mut current: Option<(Vec<String>, String)> = None;

        for line in source[start..].lines() {
            if depth == 1 {
                let trimmed = line.trim_start();
                if trimmed.starts_with("_ =>") {
                    break;
                }
                let commands = extract_match_pattern_strings(trimmed);
                if !commands.is_empty() {
                    current = Some((commands, String::new()));
                }
            }
            if let Some((_, body)) = current.as_mut() {
                body.push_str(line);
                body.push('\n');
            }

            depth += bracket_delta_ignoring_strings(line);
            if depth == 1 {
                arms.extend(current.take());
            } else if depth == 0 {
                break;
            }
        }

        arms
    }

    pub(super) fn extract_generate_handler_commands(source: &str) -> BTreeSet<String> {
        let marker = "tauri::generate_handler![";
        let start = source
            .find(marker)
            .expect("generate_handler! command list should exist")
            + marker.len();
        let end = source[start..]
            .find("])")
            .expect("generate_handler! command list should end")
            + start;

        source[start..end]
            .lines()
            .filter_map(|line| {
                let entry = line.split("//").next()?.trim().trim_end_matches(',').trim();
                if entry.is_empty() {
                    None
                } else {
                    Some(entry.rsplit("::").next().unwrap_or(entry).to_string())
                }
            })
            .collect()
    }

    fn extract_dispatch_commands(source: &str) -> BTreeSet<String> {
        let marker = "match command {";
        let start = source
            .find(marker)
            .expect("dispatch command match should exist")
            + marker.len();
        let mut depth = 1isize;
        let mut commands = BTreeSet::new();

        for line in source[start..].lines() {
            if depth == 1 {
                let trimmed = line.trim_start();
                if trimmed.starts_with("_ =>") {
                    break;
                }
                commands.extend(extract_match_pattern_strings(trimmed));
            }

            depth += brace_delta_ignoring_strings(line);
            if depth == 0 {
                break;
            }
        }

        commands
    }

    fn extract_match_pattern_strings(line: &str) -> Vec<String> {
        let Some((patterns, _)) = line.split_once("=>") else {
            return Vec::new();
        };

        let mut names = Vec::new();
        let mut rest = patterns;
        while let Some(start) = rest.find('"') {
            rest = &rest[start + 1..];
            let Some(end) = rest.find('"') else {
                break;
            };
            names.push(rest[..end].to_string());
            rest = &rest[end + 1..];
        }

        names
    }

    fn brace_delta_ignoring_strings(line: &str) -> isize {
        delta_ignoring_strings(line, &[('{', '}')])
    }

    /// `brace_delta_ignoring_strings` over parens and square brackets too.
    fn bracket_delta_ignoring_strings(line: &str) -> isize {
        delta_ignoring_strings(line, &[('(', ')'), ('[', ']'), ('{', '}')])
    }

    /// How many of `brackets` `line` leaves open (negative for closed),
    /// skipping string literals and a trailing `//` comment.
    fn delta_ignoring_strings(line: &str, brackets: &[(char, char)]) -> isize {
        let mut delta = 0;
        let mut chars = line.chars().peekable();
        let mut in_string = false;
        let mut escaped = false;

        while let Some(ch) = chars.next() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }

            match ch {
                '"' => in_string = true,
                '/' if chars.peek() == Some(&'/') => break,
                _ if brackets.iter().any(|&(open, _)| open == ch) => delta += 1,
                _ if brackets.iter().any(|&(_, close)| close == ch) => delta -= 1,
                _ => {}
            }
        }

        delta
    }
}

#[cfg(test)]
mod invoke_tests {
    use super::*;
    use tower::ServiceExt;

    /// Stands in for `invoke_command` with the same extractors, reporting what
    /// reached dispatch so the test can tell a parsed body from a refused one.
    async fn echo_len(Path(command): Path<String>, Json(args): Json<Value>) -> Json<Value> {
        let data_len = args["data"].as_str().map_or(0, str::len);
        Json(serde_json::json!({ "command": command, "dataLen": data_len }))
    }

    fn invoke(body: String) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/invoke/create_image_from_data")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn invoke_route_admits_a_cap_sized_pdf_body_and_refuses_larger() {
        let router: Router = invoke_router(echo_len);

        // The base64 of a PDF right at MAX_PDF_INPUT_SIZE: the largest body
        // `create_image_from_data` admits, and far past axum's 2 MiB default.
        let data_len = (crate::note_media::files::MAX_PDF_INPUT_SIZE.div_ceil(3) * 4) as usize;
        let body = format!(r#"{{"data":"{}"}}"#, "A".repeat(data_len));
        assert!(body.len() > 2 * 1024 * 1024 && body.len() <= INVOKE_BODY_LIMIT);
        let response = router.clone().oneshot(invoke(body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let reply: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(reply["command"], "create_image_from_data");
        assert_eq!(reply["dataLen"], data_len);

        // One byte over the limit never reaches the handler.
        let body = format!(r#"{{"data":"{}"}}"#, "A".repeat(INVOKE_BODY_LIMIT - 10));
        assert_eq!(body.len(), INVOKE_BODY_LIMIT + 1);
        let response = router.oneshot(invoke(body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        // axum answers with a plain-text rejection, not the `{ "error" }` JSON
        // `invoke_command` emits, so `transport.ts` surfaces this text as-is.
        let text = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let text = std::str::from_utf8(&text).unwrap();
        assert!(
            serde_json::from_str::<Value>(text).is_err(),
            "unexpected JSON body: {text}"
        );
    }
}

#[cfg(test)]
mod media_tests {
    use super::*;
    use crate::note_media::tests::{Fixture, PNG};
    use tower::ServiceExt;

    /// The browser's PDF viewer loads a PDF attachment in an `<iframe>`, so
    /// the response keeps the stored MIME and Range support but must not
    /// carry the `sandbox` directive, which blanks the viewer in WebKit.
    /// The viewer also negotiates br/gzip, so the PDF has to bypass the
    /// compression layer (the video test holds the control proving the
    /// same layer is active) or it loses `Content-Length` and
    /// `Accept-Ranges`.
    #[tokio::test]
    async fn pdf_media_is_served_without_the_sandbox_directive() {
        let fixture = Fixture::new();
        let pdf =
            crate::note_media::pdf::tests::tiny_pdf(&[crate::note_media::pdf::tests::PAGE_ONE]);
        let image = crate::note_media::files::store_sniffed_image(
            &fixture.store,
            None,
            &fixture.project.id,
            None,
            "doc.pdf",
            &pdf,
        )
        .unwrap();
        let sessions = Arc::new(Mutex::new(HashSet::new()));
        let router = media_router_for_store(fixture.store.clone(), "bearer-token".into(), sessions)
            .layer(compression_layer());
        let request = |uri: String, range: Option<&str>| {
            let mut request = axum::http::Request::builder()
                .uri(uri)
                .header("Authorization", "Bearer bearer-token")
                .header("Accept-Encoding", "br, gzip");
            if let Some(range) = range {
                request = request.header("Range", range);
            }
            request.body(axum::body::Body::empty()).unwrap()
        };
        let response = router
            .clone()
            .oneshot(request(format!("/api/media/{}.pdf", image.id), None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "application/pdf");
        assert!(response.headers().get("content-encoding").is_none());
        assert_eq!(
            response.headers()["content-length"],
            pdf.len().to_string().as_str()
        );
        assert_eq!(response.headers()["accept-ranges"], "bytes");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(
            response.headers()["content-security-policy"],
            UNTRUSTED_PDF_CSP
        );
        assert!(!UNTRUSTED_PDF_CSP.contains("sandbox"));
        assert_eq!(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .as_ref(),
            &pdf[..]
        );

        let response = router
            .clone()
            .oneshot(request(
                format!("/api/media/{}.pdf", image.id),
                Some("bytes=0-4"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()["accept-ranges"], "bytes");
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 100)
                .await
                .unwrap()
                .as_ref(),
            b"%PDF-"
        );

        // Images keep the sandboxed policy.
        std::fs::write(fixture.dir.path().join("shot.png"), PNG).unwrap();
        let outcome = fixture.ingest("", "![a](shot.png)");
        let id = outcome.kept_ids.iter().next().unwrap();
        let response = router
            .oneshot(request(format!("/api/media/{id}.png"), None))
            .await
            .unwrap();
        assert_eq!(
            response.headers()["content-security-policy"],
            UNTRUSTED_MEDIA_CSP
        );
    }

    #[tokio::test]
    async fn media_route_requires_auth_and_streams_ranges() {
        let fixture = Fixture::new();
        std::fs::write(fixture.dir.path().join("shot.png"), PNG).unwrap();
        let outcome = fixture.ingest("", "![a](shot.png)");
        let id = outcome.kept_ids.iter().next().unwrap();
        let store = fixture.store.clone();
        let sessions = Arc::new(Mutex::new(HashSet::from(["session-token".into()])));
        let router = media_router_for_store(store, "bearer-token".into(), sessions);
        let uri = format!("/api/media/{id}.png");
        for (auth, expected) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some("wrong"), StatusCode::UNAUTHORIZED),
            (Some("session-token"), StatusCode::PARTIAL_CONTENT),
        ] {
            let mut request = axum::http::Request::builder()
                .uri(&uri)
                .header("Range", "bytes=2-5");
            if let Some(auth) = auth {
                request = request.header("Cookie", format!("staged_session={auth}"));
            }
            let response = router
                .clone()
                .oneshot(request.body(axum::body::Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
            if expected == StatusCode::PARTIAL_CONTENT {
                assert_eq!(response.headers()["content-type"], "image/png");
                assert_eq!(response.headers()["accept-ranges"], "bytes");
                assert_eq!(
                    response.headers()["cache-control"],
                    "private, max-age=31536000, immutable"
                );
                assert_eq!(
                    axum::body::to_bytes(response.into_body(), 100)
                        .await
                        .unwrap()
                        .as_ref(),
                    &PNG[2..6]
                );
            }
        }
        let request = axum::http::Request::builder()
            .uri(&uri)
            .header("Authorization", "Bearer bearer-token")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(
            response.headers()["content-security-policy"],
            UNTRUSTED_MEDIA_CSP
        );
        assert_eq!(
            response.headers()["cache-control"],
            "private, max-age=31536000, immutable"
        );
        let etag = response.headers()["etag"].clone();
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 100)
                .await
                .unwrap()
                .as_ref(),
            PNG
        );

        // The cache policy covers every 2xx and the 304 a conditional
        // request gets back, but not a 416 (or 412), which describes the
        // request rather than the bytes and must not be stored for a year.
        let bearer = |method: &str, headers: &[(&str, &str)]| {
            let mut request = axum::http::Request::builder()
                .method(method)
                .uri(&uri)
                .header("Authorization", "Bearer bearer-token");
            for (name, value) in headers {
                request = request.header(*name, *value);
            }
            request.body(axum::body::Body::empty()).unwrap()
        };
        let response = router.clone().oneshot(bearer("HEAD", &[])).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["cache-control"],
            "private, max-age=31536000, immutable"
        );
        let response = router
            .clone()
            .oneshot(bearer("GET", &[("If-None-Match", etag.to_str().unwrap())]))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            response.headers()["cache-control"],
            "private, max-age=31536000, immutable"
        );
        let beyond_eof = format!("bytes={}-", PNG.len());
        let response = router
            .clone()
            .oneshot(bearer("GET", &[("Range", beyond_eof.as_str())]))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert!(response.headers().get("cache-control").is_none());

        let request = axum::http::Request::builder()
            .uri("/api/media/not-a-media-id.png")
            .header("Authorization", "Bearer bearer-token")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            router.oneshot(request).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }

    /// Downloads, "open in new tab", and `fetch()` negotiate br/gzip, unlike
    /// `<video>` playback, and a compressed body loses `Content-Length` and
    /// `Accept-Ranges`. Videos must therefore bypass the compression layer
    /// that the rest of the app goes through, and a multi-range request
    /// gets the whole file rather than a 416.
    #[tokio::test]
    async fn video_media_is_served_uncompressed_and_whole_for_multi_range() {
        // Large enough that the size predicate alone would not exempt it.
        let mp4: Vec<u8> = [&b"\0\0\0\x18ftypisom\0\0\0\0"[..], &[0u8; 100]].concat();
        let fixture = Fixture::new();
        std::fs::write(fixture.dir.path().join("clip.mp4"), &mp4).unwrap();
        let outcome = fixture.ingest("", "![a](clip.mp4)");
        let id = outcome.kept_ids.iter().next().unwrap();
        let sessions = Arc::new(Mutex::new(HashSet::new()));
        let router = media_router_for_store(fixture.store.clone(), "bearer-token".into(), sessions)
            .route("/text", get(|| async { "x".repeat(100) }))
            .layer(compression_layer());
        let request = |uri: &str, headers: &[(&str, &str)]| {
            let mut request = axum::http::Request::builder()
                .uri(uri)
                .header("Authorization", "Bearer bearer-token")
                .header("Accept-Encoding", "br, gzip");
            for (name, value) in headers {
                request = request.header(*name, *value);
            }
            request.body(axum::body::Body::empty()).unwrap()
        };
        // Control: the layer is active for everything else.
        let response = router.clone().oneshot(request("/text", &[])).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-encoding"], "br");

        let uri = format!("/api/media/{id}.mp4");
        let response = router.clone().oneshot(request(&uri, &[])).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "video/mp4");
        assert!(response.headers().get("content-encoding").is_none());
        assert_eq!(
            response.headers()["content-length"],
            mp4.len().to_string().as_str()
        );
        assert_eq!(response.headers()["accept-ranges"], "bytes");
        assert_eq!(
            response.headers()["cache-control"],
            "private, max-age=31536000, immutable"
        );
        assert_eq!(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .as_ref(),
            &mp4[..]
        );

        let response = router
            .clone()
            .oneshot(request(&uri, &[("Range", "bytes=0-1,8-11")]))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("content-range").is_none());
        assert_eq!(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .as_ref(),
            &mp4[..]
        );

        let response = router
            .oneshot(request(&uri, &[("Range", "bytes=0-1")]))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()["content-range"], "bytes 0-1/116");
        assert_eq!(response.headers()["content-length"], "2");
    }

    #[tokio::test]
    async fn tool_image_route_serves_decoded_block_or_404() {
        use base64::Engine;
        let fixture = Fixture::new();
        let store = fixture.store.clone();
        // An SVG is an image to the store and the client, but opened as a
        // document it is a script host; the route must serve it sandboxed.
        const SVG: &[u8] =
            b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>";
        let id = store
            .add_session_message(&fixture.session.id, store::MessageRole::ToolCall, "Read")
            .unwrap();
        store
            .update_message_acp_metadata(
                id,
                &store::AcpMessageMetadata {
                    acp_event_kind: Some("tool_call".into()),
                    acp_tool_status: Some("completed".into()),
                    acp_raw_output: Some(serde_json::json!([
                        {"type": "text", "text": "ok"},
                        {"type": "image", "source": {
                            "type": "base64", "media_type": "image/png",
                            "data": base64::engine::general_purpose::STANDARD.encode(PNG),
                        }},
                        {"type": "image", "source": {
                            "type": "base64", "media_type": "image/svg+xml",
                            "data": base64::engine::general_purpose::STANDARD.encode(SVG),
                        }},
                    ])),
                    ..Default::default()
                },
            )
            .unwrap();
        // The same block on a row that has not settled: never projected to a
        // ref, and not served either, since the response would be cached as
        // immutable while a later update could still replace the block.
        let in_flight_id = store
            .add_session_message(&fixture.session.id, store::MessageRole::ToolCall, "Read")
            .unwrap();
        store
            .update_message_acp_metadata(
                in_flight_id,
                &store::AcpMessageMetadata {
                    acp_event_kind: Some("tool_call".into()),
                    acp_tool_status: Some("in_progress".into()),
                    acp_raw_output: Some(serde_json::json!([
                        {"type": "text", "text": "ok"},
                        {"type": "image", "source": {
                            "type": "base64", "media_type": "image/png",
                            "data": base64::engine::general_purpose::STANDARD.encode(PNG),
                        }},
                    ])),
                    ..Default::default()
                },
            )
            .unwrap();
        let sessions = Arc::new(Mutex::new(HashSet::new()));
        let router = media_router_for_store(store, "bearer-token".into(), sessions);
        let get = |uri: String, auth: Option<&str>| {
            let mut request = axum::http::Request::builder().uri(uri);
            if let Some(auth) = auth {
                request = request.header("Authorization", format!("Bearer {auth}"));
            }
            router
                .clone()
                .oneshot(request.body(axum::body::Body::empty()).unwrap())
        };

        let response = get(
            format!("/api/tool-images/{id}/output/1"),
            Some("bearer-token"),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "image/png");
        assert_eq!(
            response.headers()["cache-control"],
            "private, max-age=31536000, immutable"
        );
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(
            response.headers()["content-security-policy"],
            UNTRUSTED_MEDIA_CSP
        );
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 100)
                .await
                .unwrap()
                .as_ref(),
            PNG
        );

        // The SVG is still served with its own type (the `<img>` needs it),
        // only ever under the sandboxing policy.
        let response = get(
            format!("/api/tool-images/{id}/output/2"),
            Some("bearer-token"),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "image/svg+xml");
        assert_eq!(
            response.headers()["content-security-policy"],
            UNTRUSTED_MEDIA_CSP
        );
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 1000)
                .await
                .unwrap()
                .as_ref(),
            SVG
        );

        assert_eq!(
            get(format!("/api/tool-images/{id}/output/1"), None)
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        for uri in [
            format!("/api/tool-images/{id}/output/0"),
            format!("/api/tool-images/{id}/output/3"),
            format!("/api/tool-images/{id}/content/0"),
            format!("/api/tool-images/{id}/source/1"),
            format!("/api/tool-images/{in_flight_id}/output/1"),
            format!("/api/tool-images/{}/output/1", in_flight_id + 1),
            "/api/tool-images/not-an-id/output/1".to_string(),
        ] {
            assert_eq!(
                get(uri.clone(), Some("bearer-token"))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NOT_FOUND,
                "{uri}"
            );
        }
    }

    #[tokio::test]
    async fn tool_body_route_serves_the_row_bodies_or_404() {
        use base64::Engine;
        let fixture = Fixture::new();
        let store = fixture.store.clone();
        let stdout = "x".repeat(store::DEFERRED_TOOL_BODY_MIN_BYTES);
        let id = store
            .add_session_message(&fixture.session.id, store::MessageRole::ToolCall, "Bash")
            .unwrap();
        store
            .update_message_acp_metadata(
                id,
                &store::AcpMessageMetadata {
                    acp_event_kind: Some("tool_call".into()),
                    acp_tool_status: Some("completed".into()),
                    acp_raw_input: Some(serde_json::json!({"command": "make"})),
                    acp_raw_output: Some(serde_json::json!({"stdout": stdout})),
                    acp_content: Some(serde_json::json!([{"type": "content", "content": {
                        "type": "image", "mimeType": "image/png",
                        "data": base64::engine::general_purpose::STANDARD.encode(PNG),
                    }}])),
                    ..Default::default()
                },
            )
            .unwrap();
        // The transcript ships the output as a marker…
        let shipped = &store.get_session_messages(&fixture.session.id).unwrap()[0];
        assert_eq!(
            shipped.acp.acp_raw_output.as_ref().unwrap()["$deferred"]["messageId"],
            serde_json::json!(id)
        );
        // An in-flight row never gets a marker, and the route must not serve
        // it under the `immutable` cache header either.
        let in_flight_id = store
            .add_session_message(&fixture.session.id, store::MessageRole::ToolCall, "Bash")
            .unwrap();
        store
            .update_message_acp_metadata(
                in_flight_id,
                &store::AcpMessageMetadata {
                    acp_event_kind: Some("tool_call".into()),
                    acp_tool_status: Some("in_progress".into()),
                    acp_raw_input: Some(serde_json::json!({"command": "make"})),
                    acp_raw_output: Some(serde_json::json!({"stdout": stdout.clone()})),
                    ..Default::default()
                },
            )
            .unwrap();

        let sessions = Arc::new(Mutex::new(HashSet::new()));
        let router = media_router_for_store(store, "bearer-token".into(), sessions);
        let get = |uri: String, auth: Option<&str>| {
            let mut request = axum::http::Request::builder().uri(uri);
            if let Some(auth) = auth {
                request = request.header("Authorization", format!("Bearer {auth}"));
            }
            router
                .clone()
                .oneshot(request.body(axum::body::Body::empty()).unwrap())
        };

        // …and the route serves the bodies, images projected to refs.
        let response = get(format!("/api/tool-bodies/{id}"), Some("bearer-token"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_eq!(
            response.headers()["cache-control"],
            "private, max-age=31536000, immutable"
        );
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "messageId": id,
                "acpRawInput": {"command": "make"},
                "acpRawOutput": {"stdout": stdout},
                "acpContent": [{"type": "content", "content": {
                    "type": "image", "ref": format!("{id}/content/0"), "mimeType": "image/png",
                }}],
            })
        );

        assert_eq!(
            get(format!("/api/tool-bodies/{id}"), None)
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        for uri in [
            format!("/api/tool-bodies/{in_flight_id}"),
            format!("/api/tool-bodies/{}", in_flight_id + 1),
            "/api/tool-bodies/not-an-id".to_string(),
        ] {
            assert_eq!(
                get(uri.clone(), Some("bearer-token"))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NOT_FOUND,
                "{uri}"
            );
        }
    }
}
