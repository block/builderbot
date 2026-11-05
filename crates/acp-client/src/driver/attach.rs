//! Attaching a fresh connection to a Blox session's remote agent process:
//! catching the transcript up, following a turn still in flight to its end,
//! and cancelling one.
//!
//! The turn was prompted over a connection that is gone, so its
//! `session/prompt` response never matches a request here (the relay
//! broadcasts it, and the SDK drops it as unknown). Its end is read instead
//! from the session's own raw SDK frames — `result`, then
//! `session_state_changed: idle` — where the agent sends them (the Claude
//! bridge), and from the workstation process status for every agent
//! ([`RemoteProcessStatus`]).
use super::*;

use tokio::process::ChildStdin;
use tokio_util::compat::Compat;

use catch_up::LatestPrompt;

/// How an [`AcpDriver::attach`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachOutcome {
    /// The turn is over, and the transcript caught up with it. `result`
    /// comes from the session's last `result` frame: `None` when the agent
    /// sends none, or the turn ended before the attach could see it.
    /// `latest_prompt` says whether the agent ever received the newest
    /// prompt the transcript holds: if it didn't, the turn that ended is an
    /// earlier prompt's. `background_work_live` says the session was still
    /// working, as far as this connection saw: background tasks it saw start
    /// and not settle, or the session still busy when the process read
    /// `IDLE` (which only says every prompt was answered), as in a
    /// continuation cycle a settled task woke. Tasks started before the
    /// attach are in the process's recording only (see
    /// [`AcpDriver::recorded_turn`]).
    Ended {
        result: Option<TurnResult>,
        latest_prompt: LatestPrompt,
        background_work_live: bool,
    },
    /// Detached ([`AttachedSession::detach_handle`]) before the turn was
    /// seen to end: it may still be running remotely.
    Live,
    /// The session's process is gone, and the turn with it: Blox listed it
    /// as ended (`message` is what it recorded, e.g. "bloxlet restarted
    /// while command was running"), or the proxy started a new process in
    /// its place, whose replay the transcript caught up from.
    /// `latest_prompt` is as for [`AttachOutcome::Ended`].
    ProcessGone {
        message: Option<String>,
        latest_prompt: LatestPrompt,
    },
    /// The connection to the workstation was lost, the proxy's report that
    /// its remote process went away included; the turn's fate is unknown.
    TransportLost,
}

/// How a turn ended, by the Claude Agent SDK `result` frame that closed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnResult {
    /// The turn ended for this reason.
    Stopped(StopReason),
    /// The turn failed: the bridge answered (or would have answered) its
    /// prompt with an error. `message` is the error the result carries.
    Failed { message: Option<String> },
}

/// The remote checkout an attach-cancel's catch-up loads the session in,
/// still being resolved by the caller (see
/// [`AcpDriver::attach_cancel_and_catch_up`]): awaited only once the
/// cancel's outcome is known, so resolving it never holds the cancel up.
pub type PendingCheckout<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<PathBuf, String>> + 'a>>;

/// How an [`AcpDriver::attach_cancel`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelOutcome {
    /// The session is idle: the cancel landed, or nothing was running.
    Stopped,
    /// The session's process is gone, and its turn with it (see
    /// [`AttachOutcome::ProcessGone`]).
    ProcessGone { message: Option<String> },
    /// The cancel went out, but nothing confirmed the turn stopped before
    /// the deadline.
    Unconfirmed,
    /// The connection was lost before the cancel was confirmed.
    TransportLost,
}

/// An attach in progress; see [`AcpDriver::attach`].
pub struct AttachedSession {
    caught_up: watch::Receiver<bool>,
    task: tokio::task::JoinHandle<Result<AttachOutcome, String>>,
    transport: Arc<TransportMonitor>,
}

impl AttachedSession {
    /// Wait until the replay is recorded and the attach follows the live
    /// stream. `false` when the attach ended first (see [`Self::settled`]).
    pub async fn caught_up(&mut self) -> bool {
        self.caught_up
            .wait_for(|caught_up| *caught_up)
            .await
            .is_ok()
    }

    /// Stop following without stopping the turn: the proxy is SIGKILLed with
    /// no SIGINT, and the attach settles [`AttachOutcome::Live`] unless it
    /// already saw the turn end.
    pub fn detach_handle(&self) -> DetachHandle {
        DetachHandle::new(Arc::clone(&self.transport))
    }

    /// The attach's outcome, once the proxy is stopped.
    pub async fn settled(self) -> Result<AttachOutcome, String> {
        self.task
            .await
            .map_err(|e| format!("ACP attach task failed: {e}"))?
    }
}

