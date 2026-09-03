//! `kick(branch_id)`: ask the backend to move a branch's queue along.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::Manager;

use super::retry::QueuedStartAttempts;
use crate::store::Store;

/// Why a branch was kicked; logged per kick so a stuck queue can be traced
/// to the event that should have moved it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KickSource {
    /// A session on the branch reached a terminal state.
    TerminalState,
    /// An immediate start failed or was superseded before it launched an agent.
    StartFailed,
    /// A queued row's start failed after the claim had made it `running`.
    ClaimedStartFailed,
    /// A queued row's start failed before its claim and the delay before its
    /// next attempt has passed.
    StartRetry,
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
    /// Launch resume put rows the last quit ended back on the branch queue.
    LaunchResume,
    /// The reconcile service settled a `disconnected` Blox row on the
    /// branch.
    Reconciled,
    /// The branch's project moved between local and remote.
    ProjectTransferred,
    /// A transfer of the branch's project failed or was cancelled before
    /// it moved: the work held for the transfer starts where the project
    /// still is.
    TransferFailed,
    /// A project-note session in the branch's project reached a terminal
    /// state; a deferred push on the branch may have been waiting for it.
    ProjectSessionEnded,
    /// A rate-limited session paused on the branch and is waiting to
    /// auto-resume; its queued work must not wait with it.
    RateLimitRecovery,
}

impl KickSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::TerminalState => "terminal state",
            Self::StartFailed => "start failed",
            Self::ClaimedStartFailed => "claimed start failed",
            Self::StartRetry => "start retry",
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
            Self::LaunchResume => "launch resume",
            Self::Reconciled => "reconciled",
            Self::ProjectTransferred => "project transferred",
            Self::TransferFailed => "transfer failed",
            Self::ProjectSessionEnded => "project session ended",
            Self::RateLimitRecovery => "rate limit recovery",
        }
    }
}

type DrainFuture = Pin<Box<dyn Future<Output = Result<bool, String>> + Send>>;
/// The drain a kick runs: given a branch id, start what can start. Injected
/// so the coalescing can be tested without the session runner.
pub(crate) type DrainFn = Arc<dyn Fn(String) -> DrainFuture + Send + Sync>;
/// What a kick's companion for the replies held while a branch's project
/// was transferred runs: given a branch id, send the follow-ups held for it
/// that the branch can take now (see [`super::follow_ups`]). Injected like
/// the drain, so the provisioning paths — generic over the runtime for the
/// mock-app tests — can ask for it without the session runner.
pub(crate) type FlushFn = Arc<dyn Fn(String) + Send + Sync>;

/// Per-branch coalescing of kicks, managed as app state.
///
/// One drain runs per branch at a time. A kick that arrives while a drain is
/// in flight marks the branch for one more pass — not one per kick — because
/// a drain reads the queue fresh each time, so every event that landed during
/// it is covered by the single rerun.
pub(crate) struct BranchQueueKicker {
    drain: DrainFn,
    flush: FlushFn,
    /// Branches with a drain in flight, and whether a rerun was requested.
    in_flight: Mutex<HashMap<String, bool>>,
    /// Queued rows whose pre-claim start has failed, and how often; the
    /// drain consults it to retry a bounded number of times before ending
    /// the row (see [`super::retry`]).
    start_attempts: QueuedStartAttempts,
    #[cfg(test)]
    kicks: Mutex<Vec<(String, KickSource)>>,
    #[cfg(test)]
    scheduled: Mutex<Vec<(String, Duration, KickSource)>>,
    #[cfg(test)]
    flushes: Mutex<Vec<String>>,
}

impl BranchQueueKicker {
    /// A kicker that drains with `drain` and has nothing to send held
    /// follow-ups with: a flush only records and does nothing.
    #[cfg(test)]
    pub(crate) fn new(drain: DrainFn) -> Self {
        Self::with_flush(drain, Arc::new(|_| {}))
    }

    pub(crate) fn with_flush(drain: DrainFn, flush: FlushFn) -> Self {
        Self {
            drain,
            flush,
            in_flight: Mutex::new(HashMap::new()),
            start_attempts: QueuedStartAttempts::default(),
            #[cfg(test)]
            kicks: Mutex::new(Vec::new()),
            #[cfg(test)]
            scheduled: Mutex::new(Vec::new()),
            #[cfg(test)]
            flushes: Mutex::new(Vec::new()),
        }
    }

    /// The drain's record of failed pre-claim starts.
    pub(crate) fn start_attempts(&self) -> &QueuedStartAttempts {
        &self.start_attempts
    }

