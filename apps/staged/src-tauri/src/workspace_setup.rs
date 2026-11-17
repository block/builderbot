//! Branch setup for remote (Blox) branches.
//!
//! A remote branch is usable once two things are true: its workstation is
//! running, and the workstation's clone is on the branch. Blox only knows
//! about the first. This module owns the second, and with it the one write
//! that says both hold: it is the only code that moves a branch that is not
//! `Running` to `Running`, and it does so only after the checkout succeeded
//! and the clone reads as on the branch ([`verify`]). A branch marked
//! `Running` on a workstation still on its base branch has its sessions
//! commit onto the base.
//!
//! A failure leaves the branch in `Error`, and `Error` always has a way out:
//! the reason is recorded and announced ([`failures`]), a retry timer is set
//! ([`retry`]), and the error view's Retry asks for setup again.
//!
//! Setup is level-triggered. Whoever sees a branch that needs it asks:
//!
//! - the frontend, through the `start_workspace` command, for every
//!   `Starting` branch it holds and for an `Error` one the user retries;
//! - the status pollers ([`poll`]), which see a running workstation behind a
//!   row that doesn't say `Running`. They never promote it: a `Starting` row
//!   gets setup triggered, a `Stopped`, `Suspended` or statusless one is
//!   moved to `Starting` first, and an `Error` one gets its retry timer;
//! - `resume_workspace`, once the workstation is back;
//! - the startup pass ([`resume_pending_setups`]), for branches a previous
//!   run left in `Starting` or `Error`;
//! - a retry timer ([`SetupTrigger::Retry`]).
//!
//! One more caller asks for a branch that is already `Running`: an agent or
//! pipeline start that read the clone and found it off the branch
//! ([`verify::ensure_session_checkout`], [`SetupTrigger::Repair`]).
//!
//! All of them go through one [`guard::SetupGuard`], so a second request for
//! a branch joins the run in flight, and two branches never run setup in one
//! workstation at once. A caller that joined a run started for another
//! reason does not take that run's "nothing to do" for its own answer
//! ([`run_guarded`]). The guard is per process: a second Staged instance on
//! the same database runs its own.
//!
//! The checkout itself ([`checkout`]) never loses work or moves it between
//! branches, and never runs in a clone a live session is working in
//! ([`clone`]).

pub(crate) mod checkout;
pub(crate) mod clone;
pub(crate) mod failures;
pub(crate) mod guard;
pub(crate) mod host;
pub(crate) mod poll;
pub(crate) mod retry;
#[cfg(test)]
mod tests;
pub(crate) mod verify;
mod workstation;

use std::sync::Arc;

use crate::branches;
use crate::store::{self, Store, WorkspaceStatus};
use crate::PollWorkspaceResult;

use checkout::CheckoutError;
use clone::{check_out_in_clone, checkout_for};
use guard::{Patience, Progress, SetupGuard};
use host::{blox_host, reports_booting, reports_running, SetupHost};
use verify::CloneHead;
use workstation::{start_workstation, wait_until_running};

/// Who asked for a branch to be set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupTrigger {
    /// A caller that wants the branch set up whatever its row says, short of
    /// `Running`: the `start_workspace` command (a new branch, or the user's
    /// Retry on a failed one), or a repo joining a running workspace.
    Requested,
    /// Something that saw the row in `Starting` — a status poll, a resume,
    /// the startup pass. It acts only while the row still says so, so a
    /// trigger that arrives after setup settled the branch is a no-op.
    Observed,
    /// A retry timer for a row in `Error` ([`retry`]). It acts only while
    /// the row still says so and Blox reports the workstation running: it
    /// never starts or resumes one.
    Retry,
    /// An agent or pipeline start that read the clone of a `Running` branch
    /// and found it off the branch (see [`verify`]). The only trigger that
    /// acts on a `Running` row, and only while the clone still reads wrong
    /// when its turn comes. The row goes back to `Starting` for the run, so
    /// nothing else starts on the branch meanwhile.
    Repair,
}

impl SetupTrigger {
    /// Whether this trigger may create or boot a workstation that isn't
    /// running. Only the ones that stand for a branch being provisioned: a
    /// retry waits for a running one, and a repair of a `Running` branch
    /// was asked for by a session that had just read its clone.
    fn may_start_workstation(self) -> bool {
        matches!(self, SetupTrigger::Requested | SetupTrigger::Observed)
    }
}

