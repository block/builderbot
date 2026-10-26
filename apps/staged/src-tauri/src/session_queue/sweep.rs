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
//! The sweep defines a stranded row structurally: owned by this process, not
//! registered in the [`SessionRegistry`], and quiet for longer than a grace
//! window. The grace exists because two paths legitimately hold an
//! unregistered `running` row for a while — the immediate start's second
//! phase (context build, then runner handoff) and the immediate pull marker —
//! and because a session thread deregisters *before* its post-completion
//! hooks and terminal write, so a long turn's row is unregistered and stale
//! for the length of its hooks. The registry's deregistration stamp covers
//! that last stretch; the grace covers the rest.

use std::sync::Arc;
use std::time::Duration;

use crate::session_runner::SessionRegistry;
use crate::store::{CompletionReason, Session, SessionStatus, Store};

use super::{kick, KickSource};

/// How long a row may sit `running`, unregistered and untouched before the
/// sweep ends it. Conservative: the immediate start's context build is tens of
/// seconds at worst, and post-completion hooks are seconds.
pub(crate) const STRANDED_GRACE_MS: i64 = 10 * 60 * 1000;

/// How often the sweep runs. Also how late it can be: a row is ended between
/// one and two intervals after its grace expires.
const SWEEP_INTERVAL_SECS: u64 = 60;

/// The error recorded on a row the sweep ends.
pub(crate) const STRANDED_SESSION_MESSAGE: &str = "Staged lost track of this session";

/// What the registry knows about a row, as the decision needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Liveness {
    /// A cancellation token is registered: a thread or task owns the row.
    pub(crate) registered: bool,
    /// When the row last deregistered, if recently enough to still be stamped.
    pub(crate) deregistered_at: Option<i64>,
}

impl Liveness {
    pub(crate) fn of(registry: &SessionRegistry, session_id: &str) -> Self {
        Self {
            registered: registry.is_running(session_id),
            deregistered_at: registry.deregistered_at(session_id),
        }
    }
}

/// Whether `session` is a row nothing will finish.
///
/// Pure: `now` and `grace_ms` are parameters so the decision is testable
/// against a fixed clock. `own_pid` is this process; rows under other pids
/// belong to another instance on the same data dir and are left alone, like
/// everywhere else.
pub(crate) fn is_stranded(
    session: &Session,
    liveness: Liveness,
    own_pid: u32,
    now: i64,
    grace_ms: i64,
) -> bool {
    if session.status != SessionStatus::Running || session.owner_pid != Some(own_pid) {
        return false;
    }
    if liveness.registered {
        return false;
    }
    let last_alive_at = liveness
        .deregistered_at
        .map_or(session.updated_at, |at| at.max(session.updated_at));
    now.saturating_sub(last_alive_at) > grace_ms
}

/// End every stranded row this process owns and kick the branches they held.
///
/// Returns the ids of the rows it ended. The terminal write is a
/// `transition_from_running` CAS, so an owner that turns out to be alive and
/// writes first wins and the sweep moves on; the `warn` makes any firing
/// visible, since a legitimate owner losing this race would be a bug in the
/// grace, not in the owner.
pub(crate) fn reconcile_stranded_running_sessions<R: tauri::Runtime>(
    store: &Arc<Store>,
    registry: &SessionRegistry,
    app_handle: &tauri::AppHandle<R>,
    now: i64,
    grace_ms: i64,
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
        let liveness = Liveness::of(registry, &session.id);
        if !is_stranded(&session, liveness, own_pid, now, grace_ms) {
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
            continue;
        }
        let pending_start =
            crate::session_commands::pending_branch_start_agent_id(session.agent_id.as_deref())
                .is_some();
        log::warn!(
            "[session_queue] ended stranded session {} (branch {:?}, quiet for {}s, {})",
            session.id,
            session.branch_id,
            now.saturating_sub(session.updated_at) / 1000,
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

    // Stamps older than the grace can no longer change a decision.
    registry.forget_deregistrations_before(now.saturating_sub(grace_ms));
    ended
}

/// Spawn the periodic sweep. Call once the store is ready, alongside the other
/// store-backed services; the store handle stays valid for the process
/// lifetime for the same reason `pr_poll_scheduler::spawn`'s does.
///
/// The first pass runs one interval in: at startup every row this process
/// could own is younger than the grace, and the dead-owner recovery has
/// already handled the rest.
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
            reconcile_stranded_running_sessions(
                &store,
                &registry,
                &app_handle,
                crate::store::now_timestamp(),
                STRANDED_GRACE_MS,
            );
        }
    });
}