/// Pacing for an attach's waits on the remote process.
#[derive(Debug, Clone, Copy)]
pub(super) struct AttachTiming {
    /// Between process-status reads. Each read is one or two `sq` calls.
    pub(super) poll: Duration,
    /// How long an `IDLE` process gets for the session's own `idle` frame
    /// (and the `result` before it) to arrive, when the agent sends frames.
    pub(super) idle_drain: Duration,
    /// How long [`AcpDriver::attach_cancel_and_catch_up`]'s `session/load`
    /// gets, once the cancel is confirmed.
    pub(super) cancel_catch_up: Duration,
}

impl Default for AttachTiming {
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(5),
            idle_drain: Duration::from_secs(2),
            cancel_catch_up: Duration::from_secs(30),
        }
    }
}

/// Where an attach-cancel's catch-up reads the stored transcript from and
/// records what it lacks through, and who hears the cancel's outcome before
/// the catch-up starts (see [`AcpDriver::attach_cancel_and_catch_up`]).
pub(super) struct CatchUpThrough<'a> {
    pub(super) store: &'a Arc<dyn Store>,
    pub(super) writer: &'a Arc<dyn MessageWriter>,
    pub(super) on_cancelled: Box<dyn FnOnce(&CancelOutcome) + 'a>,
    /// The checkout to load the session in, when the caller is still
    /// resolving it; `None` loads it in the driver's remote working
    /// directory.
    pub(super) checkout: Option<PendingCheckout<'a>>,
}

type ProxyStreams =
    ByteStreams<Compat<ChildStdin>, Compat<Box<dyn tokio::io::AsyncRead + Send + Unpin>>>;

/// A spawned `sq blox acp` attached to the session's process.
struct AttachProxy {
    child: tokio::process::Child,
    transport: Arc<TransportMonitor>,
    streams: ProxyStreams,
}

impl AcpDriver {
    /// Attach to a Blox session's remote agent process, catch the transcript
    /// up with it, and follow its turn, if one is running, to the end.
    ///
    /// `command_id` pins the process the session last ran on; without one,
    /// the session's own argv finds it (see [`AcpDriver::for_workspace`]).
    /// The connection sends `initialize` and `session/load` with the usual
    /// `_meta`, records the replay past what `store` already holds for
    /// `session_id` through `writer` (deduplicated by message and tool-call
    /// id, and accepting a replay that ends mid-turn), then follows the live
    /// stream until the turn is seen to end, the process goes, the transport
    /// is lost, or the caller detaches.
    ///
    /// When the proxy reports starting a new process in place of the
    /// session's (`Requested ACP process has exited`), the turn is over: the
    /// attach backfills from the new process's replay and settles
    /// [`AttachOutcome::ProcessGone`].
    ///
    /// Must be called from within a Tokio `LocalSet`.
    #[allow(clippy::too_many_arguments)]
    pub fn attach(
        &self,
        session_id: &str,
        agent_session_id: &str,
        command_id: Option<&str>,
        working_dir: &Path,
        store: &Arc<dyn Store>,
        writer: &Arc<dyn MessageWriter>,
        auth_selection: Option<&AcpAuthenticationSelection>,
    ) -> Result<AttachedSession, String> {
        self.attach_with_timing(
            session_id,
            agent_session_id,
            command_id,
            working_dir,
            store,
            writer,
            auth_selection,
            AttachTiming::default(),
        )
    }

