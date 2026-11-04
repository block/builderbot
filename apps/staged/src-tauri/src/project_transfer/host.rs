//! What a transfer needs beyond its two endpoints: where things live on this
//! machine, and the Blox workspace lifecycle.
//!
//! [`LiveHost`] is the real thing. The orchestrator only reaches the outside
//! world through [`TransferHost`], so its tests stand in temp directories for
//! the clones and worktrees and a directory per "workspace".
//!
//! Every method blocks: run them from the blocking pool.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::blox;

use super::endpoint::{BloxEndpoint, LocalEndpoint, TransferEndpoint};
use super::TransferError;

/// A Blox workspace's state, as far as a transfer cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceState {
    Running,
    Suspended,
    /// Provisioning, or a state this code does not recognise.
    Starting,
    Stopped,
    Error,
    /// Never created, or deleted.
    NotFound,
}

/// Why Blox cannot be used right now: a preflight blocker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BloxUnavailable {
    pub code: &'static str,
    pub message: String,
}

pub trait TransferHost: Send + Sync {
    /// This machine's end, with scripts seeing `env_dir`'s environment and
    /// transfer files under [`Self::transfers_dir`].
    fn local_endpoint(&self, env_dir: &Path) -> Result<Box<dyn TransferEndpoint>, String>;

    /// `~/.staged/transfers`: one directory per transfer, kept as the backup.
    fn transfers_dir(&self) -> Result<PathBuf, String>;

    /// Where the shared local clone of `github_repo` lives, whether or not it
    /// exists yet.
    fn local_clone_path(&self, github_repo: &str) -> Result<PathBuf, String>;

    /// The shared local clone of `github_repo`, cloned if missing.
    fn ensure_local_clone(&self, github_repo: &str) -> Result<PathBuf, String>;

    /// Where a project's worktree for `branch` goes.
    fn local_worktree_path(
        &self,
        project_id: &str,
        github_repo: &str,
        branch: &str,
    ) -> Result<PathBuf, String>;

    /// The `working_dir` a remote branch's sessions record: a placeholder
    /// shared by every branch on the repo.
    fn remote_session_working_dir(&self, github_repo: &str) -> String;

    /// Whether `sq` is installed and logged in.
    fn blox_ready(&self) -> Result<(), BloxUnavailable>;

    fn workspace_state(&self, name: &str) -> Result<WorkspaceState, String>;