    /// The production kicker: drains through
    /// [`crate::session_commands::drain_branch_queue`], resolving the store and
    /// registry from app state at drain time so it is valid across the
    /// needs-reset flow.
    pub(crate) fn for_app(app_handle: tauri::AppHandle) -> Self {
        let flush_app = app_handle.clone();
        Self::with_flush(
            Arc::new(move |branch_id: String| {
                let app_handle = app_handle.clone();
                Box::pin(async move {
                    let store = crate::get_store(&app_handle.state::<Mutex<Option<Arc<Store>>>>())?;
                    let registry = Arc::clone(
                        &app_handle.state::<Arc<crate::session_runner::SessionRegistry>>(),
                    );
                    crate::session_commands::drain_branch_queue(
                        store, registry, app_handle, branch_id, None,
                    )
                    .await
                })
            }),
            Arc::new(move |branch_id: String| {
                let store = match crate::get_store(&flush_app.state::<Mutex<Option<Arc<Store>>>>())
                {
                    Ok(store) => store,
                    Err(e) => {
                        log::warn!(
                            "[session_queue] cannot flush the follow-ups held on branch \
                             {branch_id}: {e}"
                        );
                        return;
                    }
                };
                super::follow_ups::flush_follow_ups_for_branch(&flush_app, &store, &branch_id);
            }),
        )
    }

    /// Send the follow-ups held for `branch_id` while its project was
    /// transferred, if the branch can take them now.
    pub(crate) fn flush_follow_ups(&self, branch_id: &str) {
        #[cfg(test)]
        self.flushes.lock().unwrap().push(branch_id.to_string());
        (self.flush)(branch_id.to_string());
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
                // The drain runs on a task of its own, so a panic inside it
                // (an `unwrap` on a row the store handed back malformed, say)
                // comes back here as a `JoinError` and is handled like any
                // other failed drain. Were it to unwind *this* task instead,
                // the `in_flight` entry below would never be removed and
                // every later kick for the branch would be "folded into the
                // drain in flight" forever — the parked queue this module
                // exists to remove, now with no client drain to paper over
                // it.
                let drained =
                    match tauri::async_runtime::spawn((kicker.drain)(branch_id.clone())).await {
                        Ok(drained) => drained,
                        Err(e) => Err(format!("drain panicked: {e}")),
                    };
                match drained {
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

    /// Kick `branch_id` once `delay` has passed. The drain schedules these
    /// for a queued row whose start failed before its claim: nothing else
    /// would re-kick a provisioned branch for such a row, so the retry has
    /// to carry its own kick.
    pub(crate) fn kick_after(
        self: &Arc<Self>,
        branch_id: &str,
        delay: Duration,
        source: KickSource,
    ) {
        #[cfg(test)]
        self.scheduled
            .lock()
            .unwrap()
            .push((branch_id.to_string(), delay, source));

        log::info!(
            "[session_queue] kick for branch {branch_id} ({}) scheduled in {delay:?}",
            source.as_str()
        );
        let kicker = Arc::clone(self);
        let branch_id = branch_id.to_string();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(delay).await;
            kicker.kick(&branch_id, source);
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

    /// Every delayed kick scheduled so far, oldest first.
    #[cfg(test)]
    pub(crate) fn scheduled(&self) -> Vec<(String, Duration, KickSource)> {
        self.scheduled.lock().unwrap().clone()
    }

    /// The branches whose held follow-ups were asked for so far, oldest
    /// first.
    #[cfg(test)]
    pub(crate) fn flushed(&self) -> Vec<String> {
        self.flushes.lock().unwrap().clone()
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

/// Send the follow-ups held for `branch_id` through the app's managed
/// [`BranchQueueKicker`], the way [`kick`] kicks: the provisioning paths
/// that call both are generic over the runtime, and only the kicker holds
/// the handle the runner needs. With no kicker managed, logs and does
/// nothing.
pub(crate) fn flush_follow_ups<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    branch_id: &str,
) {
    match app_handle.try_state::<Arc<BranchQueueKicker>>() {
        Some(kicker) => kicker.flush_follow_ups(branch_id),
        None => log::debug!(
            "[session_queue] no kicker managed; leaving the follow-ups held on branch \
             {branch_id} queued"
        ),
    }
}

/// Kick every branch with a deferred push ("Push after current work") armed,
/// within `project_id` when given.
///
/// The drain's tail fires the push once the branch and its project are idle,
/// so this is how a project-note session's end — which no branch kick sees —
/// reaches the branches waiting on it.
pub(crate) fn kick_branches_with_deferred_push<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Store,
    project_id: Option<&str>,
    source: KickSource,
) {
    let branch_ids = match store.branch_ids_with_deferred_push(project_id) {
        Ok(ids) => ids,
        Err(e) => {
            log::warn!("[session_queue] failed to list branches with a deferred push: {e}");
            return;
        }
    };
    if !branch_ids.is_empty() {
        log::info!(
            "[session_queue] {} branch(es) have a deferred push at {}",
            branch_ids.len(),
            source.as_str()
        );
    }
    for branch_id in branch_ids {
        kick(app_handle, &branch_id, source);
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
