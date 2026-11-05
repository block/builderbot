//! Jittered auto-recovery for sessions paused by provider API limits.
//!
//! Retry bookkeeping is stored under `provider_metadata.staged` so it survives
//! restarts without widening the stable session schema. The scheduler only
//! resumes ACP sessions that can be replayed safely via `load_session`; sessions
//! that stopped with an unmatched tool call stay paused for manual review.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::AppHandle;

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::store::{MessageRole, Session, Store};

pub(crate) const AUTO_RETRY_RESUME_PROMPT: &str = "Continue where you left off.";

const TICK_INTERVAL_SECS: u64 = 15;
const MAX_AUTO_RETRY_ATTEMPTS: u32 = 8;
const GIVE_UP_AFTER_MS: i64 = 24 * 60 * 60 * 1_000;
const BASE_BACKOFF_MS: i64 = 60_000;
const MAX_BACKOFF_MS: i64 = 30 * 60_000;
const MIN_RETRY_DELAY_MS: i64 = 5_000;
const RESET_GRACE_MS: i64 = 30_000;
const RESET_JITTER_MS: i64 = 5 * 60_000;
const MAX_RESUMES_PER_TICK: usize = 2;
/// The longest a provider-reported reset is trusted to sit in the future.
///
/// Real provider reset windows top out around a day; anything further —
/// typically an epoch in the wrong scale (microseconds read as millis land
/// tens of millennia ahead) — would park the session on a bogus
/// `next_retry_at` forever, because the 24h give-up is only evaluated when
/// recovery state is (re)built, never on ticks. Implausible resets fall back
/// to plain backoff instead.
pub(crate) const MAX_PLAUSIBLE_RESET_LEAD_MS: i64 = 48 * 60 * 60 * 1_000;

#[derive(Default)]
pub struct RateLimitScheduler {
    in_flight: Mutex<HashSet<String>>,
    tick_loop_spawned: AtomicBool,
}

impl RateLimitScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    fn try_mark_in_flight(&self, session_id: &str) -> bool {
        self.in_flight
            .lock()
            .unwrap()
            .insert(session_id.to_string())
    }

    fn clear_in_flight(&self, session_id: &str) {
        self.in_flight.lock().unwrap().remove(session_id);
    }

    /// Claim the right to run this scheduler's tick loop, once per instance.
    ///
    /// `start_store_services` runs both at setup and after a confirmed DB
    /// reset; a second loop over the same scheduler would double every resume
    /// attempt, so later claims lose.
    fn try_claim_tick_loop(&self) -> bool {
        !self.tick_loop_spawned.swap(true, Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RateLimitAutoRecovery {
    pub attempt: u32,
    pub max_attempts: u32,
    pub next_retry_at: Option<i64>,
    pub first_limited_at: i64,
    pub updated_at: i64,
    pub gave_up_at: Option<i64>,
    pub unsafe_to_auto_resume: bool,
    pub reason: Option<String>,
}

pub fn spawn(
    scheduler: Arc<RateLimitScheduler>,
    store: Arc<Store>,
    registry: Arc<crate::session_runner::SessionRegistry>,
    action_executor: Arc<ActionExecutor>,
    action_registry: Arc<ActionRegistry>,
    app_handle: AppHandle,
) {
    if !scheduler.try_claim_tick_loop() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(TICK_INTERVAL_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            interval.tick().await;
            run_tick(
                Arc::clone(&scheduler),
                Arc::clone(&store),
                Arc::clone(&registry),
                Arc::clone(&action_executor),
                Arc::clone(&action_registry),
                app_handle.clone(),
            )
            .await;
        }
    });
}

async fn run_tick(
    scheduler: Arc<RateLimitScheduler>,
    store: Arc<Store>,
    registry: Arc<crate::session_runner::SessionRegistry>,
    action_executor: Arc<ActionExecutor>,
    action_registry: Arc<ActionRegistry>,
    app_handle: AppHandle,
) {
    let sessions = match store.get_rate_limited_paused_sessions() {
        Ok(sessions) => sessions,
        Err(e) => {
            log::warn!("[rate_limit_recovery] failed to list paused sessions: {e}");
            return;
        }
    };
    let actionable_backlog = sessions
        .iter()
        .filter(|session| is_actionable_for_auto_resume(session))
        .count();
    crate::rate_limit_metrics::record_scheduled_resume_backlog(actionable_backlog);
    let now = crate::store::now_timestamp();
    let mut started = 0usize;
    let mut drained_branches: HashSet<String> = HashSet::new();

    for session in sessions {
        if started >= MAX_RESUMES_PER_TICK {
            break;
        }
        if !scheduler.try_mark_in_flight(&session.id) {
            continue;
        }

        let state = match ensure_auto_recovery_state(&store, &session, now) {
            Ok(Some(state)) => state,
            Ok(None) => {
                scheduler.clear_in_flight(&session.id);
                continue;
            }
            Err(e) => {
                log::warn!(
                    "[rate_limit_recovery] failed to prepare session {}: {e}",
                    session.id
                );
                scheduler.clear_in_flight(&session.id);
                continue;
            }
        };

        if !is_due(&state, now) {
            scheduler.clear_in_flight(&session.id);
            continue;
        }

        let ticket = match claim_for_auto_resume(&store, &session.id) {
            Ok(AutoResumeClaim::Claimed(ticket)) => *ticket,
            Ok(AutoResumeClaim::Deferred {
                reason,
                drainable_branch_id,
            }) => {
                log::info!(
                    "[rate_limit_recovery] deferring auto-resume for session {}: {reason}",
                    session.id
                );
                scheduler.clear_in_flight(&session.id);
                if let Some(branch_id) = drainable_branch_id {
                    if drained_branches.insert(branch_id.clone()) {
                        spawn_queue_drain(&app_handle, &branch_id);
                    }
                }
                continue;
            }
            Ok(AutoResumeClaim::Lost) => {
                log::info!(
                    "[rate_limit_recovery] skipping auto-resume for session {}: no longer paused by a rate limit",
                    session.id
                );
                scheduler.clear_in_flight(&session.id);
                continue;
            }
            Err(e) => {
                log::warn!(
                    "[rate_limit_recovery] failed to claim session {} for auto-resume: {e}",
                    session.id
                );
                scheduler.clear_in_flight(&session.id);
                continue;
            }
        };

        started += 1;
        crate::rate_limit_metrics::record_recovery_retry_attempt(
            &session.id,
            session.provider.as_deref(),
            session.rate_limit_kind.as_deref(),
            state.attempt,
            state.max_attempts,
        );
        let scheduler_for_task = Arc::clone(&scheduler);
        let store_for_task = Arc::clone(&store);
        let registry_for_task = Arc::clone(&registry);
        let executor_for_task = Arc::clone(&action_executor);
        let action_registry_for_task = Arc::clone(&action_registry);
        let app_for_task = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            let session_id = session.id.clone();
            log::info!(
                "[rate_limit_recovery] auto-resuming session {} (attempt {}/{})",
                session_id,
                state.attempt,
                state.max_attempts
            );
            let result = crate::session_commands::auto_resume_rate_limited_session(
                Arc::clone(&store_for_task),
                registry_for_task,
                executor_for_task,
                action_registry_for_task,
                app_for_task,
                ticket,
            )
            .await;
            if let Err(e) = result {
                log::warn!("[rate_limit_recovery] auto-resume failed for {session_id}: {e}");
                crate::rate_limit_metrics::record_recovery_start_failed(
                    &session_id,
                    "resume_start_failed",
                );
                reschedule_after_start_failure(
                    &store_for_task,
                    &session_id,
                    crate::store::now_timestamp(),
                );
            }
            scheduler_for_task.clear_in_flight(&session_id);
        });
    }
}