    /// Create a workspace from `github_repo` at `base`, and wait until it is
    /// running and bootstrapped. `progress` gets a line per state change.
    /// The wait stops early, with [`TransferError::Cancelled`]'s message,
    /// once `cancelled` says so.
    fn start_workspace(
        &self,
        name: &str,
        github_repo: &str,
        base: &str,
        progress: &dyn Fn(&str),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String>;

    /// Resume a suspended workspace and wait until it is running, or until
    /// `cancelled` says to stop waiting.
    fn resume_workspace(&self, name: &str, cancelled: &dyn Fn() -> bool) -> Result<(), String>;

    /// Delete a workspace; one that is already gone counts as deleted.
    fn delete_workspace(&self, name: &str) -> Result<(), String>;

    fn workspace_endpoint(&self, name: &str) -> Box<dyn TransferEndpoint>;

    /// The clone of `github_repo` in a workspace: `~/<repo name>`.
    fn workspace_repo_dir(&self, name: &str, github_repo: &str) -> Result<String, String>;

    /// What a workspace clones `github_repo` from.
    fn workspace_clone_url(&self, github_repo: &str) -> String;

    /// Apply Staged's git config to a fresh workspace clone. Best-effort.
    fn configure_workspace_clone(&self, name: &str, repo_dir: &str);
}

/// [`TransferHost`] over `~/.staged`, git and `sq blox`.
pub struct LiveHost;

/// How long a new or resumed workspace may take to come up and bootstrap.
const WORKSPACE_READY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const WORKSPACE_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// How often a wait between polls looks for a cancel.
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(200);

fn is_not_found(e: &blox::BloxError) -> bool {
    matches!(e, blox::BloxError::CommandFailed(msg) if msg.to_lowercase().contains("not found"))
}

/// Blox's raw status, read the way a transfer needs it. Unlike
/// [`crate::branches::map_blox_status_to_workspace_status`] there is no
/// database status to lean on, and `deleted` means there is nothing there.
fn workspace_state_from_status(status: Option<&str>) -> WorkspaceState {
    match status.map(str::to_ascii_lowercase).as_deref() {
        Some("running" | "ready" | "active") => WorkspaceState::Running,
        Some("suspended") => WorkspaceState::Suspended,
        Some("stopped" | "shutting_down") => WorkspaceState::Stopped,
        Some("deleted") => WorkspaceState::NotFound,
        Some("error" | "failed" | "degraded") => WorkspaceState::Error,
        _ => WorkspaceState::Starting,
    }
}

/// Which bootstrap step a workspace is on: `(step, total)` while any of its
/// bootstrap commands has yet to finish, `None` once all have (or none are
/// listed). A failed command counts as finished; the workspace's own state
/// says whether it came up.
fn bootstrap_step(commands: &[blox::WorkspaceCommand]) -> Option<(usize, usize)> {
    let bootstrap: Vec<_> = commands.iter().filter(|c| c.is_bootstrap).collect();
    let finished = bootstrap.iter().filter(|c| c.status.is_finished()).count();
    (finished < bootstrap.len()).then(|| (finished + 1, bootstrap.len()))
}

impl LiveHost {
    /// Poll until the workspace is running and its bootstrap commands have
    /// all finished, or `cancelled` says to stop.
    fn wait_until_ready(
        &self,
        name: &str,
        progress: &dyn Fn(&str),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let deadline = Instant::now() + WORKSPACE_READY_TIMEOUT;
        let mut last = String::new();
        let mut report = |line: String| {
            if line != last {
                progress(&line);
                last = line;
            }
        };
        loop {
            if cancelled() {
                return Err(TransferError::Cancelled.to_string());
            }
            match blox::ws_info(name) {
                Ok(info) => {
                    if let Some(id) = info.workstation_id {
                        if let Ok(mut cache) = crate::branches::workstation_id_cache().lock() {
                            cache.insert(name.to_string(), id);
                        }
                    }
                    let state = workspace_state_from_status(info.status.as_deref());
                    match state {
                        // Blox runs the bootstrap while the workspace is
                        // starting and reports it running as the bootstrap
                        // ends, give or take a second, so the commands are
                        // read in both states.
                        WorkspaceState::Running | WorkspaceState::Starting => {
                            match blox::ws_commands(name) {
                                Ok(commands) => match bootstrap_step(&commands) {
                                    Some((step, total)) => report(format!(
                                        "Bootstrapping the workspace (step {step} of {total})"
                                    )),
                                    None if state == WorkspaceState::Running => {
                                        if commands.iter().any(|c| {
                                            c.is_bootstrap
                                                && c.status == blox::WorkspaceCommandStatus::Failed
                                        }) {
                                            log::warn!(
                                                "[project_transfer] {name} is running, but a bootstrap command failed"
                                            );
                                        }
                                        return Ok(());
                                    }
                                    None => {
                                        report("Waiting for the workspace to start".to_string())
                                    }
                                },
                                // Running is what the rest of the app waits
                                // for; an unreadable command list does not
                                // hold it up.
                                Err(e) if state == WorkspaceState::Running => {
                                    log::warn!("[project_transfer] ws commands {name}: {e}");
                                    return Ok(());
                                }
                                Err(e) => {
                                    log::debug!("[project_transfer] ws commands {name}: {e}");
                                    report("Waiting for the workspace to start".to_string())
                                }
                            }
                        }
                        WorkspaceState::Error => {
                            return Err(format!("Workspace {name} failed to start"))
                        }
                        _ => report("Waiting for the workspace to start".to_string()),
                    }
                }
                Err(blox::BloxError::NotAuthenticated) => {
                    return Err("Not authenticated with Blox. Run: sq login".to_string())
                }
                Err(e) if is_not_found(&e) => {
                    report("Waiting for the workspace to appear".to_string())
                }
                Err(e) => log::warn!("[project_transfer] ws info {name}: {e}"),
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "Workspace {name} did not become ready within {} minutes",
                    WORKSPACE_READY_TIMEOUT.as_secs() / 60
                ));
            }
            let next_poll = Instant::now() + WORKSPACE_POLL_INTERVAL;
            while Instant::now() < next_poll {
                if cancelled() {
                    return Err(TransferError::Cancelled.to_string());
                }
                std::thread::sleep(CANCEL_POLL_INTERVAL);
            }
        }
    }
}