/// One request for setup: why, and on whose behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SetupRequest {
    pub(crate) trigger: SetupTrigger,
    /// The session a repair was asked for by. It may already be `running`
    /// (a claimed resume), and is not a reason to leave the clone alone.
    pub(crate) session: Option<String>,
}

impl SetupRequest {
    pub(crate) fn new(trigger: SetupTrigger) -> Self {
        Self {
            trigger,
            session: None,
        }
    }

    pub(crate) fn repair_for(session_id: &str) -> Self {
        Self {
            trigger: SetupTrigger::Repair,
            session: Some(session_id.to_string()).filter(|id| !id.is_empty()),
        }
    }
}

/// What a setup run did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupOutcome {
    /// The branch is checked out, read to be so, and marked `Running`.
    SetUp,
    /// The run's trigger had nothing to do by the time its turn came: the
    /// row had moved on, a repair's clone read right, a retry's workstation
    /// wasn't running. The row is as the run found it.
    NothingToDo,
}

/// Set `branch_id` up and mark it `Running`, joining a run already in flight
/// for it, and wait for the result. For a caller whose next step needs the
/// branch checked out: a repo joining a running workspace.
pub(crate) async fn ensure_branch_set_up<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Arc<Store>,
    branch_id: &str,
    trigger: SetupTrigger,
) -> Result<(), String> {
    let host = blox_host(app_handle, store);
    run_guarded(
        SetupGuard::global(),
        store,
        &host,
        branch_id,
        SetupRequest::new(trigger),
        Patience::ToTheEnd,
    )
    .await
    .map(|_| ())
}

/// The `start_workspace` command and web arm: ask for `branch_id` to be set
/// up and return the status its row holds.
///
/// `running` once the branch is checked out and read to be so. `starting`
/// when the run is waiting for the workstation to boot: the call returns
/// rather than hold a caller (and one of the frontend's few setup slots) for
/// the minutes that can take. The run goes on, its outcome lands on the row,
/// and a failure's reason is announced ([`failures`]). An error is the
/// failure that left the row in `Error`.
pub(crate) async fn request_branch_setup<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<PollWorkspaceResult, String> {
    let host = blox_host(app_handle, store);
    let status = run_guarded(
        SetupGuard::global(),
        store,
        &host,
        branch_id,
        SetupRequest::new(SetupTrigger::Requested),
        Patience::WhileWorking,
    )
    .await?;
    let workspace = store
        .get_branch(branch_id)
        .ok()
        .flatten()
        .and_then(|branch| branch.workspace_name);
    Ok(PollWorkspaceResult {
        status: status
            .unwrap_or(WorkspaceStatus::Starting)
            .as_str()
            .to_string(),
        workstation_id: workspace.and_then(|name| branches::cached_workstation_id(&name)),
    })
}

/// Trigger setup for a branch seen in `Starting`, without waiting for it.
/// The outcome lands on the row: `Running`, or `Error` with its reason
/// announced.
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

/// Set the retry timer for a branch seen in `Error`, unless one is out for
/// it already ([`retry::arm`]).
pub(crate) fn arm_retry<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Arc<Store>,
    branch_id: &str,
) {
    retry::arm(
        SetupGuard::global(),
        blox_host(app_handle, store).as_ref(),
        branch_id,
    );
}

/// A branch was put in `Error` by something other than setup: a failed
/// resume, a poll that found `sq` signed out or the workstation failed.
/// Record why, so the UI can say, and set its retry timer.
pub(crate) fn branch_failed<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Arc<Store>,
    branch_id: &str,
    reason: &str,
) {
    failures::set(app_handle, branch_id, Some(reason));
    arm_retry(app_handle, store, branch_id);
}

/// The startup pass: set up every remote branch a previous run left in
/// `Starting`, so recovery doesn't wait for a window to load the project,
/// and set the retry timer of every one it left in `Error`.
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
        "[workspace_setup] {} branch(es) left in Starting or Error by a previous run",
        pending.len()
    );
    for branch in pending {
        match startup_action(&branch) {
            Some(StartupAction::SetUp) => trigger_branch_setup(app_handle, store, &branch.id),
            Some(StartupAction::ArmRetry) => arm_retry(app_handle, store, &branch.id),
            None => {}
        }
    }
}

