//! Branch setup for remote (Blox) branches.
//!
//! A remote branch is usable once two things are true: its workstation is
//! running, and the workstation's clone is on the branch. Blox only knows
//! about the first. This module owns the second, and with it the one write
//! that says both hold: it is the only code that moves a branch from
//! `Starting` to `Running`, and it does so only after the checkout succeeded.
//! A failure leaves the branch in `Error` and is returned, never logged and
//! dropped — a branch marked `Running` on a workstation still on its base
//! branch has its sessions commit onto the base.
//!
//! Setup is level-triggered. Whoever sees a `Starting` branch asks for it:
//!
//! - the frontend, through the `start_workspace` command, for every
//!   `Starting` branch it holds;
//! - the status pollers ([`poll`]), which see a running workstation whose
//!   row is still `Starting` and trigger setup instead of promoting it;
//! - `resume_workspace`, once the workstation is back;
//! - the startup pass ([`resume_pending_setups`]), for branches a previous
//!   run left in `Starting`.
//!
//! All of them go through one [`guard::SetupGuard`], so a second request for
//! a branch joins the run in flight, and two branches never run setup in one
//! workstation at once. The guard is per process: a second Staged instance
//! on the same database runs its own.

pub(crate) mod guard;
pub(crate) mod poll;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::blox::{self, BloxError};
use crate::branches;
use crate::store::{self, Store, WorkspaceStatus};

use guard::SetupGuard;

/// Who asked for a branch to be set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupTrigger {
    /// A caller that wants the branch set up whatever its row says, short of
    /// `Running`: the `start_workspace` command, or a repo joining a running
    /// workspace.
    Requested,
    /// Something that saw the row in `Starting` — a status poll, a resume,
    /// the startup pass. It acts only while the row still says so, so a
    /// trigger that arrives after setup settled the branch is a no-op.
    Observed,
}

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
    /// never come.
    pub(crate) const DEFAULT: ReadyWait = ReadyWait {
        timeout: Duration::from_secs(300),
        interval: Duration::from_secs(5),
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
    /// ([`branches::check_out_workspace_branch`]).
    async fn check_out(&self, checkout: &Checkout) -> Result<(), String>;

    /// Called with what [`Store::update_branch_workspace_status`] reported
    /// for the `Running` write (see [`branches::workspace_status_written`]).
    fn status_written(&self, branch_id: &str, changed: bool, status: &WorkspaceStatus);

    fn ready_wait(&self) -> ReadyWait {
        ReadyWait::DEFAULT
    }
}

/// The real host: `sq blox` and the workstation's own git.
struct BloxHost<R: tauri::Runtime> {
    app_handle: tauri::AppHandle<R>,
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

    async fn check_out(&self, checkout: &Checkout) -> Result<(), String> {
        match checkout.clone_dir.as_deref() {
            Some(clone_dir) => {
                branches::clone_repo_into_workspace(
                    &checkout.workspace,
                    clone_dir,
                    &checkout.repo_slug,
                    &checkout.base_ref,
                    &checkout.branch_name,
                )
                .await
            }
            None => branches::check_out_branch_in_workspace(
                &checkout.workspace,
                None,
                &checkout.base_ref,
                &checkout.branch_name,
            )
            .await
            .map_err(|e| {
                format!(
                    "Failed to set up branch '{}' in workspace '{}': {e}",
                    checkout.branch_name, checkout.workspace
                )
            }),
        }
    }

    fn status_written(&self, branch_id: &str, changed: bool, status: &WorkspaceStatus) {
        branches::workspace_status_written(&self.app_handle, branch_id, changed, status);
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
fn reports_booting(blox_status: Option<&str>) -> bool {
    blox_status.is_some_and(branches::is_blox_booting_status)
}

/// Set `branch_id` up and mark it `Running`, joining a run already in flight
/// for it. The one entry point for every trigger.
pub(crate) async fn ensure_branch_set_up<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Arc<Store>,
    branch_id: &str,
    trigger: SetupTrigger,
) -> Result<(), String> {
    let host: Arc<dyn SetupHost> = Arc::new(BloxHost {
        app_handle: app_handle.clone(),
    });
    run_guarded(SetupGuard::global(), store, &host, branch_id, trigger).await
}

/// Trigger setup for a branch seen in `Starting`, without waiting for it.
/// The outcome lands on the row: `Running`, or `Error`.
pub(crate) fn trigger_branch_setup<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Arc<Store>,
    branch_id: &str,
) {
    // A poll every 3 s would otherwise spawn a task per tick just to join.
    if SetupGuard::global().is_in_flight(branch_id) {
        return;
    }
    let app_handle = app_handle.clone();
    let store = Arc::clone(store);
    let branch_id = branch_id.to_string();
    tauri::async_runtime::spawn(async move {
        if let Err(e) =
            ensure_branch_set_up(&app_handle, &store, &branch_id, SetupTrigger::Observed).await
        {
            log::warn!("[workspace_setup] setup for branch {branch_id} failed: {e}");
        }
    });
}

