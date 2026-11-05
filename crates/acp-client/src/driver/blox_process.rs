//! The workstation process behind a Blox session, and reading its status.
//!
//! The process status is the one turn signal every agent has: Blox marks an
//! ACP process `RUNNING` while any `session/prompt` on it is unanswered and
//! `IDLE` once all are. On a process dedicated to one Staged session that is
//! that session's turn state; on a shared one it is process-wide, so `IDLE`
//! still means this session's turn is over but `RUNNING` may be another
//! session's.
use super::*;

use crate::types::BloxAcpCommand;

/// One reading of a Blox session's remote ACP process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteProcessStatus {
    /// A `session/prompt` is unanswered on the process: a turn in flight, or
    /// one blocked on a permission request.
    Running,
    /// Every prompt on the process has been answered.
    Idle,
    /// The process is no longer running. `message` is what Blox recorded when
    /// it ended, when there is a record (e.g. "bloxlet restarted while
    /// command was running").
    Gone { message: Option<String> },
}

/// Reads a session's remote process status; `sq blox ws commands` outside
/// tests.
#[async_trait]
pub(crate) trait RemoteProcessProbe: Send + Sync {
    /// The status of the session's process: the one pinned by `command_id`
    /// when given, otherwise the newest active process running its argv.
    async fn status(&self, command_id: Option<&str>) -> Result<RemoteProcessStatus, String>;

    /// Why the session's newest *ended* process ended, for when `blox acp`
    /// reported replacing it. `None` when no ended record is listed, or it
    /// carries no message.
    async fn ended_message(&self, command_id: Option<&str>) -> Result<Option<String>, String>;
}

/// The Blox side of a remote driver: which workstation process serves the
/// Staged session, and how to read its status.
#[derive(Clone)]
pub(crate) struct BloxProcess {
    pub(crate) command: BloxAcpCommand,
    pub(crate) probe: Arc<dyn RemoteProcessProbe>,
}

impl BloxProcess {
    pub(crate) fn new(workspace_name: &str, command: BloxAcpCommand) -> Self {
        let probe = Arc::new(BloxCommandsProbe {
            workspace_name: workspace_name.to_string(),
            argv: command.argv.clone(),
        });
        Self { command, probe }
    }
}

/// [`RemoteProcessProbe`] over `sq blox ws commands --json`. Each read is one
/// or two CLI calls (about 1.5 s each), run off the async thread.
struct BloxCommandsProbe {
    workspace_name: String,
    argv: Vec<String>,
}

/// One blocking read over a workstation's listing: the workspace, the pinned
/// command id, and the session's argv.
type ListingRead<T> = fn(&str, Option<&str>, &[String]) -> Result<T, blox_cli::BloxError>;

impl BloxCommandsProbe {
    async fn listing<T: Send + 'static>(
        &self,
        command_id: Option<&str>,
        read: ListingRead<T>,
    ) -> Result<T, String> {
        let workspace_name = self.workspace_name.clone();
        let command_id = command_id.map(str::to_string);
        let argv = self.argv.clone();
        tokio::task::spawn_blocking(move || read(&workspace_name, command_id.as_deref(), &argv))
            .await
            .map_err(|e| format!("Blox process status read failed: {e}"))?
            .map_err(|e| e.to_string())
    }
}

#[async_trait]
impl RemoteProcessProbe for BloxCommandsProbe {
    async fn status(&self, command_id: Option<&str>) -> Result<RemoteProcessStatus, String> {
        self.listing(command_id, read_process_status).await
    }

    async fn ended_message(&self, command_id: Option<&str>) -> Result<Option<String>, String> {
        self.listing(command_id, |workspace_name, command_id, argv| {
            let all = blox_cli::ws_command_list(workspace_name, blox_cli::CommandFilter::All)?;
            Ok(blox_cli::find_ended_acp_process(&all, command_id, argv)
                .and_then(|command| command.message.clone()))
        })
        .await
    }
}

/// The active listing first (cheap, and what reuse is decided by); the full
/// one only to say why a process that is not active ended.
fn read_process_status(
    workspace_name: &str,
    command_id: Option<&str>,
    argv: &[String],
) -> Result<RemoteProcessStatus, blox_cli::BloxError> {
    let active = blox_cli::ws_command_list(workspace_name, blox_cli::CommandFilter::Active)?;
    if let Some(command) = blox_cli::find_acp_process(&active, command_id, argv) {
        return Ok(match command.status {
            blox_cli::CommandStatus::Idle => RemoteProcessStatus::Idle,
            _ => RemoteProcessStatus::Running,
        });
    }
    let all = blox_cli::ws_command_list(workspace_name, blox_cli::CommandFilter::All)?;
    let message = blox_cli::find_acp_process(&all, command_id, argv)
        .filter(|command| !command.status.is_active())
        .and_then(|command| command.message.clone());
    Ok(RemoteProcessStatus::Gone { message })
}

impl AcpDriver {
    /// Read this Blox session's remote process status: the process pinned by
    /// `command_id` when one was recorded, otherwise the one running this
    /// session's argv. `None` for a local driver.
    pub async fn remote_process_status(
        &self,
        command_id: Option<&str>,
    ) -> Option<Result<RemoteProcessStatus, String>> {
        let blox = self.blox_process.as_ref()?;
        Some(blox.probe.status(command_id).await)
    }
}