    /// [`AcpDriver::attach`] with its pacing as a parameter (tests).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn attach_with_timing(
        &self,
        session_id: &str,
        agent_session_id: &str,
        command_id: Option<&str>,
        working_dir: &Path,
        store: &Arc<dyn Store>,
        writer: &Arc<dyn MessageWriter>,
        auth_selection: Option<&AcpAuthenticationSelection>,
        timing: AttachTiming,
    ) -> Result<AttachedSession, String> {
        // Without what is stored, the whole replay would be recorded again.
        let stored = store.get_session_replay_boundaries(session_id)?;
        let acp_working_dir =
            resolve_acp_working_dir(working_dir, true, self.remote_working_dir.as_deref())?;
        let AttachProxy {
            mut child,
            transport,
            streams,
        } = self.spawn_attach_proxy(session_id, command_id, working_dir)?;
        let handler = Arc::new(
            AcpNotificationHandler::new(
                Arc::clone(writer),
                false,
                Vec::new(),
                CancellationToken::new(),
            )
            .catching_up(&stored)
            .with_transport_monitor(Arc::clone(&transport)),
        );
        let (caught_up_tx, caught_up) = watch::channel(false);

        let context = AttachContext {
            our_session_id: session_id.to_string(),
            agent_session_id: agent_session_id.to_string(),
            command_id: command_id.map(str::to_string),
            working_dir: acp_working_dir,
            store: Arc::clone(store),
            mcp_servers: self.mcp_servers.clone(),
            agent_label: self.agent_label.clone(),
            auth_selection: auth_selection.cloned(),
            unattended_session_mode: self.unattended_session_mode.clone(),
            blox: self.blox_process.clone(),
            timing,
        };
        let writer = Arc::clone(writer);
        let task_transport = Arc::clone(&transport);
        let task = tokio::task::spawn_local(async move {
            let transport = task_transport;
            let protocol_result = staged_client(&handler)
                .connect_with(streams, async |connection| {
                    run_attach(&connection, &handler, &context, caught_up_tx)
                        .await
                        .map_err(agent_client_protocol::util::internal_error)
                })
                .await
                .map_err(|e| format!("ACP protocol failed: {e:?}"));

            handler.cancel_pending_permissions();
            writer.finalize().await;
            stop_proxy(&mut child, &transport).await;
            match protocol_result {
                Ok(outcome) => Ok(outcome),
                // The SDK ended the connection under the attach.
                Err(_) if transport.first_signal().is_some() => Ok(lost_outcome(&transport)),
                Err(e) => Err(e),
            }
        });

        Ok(AttachedSession {
            caught_up,
            task,
            transport,
        })
    }

    /// Cancel a Blox session's turn from a fresh connection: attach,
    /// `initialize`, then `session/cancel` (no `session/load`), and wait up
    /// to `deadline` for the session to go idle — its own `idle` frame where
    /// the agent sends one, otherwise the process reading `IDLE`.
    ///
    /// Must be called from within a Tokio `LocalSet`.
    pub async fn attach_cancel(
        &self,
        session_id: &str,
        agent_session_id: &str,
        command_id: Option<&str>,
        working_dir: &Path,
        deadline: Duration,
    ) -> Result<CancelOutcome, String> {
        self.attach_cancel_with_timing(
            session_id,
            agent_session_id,
            command_id,
            working_dir,
            None,
            deadline,
            AttachTiming::default(),
        )
        .await
        .map(|(outcome, _)| outcome)
    }

    /// [`AcpDriver::attach_cancel`], then catch the transcript up with the
    /// turn the cancel ended, as [`AcpDriver::attach`] does: once the turn
    /// is confirmed over (or its process replaced), a `session/load` on the
    /// same connection records what `store` lacks for `session_id` through
    /// `writer` — the end of the tool call that was running included, which
    /// would otherwise stay running in the transcript for good.
    ///
    /// The catch-up never holds up or fails the cancel: it goes out only
    /// after the cancel, and a catch-up that fails or runs out of time is
    /// logged, leaving the outcome as the cancel had it. Nothing is loaded
    /// when the cancel is unconfirmed or the transport is lost.
    ///
    /// `on_cancelled` hears the cancel's outcome as soon as it is known,
    /// before any catch-up starts: the load takes longer the longer the
    /// session's transcript, and a caller ending the session on the outcome
    /// needn't wait on it. It is not called when the cancel fails outright
    /// (an `Err`).
    ///
    /// The session is loaded in `checkout` when given — the caller resolving
    /// it alongside the cancel, which doesn't need it — else in the driver's
    /// remote working directory.
    ///
    /// Returns, with the outcome, whether the replay showed the agent
    /// receiving the newest prompt the transcript holds, as an attach's
    /// [`AttachOutcome::Ended`] does: [`LatestPrompt::Unknown`] when nothing
    /// was loaded.
    ///
    /// Must be called from within a Tokio `LocalSet`.
    #[allow(clippy::too_many_arguments)]
    pub async fn attach_cancel_and_catch_up(
        &self,
        session_id: &str,
        agent_session_id: &str,
        command_id: Option<&str>,
        working_dir: &Path,
        store: &Arc<dyn Store>,
        writer: &Arc<dyn MessageWriter>,
        deadline: Duration,
        on_cancelled: impl FnOnce(&CancelOutcome),
        checkout: Option<PendingCheckout<'_>>,
    ) -> Result<(CancelOutcome, LatestPrompt), String> {
        self.attach_cancel_with_timing(
            session_id,
            agent_session_id,
            command_id,
            working_dir,
            Some(CatchUpThrough {
                store,
                writer,
                on_cancelled: Box::new(on_cancelled),
                checkout,
            }),
            deadline,
            AttachTiming::default(),
        )
        .await
    }

    /// [`AcpDriver::attach_cancel`], catching up through `catch_up` when
    /// given (see [`AcpDriver::attach_cancel_and_catch_up`]), with its
    /// pacing as a parameter (tests).
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn attach_cancel_with_timing(
        &self,
        session_id: &str,
        agent_session_id: &str,
        command_id: Option<&str>,
        working_dir: &Path,
        catch_up: Option<CatchUpThrough<'_>>,
        deadline: Duration,
        timing: AttachTiming,
    ) -> Result<(CancelOutcome, LatestPrompt), String> {
        let (catch_up, on_cancelled) = match catch_up {
            Some(CatchUpThrough {
                store,
                writer,
                on_cancelled,
                checkout,
            }) => {
                let catch_up = match self.cancel_catch_up(session_id, store, writer, checkout) {
                    Ok(catch_up) => Some(catch_up),
                    Err(e) => {
                        log::warn!(
                            "ACP session {agent_session_id}: cancelling without catching \
                             session {session_id} up: {e}"
                        );
                        None
                    }
                };
                (catch_up, Some(on_cancelled))
            }
            None => (None, None),
        };
        let on_cancelled = std::cell::RefCell::new(on_cancelled);
        let catch_up = std::cell::RefCell::new(catch_up);
        let AttachProxy {
            mut child,
            transport,
            streams,
        } = self.spawn_attach_proxy(session_id, command_id, working_dir)?;
        let (writer, handler) = match &*catch_up.borrow() {
            Some(catch_up) => (
                Arc::clone(&catch_up.writer),
                AcpNotificationHandler::new(
                    Arc::clone(&catch_up.writer),
                    false,
                    Vec::new(),
                    CancellationToken::new(),
                )
                .catching_up(&catch_up.stored),
            ),
            // Nothing is loaded, so nothing is recorded.
            None => {
                let writer: Arc<dyn MessageWriter> = Arc::new(BasicMessageWriter::new());
                let handler = AcpNotificationHandler::new(
                    Arc::clone(&writer),
                    false,
                    Vec::new(),
                    CancellationToken::new(),
                );
                (writer, handler)
            }
        };
        let handler = Arc::new(handler.with_transport_monitor(Arc::clone(&transport)));
        let sent = std::cell::Cell::new(false);
        let initialized = std::cell::RefCell::new(None);
        let protocol_result = staged_client(&handler)
            .connect_with(streams, async |connection| {
                let cancel = async {
                    *initialized.borrow_mut() = Some(
                        initialize_connection(
                            &connection,
                            &handler,
                            session_id,
                            &self.agent_label,
                            &[],
                        )
                        .await?,
                    );
                    // A catch-up's frames stay held until its load: the
                    // cancel's own frames would land ahead of the replay.
                    handler.name_session_holding(agent_session_id).await;
                    if transport.process_replaced() {
                        return Ok(CancelOutcome::ProcessGone {
                            message: ended_message(self.blox_process.as_ref(), command_id).await,
                        });
                    }
                    send_session_cancel(&connection, agent_session_id)?;
                    sent.set(true);
                    log::info!(
                        "ACP session {agent_session_id}: sent session/cancel from an attach \
                         for session {session_id}"
                    );
                    Ok::<_, String>(
                        match wait_for_turn_end(
                            &handler,
                            self.blox_process.as_ref(),
                            command_id,
                            timing,
                        )
                        .await
                        {
                            TurnEnd::Idle => CancelOutcome::Stopped,
                            TurnEnd::ProcessGone { message } => {
                                CancelOutcome::ProcessGone { message }
                            }
                            TurnEnd::TransportLost => CancelOutcome::TransportLost,
                        },
                    )
                };
                let outcome = match tokio::time::timeout(deadline, cancel).await {
                    Ok(Ok(outcome)) => outcome,
                    Ok(Err(e)) => return Err(agent_client_protocol::util::internal_error(e)),
                    Err(_) if sent.get() => CancelOutcome::Unconfirmed,
                    Err(_) => {
                        return Err(agent_client_protocol::util::internal_error(format!(
                            "Timed out after {}s before session/cancel could be sent",
                            deadline.as_secs()
                        )))
                    }
                };
                if let Some(on_cancelled) = on_cancelled.borrow_mut().take() {
                    on_cancelled(&outcome);
                }
                let initialized = initialized.borrow_mut().take();
                let catch_up = catch_up.borrow_mut().take();
                let mut latest_prompt = LatestPrompt::Unknown;
                if let (Some(mut catch_up), Some((init_response, mode))) = (catch_up, initialized) {
                    let over = matches!(
                        outcome,
                        CancelOutcome::Stopped | CancelOutcome::ProcessGone { .. }
                    );
                    if over && transport.first_signal().is_none() {
                        match catch_up
                            .checkout(working_dir, self.remote_working_dir.as_deref())
                            .await
                        {
                            // Unless the transport went while it resolved.
                            Ok(checkout) if transport.first_signal().is_none() => {
                                let context = AcpSessionSetupContext {
                                    connection: &connection,
                                    working_dir: &checkout,
                                    store: &catch_up.store,
                                    handler: &handler,
                                    our_session_id: session_id,
                                    acp_session_id: Some(agent_session_id),
                                    config_options: &[],
                                    // As `initialize` was checked for: the
                                    // connection only reads the transcript.
                                    mcp_servers: &[],
                                    agent_label: &self.agent_label,
                                    auth_selection: None,
                                    unattended_session_mode: self
                                        .unattended_session_mode
                                        .as_deref(),
                                };
                                latest_prompt = catch_up_after_cancel(
                                    &context,
                                    &init_response,
                                    mode,
                                    timing.cancel_catch_up,
                                )
                                .await;
                            }
                            Ok(_) => {}
                            Err(e) => log::warn!(
                                "ACP session {agent_session_id}: not catching session \
                                 {session_id} up after the cancel: {e}"
                            ),
                        }
                    }
                }
                Ok((outcome, latest_prompt))
            })
            .await
            .map_err(|e| format!("ACP protocol failed: {e:?}"));

        handler.cancel_pending_permissions();
        writer.finalize().await;
        stop_proxy(&mut child, &transport).await;
        match protocol_result {
            Ok(caught_up) => Ok(caught_up),
            Err(_) if transport.first_signal().is_some() => {
                Ok((CancelOutcome::TransportLost, LatestPrompt::Unknown))
            }
            Err(e) => Err(e),
        }
    }

    /// What [`AcpDriver::attach_cancel_and_catch_up`] needs before the proxy
    /// spawns: the transcript stored for `session_id`. The remote checkout
    /// to load the session in is resolved only for the load.
    fn cancel_catch_up<'a>(
        &self,
        session_id: &str,
        store: &Arc<dyn Store>,
        writer: &Arc<dyn MessageWriter>,
        checkout: Option<PendingCheckout<'a>>,
    ) -> Result<CancelCatchUp<'a>, String> {
        Ok(CancelCatchUp {
            // Without what is stored, the whole replay would be recorded
            // again.
            stored: store.get_session_replay_boundaries(session_id)?,
            pending_checkout: checkout,
            store: Arc::clone(store),
            writer: Arc::clone(writer),
        })
    }

    /// Spawn `sq blox acp` for an attach: this driver's argv, pinned to
    /// `command_id` when one is known, wired like [`AcpDriver::connect`]'s
    /// remote spawn (own process group, transport monitor, stderr markers).
    fn spawn_attach_proxy(
        &self,
        session_id: &str,
        command_id: Option<&str>,
        working_dir: &Path,
    ) -> Result<AttachProxy, String> {
        if !self.is_remote {
            return Err("Only a remote (Blox) session can be attached to".to_string());
        }
        let spawn_working_dir = resolve_spawn_working_dir(working_dir, true);
        let mut args = self.acp_args.clone();
        if let Some(id) = command_id {
            args.push(format!("--command-id={id}"));
        }
        let spawn_command = acp_spawn_command(
            &self.binary_path,
            &args,
            self.interpreter_env_snapshot.as_deref(),
        );
        let mut cmd = Command::new(&spawn_command.program);
        cmd.args(&spawn_command.args)
            .current_dir(&spawn_working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        for (k, v) in &self.extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|e| {
            format!(
                "Failed to spawn {} for an attach (binary: {}): {e}",
                self.agent_label,
                self.binary_path.display()
            )
        })?;

        let transport = TransportMonitor::new();
        #[cfg(unix)]
        if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
            transport.arm_proxy_group(pid);
        }
        if self.shares_remote_process() {
            transport.mark_shared_process();
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_stderr_reader(
                stderr,
                self.agent_label.clone(),
                session_id.to_string(),
                CancellationToken::new(),
                Arc::clone(&transport),
            );
        }
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to get stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to get stdout".to_string())?;
        let streams = ByteStreams::new(
            stdin.compat_write(),
            normalized_stdout(stdout, true, Arc::clone(&transport)).compat(),
        );
        Ok(AttachProxy {
            child,
            transport,
            streams,
        })
    }
}

