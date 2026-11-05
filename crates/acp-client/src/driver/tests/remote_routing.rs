//! Routing of remote ACP frames between sessions sharing one agent process.

use super::*;
use crate::driver::remote_routing::{RemoteLoss, RemoteSessionRouter, RemoteTransport};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

fn attach(router: &RemoteSessionRouter) -> (u64, mpsc::UnboundedReceiver<String>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (router.attach(tx), rx)
}

fn received(rx: &mut mpsc::UnboundedReceiver<String>) -> Vec<String> {
    let mut frames = Vec::new();
    while let Ok(line) = rx.try_recv() {
        frames.push(describe(&serde_json::from_str(&line).unwrap()));
    }
    frames
}

/// A compact rendering of a frame: method, session and text for messages;
/// id and result for responses.
fn describe(frame: &Value) -> String {
    match frame.get("method").and_then(Value::as_str) {
        Some(method) => {
            let id = frame
                .get("id")
                .map(|id| format!(" #{id}"))
                .unwrap_or_default();
            let session = frame["params"]["sessionId"].as_str().unwrap_or("-");
            let text = frame["params"]["update"]["content"]["text"]
                .as_str()
                .unwrap_or_default();
            format!("{method}{id} {session} {text}")
                .trim_end()
                .to_string()
        }
        None => format!("response #{} {}", frame["id"], frame["result"]),
    }
}

fn wire_id(line: &str) -> Value {
    serde_json::from_str::<Value>(line).unwrap()["id"].clone()
}

fn session_new(id: u64) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": "session/new",
           "params": {"cwd": "/work", "mcpServers": []}})
    .to_string()
}

fn prompt(id: u64, session: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
           "params": {"sessionId": session, "prompt": []}})
    .to_string()
}

fn chunk(session: &str, text: &str) -> String {
    json!({"jsonrpc": "2.0", "method": "session/update", "params": {
        "sessionId": session,
        "update": {"sessionUpdate": "agent_message_chunk",
                   "content": {"type": "text", "text": text}},
    }})
    .to_string()
}