impl TransferHost for LiveHost {
    fn local_endpoint(&self, env_dir: &Path) -> Result<Box<dyn TransferEndpoint>, String> {
        LocalEndpoint::in_transfers_dir(env_dir)
            .map(|endpoint| Box::new(endpoint) as Box<dyn TransferEndpoint>)
            .map_err(|e| e.to_string())
    }

    fn transfers_dir(&self) -> Result<PathBuf, String> {
        crate::paths::transfers_dir().ok_or_else(|| "Cannot resolve the home directory".into())
    }

    fn local_clone_path(&self, github_repo: &str) -> Result<PathBuf, String> {
        crate::paths::clone_path_for(github_repo)
            .ok_or_else(|| "Cannot resolve the home directory".into())
    }

    fn ensure_local_clone(&self, github_repo: &str) -> Result<PathBuf, String> {
        crate::git::ensure_local_clone(github_repo).map_err(|e| e.to_string())
    }

    fn local_worktree_path(
        &self,
        project_id: &str,
        github_repo: &str,
        branch: &str,
    ) -> Result<PathBuf, String> {
        crate::git::project_worktree_path_for(project_id, github_repo, branch)
            .map_err(|e| e.to_string())
    }

    fn remote_session_working_dir(&self, github_repo: &str) -> String {
        crate::paths::repos_dir()
            .map(|d| d.join(github_repo))
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .to_string_lossy()
            .into_owned()
    }

    fn blox_ready(&self) -> Result<(), BloxUnavailable> {
        if !blox::is_sq_available() {
            return Err(BloxUnavailable {
                code: "sq_unavailable",
                message: "The sq CLI is not installed, so Blox workspaces can't be reached."
                    .to_string(),
            });
        }
        blox::check_auth().map_err(|e| BloxUnavailable {
            code: "blox_not_authenticated",
            message: match e {
                blox::BloxError::NotAuthenticated => {
                    "Not authenticated with Blox. Run: sq login".to_string()
                }
                e => format!("Blox is unavailable: {e}"),
            },
        })
    }

    fn workspace_state(&self, name: &str) -> Result<WorkspaceState, String> {
        match blox::ws_info(name) {
            Ok(info) => Ok(workspace_state_from_status(info.status.as_deref())),
            Err(e) if is_not_found(&e) => Ok(WorkspaceState::NotFound),
            Err(e) => Err(e.to_string()),
        }
    }

    fn start_workspace(
        &self,
        name: &str,
        github_repo: &str,
        base: &str,
        progress: &dyn Fn(&str),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let source = format!("https://github.com/{github_repo}.git?ref={base}");
        progress("Creating the workspace");
        match blox::ws_start(
            name,
            Some(&source),
            Some(crate::branches::WORKSPACE_IDLE_TIMEOUT_MINUTES),
        ) {
            Ok(_) => {}
            Err(blox::BloxError::NotAuthenticated) => {
                return Err("Not authenticated with Blox. Run: sq login".to_string())
            }
            Err(e) if crate::branches::is_blox_onboarding_precondition_error(&e) => {
                return Err(e.to_string())
            }
            // `ws start` can time out while the workspace goes on booting, as
            // in `branches::start_workspace`; the poll below decides.
            Err(e) => log::warn!("[project_transfer] ws start {name}: {e}; polling for it"),
        }
        self.wait_until_ready(name, progress, cancelled)
    }