/// An attach-cancel's catch-up (see [`AcpDriver::attach_cancel_and_catch_up`]).
struct CancelCatchUp<'a> {
    stored: Vec<ReplayBoundary>,
    /// The checkout the caller is still resolving, if it is.
    pending_checkout: Option<PendingCheckout<'a>>,
    store: Arc<dyn Store>,
    writer: Arc<dyn MessageWriter>,
}

impl CancelCatchUp<'_> {
    /// The checkout to load the session in: the caller's, once resolved,
    /// else the driver's `remote_working_dir`.
    async fn checkout(
        &mut self,
        working_dir: &Path,
        remote_working_dir: Option<&Path>,
    ) -> Result<PathBuf, String> {
        match self.pending_checkout.take() {
            Some(pending) => {
                let checkout = pending.await?;
                resolve_acp_working_dir(working_dir, true, Some(&checkout))
            }
            None => resolve_acp_working_dir(working_dir, true, remote_working_dir),
        }
    }
}

/// The `session/load` of an attach-cancel's catch-up, on the connection the
/// cancel went out on, bounded by `limit`, returning whether its replay
/// showed the agent receiving the newest stored prompt. A failure is logged,
/// and tells nothing about the prompt: the cancel stands either way.
async fn catch_up_after_cancel(
    context: &AcpSessionSetupContext<'_>,
    init_response: &InitializeResponse,
    task_tracking_mode: TaskTrackingMode,
    limit: Duration,
) -> LatestPrompt {
    let handler = context.handler;
    let agent_session_id = context.acp_session_id.unwrap_or_default();
    handler.begin_replay().await;
    let load = tokio::time::timeout(
        limit,
        load_acp_session(context, agent_session_id, init_response, task_tracking_mode),
    );
    let loaded = tokio::select! {
        loaded = load => loaded,
        // A proxy that lost its remote process keeps stdout open.
        _ = handler.transport.lost().cancelled() => {
            log::warn!(
                "ACP session {agent_session_id}: lost the transport catching session {} up \
                 after the cancel",
                context.our_session_id
            );
            return LatestPrompt::Unknown;
        }
    };
    match loaded {
        Ok(Ok(())) => {
            handler.finish_catch_up().await;
            let latest_prompt = handler.latest_prompt().await;
            log::info!(
                "ACP session {agent_session_id}: caught session {} up after the cancel \
                 (newest prompt: {latest_prompt:?})",
                context.our_session_id
            );
            return latest_prompt;
        }
        Ok(Err(e)) => log::warn!(
            "ACP session {agent_session_id}: catching session {} up after the cancel failed: {e}",
            context.our_session_id
        ),
        Err(_) => log::warn!(
            "ACP session {agent_session_id}: catching session {} up after the cancel timed out \
             after {}s",
            context.our_session_id,
            limit.as_secs()
        ),
    }
    LatestPrompt::Unknown
}

