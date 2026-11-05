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

/// A Blox session's remote process as one listing found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteProcess {
    pub status: RemoteProcessStatus,
    /// The process's Blox command id: the active process's, or the ended
    /// record's when it is gone. `None` when the listing holds no record of
    /// it at all.
    pub command_id: Option<String>,
    /// For an [`RemoteProcessStatus::Idle`] process, when it last changed
    /// state, in ms since the epoch (the listing's `updatedAt`): for a
    /// session's own process, when its last prompt was answered — the end
    /// of its last turn. `None` for any other status, or when the listing
    /// gives no time.
    pub idle_since: Option<i64>,
}

/// The Blox side of a remote driver: which workstation process serves the
/// Staged session, and how to read its status.
#[derive(Clone)]
pub(crate) struct BloxProcess {
    pub(crate) workspace_name: String,
    pub(crate) command: BloxAcpCommand,
    pub(crate) probe: Arc<dyn RemoteProcessProbe>,
}

impl BloxProcess {
    pub(crate) fn new(workspace_name: &str, command: BloxAcpCommand) -> Self {
        let probe = Arc::new(BloxCommandsProbe {
            workspace_name: workspace_name.to_string(),
            argv: command.argv.clone(),
        });
        Self {
            workspace_name: workspace_name.to_string(),
            command,
            probe,
        }
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

fn read_process_status(
    workspace_name: &str,
    command_id: Option<&str>,
    argv: &[String],
) -> Result<RemoteProcessStatus, blox_cli::BloxError> {
    locate_process(workspace_name, command_id, argv).map(|process| process.status)
}

/// The active listing first (cheap, and what reuse is decided by); the full
/// one only to say why a process that is not active ended, and which record
/// that was.
fn locate_process(
    workspace_name: &str,
    command_id: Option<&str>,
    argv: &[String],
) -> Result<RemoteProcess, blox_cli::BloxError> {
    let active = blox_cli::ws_command_list(workspace_name, blox_cli::CommandFilter::Active)?;
    if let Some(command) = blox_cli::find_acp_process(&active, command_id, argv) {
        return Ok(active_process(command));
    }
    let all = blox_cli::ws_command_list(workspace_name, blox_cli::CommandFilter::All)?;
    let ended = blox_cli::find_acp_process(&all, command_id, argv)
        .filter(|command| !command.status.is_active());
    Ok(RemoteProcess {
        status: RemoteProcessStatus::Gone {
            message: ended.and_then(|command| command.message.clone()),
        },
        command_id: ended
            .map(|command| command.command_id.clone())
            .or_else(|| command_id.map(str::to_string)),
        idle_since: None,
    })
}

/// The [`RemoteProcess`] an active listing record describes.
fn active_process(command: &blox_cli::WorkstationCommand) -> RemoteProcess {
    let idle = command.status == blox_cli::CommandStatus::Idle;
    RemoteProcess {
        status: if idle {
            RemoteProcessStatus::Idle
        } else {
            RemoteProcessStatus::Running
        },
        command_id: Some(command.command_id.clone()),
        idle_since: command
            .updated_at
            .as_deref()
            .filter(|_| idle)
            .and_then(super::remote_liveness::parse_utc_timestamp)
            .map(|seconds| seconds * 1000),
    }
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

    /// Find this Blox session's remote process, as
    /// [`AcpDriver::remote_process_status`] does, and also say which Blox
    /// command it is, so a caller can pin that process for later reads and
    /// attaches. Blocking (one or two `sq` calls); `None` for a local driver.
    ///
    /// Pinning is what keeps a gone process gone: matched by argv alone, the
    /// replacement a later attach starts in its place (idle, having run no
    /// prompt) would read as a turn that ended.
    pub fn locate_remote_process(
        &self,
        command_id: Option<&str>,
    ) -> Option<Result<RemoteProcess, String>> {
        let blox = self.blox_process.as_ref()?;
        Some(
            locate_process(&blox.workspace_name, command_id, &blox.command.argv)
                .map_err(|e| e.to_string()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(status: &str, updated_at: Option<&str>) -> blox_cli::WorkstationCommand {
        serde_json::from_value(serde_json::json!({
            "commandId": "27294439",
            "status": status,
            "updatedAt": updated_at,
        }))
        .unwrap()
    }

    /// An idle process says since when, in ms: its last turn's end.
    #[test]
    fn an_idle_process_carries_when_it_went_idle() {
        let process = active_process(&listed(
            "COMMAND_STATUS_IDLE",
            Some("2026-11-05T01:07:33.250Z"),
        ));
        assert_eq!(process.status, RemoteProcessStatus::Idle);
        assert_eq!(process.command_id.as_deref(), Some("27294439"));
        assert_eq!(process.idle_since, Some(1_793_840_853_000));
    }

    #[test]
    fn a_busy_or_undated_process_has_no_idle_time() {
        let running = active_process(&listed(
            "COMMAND_STATUS_RUNNING",
            Some("2026-11-05T01:07:33Z"),
        ));
        assert_eq!(running.status, RemoteProcessStatus::Running);
        assert_eq!(running.idle_since, None);
        assert_eq!(
            active_process(&listed("COMMAND_STATUS_IDLE", None)).idle_since,
            None
        );
    }
}
