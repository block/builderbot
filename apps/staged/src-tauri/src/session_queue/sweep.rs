//! The stranded-`running` sweep: the safety net under the queue.
//!
//! A `running` row is the branch's busy marker, and its release is a terminal
//! write by whoever owns the work. When that owner is gone — the request
//! future that inserted it was dropped, the task panicked — the row stays
//! `running` with a live `owner_pid` and nothing times it out: every later
//! note, commit, review and push on the branch queues behind it and "Start
//! now" is refused. The startup dead-pid pass doesn't catch it because the
//! pid is this process.
//!
//! The sweep defines a stranded row structurally, with no clock: owned by
//! this process and not live in the [`SessionRegistry`]. That is sound
//! because of one invariant every writer of a `running` row keeps — **the row
//! has a registry entry for its entire time in `running`**. Each owner takes
//! its entry (a full `register` for a thread that observes a cancel token, a
//! [`SessionRegistry::hold`] for a task that only finishes the row) *before*
//! the write that makes the row `running`, and releases it *after* the
//! terminal write. A session thread's entry therefore spans its
//! post-completion hooks too, however long they take; a turn is never "quiet
//! enough" to be mistaken for stranded, because quietness is not what is
//! asked.
//!
//! Every row the sweep ends is a `warn` in the log, and [`queue_health`]
//! gives the Doctor the same view — what is stranded right now, and how many
//! rows have been swept so far — so a recurrence shows up in the app rather
//! than only in the log.

use std::sync::Arc;
use std::time::Duration;

use crate::session_runner::SessionRegistry;
use crate::store::{CompletionReason, Session, SessionStatus, Store};

use super::{kick, KickSource};

/// How often the sweep runs. Also how late it can be: a stranded row is ended
/// within one interval of being stranded.
const SWEEP_INTERVAL_SECS: u64 = 60;

/// The error recorded on a row the sweep ends.
pub(crate) const STRANDED_SESSION_MESSAGE: &str = "Staged lost track of this session";

/// Whether `session` is a row nothing will finish.
///
/// Pure: `live` is the registry's answer for the row ([`SessionRegistry::is_live`])
/// and `own_pid` is this process. Rows under other pids belong to another
/// instance on the same data dir and are left alone, like everywhere else.
pub(crate) fn is_stranded(session: &Session, live: bool, own_pid: u32) -> bool {
    session.status == SessionStatus::Running && session.owner_pid == Some(own_pid) && !live
}

/// End every stranded row this process owns and kick the branches they held.
///
/// Returns the ids of the rows it ended. The row is read before the registry
/// is asked, and the terminal write is a `transition_from_running` CAS, which
/// together make the only benign interleaving harmless: an owner that wrote
/// its terminal state and deregistered between the two reads has left the
/// row no longer `running`, so the CAS finds nothing to do. (The other order
/// would not be safe — a row read as `running` *after* the registry said
/// "not live" could be a fresh start whose entry was taken between the two.)
pub(crate) fn reconcile_stranded_running_sessions<R: tauri::Runtime>(
    store: &Arc<Store>,
    registry: &SessionRegistry,
    app_handle: &tauri::AppHandle<R>,
) -> Vec<String> {
    let running = match store.get_running_sessions() {
        Ok(sessions) => sessions,
        Err(e) => {
            log::warn!("[session_queue] sweep failed to query running sessions: {e}");
            return Vec::new();
        }
    };
    let own_pid = std::process::id();

    let mut ended = Vec::new();
    for session in running {
        if !is_stranded(&session, registry.is_live(&session.id), own_pid) {
            continue;
        }
        let transitioned = store
            .transition_from_running(
                &session.id,
                SessionStatus::Error,
                Some(STRANDED_SESSION_MESSAGE),
                Some(&CompletionReason::Crashed),
            )
            .unwrap_or(false);
        if !transitioned {
            // The owner finished between our row read and our registry read;
            // see the function doc. Nothing was stranded.
            log::debug!(
                "[session_queue] sweep skipped session {}: its owner finished it first",
                session.id
            );
            continue;
        }
        let pending_start =
            crate::session_commands::pending_branch_start_agent_id(session.agent_id.as_deref())
                .is_some();
        log::warn!(
            "[session_queue] ended stranded session {} (branch {:?}, running since {}, {})",
            session.id,
            session.branch_id,
            session.updated_at,
            if pending_start {
                "an immediate start that never handed off to the runner"
            } else {
                "no runner thread or task owns it"
            }
        );
        let project_id = store.get_project_id_for_session(&session.id).ok().flatten();
        crate::session_runner::emit_status(
            app_handle,
            &session.id,
            "error",
            Some(STRANDED_SESSION_MESSAGE.to_string()),
            Some(&CompletionReason::Crashed),
            session.branch_id.clone(),
            project_id,
        );
        if let Some(branch_id) = session.branch_id.as_deref() {
            kick(app_handle, branch_id, KickSource::Sweep);
        }
        ended.push(session.id);
    }
    ended
}