    fn resume_workspace(&self, name: &str, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
        blox::ws_resume(name).map_err(|e| e.to_string())?;
        self.wait_until_ready(
            name,
            &|line| log::info!("[project_transfer] resuming {name}: {line}"),
            cancelled,
        )
    }

    fn delete_workspace(&self, name: &str) -> Result<(), String> {
        match blox::ws_delete(name) {
            Ok(()) => Ok(()),
            Err(e) if is_not_found(&e) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn workspace_endpoint(&self, name: &str) -> Box<dyn TransferEndpoint> {
        Box::new(BloxEndpoint::new(name))
    }

    fn workspace_repo_dir(&self, name: &str, github_repo: &str) -> Result<String, String> {
        let dir = crate::branches::repo_name_from_github_repo(github_repo);
        crate::branches::resolve_workspace_repo_path(name, &format!("home:{dir}"))
            .map_err(|e| e.to_string())
    }

    fn workspace_clone_url(&self, github_repo: &str) -> String {
        format!("https://github.com/{github_repo}.git")
    }

    fn configure_workspace_clone(&self, name: &str, repo_dir: &str) {
        tauri::async_runtime::block_on(crate::git::config_apply::apply_to_blox_clone(
            name, repo_dir,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_blox_statuses_without_a_database_status() {
        assert_eq!(
            workspace_state_from_status(Some("RUNNING")),
            WorkspaceState::Running
        );
        assert_eq!(
            workspace_state_from_status(Some("suspended")),
            WorkspaceState::Suspended
        );
        assert_eq!(
            workspace_state_from_status(Some("deleted")),
            WorkspaceState::NotFound
        );
        assert_eq!(
            workspace_state_from_status(Some("shutting_down")),
            WorkspaceState::Stopped
        );
        assert_eq!(
            workspace_state_from_status(Some("degraded")),
            WorkspaceState::Error
        );
        assert_eq!(
            workspace_state_from_status(Some("provisioning")),
            WorkspaceState::Starting
        );
        assert_eq!(workspace_state_from_status(None), WorkspaceState::Starting);
    }

    fn commands(json: &str) -> Vec<blox::WorkspaceCommand> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn counts_bootstrap_steps_from_the_command_list() {
        // As `sq blox ws commands --json` prints it now: newest first, with
        // `isBootstrap` left out unless it is true.
        let starting = r#"[
  {"commandId": "3", "commandType": "COMMAND_TYPE_EXECUTE_PROCESS", "status": "COMMAND_STATUS_RUNNING"},
  {"commandId": "2", "commandType": "COMMAND_TYPE_EXECUTE_BOOTSTRAP", "status": "COMMAND_STATUS_PENDING", "isBootstrap": true},
  {"commandId": "1", "commandType": "COMMAND_TYPE_EXECUTE_BOOTSTRAP", "status": "COMMAND_STATUS_SUCCEEDED", "isBootstrap": true}
]"#;
        assert_eq!(bootstrap_step(&commands(starting)), Some((2, 2)));
        assert_eq!(
            bootstrap_step(&commands(&starting.replace("PENDING", "RUNNING"))),
            Some((2, 2))
        );
        // A process still running is not part of the bootstrap.
        assert_eq!(
            bootstrap_step(&commands(&starting.replace("PENDING", "SUCCEEDED"))),
            None
        );
        assert_eq!(
            bootstrap_step(&commands(&starting.replace("PENDING", "FAILED"))),
            None
        );
        // The older form still counts.
        assert_eq!(
            bootstrap_step(&commands(
                r#"[{"command_id": "1", "command_type": 3, "status": 1, "is_bootstrap": true, "dependencies": []}]"#
            )),
            Some((1, 1))
        );
        // A workspace past its 50 newest commands no longer lists a bootstrap.
        assert_eq!(
            bootstrap_step(&commands(
                r#"[{"commandId": "9", "commandType": "COMMAND_TYPE_EXECUTE_PROCESS", "status": "COMMAND_STATUS_SUCCEEDED"}]"#
            )),
            None
        );
    }
}
