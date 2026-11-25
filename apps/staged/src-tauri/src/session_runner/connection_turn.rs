//! One connection of a run's turn loop: open it, send the turn's prompt, and
//! wait for the connection to settle.
//!
//! Without a background hold the connection tears the bridge process down as
//! soon as the prompt resolves (so a connection serves exactly one turn);
//! with one, the prompt resolves at hold entry while the bridge keeps serving
//! background work. Either way this returns only once the connection has
//! settled, so Pikchr validation, post-completion hooks and the terminal
//! status all happen on actual teardown, not on the prompt resolving.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use acp_client::{
    AcpSessionConfigOptionSelection, AgentRunOutcome, PromptImage, SessionConnection,
    SessionSettleReason, SessionSettled, SetupFailure,
};
use tokio_util::sync::CancellationToken;

use crate::agent::AcpDriver;

/// What one connection is opened with and sent.
pub(super) struct ConnectionTurnRequest<'a> {
    pub(super) session_id: &'a str,
    pub(super) working_dir: &'a Path,
    pub(super) store: &'a Arc<dyn acp_client::Store>,
    pub(super) writer: &'a Arc<dyn acp_client::MessageWriter>,
    /// The token the connection observes.
    pub(super) cancel: &'a CancellationToken,
    /// The agent session to load; `None` starts one with `session/new`.
    pub(super) agent_session_id: Option<&'a str>,
    pub(super) config_options: &'a [AcpSessionConfigOptionSelection],
    pub(super) prompt: &'a str,
    pub(super) images: &'a [PromptImage],
}

/// How one connection's turn ended.
pub(super) struct ConnectionTurn {
    /// The turn's result, folded with the settled outcome.
    pub(super) result: Result<AgentRunOutcome, String>,
    /// How the connection settled. `None` when it never opened (the agent
    /// did not spawn) or its task died without settling.
    pub(super) settled: Option<SessionSettled>,
}

impl ConnectionTurn {
    /// Why the connection's setup failed, when starting over can fix it (see
    /// `startup_restart`). Read off the settle, never the error text, which
    /// reaches here wrapped by the SDK.
    pub(super) fn setup_failure(&self) -> Option<SetupFailure> {
        self.settled
            .as_ref()
            .and_then(|settled| settled.setup_failure)
    }
}

/// Open a connection on `driver`, hand it to `on_connected` (to expose its
/// handles), send the prompt, and wait for the connection to settle.
///
/// The prompt is raced against `interrupt` when there is one: if it
/// resolves first, `on_interrupt` gets its output and is expected to cancel
/// the connection's token, and the prompt is then awaited to its end. The
/// runner's inactivity watchdog uses this: a finished note behind a silent
/// connection cancels the turn on this side (the agent, if it is still
/// there, gets a `session/cancel`; a hung proxy never answers and the driver
/// gives up after its ack deadline).
pub(super) async fn run_connection_turn<Interrupt, Found>(
    driver: &AcpDriver,
    request: ConnectionTurnRequest<'_>,
    on_connected: impl FnOnce(&SessionConnection),
    interrupt: Option<Interrupt>,
    on_interrupt: impl FnOnce(Found),
) -> ConnectionTurn
where
    Interrupt: Future<Output = Found>,
{
    let ConnectionTurnRequest {
        session_id,
        working_dir,
        store,
        writer,
        cancel,
        agent_session_id,
        config_options,
        prompt,
        images,
    } = request;
    let mut connection = match driver
        .connect(
            session_id,
            working_dir,
            store,
            writer,
            cancel,
            agent_session_id,
            config_options,
            None,
        )
        .await
    {
        Ok(connection) => connection,
        Err(error) => {
            return ConnectionTurn {
                result: Err(error),
                settled: None,
            }
        }
    };
    on_connected(&connection);
    let settled_rx = connection.take_settled_receiver();
    let prompt_future = connection.prompt(prompt, images);
    tokio::pin!(prompt_future);
    let turn_result = match interrupt {
        Some(interrupt) => tokio::select! {
            biased;
            result = &mut prompt_future => result,
            found = interrupt => {
                on_interrupt(found);
                prompt_future.await
            }
        },
        None => prompt_future.await,
    };
    let Some(rx) = settled_rx else {
        return ConnectionTurn {
            result: turn_result,
            settled: None,
        };
    };
    match rx.await {
        Ok(settled) => {
            match settled.reason {
                SessionSettleReason::HeldUntilCap => log::warn!(
                    "Session {session_id}: background hold cap expired before quiescence; \
                     post-completion hooks run best-effort"
                ),
                SessionSettleReason::HoldStopped => log::info!(
                    "Session {session_id}: background hold stopped before quiescence; the \
                     completed turn stands and post-completion hooks run best-effort"
                ),
                _ => {}
            }
            ConnectionTurn {
                result: settled.fold_turn_result(turn_result),
                settled: Some(settled),
            }
        }
        // The connection task died without settling; the turn result is the
        // best information available.
        Err(_) => ConnectionTurn {
            result: turn_result,
            settled: None,
        },
    }
}
