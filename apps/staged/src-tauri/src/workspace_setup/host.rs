//! What setup does outside the database, behind a trait so tests drive the
//! routine without `sq`: Blox's word on the workstation, the checkout in its
//! clone, and the app-side effects of a run (status events, failure reasons,
//! retry timers, a nudge to the reconcile service).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::blox::{self, BloxError};
use crate::branches;
use crate::store::{Store, WorkspaceStatus};

use super::checkout::{BranchCheckout, CheckoutError};
use super::failures;
use super::guard::SetupGuard;
use super::retry::{self, RetryPolicy};
use super::verify::{self, CloneHead, HeadRead};

/// How long setup waits for a workstation to report running after `sq blox
/// ws start` failed without saying why the workstation can't exist.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReadyWait {
    pub(crate) timeout: Duration,
    pub(crate) interval: Duration,
}

impl ReadyWait {
    /// `ws start` itself gives up after 120 s while the workstation keeps
    /// booting. Five more minutes covers a slow bootstrap; past that the
    /// branch fails visibly rather than waiting on a workstation that may
    /// never come. It is then retried on a timer ([`retry`]), so one that
    /// comes up later is still set up.
    pub(crate) const DEFAULT: ReadyWait = ReadyWait {
        timeout: Duration::from_secs(300),
        interval: Duration::from_secs(5),
    };

    /// How long a checkout waits for a `disconnected` session in its clone
    /// to be settled by the reconcile service before it treats the session
    /// as still working there ([`settle_disconnected_sessions`]).
    pub(crate) const DISCONNECTED_SESSION: ReadyWait = ReadyWait {
        timeout: Duration::from_secs(60),
        interval: Duration::from_secs(2),
    };
}

/// The clone a branch is checked out in, and what to put it on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Checkout {
    pub(crate) workspace: String,
    /// `home:<clone>` from [`branches::resolve_branch_clone_dir`]; `None` for
    /// a branch with no project repo, whose git runs in the workstation's
    /// default directory.
    pub(crate) clone_dir: Option<String>,
    pub(crate) repo_slug: String,
    pub(crate) base_ref: String,
    pub(crate) branch_name: String,
    /// Whether another branch in this clone is the base branch itself (see
    /// [`BranchCheckout::base_is_worked_on`]).
    pub(crate) base_is_worked_on: bool,
}

impl Checkout {
    pub(crate) fn branch_checkout(&self) -> BranchCheckout<'_> {
        BranchCheckout {
            base_ref: &self.base_ref,
            branch_name: &self.branch_name,
            base_is_worked_on: self.base_is_worked_on,
        }
    }
}

/// Everything setup does outside the database, so tests drive the routine
/// without `sq`. [`SetupHost::check_out`] is where a test substitutes a
/// [`branches::GitRunner`] on a local clone for the workstation's.
#[async_trait]
pub(crate) trait SetupHost: Send + Sync + 'static {
    /// Whether the user is signed in to Blox. Without it `ws start` can hang.
    async fn check_auth(&self) -> Result<(), BloxError>;

    /// The status Blox reports for the workstation, as `ws info` words it.
    async fn workspace_status(&self, workspace: &str) -> Result<Option<String>, BloxError>;

    /// `sq blox ws start <workspace> <source>`; returns once bootstrap is
    /// done.
    async fn start_workspace(&self, workspace: &str, source: &str) -> Result<(), BloxError>;

    /// Make sure the clone exists and put it on the branch
    /// ([`checkout::check_out_workspace_branch`]).
    async fn check_out(&self, checkout: &Checkout) -> Result<(), CheckoutError>;

    /// What the clone has checked out: one cheap read, no fetch
    /// ([`verify::read_clone_head`]). `Err` when the read didn't answer,
    /// which says nothing about the clone.
    async fn clone_head(&self, checkout: &Checkout) -> Result<CloneHead, String>;

    /// Called with what [`Store::update_branch_workspace_status`] reported
    /// for a `Running` write (see [`branches::workspace_status_written`]).
    fn status_written(&self, branch_id: &str, changed: bool, status: &WorkspaceStatus);

    /// The reason a branch is in `Error` changed: `Some` when a run failed
    /// it, `None` when a new run began ([`failures`]).
    fn failure_changed(&self, branch_id: &str, error: Option<&str>);

    /// Call [`retry::fire`] for `branch_id` with `ticket` once `delay` has
    /// passed.
    fn retry_later(&self, branch_id: &str, delay: Duration, ticket: u64);

    /// Ask the reconcile service to settle a `disconnected` session now.
    fn check_disconnected_session(&self, session_id: &str);

    fn ready_wait(&self) -> ReadyWait {
        ReadyWait::DEFAULT
    }

    fn disconnected_session_wait(&self) -> ReadyWait {
        ReadyWait::DISCONNECTED_SESSION
    }

    fn head_read(&self) -> HeadRead {
        HeadRead::DEFAULT
    }

    fn retry_policy(&self) -> RetryPolicy {
        RetryPolicy::DEFAULT
    }
}

