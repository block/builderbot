//! The harness's loopback server: the web API and frontend, and `/perf/*`.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use axum_extra::extract::cookie::CookieJar;
use tauri::Manager;
use tokio::sync::broadcast;

use super::guard::Config;
use super::metrics;
use crate::web_server::{self, WebAppState, WebEvent};

const INVOKE_PREFIX: &str = "/api/invoke/";
const PERF_PREFIX: &str = "/perf/";
/// A result is a page's whole measurement, so well over axum's 2 MiB.
const RESULT_BODY_LIMIT: usize = 64 * 1024 * 1024;
/// The header the harness driver sends the run's token in.
const TOKEN_HEADER: &str = "x-perf-token";
/// The cookie the harness's own page sends it in. The driver puts it in
/// the page's cookie store before the page loads (see `driver/frontend.mjs`),
/// so the app's frontend code never handles the token, and the browser adds
/// it to everything the page asks this port for: the document, its assets,
/// `fetch` and the WebSocket handshake of `/api/events`, on which a page
/// cannot set a header.
const TOKEN_COOKIE: &str = "staged_perf_token";

struct Shared {
    app: tauri::AppHandle,
    results: Mutex<Vec<String>>,
}

/// Who may use the port. Every route is behind it: the web API and the
/// event socket run commands and carry session text, so with only `/perf/*`
/// behind the token any local process could have driven the app during a
/// run.
struct Gate {
    token: String,
    /// The only `Host` served and the only `Origin` accepted, so a page in
    /// the user's browser cannot reach this port by name or by fetch.
    host: String,
    origin: String,
}

impl Gate {
    fn new(port: u16, token: String) -> Self {
        Self {
            token,
            host: format!("127.0.0.1:{port}"),
            origin: format!("http://127.0.0.1:{port}"),
        }
    }

    /// Why a request is refused, or `None` when it may go on.
    fn refusal(&self, headers: &HeaderMap, uri: &axum::http::Uri) -> Option<StatusCode> {
        let host = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .or_else(|| uri.authority().map(|a| a.as_str()));
        let origin = headers
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok());
        if host != Some(self.host.as_str()) || origin.is_some_and(|o| o != self.origin) {
            return Some(StatusCode::FORBIDDEN);
        }
        let by_header = headers
            .get(TOKEN_HEADER)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|given| same(given, &self.token));
        // The cookie is the page's credential. The harness's own routes take
        // the header alone: no page needs them without it, and a cookie is
        // sent by whatever the browser loads from this host.
        let by_cookie = !uri.path().starts_with(PERF_PREFIX)
            && CookieJar::from_headers(headers)
                .get(TOKEN_COOKIE)
                .is_some_and(|cookie| same(cookie.value(), &self.token));
        (!by_header && !by_cookie).then_some(StatusCode::UNAUTHORIZED)
    }
}

/// Equality that takes as long for a near miss as for a far one.
fn same(given: &str, token: &str) -> bool {
    given.len() == token.len()
        && given
            .bytes()
            .zip(token.bytes())
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// `router` with the gate in front of every route, and the commands that
/// get through it counted.
fn gated(router: Router, gate: Arc<Gate>) -> Router {
    router
        .layer(middleware::from_fn(count_commands))
        .layer(middleware::from_fn_with_state(gate, admit))
}

pub(super) async fn serve(app: tauri::AppHandle, config: Config) {
    // Spawned from the top of setup: wait for the state the routes read.
    let event_tx = loop {
        if let Some(tx) = app.try_state::<broadcast::Sender<WebEvent>>() {
            break tx.inner().clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let shared = Arc::new(Shared {
        app: app.clone(),
        results: Mutex::new(Vec::new()),
    });
    let perf = Router::new()
        .route("/perf/eval", post(eval))
        .route("/perf/result", post(push_result))
        .route("/perf/results", get(take_results))
        .route("/perf/metrics", get(read_metrics))
        .route("/perf/outdated", post(read_outdated))
        .route("/perf/metrics/reset", post(reset_metrics))
        .layer(DefaultBodyLimit::max(RESULT_BODY_LIMIT))
        .with_state(shared);
    let state = WebAppState {
        app_handle: app,
        event_tx,
        auth_token: web_server::generate_token(),
        sessions: Default::default(),
    };
    let router = gated(
        web_server::app_router(state, &config.dist).merge(perf),
        Arc::new(Gate::new(config.port, config.token)),
    );
    let listener = match tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, config.port)).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("[perf-harness] cannot bind 127.0.0.1:{}: {e}", config.port);
            std::process::exit(78);
        }
    };
    eprintln!("[perf-harness] serving on 127.0.0.1:{}", config.port);
    if let Err(e) = axum::serve(listener, router).await {
        eprintln!("[perf-harness] server stopped: {e}");
    }
}

