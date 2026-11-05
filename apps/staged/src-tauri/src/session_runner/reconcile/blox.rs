//! What reconcile asks of Blox, behind [`ReconcileBlox`] so the decisions can
//! be tested without a workstation. [`LiveBlox`] is the `sq` implementation.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use acp_client::{AttachOutcome, CancelOutcome, DetachHandle, RecordedTurn, RemoteProcess};
use tokio_util::sync::CancellationToken;

use crate::agent::{AcpDriver, MessageWriter};
use crate::store::{Session, Store};

/// A `disconnected` session, resolved to everything reconciling it needs.
#[derive(Debug, Clone)]
pub(crate) struct ReconcileTarget {
    pub session_id: String,
    pub workspace_name: String,
    pub provider: Option<String>,
    /// The ACP session the turn runs in.
    pub agent_session_id: String,
    /// The Blox process the turn was pinned to, once known.
    pub command_id: Option<String>,
    pub working_dir: PathBuf,
    /// The branch's checkout on the workstation; resolved after the
    /// workstation is known to be running, since that takes a Blox call.
    pub remote_working_dir: Option<PathBuf>,
    /// The repo subpath `remote_working_dir` is resolved from.
    pub workspace_subpath: Option<String>,
    pub pre_head_sha: Option<String>,
    pub branch_id: Option<String>,
    pub project_id: Option<String>,
    pub session_type: Option<String>,
}

/// Why a `disconnected` row can't be reconciled at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Unresolvable {
    /// Its branch is gone, or no longer on a workstation.
    NoWorkspace,
    /// It never got an ACP session to attach to.
    NoAgentSession,
}

impl ReconcileTarget {
    pub(super) fn resolve(store: &Store, session: &Session) -> Result<Self, Unresolvable> {
        let info = crate::session_commands::project_active_session(store, session);
        let branch = info
            .branch_id
            .as_deref()
            .and_then(|id| store.get_branch(id).ok().flatten());
        let Some(workspace_name) = branch.as_ref().and_then(|b| b.workspace_name.clone()) else {
            return Err(Unresolvable::NoWorkspace);
        };
        let Some(agent_session_id) =
            crate::session_commands::resumable_agent_session_id(session.agent_id.as_deref())
        else {
            return Err(Unresolvable::NoAgentSession);
        };
        let workspace_subpath = branch.as_ref().and_then(|branch| {
            crate::branches::resolve_branch_workspace_subpath(store, branch)
                .ok()
                .flatten()
        });
        Ok(Self {
            session_id: session.id.clone(),
            workspace_name,
            provider: session.provider.clone(),
            agent_session_id,
            command_id: session.blox_command_id.clone(),
            working_dir: PathBuf::from(&session.working_dir),
            remote_working_dir: None,
            workspace_subpath,
            pre_head_sha: session.pre_head_sha.clone(),
            branch_id: info.branch_id,
            project_id: info.project_id,
            session_type: info.session_type,
        })
    }

    /// Resolve [`Self::remote_working_dir`] from the branch's subpath, if it
    /// isn't yet: an attach, and a cancel's backfill, load the session in
    /// that checkout. A Blox call; only worth making once the workstation is
    /// known to be running. A branch with no project repo has no subpath
    /// and nothing to resolve.
    pub(super) fn resolve_remote_working_dir(
        &mut self,
        blox: &dyn ReconcileBlox,
    ) -> Result<(), String> {
        if self.remote_working_dir.is_some() {
            return Ok(());
        }
        let Some(subpath) = self.workspace_subpath.as_deref() else {
            return Ok(());
        };
        self.remote_working_dir = Some(blox.remote_working_dir(&self.workspace_name, subpath)?);
        Ok(())
    }

    /// The routing half of a target, for ending a row that
    /// [`Self::resolve`] refused: the row's events still need its branch and
    /// project.
    pub(super) fn unresolved(store: &Store, session: &Session) -> Self {
        let info = crate::session_commands::project_active_session(store, session);
        Self {
            session_id: session.id.clone(),
            workspace_name: String::new(),
            provider: None,
            agent_session_id: String::new(),
            command_id: None,
            working_dir: PathBuf::from(&session.working_dir),
            remote_working_dir: None,
            workspace_subpath: None,
            pre_head_sha: None,
            branch_id: info.branch_id,
            project_id: info.project_id,
            session_type: info.session_type,
        }
    }
}