/// Start whatever the branch has queued instead of waiting for it to finish.
///
/// Queued sessions only start when something drains them, and the pause path
/// deliberately skips the terminal-hook drain cascade — a paused session has
/// not finished. So a session that pauses with work queued behind it leaves the
/// branch deadlocked: the queue never drains (no terminal hook ran) and the
/// pause never resumes (the queue is non-empty). Until an unrelated drain fired
/// — project open, workspace ready, post-pull — nothing moved, which is exactly
/// the unattended overnight recovery this feature exists for.
///
/// Draining breaks the deadlock in either direction: the drained session
/// completes and its terminal hook restarts the cascade, or it hits the same
/// limit and joins recovery with its own backoff.
/// Ask the queue kicker to drain the branch's queued work while the
/// rate-limited session waits to auto-resume. Only the kicker may run a drain
/// (it serialises per branch), so this is a kick, not a drain of our own.
fn spawn_queue_drain(app_handle: &AppHandle, branch_id: &str) {
    log::info!(
        "[rate_limit_recovery] kicking queued work for branch {branch_id} while a session waits to auto-resume"
    );
    crate::session_queue::kick(
        app_handle,
        branch_id,
        crate::session_queue::KickSource::RateLimitRecovery,
    );
}

fn is_due(state: &RateLimitAutoRecovery, now: i64) -> bool {
    if state.unsafe_to_auto_resume || state.gave_up_at.is_some() {
        return false;
    }
    state
        .next_retry_at
        .map(|next_retry_at| next_retry_at <= now)
        .unwrap_or(false)
}

fn ensure_auto_recovery_state(
    store: &Store,
    session: &Session,
    now: i64,
) -> Result<Option<RateLimitAutoRecovery>, String> {
    if let Some(state) = auto_recovery_from_metadata(session.provider_metadata.as_ref()) {
        return Ok(Some(state));
    }

    record_rate_limit_pause_for_session(store, session, now)?;
    Ok(store
        .get_session(&session.id)
        .map_err(|e| e.to_string())?
        .and_then(|session| auto_recovery_from_metadata(session.provider_metadata.as_ref())))
}

/// Park a running session as rate-limited, with its retry bookkeeping.
///
/// This is the session runner's pause path. The status flip and the
/// `staged.rateLimitAutoRecovery` fragment land in one guarded store write, so
/// a scheduler tick can never observe a paused row without bookkeeping: with
/// the fragment written *after* the flip, a tick in that window built attempt
/// 1 itself via [`ensure_auto_recovery_state`], and the pause path then read
/// that back as `existing` and bumped it to attempt 2 with a fresh
/// `next_retry_at` — burning retry budget on a race, not a limit.
///
/// The bookkeeping is planned from the detected `resets_at` rather than the
/// row's column (not yet written) and from the row as it stands, which is all
/// the blocker scan needs: `agent_id` and the message history, not status.
///
/// Returns whether this call won the `running → paused` transition. A lost
/// race (a Stop cancelled the row first) writes nothing, including the
/// fragment.
pub(crate) fn pause_for_rate_limit(
    store: &Store,
    session_id: &str,
    error_message: Option<&str>,
    resets_at: Option<&str>,
    kind: Option<&str>,
    now: i64,
) -> Result<bool, String> {
    let Some(session) = store.get_session(session_id).map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    let state = plan_recovery_state(store, &session, resets_at, now)?;
    let transitioned = store
        .transition_running_to_rate_limited_with_provider_metadata(
            session_id,
            error_message,
            resets_at,
            kind,
            Some(&metadata_for_state(&state)),
        )
        .map_err(|e| e.to_string())?;
    if transitioned {
        log_recovery_state(&session, kind, &state);
    }
    Ok(transitioned)
}