/// Every request: refuse another host or origin, and one without the
/// run's token.
async fn admit(State(gate): State<Arc<Gate>>, request: Request, next: Next) -> Response {
    match gate.refusal(request.headers(), request.uri()) {
        Some(StatusCode::UNAUTHORIZED) => (
            StatusCode::UNAUTHORIZED,
            "perf-harness: this port takes the run's token",
        )
            .into_response(),
        Some(status) => status.into_response(),
        None => next.run(request).await,
    }
}

/// Count the commands served, the driver's apart from the page's.
async fn count_commands(request: Request, next: Next) -> Response {
    let Some(command) = request.uri().path().strip_prefix(INVOKE_PREFIX) else {
        return next.run(request).await;
    };
    let command = command.to_string();
    let driver = request.headers().contains_key("x-perf-driver");
    let started = Instant::now();
    let (parts, body) = next.run(request).await.into_parts();
    // Read to the end so the bytes counted are the bytes sent.
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    metrics::command(
        &command,
        driver,
        parts.status.is_success(),
        bytes.len() as u64,
        started.elapsed(),
    );
    Response::from_parts(parts, Body::from(bytes))
}

/// Run a script in every desktop webview. Returns the windows it ran in.
async fn eval(State(shared): State<Arc<Shared>>, script: String) -> Json<Vec<String>> {
    let mut labels = Vec::new();
    for (label, window) in shared.app.webview_windows() {
        if window.eval(script.as_str()).is_ok() {
            labels.push(label);
        }
    }
    Json(labels)
}

/// A script's answer, posted by the page it ran in.
async fn push_result(State(shared): State<Arc<Shared>>, body: String) -> StatusCode {
    shared
        .results
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(body);
    StatusCode::OK
}

async fn take_results(State(shared): State<Arc<Shared>>) -> Json<Vec<String>> {
    Json(std::mem::take(
        &mut *shared.results.lock().unwrap_or_else(|e| e.into_inner()),
    ))
}

async fn read_metrics() -> Json<serde_json::Value> {
    Json(metrics::snapshot())
}

#[derive(serde::Deserialize)]
struct OutdatedQuery {
    /// When the rows were read (epoch ms).
    at: Option<u64>,
}

/// For the stamp each row shows, whether the store holds a newer one, and
/// how many it held at `at`.
async fn read_outdated(
    Query(query): Query<OutdatedQuery>,
    Json(shown): Json<HashMap<String, u64>>,
) -> Json<serde_json::Value> {
    Json(metrics::outdated(&shown, query.at))
}

