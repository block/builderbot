//! `kick(branch_id)`: ask the backend to move a branch's queue along.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tauri::Manager;

use crate::store::Store;

/// Why a branch was kicked; logged per kick so a stuck queue can be traced
/// to the event that should have moved it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KickSource {
    /// A session on the branch reached a terminal state.
    TerminalState,
    /// An immediate start failed or was superseded before it launched an agent.
    StartFailed,
    /// A Stop on a row with no live token took the store-write fallback.
    CancelFallback,
    /// A note, review or pending commit was deleted.
    ArtifactDeleted,
    /// A session row was deleted outright.
    SessionDeleted,
    /// An immediate (non-pipeline) pull released its marker.
    ImmediatePull,
    /// A project-session MCP tool enqueued or cancelled branch work.
    Mcp,
    /// The branch's worktree was created and linked.
    WorktreeCreated,
    /// The branch's remote workspace reached `Running`.
    WorkspaceRunning,
    /// App startup found queued rows on the branch.
    Startup,
    /// Startup recovery ended a row owned by a dead process.
    DeadOwnerRecovered,
    /// The stranded-session sweep ended a row on the branch.
    Sweep,
}

impl KickSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::TerminalState => "terminal state",
            Self::StartFailed => "start failed",
            Self::CancelFallback => "cancel fallback",
            Self::ArtifactDeleted => "artifact deleted",
            Self::SessionDeleted => "session deleted",
            Self::ImmediatePull => "immediate pull",
            Self::Mcp => "mcp",
            Self::WorktreeCreated => "worktree created",
            Self::WorkspaceRunning => "workspace running",
            Self::Startup => "startup",
            Self::DeadOwnerRecovered => "dead owner recovered",
            Self::Sweep => "sweep",
        }
    }
}

type DrainFuture = Pin<Box<dyn Future<Output = Result<bool, String>> + Send>>;
/// The drain a kick runs: given a branch id, start what can start. Injected
/// so the coalescing can be tested without the session runner.
pub(crate) type DrainFn = Arc<dyn Fn(String) -> DrainFuture + Send + Sync>;

/// Per-branch coalescing of kicks, managed as app state.
///
/// One drain runs per branch at a time. A kick that arrives while a drain is
/// in flight marks the branch for one more pass — not one per kick — because
/// a drain reads the queue fresh each time, so every event that landed during
/// it is covered by the single rerun.
pub(crate) struct BranchQueueKicker {
    drain: DrainFn,
    /// Branches with a drain in flight, and whether a rerun was requested.
    in_flight: Mutex<HashMap<String, bool>>,
    #[cfg(test)]
    kicks: Mutex<Vec<(String, KickSource)>>,
}

impl BranchQueueKicker {
    pub(crate) fn new(drain: DrainFn) -> Self {
        Self {
            drain,
            in_flight: Mutex::new(HashMap::new()),
            #[cfg(test)]
            kicks: Mutex::new(Vec::new()),
        }
    }

    /// The production kicker: drains through
    /// [`crate::session_commands::drain_branch_queue`], resolving the store and
    /// registry from app state at drain time so it is valid across the
    /// needs-reset flow.
    pub(crate) fn for_app(app_handle: tauri::AppHandle) -> Self {
        Self::new(Arc::new(move |branch_id: String| {
            let app_handle = app_handle.clone();
            Box::pin(async move {
                let store = crate::get_store(&app_handle.state::<Mutex<Option<Arc<Store>>>>())?;
                let registry =
                    Arc::clone(&app_handle.state::<Arc<crate::session_runner::SessionRegistry>>());
                crate::session_commands::drain_branch_queue(
                    store, registry, app_handle, branch_id, None,
                )
                .await
            })
        }))
    }

    /// Kick `branch_id`: start a drain, or fold into the one in flight.
    pub(crate) fn kick(self: &Arc<Self>, branch_id: &str, source: KickSource) {
        #[cfg(test)]
        self.kicks
            .lock()
            .unwrap()
            .push((branch_id.to_string(), source));

        {
            let mut in_flight = self.in_flight.lock().unwrap();
            if let Some(rerun) = in_flight.get_mut(branch_id) {
                log::debug!(
                    "[session_queue] kick for branch {branch_id} ({}) folded into the drain in flight",
                    source.as_str()
                );
                *rerun = true;
                return;
            }
            in_flight.insert(branch_id.to_string(), false);
        }
        log::info!(
            "[session_queue] kick for branch {branch_id} ({})",
            source.as_str()
        );

        let kicker = Arc::clone(self);
        let branch_id = branch_id.to_string();
        tauri::async_runtime::spawn(async move {
            loop {
                match (kicker.drain)(branch_id.clone()).await {
                    Ok(true) => {
                        log::info!("[session_queue] started queued work on branch {branch_id}")
                    }
                    Ok(false) => {}
                    Err(e) => {
                        log::error!("[session_queue] drain failed for branch {branch_id}: {e}")
                    }
                }
                let mut in_flight = kicker.in_flight.lock().unwrap();
                match in_flight.get_mut(&branch_id) {
                    Some(rerun) if *rerun => *rerun = false,
                    _ => {
                        in_flight.remove(&branch_id);
                        break;
                    }
                }
            }
        });
    }

    /// A kicker whose drain does nothing, for tests that only need to know
    /// which branches were kicked.
    #[cfg(test)]
    pub(crate) fn recording() -> Self {
        Self::new(Arc::new(|_| Box::pin(async { Ok(false) })))
    }

    /// Every kick so far, oldest first.
    #[cfg(test)]
    pub(crate) fn kicked(&self) -> Vec<(String, KickSource)> {
        self.kicks.lock().unwrap().clone()
    }

    /// The branch ids kicked so far, oldest first.
    #[cfg(test)]
    pub(crate) fn kicked_branches(&self) -> Vec<String> {
        self.kicked()
            .into_iter()
            .map(|(branch, _)| branch)
            .collect()
    }

    /// Wait until no drain is in flight for any branch.
    #[cfg(test)]
    pub(crate) async fn wait_until_idle(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !self.in_flight.lock().unwrap().is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "kicker still draining after 10s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
}

/// Kick `branch_id` through the app's managed [`BranchQueueKicker`].
///
/// Generic over the runtime so the shared command bodies that mock-app tests
/// drive (`cancel_session_impl`, the delete bodies) can call it. A handle
/// with no kicker managed — a mock app that didn't install one — logs and
/// does nothing.
pub(crate) fn kick<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    branch_id: &str,
    source: KickSource,
) {
    match app_handle.try_state::<Arc<BranchQueueKicker>>() {
        Some(kicker) => kicker.kick(branch_id, source),
        None => log::debug!(
            "[session_queue] no kicker managed; dropping kick for branch {branch_id} ({})",
            source.as_str()
        ),
    }
}

/// Kick every branch that has queued rows.
///
/// The startup pass: queues left parked by a previous run, or by a client
/// that never reached its own drain, move without anyone connected.
pub(crate) fn kick_all_with_queued_work<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Store,
    source: KickSource,
) {
    let branch_ids = match store.branch_ids_with_queued_sessions() {
        Ok(ids) => ids,
        Err(e) => {
            log::warn!("[session_queue] failed to list branches with queued sessions: {e}");
            return;
        }
    };
    if !branch_ids.is_empty() {
        log::info!(
            "[session_queue] {} branch(es) have queued sessions at {}",
            branch_ids.len(),
            source.as_str()
        );
    }
    for branch_id in branch_ids {
        kick(app_handle, &branch_id, source);
    }
}
