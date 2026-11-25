use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

/// The JSON-RPC response in an MCP HTTP reply, which is either plain JSON or
/// an SSE stream whose priming events carry no data.
async fn rpc_result(response: reqwest::Response) -> Value {
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body = response.text().await.unwrap();
    if let Ok(message) = serde_json::from_str::<Value>(&body) {
        return message;
    }
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find(|message| message.get("id").is_some())
        .unwrap_or_else(|| panic!("missing JSON-RPC response in {body:?}"))
}

/// A stand-in for a session thread: a current-thread runtime and `LocalSet`,
/// as `session_runner` builds, that starts the session's note MCP server and
/// then blocks outright (not even polling its own runtime) until released.
/// Releasing it ends the "session", dropping the server guard.
struct BlockedSessionThread {
    port: u16,
    release: mpsc::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

fn start_blocked_session_thread(draft_path: std::path::PathBuf) -> BlockedSessionThread {
    let (port_tx, port_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async move {
            let server = crate::note_tool::start_note_mcp_server(
                "session-blocked".to_string(),
                crate::note_tool::new_submission_slot(),
                draft_path,
            )
            .unwrap();
            port_tx.send(server.port()).unwrap();
            // Parked: this thread runs nothing until the test lets it go.
            let _ = release_rx.recv();
            drop(server);
        });
    });
    let port = port_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the session thread starts its server");
    BlockedSessionThread {
        port,
        release,
        thread,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

#[tokio::test]
async fn initialize_is_answered_while_the_session_thread_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let draft_path = dir.path().join("draft.md");
    let session = start_blocked_session_thread(draft_path.clone());
    let url = format!("http://127.0.0.1:{}/mcp", session.port);
    let client = client();

    // The legacy handshake an agent makes during `session/new`.
    let initialize = client
        .post(&url)
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": {"name": "staged-test", "version": "1"}
            }
        }))
        .send()
        .await
        .expect("initialize is answered with the session thread blocked");
    let initialize = rpc_result(initialize).await;
    assert_eq!(initialize["result"]["protocolVersion"], "2025-11-25");
    assert!(
        initialize["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains(&draft_path.display().to_string()),
        "{initialize}"
    );

    // And the tool list, sessionless.
    let list = client
        .post(&url)
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/list")
        .json(&json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/list",
            "params": {"_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientInfo": {"name": "staged-test", "version": "1"},
                "io.modelcontextprotocol/clientCapabilities": {}
            }}
        }))
        .send()
        .await
        .expect("tools/list is answered with the session thread blocked");
    let list = rpc_result(list).await;
    assert_eq!(list["result"]["tools"][0]["name"], "write_note");

    // Ending the session drops the guard, which stops the server.
    session.release.send(()).unwrap();
    session.thread.join().unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", session.port))
            .await
            .is_err()
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the server still accepts connections after its session ended"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn initialize_request() -> Value {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "staged-test", "version": "1"}
        }
    })
}

/// A client that keeps its connection to the server open between requests
/// must not be able to start a new MCP session once the guard is dropped:
/// neither on that connection nor on a new one, and from the moment the
/// drop returns, not some time after.
#[tokio::test]
async fn nothing_is_served_on_a_kept_alive_connection_after_the_guard_drops() {
    let dir = tempfile::tempdir().unwrap();
    for round in 0..20 {
        let server = crate::note_tool::start_note_mcp_server(
            format!("session-dropped-{round}"),
            crate::note_tool::new_submission_slot(),
            dir.path().join("draft.md"),
        )
        .unwrap();
        let url = server.url();
        // One connection, kept in the client's pool between requests.
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(1)
            .build()
            .unwrap();
        let post = || {
            client
                .post(&url)
                .header("Accept", "application/json, text/event-stream")
                .json(&initialize_request())
                .send()
        };

        let before = rpc_result(post().await.expect("initialize while the server lives")).await;
        assert_eq!(before["result"]["protocolVersion"], "2025-11-25");

        drop(server);
        // Either the connection is already closed (and a new one refused),
        // or the request reaches the server and is turned away.
        match post().await {
            Err(_) => {}
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                assert_eq!(
                    status,
                    reqwest::StatusCode::SERVICE_UNAVAILABLE,
                    "round {round}: a dropped server answered initialize: {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn run_blocking_returns_the_value_and_reports_a_panic() {
    assert_eq!(super::run_blocking(|| 7).await, Ok(7));
    let error = super::run_blocking(|| -> u8 { panic!("boom") })
        .await
        .unwrap_err();
    assert!(
        error.starts_with("Staged could not finish the request"),
        "{error}"
    );
}
