//! The local HTTP MCP servers a session hands its agent (`builderbot`, the
//! note tool, `pikchr`, and a diagram run's `pikchr-preview`), served on the
//! app's shared multi-threaded runtime.
//!
//! A session's own thread runs one current-thread runtime that reads the
//! agent, fires the setup timers and writes the transcript. A server spawned
//! there could only answer while that thread was free, so an agent's MCP
//! `initialize` during `session/new` waited on the very thread waiting for
//! the `session/new` answer. Here the listener is created and served on
//! `tauri::async_runtime`, whose workers answer the MCP handshake however busy
//! or parked the session thread is. No handler reaches back into the
//! session thread: they hold only `Send` handles (store, registry, slots).
//!
//! The server still lives exactly as long as the session that started it.
//! Dropping the [`McpHttpServer`] guard ends every MCP session the server
//! holds, stops it accepting connections, and closes the ones it has: idle
//! keep-alive connections at once, one with a request in flight when that
//! request ends. Those closes happen on the shared runtime a moment after
//! the drop, so the guard also marks the server gone as it drops, and a
//! request that still reaches it on an open connection is refused (503)
//! instead of being served: nothing starts a new MCP session after the drop.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Router;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::ServerHandler;
use tokio_util::sync::CancellationToken;

/// A running local MCP server. Dropping it shuts the server down.
#[must_use = "dropping the guard shuts the MCP server down"]
pub struct McpHttpServer {
    port: u16,
    /// Handed to rmcp: cancelling it terminates every MCP session the server
    /// holds, which ends open SSE streams and signals in-flight tool calls
    /// through their request token. Also what [`refuse_once_gone`] reads:
    /// from the cancel on, no request is passed to rmcp.
    shutdown: CancellationToken,
    /// The accept loop. Aborting it closes the listener, and dropping its
    /// future tells the connections it already accepted (each on a task of
    /// its own) to shut down gracefully: hyper closes an idle one and lets
    /// a request in flight finish first.
    task: tauri::async_runtime::JoinHandle<()>,
}

impl McpHttpServer {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The server's MCP endpoint.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }
}

impl Drop for McpHttpServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.task.abort();
    }
}

/// Bind a loopback port and serve `factory`'s handler at `/mcp` on the shared
/// runtime.
///
/// Binding is synchronous (a loopback `bind` does not wait on anything), so
/// the port is known on return and the caller needs no runtime of its own.
/// The listener is registered with the shared runtime's reactor inside the
/// spawned task: one registered with the caller's runtime would only see
/// connections while the caller's thread polled its driver.
///
/// `label` names the server in logs.
pub fn serve_mcp<S, F>(label: &'static str, factory: F) -> Result<McpHttpServer, String>
where
    S: ServerHandler + Send + 'static,
    F: Fn() -> S + Send + Sync + 'static,
{
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to bind {label} MCP listener: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("Failed to make the {label} MCP listener non-blocking: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to get local address: {e}"))?
        .port();

    let shutdown = CancellationToken::new();
    let service = StreamableHttpService::new(
        move || Ok(factory()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().with_cancellation_token(shutdown.clone()),
    );
    let gone = shutdown.clone();
    let router = Router::new()
        .route_service("/mcp", service)
        .layer(axum::middleware::from_fn(
            move |request: Request, next: Next| refuse_once_gone(gone.clone(), request, next),
        ));

    let task = tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(e) => {
                log::error!("[{label}] could not register the MCP listener on port {port}: {e}");
                return;
            }
        };
        if let Err(e) = axum::serve(listener, router).await {
            log::error!("[{label}] HTTP server error: {e}");
        }
    });

    log::debug!("[{label}] HTTP server bound on port {port}");
    Ok(McpHttpServer {
        port,
        shutdown,
        task,
    })
}

/// Refuse a request that arrives after the server's guard was dropped.
///
/// The drop closes the server's connections, but on the shared runtime and
/// so a moment later. Until then a client could still send a request down
/// a keep-alive connection, and it reached rmcp and the handler factory of
/// a server whose session has ended (rmcp then failed an `initialize` with
/// "Session service terminated", because its token is cancelled too). The
/// token is cancelled in the drop itself, so from the moment the drop
/// returns no request gets that far.
async fn refuse_once_gone(gone: CancellationToken, request: Request, next: Next) -> Response {
    if !gone.is_cancelled() {
        return next.run(request).await;
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::CONNECTION, HeaderValue::from_static("close"))],
        "The session this MCP server belonged to has ended",
    )
        .into_response()
}

/// Run a synchronous piece of a tool handler (store access, file reads,
/// rendering) on the blocking pool, so a slow store lock or a long render
/// holds a blocking thread rather than one of the shared runtime's workers.
///
/// The error is a message fit for a tool result: the closure panicked, or the
/// runtime is shutting down.
pub(crate) async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("Staged could not finish the request: {e}"))
}

#[cfg(test)]
#[path = "mcp_http_server/tests.rs"]
mod tests;