/// What `ws info` says about a workstation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceProbe {
    Running,
    Suspended,
    /// Deleted, or not found at all.
    Deleted,
    /// Any other state (starting, stopped, degraded, ...), as Blox names it.
    NotRunning(String),
    /// Blox couldn't be asked.
    Unreachable {
        kind: UnreachableKind,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnreachableKind {
    /// No answer in time, `sq` missing, or an error other than the two below.
    Unreachable,
    NotAuthenticated,
}

/// How an attach is steered from outside while it follows a turn.
pub(crate) struct AttachControl {
    /// Fires on a Stop, or on a quit that couldn't detach: the attach lets go
    /// of the turn (without stopping it) and returns.
    pub token: CancellationToken,
    /// Receives the attach's detach handle once it is connected, so a quit
    /// can let go of the turn the way it does for any remote session.
    pub on_detach_handle: Box<dyn Fn(DetachHandle) + Send + Sync>,
    /// Called once the attach has caught the transcript up and follows the
    /// session's live stream: the turn is reattached. Not called for an
    /// attach that ends before that.
    pub on_connected: Box<dyn Fn() + Send + Sync>,
}

/// What a cancel catches the transcript up through, and who hears the
/// cancel's outcome before it does (see [`ReconcileBlox::cancel`]).
pub(crate) struct Backfill<'a> {
    pub store: &'a Arc<Store>,
    /// Told the cancel's outcome as soon as it is known, before the backfill
    /// starts; not called when the cancel fails outright.
    pub on_cancelled: Box<dyn FnOnce(&CancelOutcome) + Send + 'a>,
}

/// Blox, as reconcile uses it. Every method blocks.
pub(crate) trait ReconcileBlox: Send + Sync {
    /// `ws info` for the workstation.
    fn workspace(&self, workspace_name: &str) -> WorkspaceProbe;

    /// The branch checkout on the workstation, from its `home:` subpath.
    fn remote_working_dir(&self, workspace_name: &str, subpath: &str) -> Result<PathBuf, String>;

    /// The session's remote process: the pinned one, else the newest running
    /// the session's argv.
    fn process(&self, target: &ReconcileTarget) -> Result<RemoteProcess, String>;

    /// How the session's last turn stands in process `command_id`'s
    /// recorded output.
    fn recorded_turn(
        &self,
        target: &ReconcileTarget,
        command_id: &str,
    ) -> Result<RecordedTurn, String>;

    /// Attach to the session, catch its transcript up through `store`, and
    /// follow its turn (if one is running) until it ends or `control.token`
    /// fires.
    fn attach(
        &self,
        target: &ReconcileTarget,
        store: &Arc<Store>,
        control: &AttachControl,
    ) -> Result<AttachOutcome, String>;

    /// Cancel the session's turn over a fresh connection, waiting up to
    /// `deadline` for it to stop. With `backfill`, the transcript is then
    /// caught up through it with the turn the cancel ended, the end of the
    /// tool call that was running included (see
    /// `AcpDriver::attach_cancel_and_catch_up`); best effort, it never
    /// changes the outcome. Returns once the backfill is over too.
    fn cancel(
        &self,
        target: &ReconcileTarget,
        backfill: Option<Backfill<'_>>,
        deadline: Duration,
    ) -> Result<CancelOutcome, String>;

    /// The workstation the post-completion hooks read the checkout's HEAD
    /// on; `None` reads `target.working_dir` locally.
    fn checkout_workspace<'a>(&self, target: &'a ReconcileTarget) -> Option<&'a str>;
}

/// [`ReconcileBlox`] over `sq`.
pub(crate) struct LiveBlox;

impl LiveBlox {
    fn driver(target: &ReconcileTarget) -> Result<AcpDriver, String> {
        let mut driver = AcpDriver::for_workspace(
            &target.workspace_name,
            target.provider.as_deref(),
            &target.session_id,
        )?;
        if let Some(dir) = &target.remote_working_dir {
            driver = driver.with_remote_working_dir(dir.clone());
        }
        Ok(driver)
    }
}

/// Run `future` to completion on a fresh single-threaded runtime inside a
/// `LocalSet`, as the ACP driver needs.
fn block_on_local<F: std::future::Future>(future: F) -> Result<F::Output, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Failed to start a runtime for the attach: {e}"))?;
    let local = tokio::task::LocalSet::new();
    Ok(local.block_on(&runtime, future))
}

