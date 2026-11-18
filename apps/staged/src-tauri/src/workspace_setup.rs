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
//! The backend owns setup end to end. A client only asks for it and renders
//! what the row and the reason event say; nothing waits on a client.
//!
//! A failure leaves the branch in `Error`, and `Error` always has a way out:
//! the reason is recorded and announced ([`failures`]), a retry timer is set
//! unless the checkout refused over what is in the clone ([`retry`]), and
//! the error view's Retry asks for setup again.
//!
//! Setup is level-triggered. Whoever sees a branch that needs it asks:
//!
//! - the frontend, through the `start_workspace` command, for every
//!   `Starting` branch it holds and for an `Error` one the user retries
//!   ([`request_setup`]). The call returns once the run is under way;
//! - the status pollers ([`poll`]), which see a running workstation behind a
//!   row that doesn't say `Running`. They never promote it: a `Starting` row
//!   gets setup triggered, a `Stopped`, `Suspended` or statusless one is
//!   moved to `Starting` first, and an `Error` one gets its retry timer;
//! - `resume_workspace`, once the workstation is back;
//! - the startup pass ([`resume_pending_setups`]), for branches a previous
//!   run left in `Starting` or `Error`;
//! - a retry timer ([`SetupTrigger::Retry`]), which is quiet: the row stays
//!   `Error` with its reason until the checkout has passed.
//!
//! One more caller asks for a branch that is already `Running`: an agent or
//! pipeline start that read the clone and found it off the branch
//! ([`verify::ensure_session_checkout`], [`SetupTrigger::Repair`]).
//!
//! All of them go through one [`guard::SetupGuard`], so a second request for
//! a branch joins the run in flight, and two branches never run setup in one
//! workstation at once. The guard is per process: a second Staged instance
//! on the same database runs its own.
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

use crate::store::{self, Store, WorkspaceStatus};

use checkout::CheckoutError;
use clone::{check_out_in_clone, checkout_for};
use guard::SetupGuard;
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
    /// A retry timer for a row in `Error` ([`retry`]), which fires it only
    /// while Blox reports the workstation running: it never starts or
    /// resumes one. Nobody asked for it, so it is quiet. The row stays
    /// `Error` with its reason on show until the checkout has passed.
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
    let request = SetupRequest::new(trigger);
    run_guarded(SetupGuard::global(), store, &host, branch_id, request)
        .await
        .map(|_| ())
}

/// The `start_workspace` command and web arm ([`request_setup`]).
pub(crate) fn request_branch_setup<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<(), String> {
    let host = blox_host(app_handle, store);
    request_setup(SetupGuard::global(), store, &host, branch_id)
}

/// Ask for `branch_id` to be set up and return without waiting for it.
///
/// The run's outcome lands on the row, and a failure's reason is announced
/// ([`failures`]): that is all a client is told, and all it needs. An error
/// here means the request itself could not be made.
///
/// A request for a row that isn't `Starting` is the error view's Retry. The
/// row says `Starting` and its reason is gone before this returns, so the
/// card that asked turns at once. A request that finds a `Starting` row with
/// no run in flight is a new branch's. Both are the user's ask, and a
/// failure of the run is announced as theirs. A request for a `Starting` row
/// whose run is in flight (the startup pass's, a poll's) only joins it.
pub(crate) fn request_setup(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &Arc<dyn SetupHost>,
    branch_id: &str,
) -> Result<(), String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    if !wants_setup(&branch, SetupTrigger::Requested) {
        return Ok(());
    }
    let workspace = branch
        .workspace_name
        .as_deref()
        .ok_or("Branch has no workspace name")?;
    crate::project_transfer::ensure_branch_not_transferring(branch_id)?;

    let was_starting = branch.workspace_status == Some(WorkspaceStatus::Starting);
    if !was_starting || !guard.is_in_flight(branch_id) {
        guard.user_asked(branch_id);
    }
    if !was_starting {
        store
            .update_branch_workspace_status(branch_id, &WorkspaceStatus::Starting)
            .map_err(|e| e.to_string())?;
        host.failure_changed(branch_id, None, false);
    }
    let request = SetupRequest::new(SetupTrigger::Requested);
    start_run(guard, store, host, workspace, branch_id, request);
    Ok(())
}