/// The startup pass: set up every remote branch a previous run left in
/// `Starting`, so recovery doesn't wait for a window to load the project.
///
/// Call after interrupted transfers are recovered: a transfer's branches are
/// its own to provision.
pub(crate) fn resume_pending_setups<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Arc<Store>,
) {
    let pending = match branches_awaiting_setup(store) {
        Ok(pending) => pending,
        Err(e) => {
            log::warn!("[workspace_setup] could not list branches awaiting setup: {e}");
            return;
        }
    };
    if pending.is_empty() {
        return;
    }
    log::info!(
        "[workspace_setup] {} branch(es) left in Starting by a previous run",
        pending.len()
    );
    for branch in pending {
        trigger_branch_setup(app_handle, store, &branch.id);
    }
}

/// Every remote branch whose row says `Starting`, skipping projects a
/// transfer is rewriting.
pub(crate) fn branches_awaiting_setup(store: &Store) -> Result<Vec<store::Branch>, String> {
    let mut pending = Vec::new();
    for project in store.list_projects().map_err(|e| e.to_string())? {
        for branch in store
            .list_branches_for_project(&project.id)
            .map_err(|e| e.to_string())?
        {
            if branch.branch_type == store::BranchType::Remote
                && branch.workspace_name.is_some()
                && branch.workspace_status == Some(WorkspaceStatus::Starting)
                && !crate::project_transfer::is_branch_transferring(&branch.id)
            {
                pending.push(branch);
            }
        }
    }
    Ok(pending)
}

/// [`set_up_branch`] under `guard`: joined per branch, serialized per
/// workstation.
pub(crate) async fn run_guarded(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &Arc<dyn SetupHost>,
    branch_id: &str,
    trigger: SetupTrigger,
) -> Result<(), String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    // Checked before taking a slot in the guard, so a trigger with nothing
    // to do never becomes the run a real request joins.
    if !wants_setup(&branch, trigger) {
        return Ok(());
    }
    let workspace = branch
        .workspace_name
        .clone()
        .ok_or("Branch has no workspace name")?;

    let store = Arc::clone(store);
    let host = Arc::clone(host);
    let id = branch_id.to_string();
    guard
        .run(&workspace, branch_id, move || async move {
            set_up_branch(&store, host.as_ref(), &id, trigger).await
        })
        .await
}

/// Whether `trigger` has anything to do for a branch in this state. A
/// `Running` branch is set up already, and may have a session in its clone:
/// nothing touches it. An observed trigger acts only on `Starting`.
fn wants_setup(branch: &store::Branch, trigger: SetupTrigger) -> bool {
    match (&branch.workspace_status, trigger) {
        (Some(WorkspaceStatus::Running), _) => false,
        (Some(WorkspaceStatus::Starting), SetupTrigger::Observed) => true,
        (_, SetupTrigger::Observed) => false,
        (_, SetupTrigger::Requested) => true,
    }
}

/// Bring a branch's workstation up, check the branch out in its clone, and
/// only then mark it `Running`.
///
/// One routine for a workstation that has to be created and one that is
/// already running (a resumed workstation, a repo joining a shared one, a
/// retry after a restart). Both end in
/// [`branches::check_out_workspace_branch`], which is idempotent and never
/// resets a branch the clone already has.
///
/// Every failure past the first lookups leaves the row in `Error` and is
/// returned. Callers go through [`run_guarded`].
pub(crate) async fn set_up_branch(
    store: &Arc<Store>,
    host: &dyn SetupHost,
    branch_id: &str,
    trigger: SetupTrigger,
) -> Result<(), String> {
    // A transfer provisions its own branches; refusing leaves the row alone.
    crate::project_transfer::ensure_branch_not_transferring(branch_id)?;

    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    // Read again now that the workstation's turn has come: the run ahead of
    // this one may have settled the branch.
    if !wants_setup(&branch, trigger) {
        return Ok(());
    }

    // A request for a branch that had settled somewhere else (a failed
    // setup, a stale `starting` in the window that asked) puts it back in
    // `Starting`, where the pollers hold it rather than promote it under
    // this run.
    if branch.workspace_status != Some(WorkspaceStatus::Starting) {
        store
            .update_branch_workspace_status(branch_id, &WorkspaceStatus::Starting)
            .map_err(|e| e.to_string())?;
    }

    let result = provision_and_check_out(store, host, &branch).await;
    match &result {
        Ok(()) => {
            let changed = store
                .update_branch_workspace_status(branch_id, &WorkspaceStatus::Running)
                .map_err(|e| e.to_string())?;
            host.status_written(branch_id, changed, &WorkspaceStatus::Running);
            log::info!(
                "[workspace_setup] branch={branch_id} checked out '{}' and running",
                branch.branch_name
            );
        }
        Err(e) => {
            log::warn!("[workspace_setup] branch={branch_id} setup failed: {e}");
            // Only over `Starting`. A poll that saw the workstation
            // suspended or gone while this ran has already said something
            // truer than `Error`, and with a way forward (Resume).
            let still_starting = matches!(
                store.get_branch(branch_id),
                Ok(Some(row)) if row.workspace_status == Some(WorkspaceStatus::Starting)
            );
            if still_starting {
                store
                    .update_branch_workspace_status(branch_id, &WorkspaceStatus::Error)
                    .ok();
            }
        }
    }
    result
}