#[cfg(test)]
fn record_rate_limit_pause(store: &Store, session_id: &str, now: i64) -> Result<(), String> {
    let Some(session) = store.get_session(session_id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    record_rate_limit_pause_for_session(store, &session, now)
}

/// Write retry bookkeeping onto a row that is already paused.
///
/// Only [`ensure_auto_recovery_state`] needs this: rows paused by a build
/// that wrote the fragment after the status flip, or by a pause path that
/// could not plan one. New pauses go through [`pause_for_rate_limit`].
fn record_rate_limit_pause_for_session(
    store: &Store,
    session: &Session,
    now: i64,
) -> Result<(), String> {
    let state = plan_recovery_state(store, session, session.rate_limit_resets_at.as_deref(), now)?;
    store
        .merge_session_provider_metadata(&session.id, &metadata_for_state(&state))
        .map_err(|e| e.to_string())?;
    log_recovery_state(session, session.rate_limit_kind.as_deref(), &state);
    Ok(())
}

/// The next retry state for `session`, advancing any bookkeeping it already
/// carries.
fn plan_recovery_state(
    store: &Store,
    session: &Session,
    resets_at: Option<&str>,
    now: i64,
) -> Result<RateLimitAutoRecovery, String> {
    let blocker = auto_resume_blocker(store, session)?;
    let existing = auto_recovery_from_metadata(session.provider_metadata.as_ref());
    let jitter_ms = jitter_ms(jitter_bound_for(resets_at, now));
    Ok(build_recovery_state(
        existing.as_ref(),
        resets_at,
        now,
        blocker.as_deref(),
        jitter_ms,
    ))
}

fn log_recovery_state(session: &Session, kind: Option<&str>, state: &RateLimitAutoRecovery) {
    if state.unsafe_to_auto_resume {
        log::info!(
            "[rate_limit_recovery] session {} requires manual resume ({})",
            session.id,
            state.reason.as_deref().unwrap_or("unsafe")
        );
        crate::rate_limit_metrics::record_recovery_manual_required(
            &session.id,
            session.provider.as_deref(),
            kind,
            state.reason.as_deref(),
        );
    } else if state.gave_up_at.is_some() {
        log::warn!(
            "[rate_limit_recovery] session {} gave up after {} attempts ({})",
            session.id,
            state.attempt,
            state.reason.as_deref().unwrap_or("retry_limit")
        );
        crate::rate_limit_metrics::record_recovery_give_up(
            &session.id,
            session.provider.as_deref(),
            kind,
            state.attempt,
            state.reason.as_deref(),
        );
    } else if let Some(next_retry_at) = state.next_retry_at {
        log::info!(
            "[rate_limit_recovery] scheduled session {} retry attempt {}/{} at {}",
            session.id,
            state.attempt,
            state.max_attempts,
            next_retry_at
        );
        crate::rate_limit_metrics::record_recovery_scheduled(
            &session.id,
            session.provider.as_deref(),
            kind,
            state.attempt,
            state.max_attempts,
            next_retry_at,
            state.reason.as_deref(),
        );
    }
}

/// Park a failed auto-resume attempt back into the normal retry schedule.
///
/// `auto_resume_rate_limited_session` re-parks the session as paused when the
/// start fails, so a transient error (a store hiccup, a briefly unresolvable
/// workspace path) lands here with the retry bookkeeping intact. Scheduling a
/// regular backoff attempt lets a later tick try again; the attempt cap and
/// the 24h window in [`build_recovery_state`] still bound repeated failures,
/// so give-up is reserved for starts that keep failing rather than issued on
/// the first one.
fn reschedule_after_start_failure(store: &Store, session_id: &str, now: i64) {
    match store.get_session(session_id) {
        // Only reschedule a session the failed attempt actually parked; a
        // session in any other state was claimed by something else (a manual
        // resume, a cancel, a delete) and is that path's to manage.
        Ok(Some(session)) if session.status.as_str() == "paused" => {
            if let Err(e) = record_rate_limit_pause_for_session(store, &session, now) {
                log::warn!(
                    "[rate_limit_recovery] failed to reschedule session {session_id} after a failed start: {e}"
                );
            }
        }
        Ok(_) => {}
        Err(e) => {
            log::warn!(
                "[rate_limit_recovery] failed to read session {session_id} after a failed start: {e}"
            );
        }
    }
}

/// Whether the scheduler could still act on this paused session.
///
/// Sessions that gave up or need a manual resume stay paused indefinitely;
/// counting them in the backlog metric would report (and log) a permanent
/// nonzero backlog long after the scheduler has nothing left to do.
fn is_actionable_for_auto_resume(session: &Session) -> bool {
    auto_recovery_from_metadata(session.provider_metadata.as_ref())
        .map(|state| state.gave_up_at.is_none() && !state.unsafe_to_auto_resume)
        .unwrap_or(true)
}

fn build_recovery_state(
    existing: Option<&RateLimitAutoRecovery>,
    resets_at: Option<&str>,
    now: i64,
    blocker: Option<&str>,
    jitter_ms: i64,
) -> RateLimitAutoRecovery {
    let first_limited_at = existing.map(|s| s.first_limited_at).unwrap_or(now);

    if let Some(reason) = blocker {
        return RateLimitAutoRecovery {
            attempt: existing.map(|s| s.attempt).unwrap_or(0),
            max_attempts: MAX_AUTO_RETRY_ATTEMPTS,
            next_retry_at: None,
            first_limited_at,
            updated_at: now,
            gave_up_at: None,
            unsafe_to_auto_resume: true,
            reason: Some(reason.to_string()),
        };
    }

    let attempt = existing.map(|s| s.attempt.saturating_add(1)).unwrap_or(1);
    if attempt > MAX_AUTO_RETRY_ATTEMPTS {
        return RateLimitAutoRecovery {
            attempt,
            max_attempts: MAX_AUTO_RETRY_ATTEMPTS,
            next_retry_at: None,
            first_limited_at,
            updated_at: now,
            gave_up_at: Some(now),
            unsafe_to_auto_resume: false,
            reason: Some("retry_cap_exceeded".to_string()),
        };
    }
    if now.saturating_sub(first_limited_at) > GIVE_UP_AFTER_MS {
        return RateLimitAutoRecovery {
            attempt,
            max_attempts: MAX_AUTO_RETRY_ATTEMPTS,
            next_retry_at: None,
            first_limited_at,
            updated_at: now,
            gave_up_at: Some(now),
            unsafe_to_auto_resume: false,
            reason: Some("retry_window_elapsed".to_string()),
        };
    }

    let reset_retry_at = plausible_reset_millis(resets_at, now).map(|reset_at| {
        reset_at
            .saturating_add(RESET_GRACE_MS)
            .saturating_add(jitter_ms)
    });

    let (next_retry_at, reason) = if let Some(next_retry_at) = reset_retry_at {
        (next_retry_at, "reset_at")
    } else {
        let bounded_jitter_ms = jitter_ms.min(retry_delay_bound_ms(attempt));
        (
            now.saturating_add(bounded_jitter_ms.max(MIN_RETRY_DELAY_MS)),
            "backoff",
        )
    };

    RateLimitAutoRecovery {
        attempt,
        max_attempts: MAX_AUTO_RETRY_ATTEMPTS,
        next_retry_at: Some(next_retry_at),
        first_limited_at,
        updated_at: now,
        gave_up_at: None,
        unsafe_to_auto_resume: false,
        reason: Some(reason.to_string()),
    }
}

/// The outcome of trying to take ownership of a rate-limited session for one
/// auto-resume attempt.
enum AutoResumeClaim {
    /// The session row is `running` and this attempt owns it.
    Claimed(Box<crate::session_commands::RateLimitResumeTicket>),
    /// The branch has other work in flight, so resuming now would run two
    /// agents in the same worktree. `drainable_branch_id` is set when that work
    /// is queued-only.
    Deferred {
        reason: String,
        drainable_branch_id: Option<String>,
    },
    /// The row is no longer a paused rate-limited session: a manual resume, a
    /// cancel, or a delete won the race, and it keeps the session.
    Lost,
}

/// Claim a rate-limited session for auto-resume, or explain why not.
///
/// A rate-limited session must not auto-resume while its branch has other
/// active work: the user may have launched or queued new sessions during the
/// backoff, and resuming would run two agents in the same branch worktree. So
/// this asks the same question the branch launch path asks before starting git
/// work ([`crate::prs::branch_work_in_flight`]) under the same per-branch
/// launch lock.
///
/// The claim itself happens under that same lock, because checking there and
/// claiming afterwards only narrows the race: a branch launch does its own
/// check *and* its row insert under the lock, so it would legitimately start a
/// session in the window between the two while the paused row still looks
/// dormant. Once the row is `running`, every `has_running_session_for_branch`
/// check — launch, queue drain, branch move — sees this session.
///
/// Unlike [`auto_resume_blocker`], a busy branch is transient, so a deferral
/// leaves the retry state untouched for the next tick rather than marking the
/// session unsafe to auto-resume. Note the branch stays free to rebase or
/// squash underneath a paused conversation (#902 only queues git actions behind
/// running or queued sessions); auto-resume then replays a conversation that
/// predates the rewritten SHAs, exactly as a manual resume after a rebase does
/// today, and #901's commit reassociation repairs the recorded artifacts.
fn claim_for_auto_resume(store: &Arc<Store>, session_id: &str) -> Result<AutoResumeClaim, String> {
    let branch_id = store
        .get_branch_id_for_session(session_id)
        .map_err(|e| e.to_string())?;

    let launch_lock = branch_id
        .as_deref()
        .map(crate::session_commands::branch_session_launch_lock_for);
    let _guard = launch_lock.as_ref().map(|lock| lock.lock().unwrap());

    if let Some(branch_id) = branch_id.as_deref() {
        match crate::prs::branch_work_in_flight(store, branch_id)? {
            crate::prs::BranchWorkInFlight::Running => {
                return Ok(AutoResumeClaim::Deferred {
                    reason: format!("branch {branch_id} has a running session"),
                    drainable_branch_id: None,
                });
            }
            crate::prs::BranchWorkInFlight::Queued => {
                // Queued rows gated on this very session (a force-push behind
                // the rebase that hit the limit, say) cannot start before it:
                // `pipeline_dependency::status_gate` holds them while it is
                // paused, so the drain this deferral would spawn starts
                // nothing, and deferring on them waits forever for a queue
                // that only this resume can unblock. They are no two-agents
                // risk either — they run after this session finishes, through
                // its own terminal drain — so the branch counts as free when
                // they are all it has queued.
                let queued = store
                    .get_queued_sessions_for_branch(branch_id)
                    .map_err(|e| e.to_string())?;
                if !queued_rows_all_depend_on(&queued, session_id) {
                    return Ok(AutoResumeClaim::Deferred {
                        reason: format!("branch {branch_id} has queued work"),
                        drainable_branch_id: Some(branch_id.to_string()),
                    });
                }
            }
            crate::prs::BranchWorkInFlight::None => {}
        }
    }

    // Re-read under the lock: `ensure_auto_recovery_state` may have just
    // written the retry bookkeeping this claim has to carry forward, and the
    // rate-limit fields have to be captured before the claim clears them.
    let Some(session) = store.get_session(session_id).map_err(|e| e.to_string())? else {
        return Ok(AutoResumeClaim::Lost);
    };

    // Claim any message the user queued during the backoff before claiming the
    // row: `claim_oldest_queued_session_message` skips running sessions, so the
    // reverse order would always fall back to the synthetic retry prompt.
    // (Messages can only be queued before the pause today — enqueueing requires
    // a running session — but claiming here keeps the auto path correct if that
    // guard is ever relaxed to accept paused sessions.)
    let queued_message = store
        .claim_oldest_queued_session_message(session_id)
        .map_err(|e| e.to_string())?;

    let preserved_metadata = preserved_auto_recovery_metadata(session.provider_metadata.as_ref());
    let claimed = store
        .transition_to_running_with_provider_metadata(session_id, preserved_metadata.as_ref())
        .map_err(|e| e.to_string())?;
    if !claimed {
        if let Some(message) = &queued_message {
            let _ = store.release_queued_session_message(&message.id, None);
        }
        return Ok(AutoResumeClaim::Lost);
    }

    Ok(AutoResumeClaim::Claimed(Box::new(
        crate::session_commands::RateLimitResumeTicket {
            session_id: session_id.to_string(),
            queued_message,
            rate_limit_resets_at: session.rate_limit_resets_at.clone(),
            rate_limit_kind: session.rate_limit_kind.clone(),
        },
    )))
}

/// Whether every queued row depends on `session_id`, directly or through a
/// chain of other queued rows (A depends on the session, B depends on A).
///
/// A dependency edge pointing outside the queue — at a session that is not
/// `session_id` and not itself a queued row here — is independent work, as is
/// a row with no dependency at all. An empty queue is vacuously dependent;
/// callers only ask once they know the queue is non-empty.
fn queued_rows_all_depend_on(queued: &[Session], session_id: &str) -> bool {
    let mut dependents: HashSet<&str> = HashSet::from([session_id]);
    loop {
        let before = dependents.len();
        for row in queued {
            let depends_on = row
                .pipeline
                .as_ref()
                .and_then(|pipeline| pipeline.depends_on_session_id.as_deref());
            if depends_on.is_some_and(|dependency| dependents.contains(dependency)) {
                dependents.insert(row.id.as_str());
            }
        }
        if dependents.len() == before {
            break;
        }
    }
    queued
        .iter()
        .all(|row| dependents.contains(row.id.as_str()))
}

fn auto_resume_blocker(store: &Store, session: &Session) -> Result<Option<String>, String> {
    if session.agent_id.is_none() {
        return Ok(Some("missing_agent_session_id".to_string()));
    }

    let messages = store
        .get_session_messages(&session.id)
        .map_err(|e| e.to_string())?;
    // Count outstanding calls rather than tracking a single boolean: agents
    // routinely issue parallel tool calls, and ToolCall(A), ToolCall(B),
    // ToolResult(A) still leaves B without a result.
    //
    // Only the turn that hit the limit matters, so each User message resets
    // the count. A ToolCall left unanswered by some past turn — an app quit,
    // a crash, a cancel mid-call — never gets a ToolResult row, and counting
    // it would permanently downgrade every future pause of a long-lived
    // session to manual resume over one messy turn in its history.
    let mut outstanding_tool_calls: usize = 0;
    for message in messages {
        match message.role {
            MessageRole::User => outstanding_tool_calls = 0,
            MessageRole::ToolCall => outstanding_tool_calls += 1,
            MessageRole::ToolResult => {
                outstanding_tool_calls = outstanding_tool_calls.saturating_sub(1)
            }
            MessageRole::Assistant => {}
        }
    }
    if outstanding_tool_calls > 0 {
        Ok(Some("pending_tool_call".to_string()))
    } else {
        Ok(None)
    }
}

fn preserved_auto_recovery_metadata(
    metadata: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    auto_recovery_from_metadata(metadata.as_ref().copied()).map(|state| metadata_for_state(&state))
}

pub(crate) fn auto_recovery_from_metadata(
    metadata: Option<&serde_json::Value>,
) -> Option<RateLimitAutoRecovery> {
    metadata
        .and_then(|metadata| metadata.pointer("/staged/rateLimitAutoRecovery"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

fn metadata_for_state(state: &RateLimitAutoRecovery) -> serde_json::Value {
    json!({
        "staged": {
            "rateLimitAutoRecovery": state
        }
    })
}

fn jitter_bound_for(resets_at: Option<&str>, now: i64) -> i64 {
    if plausible_reset_millis(resets_at, now).is_some() {
        RESET_JITTER_MS
    } else {
        MAX_BACKOFF_MS
    }
}

/// A stored reset time the scheduler will actually wait for: parseable, in
/// the future, and within [`MAX_PLAUSIBLE_RESET_LEAD_MS`] of now.
fn plausible_reset_millis(resets_at: Option<&str>, now: i64) -> Option<i64> {
    resets_at
        .and_then(parse_reset_timestamp_millis)
        .filter(|reset_at| *reset_at > now)
        .filter(|reset_at| reset_at.saturating_sub(now) <= MAX_PLAUSIBLE_RESET_LEAD_MS)
}

/// Parse a stored reset timestamp into UTC millis.
///
/// The detection path normalizes what it can to RFC3339, but providers report
/// resets as numeric epochs too (claude-agent-acp forwards the SDK's numeric
/// `resetsAt` verbatim), so a bare integer that slipped through — including
/// rows written before normalization existed — is accepted here as epoch
/// seconds or millis, split by magnitude: epoch seconds passed 10^11 around
/// year 5138, while millis crossed it in 1973.
fn parse_reset_timestamp_millis(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Ok(epoch) = value.parse::<i64>() {
        if epoch <= 0 {
            return None;
        }
        return Some(if epoch >= 100_000_000_000 {
            epoch
        } else {
            epoch.saturating_mul(1_000)
        });
    }
    parse_rfc3339_millis(value)
}

fn jitter_ms(bound_ms: i64) -> i64 {
    if bound_ms <= MIN_RETRY_DELAY_MS {
        return MIN_RETRY_DELAY_MS;
    }
    let span = (bound_ms - MIN_RETRY_DELAY_MS + 1) as u64;
    MIN_RETRY_DELAY_MS + (rand::random::<u64>() % span) as i64
}

fn retry_delay_bound_ms(attempt: u32) -> i64 {
    let exponent = attempt.saturating_sub(1).min(10);
    BASE_BACKOFF_MS
        .saturating_mul(1_i64 << exponent)
        .min(MAX_BACKOFF_MS)
}

fn parse_rfc3339_millis(value: &str) -> Option<i64> {
    let value = value.trim();
    let (date, time_and_offset) = value.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i32 = date_parts.next()?.parse().ok()?;
    let month: u8 = date_parts.next()?.parse().ok()?;
    let day: u8 = date_parts.next()?.parse().ok()?;

    let (time, offset_seconds) = if let Some(time) = time_and_offset.strip_suffix('Z') {
        (time, 0)
    } else {
        let pos = time_and_offset.rfind(['+', '-'])?;
        let sign = if time_and_offset.as_bytes().get(pos) == Some(&b'-') {
            -1
        } else {
            1
        };
        let time = &time_and_offset[..pos];
        let offset = &time_and_offset[pos + 1..];
        let (hours, minutes) = offset.split_once(':')?;
        let hours: i32 = hours.parse().ok()?;
        let minutes: i32 = minutes.parse().ok()?;
        (time, sign * (hours * 3600 + minutes * 60))
    };

    let mut time_parts = time.split(':');
    let hour: u8 = time_parts.next()?.parse().ok()?;
    let minute: u8 = time_parts.next()?.parse().ok()?;
    let seconds_raw = time_parts.next()?;
    let (second_raw, millis) = match seconds_raw.split_once('.') {
        Some((second, fraction)) => {
            let mut fraction = fraction.chars().take(3).collect::<String>();
            while fraction.len() < 3 {
                fraction.push('0');
            }
            let millis = fraction.parse::<u16>().ok().unwrap_or(0);
            (second, millis)
        }
        None => (seconds_raw, 0),
    };
    let second: u8 = second_raw.parse().ok()?;

    let month = time::Month::try_from(month).ok()?;
    let date = time::Date::from_calendar_date(year, month, day).ok()?;
    let time = time::Time::from_hms_milli(hour, minute, second, millis).ok()?;
    let offset_datetime = time::PrimitiveDateTime::new(date, time)
        .assume_utc()
        .checked_sub(time::Duration::seconds(offset_seconds.into()))?;
    Some(offset_datetime.unix_timestamp().saturating_mul(1_000) + i64::from(millis))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{MessageRole, Session};
    use std::path::Path;

    #[test]
    fn schedules_reset_retry_after_reset_with_grace_and_jitter() {
        let now = parse_rfc3339_millis("2026-06-24T15:00:00Z").unwrap();
        let reset = "2026-06-24T15:45:00Z";

        let state = build_recovery_state(None, Some(reset), now, None, 12_000);

        assert_eq!(state.attempt, 1);
        assert_eq!(state.reason.as_deref(), Some("reset_at"));
        assert_eq!(
            state.next_retry_at,
            Some(parse_rfc3339_millis(reset).unwrap() + RESET_GRACE_MS + 12_000)
        );
    }

    #[test]
    fn schedules_unknown_reset_with_capped_backoff_jitter() {
        let state = build_recovery_state(None, None, 1_000_000, None, 4_000);

        assert_eq!(state.attempt, 1);
        assert_eq!(state.reason.as_deref(), Some("backoff"));
        assert_eq!(state.next_retry_at, Some(1_000_000 + MIN_RETRY_DELAY_MS));
    }

    #[test]
    fn gives_up_after_retry_cap() {
        let existing = RateLimitAutoRecovery {
            attempt: MAX_AUTO_RETRY_ATTEMPTS,
            max_attempts: MAX_AUTO_RETRY_ATTEMPTS,
            next_retry_at: Some(1),
            first_limited_at: 0,
            updated_at: 0,
            gave_up_at: None,
            unsafe_to_auto_resume: false,
            reason: Some("backoff".to_string()),
        };

        let state = build_recovery_state(Some(&existing), None, 10_000, None, 5_000);

        assert_eq!(state.attempt, MAX_AUTO_RETRY_ATTEMPTS + 1);
        assert_eq!(state.gave_up_at, Some(10_000));
        assert_eq!(state.reason.as_deref(), Some("retry_cap_exceeded"));
        assert!(state.next_retry_at.is_none());
    }

    #[test]
    fn marks_missing_agent_session_as_manual_only() {
        let state =
            build_recovery_state(None, None, 10_000, Some("missing_agent_session_id"), 5_000);

        assert!(state.unsafe_to_auto_resume);
        assert_eq!(state.reason.as_deref(), Some("missing_agent_session_id"));
        assert!(state.next_retry_at.is_none());
    }

    #[test]
    fn detects_unmatched_tool_call_as_auto_resume_blocker() {
        let store = Store::in_memory().unwrap();
        let session = Session::new_running("test", Path::new("/tmp")).with_agent("agent-session");
        store.create_session(&session).unwrap();
        store
            .add_session_message(&session.id, MessageRole::ToolCall, "{}")
            .unwrap();

        assert_eq!(
            auto_resume_blocker(&store, &session).unwrap(),
            Some("pending_tool_call".to_string())
        );

        store
            .add_session_message(&session.id, MessageRole::ToolResult, "{}")
            .unwrap();
        assert_eq!(auto_resume_blocker(&store, &session).unwrap(), None);
    }

    #[test]
    fn detects_unmatched_parallel_tool_calls_as_auto_resume_blocker() {
        let store = Store::in_memory().unwrap();
        let session = Session::new_running("test", Path::new("/tmp")).with_agent("agent-session");
        store.create_session(&session).unwrap();

        // Two parallel calls, one result: call B still has no result even
        // though the most recent message is a ToolResult.
        store
            .add_session_message(&session.id, MessageRole::ToolCall, "A")
            .unwrap();
        store
            .add_session_message(&session.id, MessageRole::ToolCall, "B")
            .unwrap();
        store
            .add_session_message(&session.id, MessageRole::ToolResult, "A")
            .unwrap();
        assert_eq!(
            auto_resume_blocker(&store, &session).unwrap(),
            Some("pending_tool_call".to_string())
        );

        store
            .add_session_message(&session.id, MessageRole::ToolResult, "B")
            .unwrap();
        assert_eq!(auto_resume_blocker(&store, &session).unwrap(), None);
    }

    #[test]
    fn ignores_unmatched_tool_calls_from_earlier_turns() {
        let store = Store::in_memory().unwrap();
        let session = Session::new_running("test", Path::new("/tmp")).with_agent("agent-session");
        store.create_session(&session).unwrap();

        // A past turn died mid-tool-call (an app quit, a crash): its ToolCall
        // row never gets a ToolResult, permanently.
        store
            .add_session_message(&session.id, MessageRole::User, "do the thing")
            .unwrap();
        store
            .add_session_message(&session.id, MessageRole::ToolCall, "A")
            .unwrap();

        // The turn that hit the limit starts at the next User message and
        // stopped outside any tool call, so the stale call above must not
        // downgrade this pause to manual resume.
        store
            .add_session_message(&session.id, MessageRole::User, "continue")
            .unwrap();
        store
            .add_session_message(&session.id, MessageRole::Assistant, "working on it")
            .unwrap();
        assert_eq!(auto_resume_blocker(&store, &session).unwrap(), None);

        // An unmatched call inside the limited turn itself still blocks.
        store
            .add_session_message(&session.id, MessageRole::ToolCall, "B")
            .unwrap();
        assert_eq!(
            auto_resume_blocker(&store, &session).unwrap(),
            Some("pending_tool_call".to_string())
        );
    }

    /// A branch whose only session is running, linked to a commit.
    fn branch_with_running_session(store: &Arc<Store>) -> (crate::store::Branch, Session) {
        use crate::store::{Branch, Commit, Project};

        let project = Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();

        let session = Session::new_running("rate limited", Path::new("/tmp"))
            .with_agent("agent-1")
            .with_branch(&branch.id);
        store.create_session(&session).unwrap();
        store
            .create_commit(&Commit::new_with_sha(&branch.id, "abc123").with_session(&session.id))
            .unwrap();

        (branch, session)
    }

    fn pause_with_rate_limit(store: &Store, session_id: &str) {
        assert!(store
            .transition_running_to_rate_limited(session_id, Some("rate limit"), Some("later"), None)
            .unwrap());
    }

    /// A branch with one rate-limited paused session linked to a commit.
    fn branch_with_paused_session(store: &Arc<Store>) -> (crate::store::Branch, Session) {
        let (branch, session) = branch_with_running_session(store);
        pause_with_rate_limit(store, &session.id);
        (branch, session)
    }

    #[test]
    fn defers_auto_resume_while_branch_has_a_running_session() {
        use crate::store::{Commit, CompletionReason, SessionStatus};

        let store = Arc::new(Store::in_memory().unwrap());
        let (branch, paused) = branch_with_paused_session(&store);

        let running =
            Session::new_running("new branch work", Path::new("/tmp")).with_branch(&branch.id);
        store.create_session(&running).unwrap();
        store
            .create_commit(&Commit::new_with_sha(&branch.id, "def456").with_session(&running.id))
            .unwrap();

        // A running session finishes on its own, so there is nothing to drain.
        match claim_for_auto_resume(&store, &paused.id).unwrap() {
            AutoResumeClaim::Deferred {
                drainable_branch_id,
                ..
            } => assert_eq!(drainable_branch_id, None),
            _ => panic!("expected a deferral while the branch has a running session"),
        }
        assert_eq!(
            store.get_session(&paused.id).unwrap().unwrap().status,
            SessionStatus::Paused
        );

        store
            .update_session_status(
                &running.id,
                SessionStatus::Completed,
                None,
                Some(&CompletionReason::TurnComplete),
            )
            .unwrap();
        assert!(matches!(
            claim_for_auto_resume(&store, &paused.id).unwrap(),
            AutoResumeClaim::Claimed(_)
        ));
    }

    #[test]
    fn drains_queued_branch_work_instead_of_waiting_for_it() {
        use crate::store::Commit;

        let store = Arc::new(Store::in_memory().unwrap());
        let (branch, paused) = branch_with_paused_session(&store);

        // Queued sessions never start on their own, and the pause path skipped
        // the drain cascade, so deferring alone would deadlock the branch.
        let queued = Session::new_queued("queued branch work").with_branch(&branch.id);
        store.create_session(&queued).unwrap();
        store
            .create_commit(&Commit::new_with_sha(&branch.id, "0789abc").with_session(&queued.id))
            .unwrap();

        match claim_for_auto_resume(&store, &paused.id).unwrap() {
            AutoResumeClaim::Deferred {
                drainable_branch_id,
                ..
            } => assert_eq!(drainable_branch_id.as_deref(), Some(branch.id.as_str())),
            _ => panic!("expected a deferral while the branch has queued work"),
        }
    }

    /// A queued push pipeline on `branch` gated on `depends_on`.
    fn queued_push_depending_on(
        store: &Arc<Store>,
        branch: &crate::store::Branch,
        depends_on: &str,
    ) -> Session {
        use crate::store::{PipelineExecution, PipelineKind};

        let mut queued = Session::new_queued("Push the current branch").with_branch(&branch.id);
        queued.pipeline = Some(
            PipelineExecution::from_steps(&[])
                .with_kind(PipelineKind::Push)
                .with_push_force(true)
                .with_depends_on_session_id(Some(depends_on.to_string())),
        );
        store.create_session(&queued).unwrap();
        queued
    }

    /// The overnight rebase-with-conflicts case: the rebase hit the limit with
    /// its force-push queued behind it. The push is gated on the rebase
    /// (`status_gate` waits on a paused dependency), so the drain a deferral
    /// spawns starts nothing and the two wait on each other forever. Rows
    /// gated on this session cannot run before it, so they are not a
    /// two-agents risk and the branch counts as free.
    #[test]
    fn claims_when_the_only_queued_work_depends_on_the_paused_session() {
        use crate::store::SessionStatus;

        let store = Arc::new(Store::in_memory().unwrap());
        let (branch, paused) = branch_with_paused_session(&store);
        let push = queued_push_depending_on(&store, &branch, &paused.id);
        // Pipelines chain: a row gated on the dependent is itself dependent.
        let chained = queued_push_depending_on(&store, &branch, &push.id);

        assert!(matches!(
            claim_for_auto_resume(&store, &paused.id).unwrap(),
            AutoResumeClaim::Claimed(_)
        ));
        assert_eq!(
            store.get_session(&paused.id).unwrap().unwrap().status,
            SessionStatus::Running
        );
        // The dependents stay queued for the resumed session's own terminal
        // drain; the claim neither starts nor skips them.
        for dependent in [&push.id, &chained.id] {
            assert_eq!(
                store.get_session(dependent).unwrap().unwrap().status,
                SessionStatus::Queued
            );
        }
    }

    /// Independent queued work still defers: the user may have queued a new
    /// prompt during the backoff, and resuming would put two agents in the
    /// worktree once the drain starts it.
    #[test]
    fn defers_when_queued_work_is_not_all_dependent_on_the_paused_session() {
        use crate::store::{Commit, SessionStatus};

        let store = Arc::new(Store::in_memory().unwrap());
        let (branch, paused) = branch_with_paused_session(&store);
        queued_push_depending_on(&store, &branch, &paused.id);
        let independent = Session::new_queued("queued branch work").with_branch(&branch.id);
        store.create_session(&independent).unwrap();
        store
            .create_commit(
                &Commit::new_with_sha(&branch.id, "0789abc").with_session(&independent.id),
            )
            .unwrap();

        match claim_for_auto_resume(&store, &paused.id).unwrap() {
            AutoResumeClaim::Deferred {
                drainable_branch_id,
                ..
            } => assert_eq!(drainable_branch_id.as_deref(), Some(branch.id.as_str())),
            _ => panic!("expected a deferral while the branch has independent queued work"),
        }
        assert_eq!(
            store.get_session(&paused.id).unwrap().unwrap().status,
            SessionStatus::Paused
        );
    }

    /// A dependency edge pointing at some other session — one that is not the
    /// paused session and not a queued row here — is independent work too.
    #[test]
    fn queued_rows_gated_on_another_session_are_not_dependents() {
        let store = Arc::new(Store::in_memory().unwrap());
        let (branch, paused) = branch_with_paused_session(&store);
        let other = Session::new_running("other", Path::new("/tmp"));
        store.create_session(&other).unwrap();
        let gated_elsewhere = queued_push_depending_on(&store, &branch, &other.id);
        let dependent = queued_push_depending_on(&store, &branch, &paused.id);

        assert!(queued_rows_all_depend_on(
            std::slice::from_ref(&dependent),
            &paused.id
        ));
        assert!(!queued_rows_all_depend_on(
            &[dependent, gated_elsewhere],
            &paused.id
        ));
    }

    /// The runner's pause path lands status and bookkeeping in one write, so
    /// the tick's `ensure_auto_recovery_state` finds attempt 1 already there
    /// and leaves it alone. With the fragment written after the flip, a tick
    /// between the two built attempt 1 itself and the pause path bumped it to
    /// 2 with a fresh `next_retry_at`.
    #[test]
    fn pause_lands_bookkeeping_with_the_status_flip() {
        use crate::store::{CompletionReason, SessionStatus};

        let store = Arc::new(Store::in_memory().unwrap());
        let (_branch, session) = branch_with_running_session(&store);

        assert!(pause_for_rate_limit(
            &store,
            &session.id,
            Some("rate limit"),
            Some("2026-06-24T15:45:00Z"),
            Some("claude_pro"),
            10_000,
        )
        .unwrap());

        let paused = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(paused.status, SessionStatus::Paused);
        assert_eq!(
            paused.completion_reason,
            Some(CompletionReason::RateLimited)
        );
        assert_eq!(
            paused.rate_limit_resets_at.as_deref(),
            Some("2026-06-24T15:45:00Z")
        );
        assert_eq!(paused.rate_limit_kind.as_deref(), Some("claude_pro"));
        let written = auto_recovery_from_metadata(paused.provider_metadata.as_ref())
            .expect("pause writes bookkeeping with the status flip");
        assert_eq!(written.attempt, 1);
        assert!(!written.unsafe_to_auto_resume);

        // What a tick does first: it must see the pause's own state, not build
        // a second one.
        let seen = ensure_auto_recovery_state(&store, &paused, 20_000)
            .unwrap()
            .unwrap();
        assert_eq!(seen, written);
        let after_tick = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(
            auto_recovery_from_metadata(after_tick.provider_metadata.as_ref()),
            Some(written)
        );
    }

    /// A pause that loses the `running → paused` race to a Stop writes
    /// nothing, so a cancelled row carries no stale bookkeeping.
    #[test]
    fn lost_pause_race_writes_no_bookkeeping() {
        use crate::store::{CompletionReason, SessionStatus};

        let store = Arc::new(Store::in_memory().unwrap());
        let (_branch, session) = branch_with_running_session(&store);
        store
            .update_session_status(
                &session.id,
                SessionStatus::Cancelled,
                None,
                Some(&CompletionReason::Interrupted),
            )
            .unwrap();

        assert!(
            !pause_for_rate_limit(&store, &session.id, Some("rate limit"), None, None, 10_000)
                .unwrap()
        );

        let row = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(row.status, SessionStatus::Cancelled);
        assert_eq!(
            auto_recovery_from_metadata(row.provider_metadata.as_ref()),
            None
        );
    }

    #[test]
    fn claims_the_paused_row_and_carries_its_limit_metadata() {
        let store = Arc::new(Store::in_memory().unwrap());
        let (_branch, paused) = branch_with_paused_session(&store);
        record_rate_limit_pause(&store, &paused.id, 10_000).unwrap();

        let ticket = match claim_for_auto_resume(&store, &paused.id).unwrap() {
            AutoResumeClaim::Claimed(ticket) => ticket,
            _ => panic!("expected the idle branch to yield a claim"),
        };
        assert_eq!(ticket.rate_limit_resets_at.as_deref(), Some("later"));
        assert!(ticket.queued_message.is_none());

        // The claim is the whole point: the row is running before the resume is
        // handed off, so a concurrent launch or drain can see it.
        let claimed = store.get_session(&paused.id).unwrap().unwrap();
        assert_eq!(claimed.status, crate::store::SessionStatus::Running);
        assert_eq!(claimed.rate_limit_resets_at, None);
        assert_eq!(
            auto_recovery_from_metadata(claimed.provider_metadata.as_ref())
                .map(|state| state.attempt),
            Some(1)
        );

        // A second tick sees its own claim as the branch's running work.
        assert!(matches!(
            claim_for_auto_resume(&store, &paused.id).unwrap(),
            AutoResumeClaim::Deferred { .. }
        ));
    }

    #[test]
    fn refuses_to_claim_a_session_that_left_the_paused_state() {
        use crate::store::{CompletionReason, SessionStatus};

        let store = Arc::new(Store::in_memory().unwrap());
        let (_branch, paused) = branch_with_paused_session(&store);

        // A manual resume that ran to completion between the scheduler's scan
        // and its claim keeps the session: nothing yanks it back to running.
        store
            .update_session_status(
                &paused.id,
                SessionStatus::Completed,
                None,
                Some(&CompletionReason::TurnComplete),
            )
            .unwrap();

        assert!(matches!(
            claim_for_auto_resume(&store, &paused.id).unwrap(),
            AutoResumeClaim::Lost
        ));
        assert_eq!(
            store.get_session(&paused.id).unwrap().unwrap().status,
            SessionStatus::Completed
        );
    }

    #[test]
    fn claims_a_backoff_queued_message_before_the_row() {
        let store = Arc::new(Store::in_memory().unwrap());
        let (_branch, session) = branch_with_running_session(&store);
        // Enqueueing requires a running session, so the message lands before
        // the limit does.
        store
            .add_queued_session_message(&session.id, "actually do this instead", &[], None)
            .unwrap();
        pause_with_rate_limit(&store, &session.id);

        let ticket = match claim_for_auto_resume(&store, &session.id).unwrap() {
            AutoResumeClaim::Claimed(ticket) => ticket,
            _ => panic!("expected the idle branch to yield a claim"),
        };

        // Claiming the row first would have made the message unclaimable:
        // `claim_oldest_queued_session_message` skips running sessions.
        assert_eq!(
            ticket
                .queued_message
                .as_ref()
                .map(|message| message.content.as_str()),
            Some("actually do this instead")
        );
    }

    #[test]
    fn preserves_only_staged_auto_recovery_metadata() {
        let metadata = json!({
            "acp": {
                "promptError": {
                    "data": {
                        "errorKind": "rate_limit"
                    }
                }
            },
            "staged": {
                "rateLimitAutoRecovery": {
                    "attempt": 2,
                    "maxAttempts": 8,
                    "nextRetryAt": 123,
                    "firstLimitedAt": 1,
                    "updatedAt": 2,
                    "gaveUpAt": null,
                    "unsafeToAutoResume": false,
                    "reason": "backoff"
                }
            }
        });

        let preserved = preserved_auto_recovery_metadata(Some(&metadata)).unwrap();
        assert!(preserved.pointer("/acp/promptError").is_none());
        assert_eq!(
            preserved
                .pointer("/staged/rateLimitAutoRecovery/attempt")
                .and_then(|value| value.as_u64()),
            Some(2)
        );
    }

    #[test]
    fn parses_offsets_to_utc_millis() {
        assert_eq!(
            parse_rfc3339_millis("2026-06-24T15:45:00+10:00"),
            parse_rfc3339_millis("2026-06-24T05:45:00Z")
        );
    }

    #[test]
    fn retry_delay_bound_caps_exponential_backoff() {
        assert_eq!(retry_delay_bound_ms(1), BASE_BACKOFF_MS);
        assert_eq!(retry_delay_bound_ms(2), BASE_BACKOFF_MS * 2);
        assert_eq!(retry_delay_bound_ms(99), MAX_BACKOFF_MS);
    }

    #[test]
    fn implausibly_distant_resets_fall_back_to_backoff() {
        let now = parse_rfc3339_millis("2026-06-24T15:00:00Z").unwrap();
        // A row written before numeric normalization existed can hold an epoch
        // in the wrong scale (microseconds read as millis land tens of
        // millennia out). Waiting for it would park the session forever: the
        // 24h give-up is only evaluated when recovery state is rebuilt.
        let micros = (parse_rfc3339_millis("2026-06-24T15:45:00Z").unwrap() * 1_000).to_string();
        let state = build_recovery_state(None, Some(&micros), now, None, 12_000);
        assert_eq!(state.reason.as_deref(), Some("backoff"));
        assert_eq!(jitter_bound_for(Some(&micros), now), MAX_BACKOFF_MS);

        // The cap is on lead time, not scale: just beyond the window falls
        // back, just within it is honored.
        let beyond_window = (now + MAX_PLAUSIBLE_RESET_LEAD_MS + 60_000).to_string();
        let state = build_recovery_state(None, Some(&beyond_window), now, None, 12_000);
        assert_eq!(state.reason.as_deref(), Some("backoff"));

        let within_window = (now + MAX_PLAUSIBLE_RESET_LEAD_MS - 60_000).to_string();
        let state = build_recovery_state(None, Some(&within_window), now, None, 12_000);
        assert_eq!(state.reason.as_deref(), Some("reset_at"));
        assert_eq!(jitter_bound_for(Some(&within_window), now), RESET_JITTER_MS);
    }

    #[test]
    fn successful_completion_clears_retry_bookkeeping_for_the_next_episode() {
        use crate::store::{CompletionReason, SessionStatus};

        let store = Arc::new(Store::in_memory().unwrap());
        let (_branch, session) = branch_with_paused_session(&store);
        record_rate_limit_pause(&store, &session.id, 10_000).unwrap();

        // The auto-resume claim carries the bookkeeping onto the running row
        // so a run that pauses again keeps its attempt count.
        assert!(matches!(
            claim_for_auto_resume(&store, &session.id).unwrap(),
            AutoResumeClaim::Claimed(_)
        ));
        store
            .merge_session_provider_metadata(
                &session.id,
                &json!({ "acp": { "promptResponse": { "stopReason": "end_turn" } } }),
            )
            .unwrap();

        // The resumed run completing ends the episode: the retry budget must
        // not carry into the next limit hit, or attempts accumulate across
        // separate episodes until an unattended run gives up despite making
        // real progress. Unrelated provider metadata survives.
        assert!(store
            .transition_from_running(
                &session.id,
                SessionStatus::Completed,
                None,
                Some(&CompletionReason::TurnComplete),
            )
            .unwrap());
        let completed = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(
            auto_recovery_from_metadata(completed.provider_metadata.as_ref()),
            None
        );
        assert!(completed
            .provider_metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/acp/promptResponse"))
            .is_some());
    }

    #[test]
    fn failed_runs_keep_retry_bookkeeping() {
        use crate::store::{CompletionReason, SessionStatus};

        let store = Arc::new(Store::in_memory().unwrap());
        let (_branch, session) = branch_with_paused_session(&store);
        record_rate_limit_pause(&store, &session.id, 10_000).unwrap();
        assert!(matches!(
            claim_for_auto_resume(&store, &session.id).unwrap(),
            AutoResumeClaim::Claimed(_)
        ));

        // A crash is not a successful episode end: the budget survives so
        // repeated failures still hit the retry cap and the 24h window.
        assert!(store
            .transition_from_running(
                &session.id,
                SessionStatus::Error,
                Some("boom"),
                Some(&CompletionReason::Crashed),
            )
            .unwrap());
        let errored = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(
            auto_recovery_from_metadata(errored.provider_metadata.as_ref())
                .map(|state| state.attempt),
            Some(1)
        );
    }

    #[test]
    fn parses_numeric_epoch_reset_timestamps_by_magnitude() {
        let millis = parse_rfc3339_millis("2026-06-24T15:45:00Z").unwrap();

        assert_eq!(
            parse_reset_timestamp_millis(&(millis / 1_000).to_string()),
            Some(millis)
        );
        assert_eq!(
            parse_reset_timestamp_millis(&millis.to_string()),
            Some(millis)
        );
        assert_eq!(
            parse_reset_timestamp_millis(" 2026-06-24T15:45:00Z "),
            Some(millis)
        );
        assert_eq!(parse_reset_timestamp_millis("0"), None);
        assert_eq!(parse_reset_timestamp_millis("-1766591100"), None);
        assert_eq!(parse_reset_timestamp_millis("later"), None);
    }

    #[test]
    fn tick_loop_claim_is_once_per_scheduler() {
        let scheduler = RateLimitScheduler::new();
        assert!(scheduler.try_claim_tick_loop());
        assert!(!scheduler.try_claim_tick_loop());
    }

    #[test]
    fn start_failure_schedules_a_backoff_retry_instead_of_giving_up() {
        let store = Store::in_memory().unwrap();
        let session = Session::new_running("test", Path::new("/tmp")).with_agent("agent-session");
        store.create_session(&session).unwrap();
        pause_with_rate_limit(&store, &session.id);
        record_rate_limit_pause(&store, &session.id, 10_000).unwrap();

        // The failed attempt re-parked the session as paused; one failure
        // burns an attempt, not the whole recovery.
        reschedule_after_start_failure(&store, &session.id, 20_000);

        let session = store.get_session(&session.id).unwrap().unwrap();
        let state = auto_recovery_from_metadata(session.provider_metadata.as_ref()).unwrap();
        assert_eq!(state.attempt, 2);
        assert_eq!(state.gave_up_at, None);
        assert!(state.next_retry_at.is_some());
    }

    #[test]
    fn repeated_start_failures_still_hit_the_retry_cap() {
        let store = Store::in_memory().unwrap();
        let session = Session::new_running("test", Path::new("/tmp")).with_agent("agent-session");
        store.create_session(&session).unwrap();
        pause_with_rate_limit(&store, &session.id);
        store
            .merge_session_provider_metadata(
                &session.id,
                &metadata_for_state(&RateLimitAutoRecovery {
                    attempt: MAX_AUTO_RETRY_ATTEMPTS,
                    max_attempts: MAX_AUTO_RETRY_ATTEMPTS,
                    next_retry_at: Some(10_000),
                    first_limited_at: 5_000,
                    updated_at: 10_000,
                    gave_up_at: None,
                    unsafe_to_auto_resume: false,
                    reason: Some("backoff".to_string()),
                }),
            )
            .unwrap();

        reschedule_after_start_failure(&store, &session.id, 20_000);

        let session = store.get_session(&session.id).unwrap().unwrap();
        let state = auto_recovery_from_metadata(session.provider_metadata.as_ref()).unwrap();
        assert_eq!(state.gave_up_at, Some(20_000));
        assert_eq!(state.reason.as_deref(), Some("retry_cap_exceeded"));
    }

    #[test]
    fn start_failure_leaves_sessions_claimed_by_another_path_alone() {
        let store = Store::in_memory().unwrap();
        let session = Session::new_running("test", Path::new("/tmp")).with_agent("agent-session");
        store.create_session(&session).unwrap();

        // Still running: a manual resume won the race after the failed start.
        reschedule_after_start_failure(&store, &session.id, 20_000);

        let session = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(
            auto_recovery_from_metadata(session.provider_metadata.as_ref()),
            None
        );
    }

    #[test]
    fn backlog_counts_only_sessions_the_scheduler_can_still_act_on() {
        let mut session = Session::new_running("test", Path::new("/tmp"));

        // No bookkeeping yet: the next tick will create it, so it counts.
        assert!(is_actionable_for_auto_resume(&session));

        let state = RateLimitAutoRecovery {
            attempt: 1,
            max_attempts: MAX_AUTO_RETRY_ATTEMPTS,
            next_retry_at: Some(10_000),
            first_limited_at: 5_000,
            updated_at: 5_000,
            gave_up_at: None,
            unsafe_to_auto_resume: false,
            reason: Some("backoff".to_string()),
        };
        session.provider_metadata = Some(metadata_for_state(&state));
        assert!(is_actionable_for_auto_resume(&session));

        session.provider_metadata = Some(metadata_for_state(&RateLimitAutoRecovery {
            gave_up_at: Some(10_000),
            ..state.clone()
        }));
        assert!(!is_actionable_for_auto_resume(&session));

        session.provider_metadata = Some(metadata_for_state(&RateLimitAutoRecovery {
            unsafe_to_auto_resume: true,
            ..state
        }));
        assert!(!is_actionable_for_auto_resume(&session));
    }
}