fn response(id: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

/// Two sessions, one agent process: everything the process writes is seen by
/// the router, but each session receives only its own frames, and both
/// clients numbering requests from zero cannot steal each other's responses.
#[test]
fn interleaved_sessions_sharing_one_process_reach_only_their_owner() {
    let router = RemoteSessionRouter::new();
    let (a, mut a_rx) = attach(&router);
    let (b, mut b_rx) = attach(&router);

    let a_new = router.outgoing(a, &session_new(0));
    let b_new = router.outgoing(b, &session_new(0));
    assert_ne!(wire_id(&a_new), wire_id(&b_new));
    router.incoming(&response(&wire_id(&b_new), json!({"sessionId": "sess-b"})));
    router.incoming(&response(&wire_id(&a_new), json!({"sessionId": "sess-a"})));

    let a_prompt = router.outgoing(a, &prompt(1, "sess-a"));
    let b_prompt = router.outgoing(b, &prompt(1, "sess-b"));
    for i in 0..3 {
        router.incoming(&chunk("sess-a", &format!("a{i}")));
        router.incoming(&chunk("sess-b", &format!("b{i}")));
    }
    router.incoming(
        &json!({"jsonrpc": "2.0", "id": 7, "method": "session/request_permission",
                "params": {"sessionId": "sess-b", "toolCall": {"toolCallId": "t"}, "options": []}})
        .to_string(),
    );
    router.incoming(
        &json!({"jsonrpc": "2.0", "method": "_claude/sdkMessage",
                "params": {"sessionId": "sess-a", "message": {"type": "result"}}})
        .to_string(),
    );
    // A session nobody here owns — another app instance's, or one whose
    // connection already went away — reaches no transcript.
    router.incoming(&chunk("sess-ghost", "lost"));
    router.incoming(&response(
        &wire_id(&b_prompt),
        json!({"stopReason": "end_turn"}),
    ));
    router.incoming(&response(
        &wire_id(&a_prompt),
        json!({"stopReason": "end_turn"}),
    ));

    assert_eq!(
        received(&mut a_rx),
        [
            r#"response #0 {"sessionId":"sess-a"}"#,
            "session/update sess-a a0",
            "session/update sess-a a1",
            "session/update sess-a a2",
            "_claude/sdkMessage sess-a",
            r#"response #1 {"stopReason":"end_turn"}"#,
        ]
    );
    assert_eq!(
        received(&mut b_rx),
        [
            r#"response #0 {"sessionId":"sess-b"}"#,
            "session/update sess-b b0",
            "session/update sess-b b1",
            "session/update sess-b b2",
            "session/request_permission #7 sess-b",
            r#"response #1 {"stopReason":"end_turn"}"#,
        ]
    );
}

/// Each session's own `sq blox acp` pipe carries the shared process's whole
/// stdout. A router per pipe keeps only that session's frames, so neither
/// session records — or answers — the other's.
#[test]
fn per_session_pipes_keep_only_their_own_frames_from_the_broadcast() {
    let a_pipe = RemoteSessionRouter::new();
    let b_pipe = RemoteSessionRouter::new();
    let (a, mut a_rx) = attach(&a_pipe);
    let (b, mut b_rx) = attach(&b_pipe);

    let a_new = a_pipe.outgoing(a, &session_new(0));
    let b_new = b_pipe.outgoing(b, &session_new(0));
    let mut broadcast = vec![
        response(&wire_id(&a_new), json!({"sessionId": "sess-a"})),
        response(&wire_id(&b_new), json!({"sessionId": "sess-b"})),
        // The proxy's own liveness probe is answered on the shared stdout.
        json!({"jsonrpc": "2.0", "id": "blox-acp-liveness-1", "error": {"code": -32601, "message": "Method not found"}}).to_string(),
    ];
    for line in &broadcast {
        a_pipe.incoming(line);
        b_pipe.incoming(line);
    }
    let a_prompt = a_pipe.outgoing(a, &prompt(1, "sess-a"));
    let b_prompt = b_pipe.outgoing(b, &prompt(1, "sess-b"));
    broadcast = vec![
        chunk("sess-b", "b0"),
        chunk("sess-a", "a0"),
        json!({"jsonrpc": "2.0", "id": 3, "method": "session/request_permission",
               "params": {"sessionId": "sess-a", "toolCall": {"toolCallId": "t"}, "options": []}})
        .to_string(),
        chunk("sess-b", "b1"),
        response(&wire_id(&a_prompt), json!({"stopReason": "end_turn"})),
        response(&wire_id(&b_prompt), json!({"stopReason": "end_turn"})),
    ];
    for line in &broadcast {
        a_pipe.incoming(line);
        b_pipe.incoming(line);
    }

    assert_eq!(
        received(&mut a_rx),
        [
            r#"response #0 {"sessionId":"sess-a"}"#,
            "session/update sess-a a0",
            "session/request_permission #3 sess-a",
            r#"response #1 {"stopReason":"end_turn"}"#,
        ]
    );
    assert_eq!(
        received(&mut b_rx),
        [
            r#"response #0 {"sessionId":"sess-b"}"#,
            "session/update sess-b b0",
            "session/update sess-b b1",
            r#"response #1 {"stopReason":"end_turn"}"#,
        ]
    );
}

/// An agent may announce a new session before answering `session/new`; those
/// frames are held and delivered first, while a co-tenant's are dropped.
#[test]
fn frames_announcing_a_new_session_precede_its_response() {
    let router = RemoteSessionRouter::new();
    let (client, mut rx) = attach(&router);

    let new = router.outgoing(client, &session_new(0));
    router.incoming(&chunk("sess-other", "co-tenant"));
    router.incoming(&chunk("sess-new", "hello"));
    router.incoming(&response(&wire_id(&new), json!({"sessionId": "sess-new"})));
    router.incoming(&chunk("sess-other", "still co-tenant"));
    router.incoming(&chunk("sess-new", "next"));

    assert_eq!(
        received(&mut rx),
        [
            "session/update sess-new hello",
            r#"response #0 {"sessionId":"sess-new"}"#,
            "session/update sess-new next",
        ]
    );
}

/// A resumed session is owned from its `session/load` request, so the replay
/// that precedes the load response reaches it.
#[test]
fn resumed_session_is_owned_from_its_load_request() {
    let router = RemoteSessionRouter::new();
    let (client, mut rx) = attach(&router);

    let load = router.outgoing(
        client,
        &json!({"jsonrpc": "2.0", "id": "load", "method": "session/load",
                "params": {"sessionId": "sess-old", "cwd": "/work", "mcpServers": []}})
        .to_string(),
    );
    router.incoming(&chunk("sess-old", "replayed"));
    router.incoming(&response(&wire_id(&load), json!({})));

    assert_eq!(
        received(&mut rx),
        ["session/update sess-old replayed", r#"response #"load" {}"#]
    );
}

fn tool_call(session: &str, kind: &str, status: &str) -> String {
    json!({"jsonrpc": "2.0", "method": "session/update", "params": {
        "sessionId": session,
        "update": {"sessionUpdate": kind, "toolCallId": "tool-1", "title": "Run the tests",
                   "status": status}}})
    .to_string()
}

#[test]
fn a_session_stalls_while_its_prompt_hears_nothing() {
    let router = RemoteSessionRouter::new();
    let (client, _rx) = attach(&router);
    let quiet = Duration::from_millis(50);

    router.incoming(&chunk("sess-a", "replayed history"));
    std::thread::sleep(quiet);
    assert!(
        !router.has_stalled_prompt(quiet),
        "a quiet session with no prompt in flight has not stalled"
    );

    let request = router.outgoing(client, &prompt(1, "sess-a"));
    assert!(
        !router.has_stalled_prompt(quiet),
        "a prompt that was just sent has not stalled, however long the session was quiet"
    );
    std::thread::sleep(quiet);
    assert!(
        router.has_stalled_prompt(quiet),
        "a prompt the agent never answered has stalled, with or without a tool call"
    );

    router.incoming(&chunk("sess-a", "thinking"));
    assert!(!router.has_stalled_prompt(quiet));
    router.incoming(&tool_call("sess-a", "tool_call", "in_progress"));
    std::thread::sleep(quiet);
    assert!(router.has_stalled_prompt(quiet));

    router.incoming(&chunk("sess-b", "another session's output"));
    assert!(
        router.has_stalled_prompt(quiet),
        "only the prompted session's own frames count"
    );

    router.incoming(&response(
        &wire_id(&request),
        json!({"stopReason": "end_turn"}),
    ));
    std::thread::sleep(quiet);
    assert!(!router.has_stalled_prompt(quiet), "the turn has ended");

    router.outgoing(client, &prompt(2, "sess-a"));
    router.detach(client);
    std::thread::sleep(quiet);
    assert!(
        !router.has_stalled_prompt(quiet),
        "a detached session's prompts are not watched"
    );
}

#[test]
fn detaching_names_the_sessions_left_with_a_prompt_in_flight() {
    let router = RemoteSessionRouter::new();
    let (a, _a_rx) = attach(&router);
    let (b, _b_rx) = attach(&router);
    let answered = router.outgoing(a, &prompt(1, "sess-a-done"));
    router.incoming(&response(
        &wire_id(&answered),
        json!({"stopReason": "end_turn"}),
    ));
    router.outgoing(a, &prompt(2, "sess-a"));
    router.outgoing(a, &session_new(3));
    router.outgoing(b, &prompt(1, "sess-b"));

    assert_eq!(router.detach(a), ["sess-a"]);
    assert_eq!(router.detach(a), Vec::<String>::new(), "already detached");

    // The proxy being stopped leaves a process `sq` reused running.
    router.close();
    assert_eq!(router.detach(b), ["sess-b"]);
}

#[test]
fn detaching_from_a_lost_agent_names_nothing_to_cancel() {
    let router = RemoteSessionRouter::new();
    let (a, _a_rx) = attach(&router);
    router.outgoing(a, &prompt(1, "sess-a"));

    router.lose(RemoteLoss {
        cause: "the agent process exited".to_string(),
        agent_may_survive: false,
    });
    assert_eq!(
        router.detach(a),
        Vec::<String>::new(),
        "nothing can be cancelled once the agent is lost"
    );

    // Nor once only the proxy went: those turns are left for reattaching.
    let router = RemoteSessionRouter::new();
    let (a, _a_rx) = attach(&router);
    router.outgoing(a, &prompt(1, "sess-a"));
    router.lose(RemoteLoss {
        cause: "`sq` exited".to_string(),
        agent_may_survive: true,
    });
    assert_eq!(router.detach(a), Vec::<String>::new());
}

/// A stopped session's `session/cancel` can still be on its way through its
/// connection when the proxy is stopped: stopping waits for the connection
/// to forward it, and for `sq` to relay it.
#[tokio::test(flavor = "current_thread")]
async fn a_cancel_a_departing_connection_sends_is_delivered_before_the_proxy_stops() {
    let (transport_stdin, agent_stdin) = tokio::io::duplex(64 * 1024);
    let (_agent_stdout, transport_stdout) = tokio::io::duplex(64 * 1024);
    let transport = RemoteTransport::spawn(transport_stdin, transport_stdout);
    let mut agent_stdin = BufReader::new(agent_stdin).lines();

    let streams = transport.attach();
    let mut outgoing = streams.outgoing;
    let cancel =
        json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": "sess-a"}});
    outgoing
        .write_all(format!("{cancel}\n").as_bytes())
        .await
        .unwrap();
    drop(outgoing);
    drop(streams.incoming);
    transport.close();

    let started = tokio::time::Instant::now();
    transport
        .deliver_queued_frames(Duration::from_secs(5))
        .await;
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "`sq` is given time to relay the cancel"
    );
    let written = tokio::time::timeout(Duration::from_millis(100), agent_stdin.next_line())
        .await
        .expect("the cancel was written before stopping")
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&written).unwrap(), cancel);
}

