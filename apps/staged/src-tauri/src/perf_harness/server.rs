//! The harness's loopback server: the web API and frontend, and `/perf/*`.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use tauri::Manager;
use tokio::sync::broadcast;

use super::guard::Config;
use super::metrics;
use crate::web_server::{self, WebAppState, WebEvent};

const INVOKE_PREFIX: &str = "/api/invoke/";
/// A result is a page's whole measurement, so well over axum's 2 MiB.
const RESULT_BODY_LIMIT: usize = 64 * 1024 * 1024;

struct Shared {
    app: tauri::AppHandle,
    token: String,
    /// The only `Host` served and the only `Origin` accepted, so a page in
    /// the user's browser cannot reach this port by name or by fetch.
    host: String,
    origin: String,
    results: Mutex<Vec<String>>,
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
        token: config.token,
        host: format!("127.0.0.1:{}", config.port),
        origin: format!("http://127.0.0.1:{}", config.port),
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
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&shared),
            require_token,
        ))
        .with_state(Arc::clone(&shared));
    let state = WebAppState {
        app_handle: app,
        event_tx,
        auth_token: web_server::generate_token(),
        sessions: Default::default(),
    };
    let router = web_server::app_router(state, &config.dist)
        .merge(perf)
        .layer(middleware::from_fn_with_state(shared, observe));
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

/// Every request: refuse another host or origin, and count the commands.
async fn observe(State(shared): State<Arc<Shared>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| request.uri().authority().map(|a| a.as_str()));
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    if host != Some(shared.host.as_str()) || origin.is_some_and(|o| o != shared.origin) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(command) = request.uri().path().strip_prefix(INVOKE_PREFIX) else {
        return next.run(request).await;
    };
    let command = command.to_string();
    let driver = headers.contains_key("x-perf-driver");
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

async fn require_token(
    State(shared): State<Arc<Shared>>,
    request: Request,
    next: Next,
) -> Response {
    let given = request
        .headers()
        .get("x-perf-token")
        .and_then(|value| value.to_str().ok());
    if given != Some(shared.token.as_str()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
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

/// For the stamp each row shows, whether the store holds a newer one.
async fn read_outdated(Json(shown): Json<HashMap<String, u64>>) -> Json<serde_json::Value> {
    Json(metrics::outdated(&shown))
}

async fn reset_metrics() -> StatusCode {
    metrics::reset();
    StatusCode::OK
}
