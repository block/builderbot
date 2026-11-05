//! Plumbing every connection to an agent shares: the child's stderr and
//! stdout, and the client-side handler registration.
use super::*;

use agent_client_protocol::{Builder, HandleDispatchFrom};

/// Drain the agent's stderr to the log so spawn/shebang failures are visible
/// instead of vanishing into a generic "server shut down", and hand every
/// line to the transport monitor (the proxy reports a dead or replaced
/// remote process there). Cancels `exited` at EOF.
pub(super) fn spawn_stderr_reader(
    stderr: tokio::process::ChildStderr,
    agent_label: String,
    session_id: String,
    exited: CancellationToken,
    transport: Arc<TransportMonitor>,
) {
    tokio::task::spawn_local(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            log::warn!("[{agent_label} stderr][session {session_id}] {line}");
            transport.observe_proxy_stderr(&line);
        }
        // stderr EOF (or a read error) means the agent process has
        // exited or torn down its stderr — either way it can no
        // longer be trusted to report background state.
        exited.cancel();
    });
}

/// The agent's stdout as the JSON-RPC transport reads it, normalized on a
/// local task.
///
/// Both normalizers record the EOF as soon as they return: the SDK sees it on
/// this same thread only after this task yields, so the signal is always in
/// before any request fails over it.
pub(super) fn normalized_stdout(
    stdout: tokio::process::ChildStdout,
    is_remote: bool,
    transport: Arc<TransportMonitor>,
) -> Box<dyn tokio::io::AsyncRead + Send + Unpin> {
    let (normalized_stdout_writer, normalized_stdout_reader) = tokio::io::duplex(64 * 1024);
    if is_remote {
        // The normalizer hands each frame over synchronously; a writer task
        // feeds them to the pipe in order, and closes it only once the
        // normalizer has returned and recorded the EOF.
        let (lines_tx, mut lines_rx) = mpsc::unbounded_channel::<String>();
        tokio::task::spawn_local(async move {
            let mut writer = normalized_stdout_writer;
            while let Some(line) = lines_rx.recv().await {
                if writer.write_all(line.as_bytes()).await.is_err()
                    || writer.write_all(b"\n").await.is_err()
                {
                    return;
                }
            }
            let _ = writer.shutdown().await;
        });
        tokio::task::spawn_local(async move {
            let emit = |line: String| {
                let _ = lines_tx.send(line);
            };
            if let Err(error) = normalize_remote_acp_stdout(stdout, emit).await {
                log::error!("remote ACP stdout normalization failed: {error}");
            }
            transport.signal(TransportSignal::StdoutClosed);
        });
    } else {
        // On the shell-spawn fallback, shell init (.zshrc, plugin banners,
        // Hermit activation) may write to stdout before `exec` replaces the
        // shell. Filter out any non-JSON lines so they don't reach the
        // JSON-RPC parser. On the direct-spawn (snapshot) path there is no
        // shell banner, so this is a harmless passthrough.
        tokio::task::spawn_local(async move {
            if let Err(error) = normalize_local_acp_stdout(stdout, normalized_stdout_writer).await {
                log::error!("local ACP stdout normalization failed: {error}");
            }
            transport.signal(TransportSignal::StdoutClosed);
        });
    }
    Box::new(normalized_stdout_reader)
}

/// The client connection with Staged's handlers registered: permission
/// requests, `session/update` frames, and extension notifications, all routed
/// through `handler`.
pub(super) fn staged_client(
    handler: &Arc<AcpNotificationHandler>,
) -> Builder<Client, impl HandleDispatchFrom<Agent>> {
    let permission_handler = Arc::clone(handler);
    let notification_handler = Arc::clone(handler);
    let ext_notification_handler = Arc::clone(handler);
    Client
        .builder()
        .name("staged-acp-client")
        .on_receive_request(
            async move |args: RequestPermissionRequest, responder, connection| {
                // The decision may wait on a person. Handlers run on
                // the connection's single dispatch task, so awaiting
                // it here would stall every other frame — the prompt
                // response that acknowledges a cancel, a steer's
                // answer — until someone clicks.
                let handler = Arc::clone(&permission_handler);
                connection.spawn(async move {
                    // A shared bridge relays every session's requests
                    // down every connection. Another session's request
                    // is left unanswered here (dropping the responder
                    // sends nothing), so the client it belongs to — or
                    // a later reattach — is the one that decides it.
                    if !handler.owns_session(args.session_id.0.as_ref()).await {
                        log::debug!(
                            "Ignoring a permission request for another ACP session ({})",
                            args.session_id.0.as_ref()
                        );
                        return Ok(());
                    }
                    let response = handler.request_permission(args).await?;
                    responder.respond(response)
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        // `session/update` frames — the typed `SessionUpdate` kinds
        // plus the asyncTasks extension's three `async_task_*`
        // discriminators the v1 enum cannot represent (see
        // [`IncomingSessionUpdate`]). The wrapper must parse the
        // typed updates on the same connection that advertises the
        // capability: an unparseable matched notification is a hard
        // dispatch error, not a fall-through.
        .on_receive_notification(
            async move |notification: IncomingSessionUpdate, _connection| match notification {
                IncomingSessionUpdate::Standard(notification) => {
                    notification_handler
                        .session_notification(notification)
                        .await
                }
                IncomingSessionUpdate::AsyncTask(notification) => {
                    notification_handler.async_task_update(notification).await
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        // Second, enum-typed handler for whatever the
        // `session/update`-typed handler doesn't match — in practice
        // the bridge's `_claude/sdkMessage` ExtNotification frames,
        // which would otherwise be actively rejected with a
        // method_not_found error frame back to the agent.
        // Registration order matters: `AgentNotification`'s
        // `matches_method` accepts every method and handlers run in
        // registration order, so this must come after the
        // `session/update`-typed handler above to avoid stealing its
        // frames.
        .on_receive_notification(
            async move |notification: AgentNotification, _connection| {
                match notification {
                    AgentNotification::ExtNotification(ext) => {
                        ext_notification_handler.ext_notification(ext).await
                    }
                    // `session/update` never lands here (consumed by
                    // the handler above); drop any other known-but-
                    // unused notification kind.
                    other => {
                        log::debug!("Ignoring unhandled ACP notification '{}'", other.method());
                        Ok(())
                    }
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
}