/// What an attach's connection task owns.
struct AttachContext {
    our_session_id: String,
    agent_session_id: String,
    command_id: Option<String>,
    working_dir: PathBuf,
    store: Arc<dyn Store>,
    mcp_servers: Vec<McpServer>,
    agent_label: String,
    auth_selection: Option<AcpAuthenticationSelection>,
    unattended_session_mode: Option<String>,
    blox: Option<BloxProcess>,
    timing: AttachTiming,
}

async fn run_attach(
    connection: &ConnectionTo<Agent>,
    handler: &Arc<AcpNotificationHandler>,
    context: &AttachContext,
    caught_up: watch::Sender<bool>,
) -> Result<AttachOutcome, String> {
    let transport = &handler.transport;
    // The session's `result` frames are this connection's to accept,
    // though no prompt of its own is in flight.
    handler
        .response
        .lock()
        .await
        .start_prompt(context.agent_session_id.clone());
    let setup = tokio::time::timeout(
        ACP_SETUP_TIMEOUT,
        setup_acp_session(AcpSessionSetupContext {
            connection,
            working_dir: &context.working_dir,
            store: &context.store,
            handler,
            our_session_id: &context.our_session_id,
            acp_session_id: Some(&context.agent_session_id),
            config_options: &[],
            mcp_servers: &context.mcp_servers,
            agent_label: &context.agent_label,
            auth_selection: context.auth_selection.as_ref(),
            unattended_session_mode: context.unattended_session_mode.as_deref(),
        }),
    );
    tokio::pin!(setup);
    let setup_result = tokio::select! {
        result = &mut setup => result,
        // A proxy that lost its remote process keeps stdout open; a reply
        // already on its way still wins.
        _ = transport.lost().cancelled() => {
            match tokio::time::timeout(TRANSPORT_LOSS_REPLY_GRACE, &mut setup).await {
                Ok(result) => result,
                Err(_) => return Ok(lost_outcome(transport)),
            }
        }
    };
    match setup_result {
        Ok(Ok(_)) => {}
        Ok(Err(e))
            if transport.first_signal().is_some() || formatted_error_is_transport_closed(&e) =>
        {
            log::info!(
                "ACP session {}: attach lost its transport during setup: {e}",
                context.agent_session_id
            );
            return Ok(lost_outcome(transport));
        }
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Err(format!(
                "Timed out attaching to the ACP session after {}s",
                ACP_SETUP_TIMEOUT.as_secs()
            ))
        }
    }
    handler.finish_catch_up().await;
    let latest_prompt = handler.latest_prompt().await;
    let _ = caught_up.send(true);
    log::info!(
        "ACP session {}: attach caught up for session {} (newest prompt: {latest_prompt:?})",
        context.agent_session_id,
        context.our_session_id
    );

    let command_id = context.command_id.as_deref();
    if transport.process_replaced() {
        return Ok(AttachOutcome::ProcessGone {
            message: ended_message(context.blox.as_ref(), command_id).await,
            latest_prompt,
        });
    }
    Ok(
        match wait_for_turn_end(handler, context.blox.as_ref(), command_id, context.timing).await {
            TurnEnd::Idle => AttachOutcome::Ended {
                result: handler.last_result().as_ref().and_then(result_turn),
                latest_prompt,
                background_work_live: background_work_live(
                    &handler.subscribe_background_activity().borrow(),
                ),
            },
            TurnEnd::ProcessGone { message } => AttachOutcome::ProcessGone {
                message,
                latest_prompt,
            },
            TurnEnd::TransportLost => lost_outcome(transport),
        },
    )
}

