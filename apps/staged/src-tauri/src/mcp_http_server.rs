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
//! The server still lives exactly as long as the session that started it:
//! dropping the [`McpHttpServer`] guard ends every MCP session the server holds
//! and stops it accepting connections.

use std::sync::Arc;

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
    /// through their request token.
    shutdown: CancellationToken,
    /// The accept loop. Aborting it stops new connections, and the
    /// connections it already accepted then shut down gracefully.
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
    let router = Router::new().route_service("/mcp", service);

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