async fn provision_and_check_out(
    store: &Arc<Store>,
    host: &dyn SetupHost,
    branch: &store::Branch,
) -> Result<(), String> {
    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;
    let workspace = branch
        .workspace_name
        .as_deref()
        .ok_or("Branch has no workspace name")?;
    let checkout = Checkout {
        workspace: workspace.to_string(),
        clone_dir: branches::resolve_branch_clone_dir(store, branch)?,
        repo_slug: branches::resolve_branch_repo_slug(store, &project, branch)?,
        base_ref: branches::normalize_branch_ref(&branch.base_branch),
        branch_name: branch.branch_name.clone(),
    };

    // Pre-flight: without it `blox ws start` can hang or fail opaquely,
    // leaving the frontend spinning for minutes (issue #99).
    host.check_auth().await.map_err(|e| e.to_string())?;

    // A workstation that is already up — a resumed one, a shared one this
    // repo joins, one a previous run started — is not started again, and
    // neither is one that is on its way: `ws start` on a name that is
    // already booting is at best a second bootstrap. A failed lookup is not
    // an answer; `ws start` settles it.
    let reported = host.workspace_status(workspace).await.ok().flatten();
    if reports_running(reported.as_deref()) {
        // Nothing to start.
    } else if reports_booting(reported.as_deref()) {
        log::info!(
            "[workspace_setup] branch={} workspace={workspace} is already booting, waiting for it",
            branch.id
        );
        wait_until_running(store, host, branch, workspace, "it was still booting").await?;
    } else {
        start_workstation(store, host, branch, &checkout).await?;
    }

    host.check_out(&checkout).await
}

/// `sq blox ws start`, and when that fails without ruling the workstation
/// out, a bounded wait for it to come up anyway.
async fn start_workstation(
    store: &Arc<Store>,
    host: &dyn SetupHost,
    branch: &store::Branch,
    checkout: &Checkout,
) -> Result<(), String> {
    let workspace = checkout.workspace.as_str();
    let source = format!(
        "https://github.com/{}.git?ref={}",
        checkout.repo_slug, checkout.base_ref
    );
    log::info!(
        "[workspace_setup] branch={} workspace={workspace} starting from {source}",
        branch.id
    );
    let start_error = match host.start_workspace(workspace, &source).await {
        Ok(()) => return Ok(()),
        Err(BloxError::NotAuthenticated) => {
            return Err("Not authenticated with Blox. Run: sq login".to_string());
        }
        // Definitive: keep the CLI's text, which carries the onboarding URL
        // and the action it wants.
        Err(e) if branches::is_blox_onboarding_precondition_error(&e) => {
            return Err(e.to_string());
        }
        Err(e) => e,
    };

    // `ws start` can fail — its own 120 s limit, a dropped connection — after
    // Blox created the workstation, which then keeps booting. Returning here
    // would leave the branch with a running workstation and no checkout, so
    // wait for it and carry on, or fail where the user can see it.
    log::warn!(
        "[workspace_setup] branch={} workspace={workspace} ws start failed, waiting for the workstation: {start_error}",
        branch.id
    );
    wait_until_running(
        store,
        host,
        branch,
        workspace,
        &format!("`sq blox ws start` failed: {start_error}"),
    )
    .await
}

/// Wait, within [`SetupHost::ready_wait`], for the workstation to report
/// running. `why` says what the wait was for, for the error when it never
/// does.
async fn wait_until_running(
    store: &Arc<Store>,
    host: &dyn SetupHost,
    branch: &store::Branch,
    workspace: &str,
    why: &str,
) -> Result<(), String> {
    let wait = host.ready_wait();
    let started = tokio::time::Instant::now();
    loop {
        match host.workspace_status(workspace).await {
            Ok(status) if reports_running(status.as_deref()) => return Ok(()),
            Ok(status)
                if branches::map_blox_status_to_workspace_status(
                    status.as_deref(),
                    Some(&WorkspaceStatus::Starting),
                ) == WorkspaceStatus::Error =>
            {
                return Err(format!(
                    "Workspace '{workspace}' failed to start (Blox reports it as {}); {why}",
                    status.as_deref().unwrap_or("error")
                ));
            }
            Err(BloxError::NotAuthenticated) => {
                return Err("Not authenticated with Blox. Run: sq login".to_string());
            }
            // Still booting, or not listed yet.
            Ok(_) | Err(_) => {}
        }
        // Deleted while it waited: nothing left to set up.
        if store.get_branch(&branch.id).ok().flatten().is_none() {
            return Err(format!("Branch not found: {}", branch.id));
        }
        if started.elapsed() >= wait.timeout {
            return Err(format!(
                "Workspace '{workspace}' did not come up within {}s; {why}",
                wait.timeout.as_secs()
            ));
        }
        tokio::time::sleep(wait.interval).await;
    }
}