/// A connection that goes away mid-turn cancels its prompt on the shared
/// agent, which would otherwise keep serving a turn nobody waits on.
#[tokio::test(flavor = "current_thread")]
async fn a_connection_detaching_mid_turn_cancels_its_prompt() {
    let (transport_stdin, agent_stdin) = tokio::io::duplex(64 * 1024);
    let (_agent_stdout, transport_stdout) = tokio::io::duplex(64 * 1024);
    let transport = RemoteTransport::spawn(transport_stdin, transport_stdout);
    let mut agent_stdin = BufReader::new(agent_stdin).lines();

    let streams = transport.attach();
    let mut outgoing = streams.outgoing;
    outgoing
        .write_all(format!("{}\n", prompt(7, "sess-a")).as_bytes())
        .await
        .unwrap();
    let sent: Value =
        serde_json::from_str(&agent_stdin.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(sent["method"], "session/prompt");

    drop(outgoing);
    drop(streams.incoming);
    let cancel: Value = tokio::time::timeout(Duration::from_secs(5), agent_stdin.next_line())
        .await
        .expect("the detached prompt is cancelled")
        .unwrap()
        .map(|line| serde_json::from_str::<Value>(&line).unwrap())
        .unwrap();
    assert_eq!(
        cancel,
        json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": "sess-a"}})
    );
}

/// A session reattached to a dead instance's turn watches for the agent's
/// answer to that instance's prompt, which reaches every attachment.
#[test]
fn a_foreign_prompts_answer_ends_only_the_watch_on_its_instance() {
    use tokio::sync::oneshot::error::TryRecvError;

    let router = RemoteSessionRouter::new();
    let (_client, mut rx) = attach(&router);
    let mut ended = router.watch_foreign_prompts(4242, None);
    let mut cancelled = router.watch_foreign_prompts(4242, Some("cancelled"));
    let mut other = router.watch_foreign_prompts(5151, None);
    // A watch nobody waits on any more is forgotten, and answers nothing.
    drop(router.watch_foreign_prompts(4242, None));

    router.incoming(&response(&json!("staged-4242-3:load-1"), json!({})));
    router.incoming(&response(
        &json!("staged-42420-3:p"),
        json!({"stopReason": "end_turn"}),
    ));
    router.incoming(&response(&json!(7), json!({"stopReason": "end_turn"})));
    assert_eq!(
        ended.try_recv(),
        Err(TryRecvError::Empty),
        "only a stop reason from that instance ends its prompt"
    );

    router.incoming(&response(
        &json!("staged-4242-3:p"),
        json!({"stopReason": "end_turn"}),
    ));
    assert_eq!(ended.try_recv(), Ok("end_turn".to_string()));
    assert_eq!(
        cancelled.try_recv(),
        Err(TryRecvError::Empty),
        "a watch on one stop reason ignores the others"
    );
    router.incoming(&response(
        &json!("staged-4242-4:q"),
        json!({"stopReason": "cancelled"}),
    ));
    assert_eq!(cancelled.try_recv(), Ok("cancelled".to_string()));
    assert_eq!(other.try_recv(), Err(TryRecvError::Empty));
    assert!(
        received(&mut rx).is_empty(),
        "another instance's responses still reach no session"
    );
}

#[test]
fn request_cancellation_names_the_namespaced_id() {
    let router = RemoteSessionRouter::new();
    let (client, _rx) = attach(&router);

    let request = router.outgoing(client, &prompt(4, "sess-a"));
    let cancel = router.outgoing(
        client,
        &json!({"jsonrpc": "2.0", "method": "$/cancel_request", "params": {"requestId": 4}})
            .to_string(),
    );

    let cancel: Value = serde_json::from_str(&cancel).unwrap();
    assert_eq!(cancel["params"]["requestId"], wire_id(&request));
}

#[test]
fn detached_session_frames_reach_nobody() {
    let router = RemoteSessionRouter::new();
    let (a, mut a_rx) = attach(&router);
    let (b, mut b_rx) = attach(&router);
    router.outgoing(a, &prompt(1, "sess-a"));
    router.outgoing(b, &prompt(1, "sess-b"));

    router.detach(a);
    router.incoming(&chunk("sess-a", "late background output"));
    router.incoming(&chunk("sess-b", "b0"));

    assert!(received(&mut a_rx).is_empty());
    assert_eq!(received(&mut b_rx), ["session/update sess-b b0"]);
}

/// End to end through the real SDK: two ACP connections on one routed
/// transport to one agent, which interleaves both sessions' output (and some
/// for a session neither owns). Each connection's handler sees only its own.
#[tokio::test(flavor = "current_thread")]
async fn two_acp_connections_share_one_agent_process_without_cross_talk() {
    use agent_client_protocol::schema::v1::{
        InitializeRequest, InitializeResponse, PromptResponse,
    };
    use agent_client_protocol::schema::ProtocolVersion;
    use agent_client_protocol::{ByteStreams, Responder};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn text(session: &str, text: &str) -> SessionNotification {
        SessionNotification::new(
            session.to_string(),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(AcpContentBlock::Text(
                TextContent::new(text),
            ))),
        )
    }

    let (transport_stdin, agent_stdin) = tokio::io::duplex(64 * 1024);
    let (agent_stdout, transport_stdout) = tokio::io::duplex(64 * 1024);
    let transport = RemoteTransport::spawn(transport_stdin, transport_stdout);

    let next_session = Arc::new(AtomicUsize::new(0));
    // Prompts the agent holds until both sessions are mid-turn.
    type HeldPrompts = Mutex<Vec<(String, Responder<PromptResponse>)>>;
    let waiting: Arc<HeldPrompts> = Arc::default();
    let agent = agent_client_protocol::Agent
        .builder()
        .on_receive_request(
            async |_: InitializeRequest, responder, _| {
                responder.respond(InitializeResponse::new(ProtocolVersion::V1))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_: NewSessionRequest, responder, connection| {
                let id = format!("sess-{}", next_session.fetch_add(1, Ordering::Relaxed));
                connection.send_notification(text(&id, "hello"))?;
                responder.respond(NewSessionResponse::new(id))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PromptRequest, responder, connection| {
                let ready = {
                    let mut waiting = waiting.lock().unwrap();
                    waiting.push((request.session_id.to_string(), responder));
                    if waiting.len() < 2 {
                        return Ok(());
                    }
                    std::mem::take(&mut *waiting)
                };
                for i in 0..3 {
                    for (session, _) in &ready {
                        connection.send_notification(text(session, &format!("chunk {i}")))?;
                    }
                }
                connection.send_notification(text("sess-ghost", "nobody's"))?;
                for (_, responder) in ready {
                    responder.respond(PromptResponse::new(StopReason::EndTurn))?;
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(ByteStreams::new(
            agent_stdout.compat_write(),
            agent_stdin.compat(),
        ));

    let run_session = || {
        let streams = transport.attach();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let recorded = Arc::clone(&seen);
        let connection = agent_client_protocol::Client
            .builder()
            .on_receive_notification(
                async move |notification: SessionNotification, _| {
                    let SessionUpdate::AgentMessageChunk(chunk) = notification.update else {
                        return Ok(());
                    };
                    let AcpContentBlock::Text(text) = chunk.content else {
                        return Ok(());
                    };
                    recorded
                        .lock()
                        .unwrap()
                        .push(format!("{} {}", notification.session_id, text.text));
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .connect_with(
                ByteStreams::new(streams.outgoing.compat_write(), streams.incoming.compat()),
                async |connection| {
                    connection
                        .send_request(InitializeRequest::new(ProtocolVersion::V1))
                        .block_task()
                        .await?;
                    let session = connection
                        .send_request(NewSessionRequest::new(PathBuf::from("/work")))
                        .block_task()
                        .await?;
                    connection
                        .send_request(PromptRequest::new(session.session_id.clone(), vec![]))
                        .block_task()
                        .await?;
                    Ok(session.session_id.to_string())
                },
            );
        async move { (connection.await.unwrap(), seen) }
    };

    let sessions = async { tokio::join!(run_session(), run_session()) };
    let ((first, first_seen), (second, second_seen)) = tokio::select! {
        result = agent => panic!("agent stopped early: {result:?}"),
        sessions = tokio::time::timeout(Duration::from_secs(10), sessions) => sessions.unwrap(),
    };

    assert_ne!(first, second);
    for (session, seen) in [(first, first_seen), (second, second_seen)] {
        assert_eq!(
            *seen.lock().unwrap(),
            ["hello", "chunk 0", "chunk 1", "chunk 2"].map(|text| format!("{session} {text}"))
        );
    }
}