/// How a wait on the session's turn ended.
enum TurnEnd {
    Idle,
    ProcessGone { message: Option<String> },
    TransportLost,
}

/// Wait for the session's turn to end: its own `idle` frame, or the process
/// reading `IDLE` (then a short drain for the frames, where the agent sends
/// them), or gone.
async fn wait_for_turn_end(
    handler: &AcpNotificationHandler,
    blox: Option<&BloxProcess>,
    command_id: Option<&str>,
    timing: AttachTiming,
) -> TurnEnd {
    let transport = &handler.transport;
    let mut activity = handler.subscribe_background_activity();
    let mut statuses = blox.map(|blox| poll_process_status(blox, command_id, timing.poll));
    loop {
        if activity.borrow_and_update().session_state == Some(SdkSessionState::Idle) {
            return TurnEnd::Idle;
        }
        tokio::select! {
            _ = transport.lost().cancelled() => return TurnEnd::TransportLost,
            changed = activity.changed() => {
                if changed.is_err() {
                    return TurnEnd::TransportLost;
                }
            }
            Some(status) = async {
                match statuses.as_mut() {
                    Some(statuses) => statuses.recv().await,
                    None => std::future::pending().await,
                }
            } => match status {
                Ok(RemoteProcessStatus::Running) => {}
                Ok(RemoteProcessStatus::Idle) => {
                    if activity.borrow().sdk_frames_seen {
                        let _ = tokio::time::timeout(
                            timing.idle_drain,
                            activity.wait_for(|activity| {
                                activity.session_state == Some(SdkSessionState::Idle)
                            }),
                        )
                        .await;
                    }
                    return TurnEnd::Idle;
                }
                Ok(RemoteProcessStatus::Gone { message }) => {
                    return TurnEnd::ProcessGone { message };
                }
                Err(e) => log::warn!("Reading the remote ACP process status failed: {e}"),
            },
        }
    }
}