async fn reset_metrics() -> StatusCode {
    metrics::reset();
    StatusCode::OK
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::sync::Arc;

    use axum::extract::ws::WebSocketUpgrade;
    use axum::routing::{get, post};
    use axum::Router;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::{gated, Gate, TOKEN_COOKIE, TOKEN_HEADER};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    /// The routes of the real router that matter here, without the app
    /// behind them, served through the real gate on a loopback port.
    async fn serve() -> u16 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let router = Router::new()
            .route("/api/invoke/{command}", post(|| async { "{}" }))
            .route(
                "/api/events",
                get(|ws: WebSocketUpgrade| async { ws.on_upgrade(|_socket| async {}) }),
            )
            .route("/perf/metrics", get(|| async { "{}" }))
            .fallback(|| async { "the frontend" });
        let router = gated(router, Arc::new(Gate::new(port, TOKEN.to_string())));
        tokio::spawn(async move { axum::serve(listener, router).await });
        port
    }

    /// The status a request gets. `head` is its request line; `headers` go
    /// after the `Host` this port serves.
    async fn status(port: u16, head: &str, headers: &[String]) -> u16 {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        let mut request = format!("{head} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
        for header in headers {
            request.push_str(header);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = Vec::new();
        let mut chunk = [0; 256];
        while !answer.windows(2).any(|pair| pair == b"\r\n") {
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0, "the connection closed before a status line");
            answer.extend_from_slice(&chunk[..read]);
        }
        let line = String::from_utf8_lossy(&answer);
        line.split(' ').nth(1).unwrap().parse().unwrap()
    }

    fn header() -> String {
        format!("{TOKEN_HEADER}: {TOKEN}")
    }

    fn cookie() -> String {
        format!("Cookie: other=1; {TOKEN_COOKIE}={TOKEN}")
    }

    fn with(base: &[&str], extra: &[String]) -> Vec<String> {
        base.iter()
            .map(|header| header.to_string())
            .chain(extra.iter().cloned())
            .collect()
    }

    const INVOKE: &str = "POST /api/invoke/list_projects";
    const INVOKE_HEADERS: [&str; 2] = ["Content-Length: 0", "Connection: close"];
    const EVENTS: &str = "GET /api/events?clientId=test";
    const EVENTS_HEADERS: [&str; 4] = [
        "Connection: Upgrade",
        "Upgrade: websocket",
        "Sec-WebSocket-Version: 13",
        "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
    ];

    #[tokio::test]
    async fn a_command_without_the_token_is_refused_and_one_with_it_is_served() {
        let port = serve().await;
        assert_eq!(status(port, INVOKE, &with(&INVOKE_HEADERS, &[])).await, 401);
        let wrong = format!("{TOKEN_HEADER}: {}", TOKEN.replace('0', "1"));
        assert_eq!(
            status(port, INVOKE, &with(&INVOKE_HEADERS, &[wrong])).await,
            401
        );
        let wrong = format!("Cookie: {TOKEN_COOKIE}=nope");
        assert_eq!(
            status(port, INVOKE, &with(&INVOKE_HEADERS, &[wrong])).await,
            401
        );
        // The driver's way and the page's.
        assert_eq!(
            status(port, INVOKE, &with(&INVOKE_HEADERS, &[header()])).await,
            200
        );
        assert_eq!(
            status(port, INVOKE, &with(&INVOKE_HEADERS, &[cookie()])).await,
            200
        );
    }

    #[tokio::test]
    async fn the_event_socket_without_the_token_is_refused_and_one_with_it_is_upgraded() {
        let port = serve().await;
        assert_eq!(status(port, EVENTS, &with(&EVENTS_HEADERS, &[])).await, 401);
        // A page cannot set a header on a WebSocket: its cookie is what the
        // handshake carries.
        assert_eq!(
            status(port, EVENTS, &with(&EVENTS_HEADERS, &[cookie()])).await,
            101
        );
        assert_eq!(
            status(port, EVENTS, &with(&EVENTS_HEADERS, &[header()])).await,
            101
        );
    }

    #[tokio::test]
    async fn the_frontend_and_the_harness_routes_are_behind_the_token_too() {
        let port = serve().await;
        let close = ["Connection: close"];
        assert_eq!(status(port, "GET /", &with(&close, &[])).await, 401);
        assert_eq!(status(port, "GET /", &with(&close, &[cookie()])).await, 200);
        assert_eq!(
            status(port, "GET /perf/metrics", &with(&close, &[])).await,
            401
        );
        assert_eq!(
            status(port, "GET /perf/metrics", &with(&close, &[header()])).await,
            200
        );
        // The harness's own routes take the header only.
        assert_eq!(
            status(port, "GET /perf/metrics", &with(&close, &[cookie()])).await,
            401
        );
    }

    #[tokio::test]
    async fn another_origin_is_refused_whatever_it_sends() {
        let port = serve().await;
        let elsewhere = "Origin: http://127.0.0.1:1".to_string();
        assert_eq!(
            status(
                port,
                INVOKE,
                &with(&INVOKE_HEADERS, &[header(), elsewhere.clone()])
            )
            .await,
            403
        );
        assert_eq!(
            status(port, EVENTS, &with(&EVENTS_HEADERS, &[cookie(), elsewhere])).await,
            403
        );
        let own = format!("Origin: http://127.0.0.1:{port}");
        assert_eq!(
            status(port, EVENTS, &with(&EVENTS_HEADERS, &[cookie(), own])).await,
            101
        );
    }
}
