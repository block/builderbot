use super::*;
use serde_json::{json, Value};

async fn rpc_result(response: reqwest::Response) -> Value {
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body = response.text().await.unwrap();
    // Ignore SSE priming events, whose data is empty.
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find(|message| message.get("id").is_some())
        .unwrap_or_else(|| panic!("missing JSON-RPC response in {body:?}"))
}

#[tokio::test]
async fn preview_serves_sessionless_and_legacy_tools() {
    let slot = Arc::new(LastRenderSlot::new());
    let (port, server) = start_pikchr_preview_mcp_server(DEFAULT_SCALE, Arc::clone(&slot))
        .await
        .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}/mcp");
    let post = || {
        client
            .post(&url)
            .header("Accept", "application/json, text/event-stream")
    };

    let discovery = post()
        .header("Mcp-Method", "server/discover")
        .header("Mcp-Protocol-Version", "2026-07-28")
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "server/discover",
            "params": {"_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientInfo": {"name": "staged-test", "version": "1"},
                "io.modelcontextprotocol/clientCapabilities": {}
            }}
        }))
        .send()
        .await
        .unwrap();
    assert!(discovery.headers().get("Mcp-Session-Id").is_none());
    let discovery = rpc_result(discovery).await;
    assert!(discovery["result"]["supportedVersions"]
        .as_array()
        .unwrap()
        .contains(&json!("2026-07-28")));

    // Modern calls carry their own metadata and need no initialize/session.
    let modern = |method: &str, id: u32, mut params: Value| {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": {"name": "staged-test", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {}
        });
        post()
            .header("Mcp-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", method)
            .json(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
    };
    let list = modern("tools/list", 10, json!({})).send().await.unwrap();
    assert!(list.headers().get("Mcp-Session-Id").is_none());
    let list = rpc_result(list).await;
    assert_eq!(list["result"]["tools"][0]["name"], "render_pikchr");
    assert_eq!(list["result"]["resultType"], "complete");
    for (id, source) in [
        (11, "box \"first request\""),
        (12, "box \"second request\""),
    ] {
        let render = modern(
            "tools/call",
            id,
            json!({
                "name": "render_pikchr", "arguments": {"pikchr": source}
            }),
        )
        .header("Mcp-Name", "render_pikchr")
        .send()
        .await
        .unwrap();
        assert!(render.headers().get("Mcp-Session-Id").is_none());
        let render = rpc_result(render).await;
        assert_eq!(render["result"]["isError"], false);
        assert_eq!(render["result"]["resultType"], "complete");
        assert!(render["result"]["content"]
            .as_array()
            .unwrap()
            .iter()
            .any(|content| content["type"] == "image" && content["mimeType"] == "image/png"));
        assert_eq!(slot.take().unwrap().source, source);
    }

    let initialize = post()
        .json(&json!({
            "jsonrpc": "2.0", "id": 2, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": {"name": "staged-test", "version": "1"}
            }
        }))
        .send()
        .await
        .unwrap();
    let session_id = initialize.headers()["Mcp-Session-Id"].clone();
    let initialize = rpc_result(initialize).await;
    assert_eq!(initialize["result"]["protocolVersion"], "2025-11-25");
    let session_post = || {
        post()
            .header("Mcp-Session-Id", &session_id)
            .header("Mcp-Protocol-Version", "2025-11-25")
    };
    let initialized = session_post()
        .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .send()
        .await
        .unwrap();
    assert_eq!(initialized.status(), reqwest::StatusCode::ACCEPTED);

    let list = rpc_result(
        session_post()
            .json(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {}}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(list["result"]["tools"][0]["name"], "render_pikchr");
    assert!(list["result"].get("resultType").is_none());

    let source = "box \"legacy session\"";
    let render = rpc_result(
        session_post()
            .json(&json!({
                "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                "params": {"name": "render_pikchr", "arguments": {"pikchr": source}}
            }))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(render["result"]["isError"], false);
    assert!(render["result"].get("resultType").is_none());
    assert!(render["result"]["content"]
        .as_array()
        .unwrap()
        .iter()
        .any(|content| content["type"] == "image" && content["mimeType"] == "image/png"));
    assert_eq!(slot.take().unwrap().source, source);

    let deleted = client
        .delete(&url)
        .header("Mcp-Session-Id", session_id)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), reqwest::StatusCode::ACCEPTED);
    server.abort();
    let _ = server.await;
}