pub(super) fn probe_from_ws_info(
    result: Result<crate::blox::WorkspaceInfo, crate::blox::BloxError>,
) -> WorkspaceProbe {
    use crate::blox::BloxError;
    match result {
        Ok(info) => match info.status.as_deref() {
            Some("running") => WorkspaceProbe::Running,
            Some("suspended") => WorkspaceProbe::Suspended,
            Some("deleted") => WorkspaceProbe::Deleted,
            other => WorkspaceProbe::NotRunning(other.unwrap_or("unknown").to_string()),
        },
        Err(BloxError::CommandFailed(message)) if message.to_lowercase().contains("not found") => {
            WorkspaceProbe::Deleted
        }
        Err(BloxError::NotAuthenticated) => WorkspaceProbe::Unreachable {
            kind: UnreachableKind::NotAuthenticated,
            message: BloxError::NotAuthenticated.to_string(),
        },
        Err(e) => WorkspaceProbe::Unreachable {
            kind: UnreachableKind::Unreachable,
            message: e.to_string(),
        },
    }
}

impl ReconcileBlox for LiveBlox {
    fn workspace(&self, workspace_name: &str) -> WorkspaceProbe {
        // `blox_cli` directly, not `crate::blox`: a successful probe must not
        // nudge the very reconcile that made it.
        probe_from_ws_info(blox_cli::ws_info(workspace_name))
    }

    fn remote_working_dir(&self, workspace_name: &str, subpath: &str) -> Result<PathBuf, String> {
        crate::branches::resolve_workspace_repo_path(workspace_name, subpath)
            .map(PathBuf::from)
            .map_err(|e| e.to_string())
    }

    fn process(&self, target: &ReconcileTarget) -> Result<RemoteProcess, String> {
        Self::driver(target)?
            .locate_remote_process(target.command_id.as_deref())
            .unwrap_or_else(|| Err("Not a Blox session".to_string()))
    }

    fn recorded_turn(
        &self,
        target: &ReconcileTarget,
        command_id: &str,
    ) -> Result<RecordedTurn, String> {
        Self::driver(target)?
            .recorded_turn(command_id, &target.agent_session_id)
            .unwrap_or_else(|| Err("Not a Blox session".to_string()))
    }

    fn attach(
        &self,
        target: &ReconcileTarget,
        store: &Arc<Store>,
        control: &AttachControl,
    ) -> Result<AttachOutcome, String> {
        let driver = Self::driver(target)?;
        block_on_local(async {
            let store_trait: Arc<dyn acp_client::Store> = store.clone();
            let writer: Arc<dyn acp_client::MessageWriter> = Arc::new(MessageWriter::catching_up(
                target.session_id.clone(),
                Arc::clone(store),
            ));
            let mut attached = driver.attach(
                &target.session_id,
                &target.agent_session_id,
                target.command_id.as_deref(),
                &target.working_dir,
                &store_trait,
                &writer,
                None,
            )?;
            let detach = attached.detach_handle();
            (control.on_detach_handle)(detach.clone());
            let caught_up = tokio::select! {
                caught_up = attached.caught_up() => caught_up,
                () = control.token.cancelled() => false,
            };
            if caught_up {
                (control.on_connected)();
            }
            let settled = attached.settled();
            tokio::pin!(settled);
            tokio::select! {
                outcome = &mut settled => outcome,
                () = control.token.cancelled() => {
                    // Let go without stopping the turn; whoever fired the
                    // token decides what happens to it.
                    detach.detach();
                    settled.await
                }
            }
        })?
    }

    fn cancel(
        &self,
        target: &ReconcileTarget,
        backfill: Option<Backfill<'_>>,
        deadline: Duration,
    ) -> Result<CancelOutcome, String> {
        let driver = Self::driver(target)?;
        block_on_local(async {
            let Some(Backfill {
                store,
                on_cancelled,
            }) = backfill
            else {
                return driver
                    .attach_cancel(
                        &target.session_id,
                        &target.agent_session_id,
                        target.command_id.as_deref(),
                        &target.working_dir,
                        deadline,
                    )
                    .await;
            };
            let store_trait: Arc<dyn acp_client::Store> = store.clone();
            let writer: Arc<dyn acp_client::MessageWriter> = Arc::new(MessageWriter::catching_up(
                target.session_id.clone(),
                Arc::clone(store),
            ));
            driver
                .attach_cancel_and_catch_up(
                    &target.session_id,
                    &target.agent_session_id,
                    target.command_id.as_deref(),
                    &target.working_dir,
                    &store_trait,
                    &writer,
                    deadline,
                    on_cancelled,
                )
                .await
        })?
    }

    fn checkout_workspace<'a>(&self, target: &'a ReconcileTarget) -> Option<&'a str> {
        Some(&target.workspace_name)
    }
}