/// What the startup pass does about a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupAction {
    /// Left in `Starting`: finish its setup.
    SetUp,
    /// Left in `Error`: retry on the timer, once its workstation is running.
    ArmRetry,
}

pub(crate) fn startup_action(branch: &store::Branch) -> Option<StartupAction> {
    match branch.workspace_status {
        Some(WorkspaceStatus::Starting) => Some(StartupAction::SetUp),
        Some(WorkspaceStatus::Error) => Some(StartupAction::ArmRetry),
        _ => None,
    }
}

/// Every remote branch the startup pass has something to do for
/// ([`startup_action`]), skipping projects a transfer is rewriting.
pub(crate) fn branches_awaiting_setup(store: &Store) -> Result<Vec<store::Branch>, String> {
    let mut pending = Vec::new();
    for project in store.list_projects().map_err(|e| e.to_string())? {
        for branch in store
            .list_branches_for_project(&project.id)
            .map_err(|e| e.to_string())?
        {
            if branch.branch_type == store::BranchType::Remote
                && branch.workspace_name.is_some()
                && startup_action(&branch).is_some()
                && !crate::project_transfer::is_branch_transferring(&branch.id)
            {
                pending.push(branch);
            }
        }
    }
    Ok(pending)
}

/// How many runs a caller will join in a row that turn out to have had
/// nothing to do, before it settles for what the row says.
const MAX_JOINED_ROUNDS: usize = 3;

