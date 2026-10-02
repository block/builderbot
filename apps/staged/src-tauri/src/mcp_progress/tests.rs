use super::*;
use axum::Router;
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{ErrorData, ServerHandler};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::mpsc;

// Test-only handler keeps real HTTP requests open until disconnect. It uses
// the same progress future and cancellation pattern as both production tools.
#[derive(Clone)]
struct WaitingHandler(mpsc::UnboundedSender<RequestContext<RoleServer>>);

impl ServerHandler for WaitingHandler {
    async fn call_tool(
        &self,
        _: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.0.send(ctx.clone()).unwrap();
        tokio::select! {
            _ = send_progress_keepalives(&ctx.peer, ctx.meta.get_progress_token(), "Working") => unreachable!(),
            _ = ctx.ct.cancelled() => Ok(CallToolResult::success(vec![]).into()),
        }
    }
}

async fn next_progress(response: &mut reqwest::Response) -> Value {
    let mut body = String::new();
    loop {
        body.push_str(std::str::from_utf8(&response.chunk().await.unwrap().unwrap()).unwrap());
        for line in body.lines().filter_map(|line| line.strip_prefix("data:")) {
            if let Ok(message) = serde_json::from_str::<Value>(line) {
                if message["method"] == "notifications/progress" {
                    return message["params"].clone();
                }
            }
        }
    }
}

#[tokio::test]
async fn sessionless_progress_opens_headers_immediately_and_disconnect_cancels() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let service = StreamableHttpService::new(
        move || Ok(WaitingHandler(tx.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route_service("/mcp", service))
            .await
            .unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let request = |token: Option<Value>| {
        let mut meta = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": {"name": "staged-test", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {}
        });
        if let Some(token) = token {
            meta["progressToken"] = token;
        }
        client
            .post(&url)
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/call")
            .header("Mcp-Name", "wait")
            .json(&json!({"jsonrpc":"2.0", "id":1, "method":"tools/call",
                "params":{"name":"wait", "arguments":{}, "_meta":meta}}))
    };

    for token in [json!(7), json!("wait-token")] {
        let mut response =
            tokio::time::timeout(Duration::from_secs(2), request(Some(token.clone())).send())
                .await
                .expect("headers must arrive before the first 30s interval")
                .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert!(response.headers().get("Mcp-Session-Id").is_none());
        let ctx = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("handler starts")
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(2), next_progress(&mut response))
            .await
            .expect("first notification");
        assert_eq!(first["progressToken"], token);
        assert_eq!(first["progress"], 0.0);
        assert!(first.get("total").is_none());
        if token.is_number() {
            // Await the actual HTTP notification; pausing Tokio time while
            // reqwest/hyper are using real sockets can race their IO drivers.
            let next = tokio::time::timeout(Duration::from_secs(35), next_progress(&mut response))
                .await
                .expect("periodic notification");
            assert_eq!(next["progressToken"], token);
            assert!(next["progress"].as_f64().unwrap() >= 30.0);
            assert!(next["message"].as_str().unwrap().contains("elapsed"));
        }
        drop(response);
        tokio::time::timeout(Duration::from_secs(2), ctx.ct.cancelled())
            .await
            .unwrap();
    }

    // No invented progress token or headers: disconnect must still cancel
    // the handler while rmcp is waiting for its first message.
    let call = tokio::spawn(request(None).send());
    let ctx = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("handler starts")
        .unwrap();
    assert!(!call.is_finished());
    call.abort();
    let _ = call.await;
    tokio::time::timeout(Duration::from_secs(2), ctx.ct.cancelled())
        .await
        .unwrap();
    server.abort();
    let _ = server.await;
}