/// The real host: `sq blox` and the workstation's own git.
struct BloxHost<R: tauri::Runtime> {
    app_handle: tauri::AppHandle<R>,
    store: Arc<Store>,
}

/// The host the app's setups run against.
pub(crate) fn blox_host<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Arc<Store>,
) -> Arc<dyn SetupHost> {
    Arc::new(BloxHost {
        app_handle: app_handle.clone(),
        store: Arc::clone(store),
    })
}

#[async_trait]
impl<R: tauri::Runtime> SetupHost for BloxHost<R> {
    async fn check_auth(&self) -> Result<(), BloxError> {
        branches::run_blox_blocking(blox::check_auth).await
    }

    async fn workspace_status(&self, workspace: &str) -> Result<Option<String>, BloxError> {
        let name = workspace.to_string();
        let info = branches::run_blox_blocking(move || blox::ws_info(&name)).await?;
        if let Some(ws_id) = info.workstation_id {
            if let Ok(mut cache) = branches::workstation_id_cache().lock() {
                cache.insert(workspace.to_string(), ws_id);
            }
        }
        Ok(info.status)
    }

    async fn start_workspace(&self, workspace: &str, source: &str) -> Result<(), BloxError> {
        let name = workspace.to_string();
        let source = source.to_string();
        branches::run_blox_blocking(move || {
            blox::ws_start(
                &name,
                Some(&source),
                Some(branches::WORKSPACE_IDLE_TIMEOUT_MINUTES),
            )
        })
        .await
        .map(|_| ())
    }

    async fn check_out(&self, checkout: &Checkout) -> Result<(), CheckoutError> {
        match checkout.clone_dir.as_deref() {
            Some(clone_dir) => {
                branches::clone_repo_into_workspace(
                    &checkout.workspace,
                    clone_dir,
                    &checkout.repo_slug,
                    &checkout.branch_checkout(),
                )
                .await
            }
            None => branches::check_out_branch_in_workspace(
                &checkout.workspace,
                None,
                &checkout.branch_checkout(),
            )
            .await
            .map_err(|e| {
                e.map_message(|m| {
                    format!(
                        "Failed to set up branch '{}' in workspace '{}': {m}",
                        checkout.branch_name, checkout.workspace
                    )
                })
            }),
        }
    }

    async fn clone_head(&self, checkout: &Checkout) -> Result<CloneHead, String> {
        verify::read_clone_head(&checkout.workspace, checkout.clone_dir.as_deref()).await
    }

    fn status_written(&self, branch_id: &str, changed: bool, status: &WorkspaceStatus) {
        branches::workspace_status_written(&self.app_handle, branch_id, changed, status);
    }

    fn failure_changed(&self, branch_id: &str, error: Option<&str>) {
        failures::set(&self.app_handle, branch_id, error);
    }

    fn retry_later(&self, branch_id: &str, delay: Duration, ticket: u64) {
        let app_handle = self.app_handle.clone();
        let store = Arc::clone(&self.store);
        let branch_id = branch_id.to_string();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(delay).await;
            let host = blox_host(&app_handle, &store);
            retry::fire(SetupGuard::global(), &store, &host, &branch_id, ticket).await;
        });
    }

    fn check_disconnected_session(&self, session_id: &str) {
        if let Err(e) = crate::session_runner::reconcile::reconcile_session(session_id.to_string())
        {
            log::debug!("[workspace_setup] could not ask for a check of session {session_id}: {e}");
        }
    }
}

/// Whether Blox's status string means the workstation takes commands: the
/// same reading the pollers use, so a poll that triggers setup and the setup
/// it triggers agree on whether there is anything to start.
pub(crate) fn reports_running(blox_status: Option<&str>) -> bool {
    blox_status.is_some()
        && branches::map_blox_status_to_workspace_status(
            blox_status,
            Some(&WorkspaceStatus::Starting),
        ) == WorkspaceStatus::Running
}

/// Whether Blox says the workstation exists and is on its way up: being
/// created and bootstrapped, or resumed. It needs waiting for, not another
/// `ws start`.
pub(super) fn reports_booting(blox_status: Option<&str>) -> bool {
    blox_status.is_some_and(branches::is_blox_booting_status)
}