/// Read the process status now, then every `every`, until the receiver is
/// dropped.
fn poll_process_status(
    blox: &BloxProcess,
    command_id: Option<&str>,
    every: Duration,
) -> mpsc::Receiver<Result<RemoteProcessStatus, String>> {
    let (tx, rx) = mpsc::channel(1);
    let probe = Arc::clone(&blox.probe);
    let command_id = command_id.map(str::to_string);
    tokio::task::spawn_local(async move {
        loop {
            let status = tokio::select! {
                _ = tx.closed() => return,
                status = probe.status(command_id.as_deref()) => status,
            };
            if tx.send(status).await.is_err() {
                return;
            }
            tokio::select! {
                _ = tx.closed() => return,
                _ = tokio::time::sleep(every) => {}
            }
        }
    });
    rx
}

/// Why the session's replaced process ended, best effort.
async fn ended_message(blox: Option<&BloxProcess>, command_id: Option<&str>) -> Option<String> {
    match blox?.probe.ended_message(command_id).await {
        Ok(message) => message,
        Err(e) => {
            log::warn!("Reading why the remote ACP process ended failed: {e}");
            None
        }
    }
}

fn lost_outcome(transport: &TransportMonitor) -> AttachOutcome {
    if transport.detach_requested() {
        AttachOutcome::Live
    } else {
        AttachOutcome::TransportLost
    }
}

/// Stop an attach's proxy (see [`ProxyStop`]). A proxy that reused the
/// session's process leaves it running either way.
async fn stop_proxy(child: &mut tokio::process::Child, transport: &TransportMonitor) {
    match transport.begin_teardown() {
        ProxyStop::Graceful => graceful_stop(child, true).await,
        ProxyStop::KillOnly => kill_without_interrupt(child, true).await,
    }
}

