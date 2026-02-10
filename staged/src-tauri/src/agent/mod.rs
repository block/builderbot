//! Agent abstraction layer.
//!
//! Separates *what* the session orchestrator needs from an agent (the
//! [`AgentDriver`] trait) from *how* a specific protocol implements it
//! (e.g. [`acp::AcpDriver`]).
//!
//! To swap the underlying agent protocol:
//! 1. Create a new module implementing [`AgentDriver`].
//! 2. Change the one `AcpDriver::new()` call in `session_runner.rs`.
//! 3. Nothing else changes — the DB writer, registry, lifecycle, and
//!    frontend are all protocol-agnostic.

pub mod acp;
pub mod writer;

use std::path::Path;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::store::Store;
use writer::MessageWriter;

/// Everything the session orchestrator needs to run one turn of an agent.
///
/// Implementors own the protocol details (spawning a process, connecting,
/// sending the prompt, translating streaming events into [`MessageWriter`]
/// calls). The orchestrator in `session_runner` handles cancellation,
/// status transitions, and event emission.
///
/// # Contract
///
/// - The driver **must** check `cancel_token` and exit promptly when it
///   fires (e.g. via `tokio::select!`).
/// - The driver **must** call `writer.finalize()` before returning — on
///   success, error, *and* cancellation — to flush any buffered text.
/// - `agent_session_id` is `Some` on follow-up turns; the driver should
///   restore conversation history however the protocol supports it.
///
/// The returned future is `!Send` — the orchestrator runs it on a
/// dedicated thread with a `LocalSet`.
#[allow(clippy::too_many_arguments)]
pub trait AgentDriver {
    /// Run a single turn: send `prompt`, stream results via `writer`.
    fn run(
        &self,
        session_id: &str,
        prompt: &str,
        working_dir: &Path,
        store: &Arc<Store>,
        writer: &Arc<MessageWriter>,
        cancel_token: &CancellationToken,
        agent_session_id: Option<&str>,
    ) -> impl std::future::Future<Output = Result<(), String>>;
}