/// [`set_up_branch`] under `guard`: joined per branch, serialized per
/// workstation. Returns the status the branch's row holds afterwards, or
/// `Starting` for a caller that left the run waiting
/// ([`Patience::WhileWorking`]).
///
/// A caller gets an answer to what it asked. The run it joins may have been
/// started for another reason: a `Requested` call can land on an `Observed`
/// run that found the row no longer `Starting`, or on a `Retry` that found
/// the workstation down, and those end with nothing done. Such an end is
/// not this caller's: it looks at the row again and, if its own trigger
/// still has something to do, runs it.
pub(crate) async fn run_guarded(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &Arc<dyn SetupHost>,
    branch_id: &str,
    request: SetupRequest,
    patience: Patience,
) -> Result<Option<WorkspaceStatus>, String> {
    let status_now = || -> Result<Option<WorkspaceStatus>, String> {
        Ok(store
            .get_branch(branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Branch not found: {branch_id}"))?
            .workspace_status)
    };
    for _ in 0..MAX_JOINED_ROUNDS {
        let branch = store
            .get_branch(branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
        // Checked before taking a slot in the guard, so a trigger with
        // nothing to do never becomes the run a real request joins.
        if !wants_setup(&branch, request.trigger) {
            return Ok(branch.workspace_status);
        }
        let workspace = branch
            .workspace_name
            .clone()
            .ok_or("Branch has no workspace name")?;

        // A request can be answered before its run has had its turn, when
        // the workstation is busy with another branch
        // ([`Patience::WhileWorking`]). The row says `Starting` from now,
        // not from whenever the turn comes: a window told `starting` and
        // then polling a row that still read `Error` or `Stopped` would stop
        // watching a branch that is being set up. Nothing awaits between
        // this write and the run being in flight, and a `Starting` row is
        // one every recovery path picks up.
        if request.trigger == SetupTrigger::Requested
            && branch.workspace_status != Some(WorkspaceStatus::Starting)
        {
            crate::project_transfer::ensure_branch_not_transferring(branch_id)?;
            store
                .update_branch_workspace_status(branch_id, &WorkspaceStatus::Starting)
                .map_err(|e| e.to_string())?;
        }

        let end = {
            let guard_for_run = Arc::clone(guard);
            let store = Arc::clone(store);
            let host = Arc::clone(host);
            let id = branch_id.to_string();
            let request = request.clone();
            guard
                .run(
                    &workspace,
                    branch_id,
                    patience,
                    move |progress| async move {
                        set_up_branch(
                            &guard_for_run,
                            &store,
                            host.as_ref(),
                            &id,
                            &request,
                            &progress,
                        )
                        .await
                    },
                )
                .await
        };
        match end.result {
            // Left with the run still waiting, for the workstation or for
            // its turn on it. A run in flight is a branch that is starting,
            // whether or not its turn has come and the row says so yet.
            None => return Ok(Some(WorkspaceStatus::Starting)),
            Some(Err(e)) => return Err(e),
            Some(Ok(SetupOutcome::SetUp)) => return status_now(),
            // This caller's own run: nothing to do is its answer.
            Some(Ok(SetupOutcome::NothingToDo)) if !end.joined => return status_now(),
            // Someone else's run had nothing to do. Ask again.
            Some(Ok(SetupOutcome::NothingToDo)) => {}
        }
    }
    status_now()
}

/// Whether `trigger` has anything to do for a branch in this state. A
/// `Running` branch is set up already, and may have a session in its clone:
/// nothing touches it but a repair, which an agent start asks for after
/// reading the clone off the branch. An observed trigger acts only on
/// `Starting`, and a retry only on `Error`. A repair acts on nothing but
/// `Running` and `Starting`: a branch that has stopped, or failed, since the
/// session read its clone is not one to start a workstation for from a
/// session start.
fn wants_setup(branch: &store::Branch, trigger: SetupTrigger) -> bool {
    use SetupTrigger::*;
    match (&branch.workspace_status, trigger) {
        (Some(WorkspaceStatus::Running), Repair) => true,
        (Some(WorkspaceStatus::Running), _) => false,
        (Some(WorkspaceStatus::Starting), Requested | Observed | Repair) => true,
        (Some(WorkspaceStatus::Error), Requested | Retry) => true,
        (_, Requested) => true,
        (_, Observed | Retry | Repair) => false,
    }
}

/// Bring a branch's workstation up, check the branch out in its clone, and
/// only then mark it `Running`.
///
/// One routine for a workstation that has to be created and one that is
/// already running (a resumed one, a repo joining a shared one, a retry
/// after a restart or a failure). Both end in [`check_out_in_clone`]:
/// `Running` is written only over a clone that reads as on the branch.
///
/// Every failure past the first lookups is returned, and leaves the row in
/// `Error` with its reason recorded and a retry timer set. One exception: a
/// repair of a `Running` branch that was refused (a worktree with changes in
/// the way, commits that would be left behind, a live session in the clone)
/// puts the row back to `Running`. The branch was usable before the session
/// that asked came along and is exactly as usable now; only that session
/// fails, with the reason.
///
/// Callers go through [`run_guarded`].
pub(crate) async fn set_up_branch(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &dyn SetupHost,
    branch_id: &str,
    request: &SetupRequest,
    progress: &Progress,
) -> Result<SetupOutcome, String> {
    let trigger = request.trigger;
    // A transfer provisions its own branches; refusing leaves the row alone.
    crate::project_transfer::ensure_branch_not_transferring(branch_id)?;

    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    // Read again now that the workstation's turn has come: the run ahead of
    // this one may have settled the branch.
    if !wants_setup(&branch, trigger) {
        return Ok(SetupOutcome::NothingToDo);
    }
    let was_running = branch.workspace_status == Some(WorkspaceStatus::Running);

    // A retry goes ahead only on a workstation Blox reports running. One
    // that is down, or can't be asked about, is looked at again later.
    if trigger == SetupTrigger::Retry {
        let reported = match branch.workspace_name.as_deref() {
            Some(workspace) => host.workspace_status(workspace).await.ok().flatten(),
            None => None,
        };
        if !reports_running(reported.as_deref()) {
            retry::schedule(guard, host, branch_id);
            return Ok(SetupOutcome::NothingToDo);
        }
    }

    // A repair was asked for by a session that read the clone off the
    // branch. The run ahead of this one (another session's repair, a
    // sibling branch's setup) may have changed that; when the clone reads
    // right now there is nothing to repair, and a `Running` branch is left
    // exactly as it is. A read that fails here settles nothing: the repair
    // goes ahead and ends in its own read.
    if trigger == SetupTrigger::Repair && was_running {
        if let Ok(checkout) = checkout_for(store, &branch) {
            if matches!(
                host.clone_head(&checkout).await,
                Ok(CloneHead::Branch(name)) if name == checkout.branch_name
            ) {
                return Ok(SetupOutcome::NothingToDo);
            }
        }
    }

    // A request for a branch that had settled somewhere else (a failed
    // setup, a stale `starting` in the window that asked, a repair of a
    // `Running` one) puts it back in `Starting`, where the pollers hold it
    // rather than promote it under this run and the session queue holds
    // whatever else would start on the branch.
    if branch.workspace_status != Some(WorkspaceStatus::Starting) {
        store
            .update_branch_workspace_status(branch_id, &WorkspaceStatus::Starting)
            .map_err(|e| e.to_string())?;
    }
    host.failure_changed(branch_id, None);

    // A repair of a row that is not `Running` is the setup that row was
    // waiting for, and provisions like one.
    let may_start_workstation =
        trigger.may_start_workstation() || (trigger == SetupTrigger::Repair && !was_running);
    let provisioned = provision_and_check_out(
        guard,
        store,
        host,
        &branch,
        request,
        may_start_workstation,
        progress,
    )
    .await;
    let error = match provisioned {
        Ok(()) => {
            let changed = store
                .update_branch_workspace_status(branch_id, &WorkspaceStatus::Running)
                .map_err(|e| e.to_string())?;
            host.status_written(branch_id, changed, &WorkspaceStatus::Running);
            guard.forget_retries(branch_id);
            log::info!(
                "[workspace_setup] branch={branch_id} checked out '{}' and running",
                branch.branch_name
            );
            return Ok(SetupOutcome::SetUp);
        }
        Err(error) => error,
    };

    log::warn!("[workspace_setup] branch={branch_id} setup failed: {error}");
    // Only over `Starting`. A poll that saw the workstation suspended or
    // gone while this ran has already said something truer than `Error`,
    // and with a way forward (Resume).
    let still_starting = matches!(
        store.get_branch(branch_id),
        Ok(Some(row)) if row.workspace_status == Some(WorkspaceStatus::Starting)
    );
    if still_starting {
        if trigger == SetupTrigger::Repair && was_running && error.is_refusal() {
            // Nothing was touched, so the branch is what it was. The write
            // kicks the session queue, which held while the row said
            // `Starting`.
            if let Ok(changed) =
                store.update_branch_workspace_status(branch_id, &WorkspaceStatus::Running)
            {
                host.status_written(branch_id, changed, &WorkspaceStatus::Running);
            }
        } else {
            store
                .update_branch_workspace_status(branch_id, &WorkspaceStatus::Error)
                .ok();
            host.failure_changed(branch_id, Some(error.message()));
            retry::schedule(guard, host, branch_id);
        }
    }
    Err(error.message().to_string())
}

async fn provision_and_check_out(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &dyn SetupHost,
    branch: &store::Branch,
    request: &SetupRequest,
    may_start_workstation: bool,
    progress: &Progress,
) -> Result<(), CheckoutError> {
    let failed = CheckoutError::Failed;
    let checkout = checkout_for(store, branch).map_err(failed)?;
    let workspace = checkout.workspace.as_str();

    // Pre-flight: without it `blox ws start` can hang or fail opaquely,
    // leaving the frontend spinning for minutes (issue #99).
    host.check_auth().await.map_err(|e| failed(e.to_string()))?;

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
        wait_until_running(
            store,
            host,
            branch,
            workspace,
            "it was still booting",
            progress,
        )
        .await
        .map_err(failed)?;
    } else if may_start_workstation {
        start_workstation(store, host, branch, &checkout, progress)
            .await
            .map_err(failed)?;
    } else {
        // A repair or a retry found the workstation down. The clone is as
        // it was, and what the row should say about a workstation that
        // isn't running is the pollers' to settle.
        return Err(CheckoutError::Refused(format!(
            "Workspace '{workspace}' is not running (Blox reports it as {}), so branch '{}' \
             could not be checked out in it",
            reported.as_deref().unwrap_or("unknown"),
            checkout.branch_name
        )));
    }

    check_out_in_clone(guard, store, host, &checkout, request).await
}