/// How the turn a Claude Agent SDK `result` frame closes ended, when it says,
/// read as the Claude bridge reads it to answer the turn's prompt: an
/// interrupt is a cancel, a refusal and the token limit are their stop
/// reasons, an error result fails the turn (the bridge answers it with an
/// error), and otherwise the turn ended, or hit a turn or budget limit.
/// `error_during_execution` is an error unless it says it is not.
pub(super) fn result_turn(message: &serde_json::Value) -> Option<TurnResult> {
    let field = |name: &str| message.get(name).and_then(serde_json::Value::as_str);
    let subtype = field("subtype")?;
    if field("terminal_reason")
        .is_some_and(|reason| reason == "aborted_streaming" || reason == "aborted_tools")
    {
        return Some(TurnResult::Stopped(StopReason::Cancelled));
    }
    match field("stop_reason") {
        Some("refusal") => return Some(TurnResult::Stopped(StopReason::Refusal)),
        Some("max_tokens") => return Some(TurnResult::Stopped(StopReason::MaxTokens)),
        _ => {}
    }
    let is_error = message.get("is_error").and_then(serde_json::Value::as_bool);
    if is_error == Some(true) || (is_error.is_none() && subtype == "error_during_execution") {
        let text = if subtype == "success" {
            field("result").map(str::to_string)
        } else {
            let errors = message
                .get("errors")
                .and_then(serde_json::Value::as_array)
                .map(|errors| {
                    errors
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            Some(if errors.is_empty() {
                subtype.to_string()
            } else {
                errors
            })
        };
        return Some(TurnResult::Failed {
            message: text.filter(|text| !text.trim().is_empty()),
        });
    }
    match subtype {
        "success" | "error_during_execution" => Some(TurnResult::Stopped(StopReason::EndTurn)),
        "error_max_turns" | "error_max_budget_usd" | "error_max_structured_output_retries" => {
            Some(TurnResult::Stopped(StopReason::MaxTurnRequests))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_frames_map_to_how_the_turn_ended() {
        let turn = |frame: serde_json::Value| result_turn(&frame);
        let stopped = |reason| Some(TurnResult::Stopped(reason));
        assert_eq!(
            turn(serde_json::json!({
                "type": "result", "subtype": "success", "is_error": false,
                "stop_reason": "end_turn", "terminal_reason": "completed"
            })),
            stopped(StopReason::EndTurn)
        );
        // Observed on a live cancel.
        assert_eq!(
            turn(serde_json::json!({
                "type": "result", "subtype": "error_during_execution", "is_error": true,
                "stop_reason": "tool_use", "terminal_reason": "aborted_tools"
            })),
            stopped(StopReason::Cancelled)
        );
        assert_eq!(
            turn(serde_json::json!({"subtype": "success", "stop_reason": "max_tokens"})),
            stopped(StopReason::MaxTokens)
        );
        // A refusal can come on any subtype, error or not.
        assert_eq!(
            turn(serde_json::json!({
                "subtype": "error_during_execution", "is_error": true, "stop_reason": "refusal"
            })),
            stopped(StopReason::Refusal)
        );
        for subtype in [
            "error_max_turns",
            "error_max_budget_usd",
            "error_max_structured_output_retries",
        ] {
            assert_eq!(
                turn(serde_json::json!({"subtype": subtype, "is_error": false})),
                stopped(StopReason::MaxTurnRequests),
                "{subtype}"
            );
        }
        assert_eq!(turn(serde_json::json!({})), None);
        assert_eq!(turn(serde_json::json!({"subtype": "something_new"})), None);
    }

    /// The live path fails a turn the bridge answers with an error; an attach
    /// must settle the same turn the same way.
    #[test]
    fn error_results_fail_the_turn() {
        let turn = |frame: serde_json::Value| result_turn(&frame);
        let failed = |message: &str| {
            Some(TurnResult::Failed {
                message: Some(message.to_string()),
            })
        };
        assert_eq!(
            turn(serde_json::json!({
                "subtype": "error_during_execution", "is_error": true,
                "terminal_reason": "completed", "errors": ["API Error: 529", "overloaded"]
            })),
            failed("API Error: 529, overloaded")
        );
        assert_eq!(
            turn(serde_json::json!({"subtype": "error_during_execution"})),
            failed("error_during_execution"),
            "an execution error that doesn't say otherwise is an error"
        );
        assert_eq!(
            turn(serde_json::json!({"subtype": "error_during_execution", "is_error": false})),
            Some(TurnResult::Stopped(StopReason::EndTurn)),
            "as the bridge reads it"
        );
        assert_eq!(
            turn(serde_json::json!({
                "subtype": "success", "is_error": true, "result": "Credit balance is too low"
            })),
            failed("Credit balance is too low")
        );
        assert_eq!(
            turn(serde_json::json!({"subtype": "error_max_turns", "is_error": true, "errors": []})),
            failed("error_max_turns")
        );
        assert_eq!(
            turn(serde_json::json!({"subtype": "success", "is_error": true, "result": " "})),
            Some(TurnResult::Failed { message: None })
        );
    }
}