/// Trigger setup for a branch seen in `Starting`, without waiting for it.
/// The outcome lands on the row: `Running`, or `Error` with its reason
/// announced. `by_user` says the user brought this about (a resume), so a
/// failure is announced as theirs.
pub(crate) fn trigger_branch_setup<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Arc<Store>,
    branch_id: &str,
    by_user: bool,
) {
    let guard = SetupGuard::global();
    if by_user {
        guard.user_asked(branch_id);
    }
    // A poll every 3 s would otherwise spawn a task per tick just to join.
    if guard.is_in_flight(branch_id) {
        return;
    }
    let host = blox_host(app_handle, store);
    let store = Arc::clone(store);
    let branch_id = branch_id.to_string();
    tauri::async_runtime::spawn(async move {
        let request = SetupRequest::new(SetupTrigger::Observed);
        if let Err(e) = run_guarded(guard, &store, &host, &branch_id, request).await {
            log::warn!("[workspace_setup] setup for branch {branch_id} failed: {e}");
        }
        // No run, no failure to announce as the user's.
        guard.take_user_asked(&branch_id);
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
    let user_asked = SetupGuard::global().take_user_asked(branch_id);
    failures::set(app_handle, store, branch_id, Some(reason), user_asked);
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
            Some(StartupAction::SetUp) => {
                trigger_branch_setup(app_handle, store, &branch.id, false)
            }
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

/// Start [`set_up_branch`] for `branch_id` under `guard`, or join the run in
/// flight for it: joined per branch, serialized per workstation.
fn start_run(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &Arc<dyn SetupHost>,
    workspace: &str,
    branch_id: &str,
    request: SetupRequest,
) -> guard::Run {
    let (guard_for_run, store, host) = (Arc::clone(guard), Arc::clone(store), Arc::clone(host));
    let id = branch_id.to_string();
    guard.start(workspace, branch_id, move || async move {
        set_up_branch(&guard_for_run, &store, host.as_ref(), &id, &request).await
    })
}

/// [`set_up_branch`] under `guard`, waited for.
///
/// A caller gets an answer to what it asked. The run it joins may have been
/// started for another reason and ended with nothing done: an `Observed` run
/// that found the row no longer `Starting`, a repair whose clone read right.
/// Such an end is not this caller's: it looks at the row once more and, if
/// its own trigger still has something to do, runs it.
pub(crate) async fn run_guarded(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &Arc<dyn SetupHost>,
    branch_id: &str,
    request: SetupRequest,
) -> Result<SetupOutcome, String> {
    for _ in 0..2 {
        let branch = store
            .get_branch(branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
        // Checked before taking a slot in the guard, so a trigger with
        // nothing to do never becomes the run a real request joins.
        if !wants_setup(&branch, request.trigger) {
            break;
        }
        let workspace = branch
            .workspace_name
            .as_deref()
            .ok_or("Branch has no workspace name")?;
        let run = start_run(guard, store, host, workspace, branch_id, request.clone());
        let joined = run.joined;
        match run.end().await? {
            SetupOutcome::NothingToDo if joined => {}
            outcome => return Ok(outcome),
        }
    }
    Ok(SetupOutcome::NothingToDo)
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
/// `Error` with its reason recorded and, unless the checkout refused over
/// what is in the clone, a retry timer set. One exception: a repair of a
/// `Running` branch that was refused (a worktree with changes in the way,
/// commits that would be left behind, a live session in the clone) puts the
/// row back to `Running`. The branch was usable before the session that
/// asked came along and is exactly as usable now; only that session fails,
/// with the reason.
///
/// Callers go through [`run_guarded`] or [`request_setup`].
pub(crate) async fn set_up_branch(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &dyn SetupHost,
    branch_id: &str,
    request: &SetupRequest,
) -> Result<SetupOutcome, String> {
    // A transfer provisions its own branches; refusing leaves the row alone.
    crate::project_transfer::ensure_branch_not_transferring(branch_id)?;

    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    // A retry that finds the row `Starting` was joined, while it waited for
    // its turn, by the user's Retry or a resume. It is that run now.
    let trigger = match (request.trigger, &branch.workspace_status) {
        (SetupTrigger::Retry, Some(WorkspaceStatus::Starting)) => SetupTrigger::Observed,
        (trigger, _) => trigger,
    };
    // Read again now that the workstation's turn has come: the run ahead of
    // this one may have settled the branch.
    if !wants_setup(&branch, trigger) {
        return Ok(SetupOutcome::NothingToDo);
    }
    let was_running = branch.workspace_status == Some(WorkspaceStatus::Running);

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

    // A run someone or something asked for puts a branch that had settled
    // somewhere else (a stale `starting` in the window that asked, a repair
    // of a `Running` one) back in `Starting`, where the pollers hold it
    // rather than promote it under this run and the session queue holds
    // whatever else would start on the branch. A retry on its own does
    // neither: the card would flip to a spinner and lose its reason for an
    // attempt nobody is watching. `Error` holds the queue as well.
    if trigger != SetupTrigger::Retry {
        if branch.workspace_status != Some(WorkspaceStatus::Starting) {
            store
                .update_branch_workspace_status(branch_id, &WorkspaceStatus::Starting)
                .map_err(|e| e.to_string())?;
        }
        host.failure_changed(branch_id, None, false);
    }

    // A repair of a row that is not `Running` is the setup that row was
    // waiting for, and provisions like one.
    let may_start_workstation =
        trigger.may_start_workstation() || (trigger == SetupTrigger::Repair && !was_running);
    let provisioned =
        provision_and_check_out(guard, store, host, &branch, request, may_start_workstation).await;
    let error = match provisioned {
        Ok(()) => {
            let changed = store
                .update_branch_workspace_status(branch_id, &WorkspaceStatus::Running)
                .map_err(|e| e.to_string())?;
            host.status_written(branch_id, changed, &WorkspaceStatus::Running);
            host.failure_changed(branch_id, None, false);
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
    // Only over `Starting`, or the `Error` a retry left in place. A poll
    // that saw the workstation suspended or gone while this ran has already
    // said something truer than `Error`, and with a way forward (Resume).
    let row = store.get_branch(branch_id).ok().flatten();
    match row.and_then(|row| row.workspace_status) {
        Some(WorkspaceStatus::Starting)
            if trigger == SetupTrigger::Repair && was_running && error.is_refusal() =>
        {
            // Nothing was touched, so the branch is what it was. The write
            // kicks the session queue, which held while the row said
            // `Starting`.
            if let Ok(changed) =
                store.update_branch_workspace_status(branch_id, &WorkspaceStatus::Running)
            {
                host.status_written(branch_id, changed, &WorkspaceStatus::Running);
            }
        }
        Some(status @ (WorkspaceStatus::Starting | WorkspaceStatus::Error)) => {
            if status == WorkspaceStatus::Starting {
                store
                    .update_branch_workspace_status(branch_id, &WorkspaceStatus::Error)
                    .ok();
            }
            let user_asked = guard.take_user_asked(branch_id);
            host.failure_changed(branch_id, Some(error.message()), user_asked);
            if error.worth_retrying() {
                retry::schedule(guard, host, branch_id);
            } else {
                // Left to the error view's Retry.
                guard.forget_retries(branch_id);
            }
        }
        _ => {}
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
        wait_until_running(store, host, branch, workspace, "it was still booting")
            .await
            .map_err(failed)?;
    } else if may_start_workstation {
        start_workstation(store, host, branch, &checkout)
            .await
            .map_err(failed)?;
    } else {
        // A repair or a retry found the workstation down. The clone is as
        // it was, and what the row should say about a workstation that
        // isn't running is the pollers' to settle.
        return Err(CheckoutError::Blocked(format!(
            "Workspace '{workspace}' is not running (Blox reports it as {}), so branch '{}' \
             could not be checked out in it",
            reported.as_deref().unwrap_or("unknown"),
            checkout.branch_name
        )));
    }

    check_out_in_clone(guard, store, host, &checkout, request).await
}