/// A `running` row the sweep would end now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StrandedRow {
    pub(crate) session_id: String,
    pub(crate) branch_id: Option<String>,
}

/// The sweep's view of the queue at one instant, for the Doctor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueueHealth {
    /// `running` rows this process owns.
    pub(crate) owned_running: usize,
    /// Of those, rows the registry knows an owner for (a registered runner or
    /// a liveness hold).
    pub(crate) live: usize,
    /// Rows [`is_stranded`] would end now.
    pub(crate) stranded: Vec<StrandedRow>,
    /// Rows the sweep has ended over the store's lifetime.
    pub(crate) swept_total: usize,
}

/// What the sweep would do now, without doing it.
///
/// Same decision as [`reconcile_stranded_running_sessions`], no writes. With
/// no CAS to absorb the row-then-registry interleaving, a row found not live
/// is re-read and only reported if it is still `running` — otherwise a
/// session finishing during the read would show as stranded for one Doctor
/// run. `swept_total` counts rows carrying [`STRANDED_SESSION_MESSAGE`], which
/// only the sweep writes.
pub(crate) fn queue_health(
    store: &Store,
    registry: &SessionRegistry,
) -> Result<QueueHealth, crate::store::StoreError> {
    let own_pid = std::process::id();
    let mut health = QueueHealth {
        owned_running: 0,
        live: 0,
        stranded: Vec::new(),
        swept_total: store.count_sessions_with_error_message(STRANDED_SESSION_MESSAGE)?,
    };
    for session in store.get_running_sessions()? {
        if session.owner_pid != Some(own_pid) {
            continue;
        }
        health.owned_running += 1;
        let live = registry.is_live(&session.id);
        if live {
            health.live += 1;
            continue;
        }
        let still_running = store
            .get_session(&session.id)?
            .is_some_and(|row| is_stranded(&row, false, own_pid));
        if still_running {
            health.stranded.push(StrandedRow {
                session_id: session.id,
                branch_id: session.branch_id,
            });
        }
    }
    Ok(health)
}

/// Spawn the periodic sweep. Call once the store is ready, alongside the other
/// store-backed services; the store handle stays valid for the process
/// lifetime for the same reason `pr_poll_scheduler::spawn`'s does.
///
/// The first pass runs one interval in: at startup the dead-owner recovery
/// has already handled every row from a previous process, and nothing this
/// process owns can be stranded before it has started anything.
pub(crate) fn spawn(
    store: Arc<Store>,
    registry: Arc<SessionRegistry>,
    app_handle: tauri::AppHandle,
) {
    tauri::async_runtime::spawn(async move {
        let period = Duration::from_secs(SWEEP_INTERVAL_SECS);
        let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if crate::app_lifecycle::is_quitting(&app_handle) {
                return;
            }
            reconcile_stranded_running_sessions(&store, &registry, &app_handle);
        }
    });
}
