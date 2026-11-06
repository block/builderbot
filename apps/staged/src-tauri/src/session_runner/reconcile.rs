//! Reconciling `disconnected` Blox sessions with their workstations.
//!
//! A `disconnected` row (see [`super::disconnect`]) is a Blox session whose
//! turn may still be running on its workstation with no Staged process
//! attached. Nothing else moves it on: it holds its branch's queue, and Resume
//! refuses it. This service asks Blox what became of the turn and settles the
//! row from the answer:
//!
//! - the workstation is gone: `error`, "The workstation no longer exists";
//! - the workstation can't be reached, Blox is signed out, or the workstation
//!   is suspended or otherwise not running: the row stays `disconnected` with
//!   a [`ReconcileReason`], and the next attempt backs off;
//! - the session's process is still running the turn: the row is claimed
//!   `running` again, the transcript caught up, and the turn followed over a
//!   fresh attach to its end, then settled like any other run;
//! - the turn is over: the transcript is caught up from the agent's replay
//!   and the row settled `completed` (running the commit, note and review
//!   hooks against the turn's persisted base), `cancelled` or `error`, as the
//!   process and its recorded output say;
//! - the turn is over but the session is still working (background tasks it
//!   started are live, or a continuation cycle they woke is running), which
//!   the process reading `IDLE` does not rule out: the row stays
//!   `disconnected` and is checked again ([`RowOutcome::Held`]) until the
//!   work ends, as the live post-turn hold waits for it, and within the same
//!   cap and ceiling (see [`Reconciler::held_work_ran_out`]);
//! - nothing says how the turn ended: `cancelled`/`interrupted` with a
//!   message — resumable — rather than a guessed `completed`.
//!
//! Reconcile runs on launch ([`reconcile_all`]), from a sweep every
//! [`SWEEP_INTERVAL`] that respects each row's backoff, after a successful
//! Blox call for the row's workstation ([`note_workspace_reachable`]), and on
//! request ([`reconcile_session`], for a "Check now"). Every write is a
//! compare-and-swap on the row as it was read, so a Stop, or another Staged
//! instance's reconcile, that got there first wins, and running the same
//! snapshot twice changes nothing the second time.
//!
//! A Stop on a `disconnected` row is handled here too
//! ([`Reconciler::request_stop`]): it cancels the remote turn over a fresh
//! connection before ending the row (see [`stop`]).

mod blox;
mod host;
mod row;
mod stop;
#[cfg(test)]
mod tests;

pub(crate) use blox::{
    AttachControl, LiveBlox, ReconcileBlox, ReconcileTarget, UnreachableKind, WorkspaceProbe,
};
pub(crate) use host::{LiveHost, ReconcileHost};
pub(crate) use stop::{
    cancel_abandoned_turn, stop_backfill_finished, stop_outliving_work, stop_unacknowledged_turn,
};

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use stop::HeldWork;

use super::SessionRegistry;
use crate::store::{now_timestamp, CompletionReason, Store};

/// How often the sweep looks for `disconnected` rows that are due.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
/// The wait after a row's first failed attempt; it doubles with each further
/// failure, up to [`BACKOFF_CAP`].
const BACKOFF_BASE: Duration = Duration::from_secs(60);
const BACKOFF_CAP: Duration = Duration::from_secs(30 * 60);
/// The least time between two attempts a successful Blox call can set off for
/// one row: a burst of `ws exec`s (a diff load) should not attach once each.
const NUDGE_MIN_SPACING: Duration = Duration::from_secs(30);
/// The wait before checking again on a row whose session is still working
/// after its turn; it doubles with each further check that finds the work
/// still going, up to [`HELD_WORK_RECHECK_CAP`]. Not a failure backoff: the
/// work can end at any moment and the branch's queue waits on it, so the cap
/// is lower. Each check reads the process's recording (one `ws output`).
const HELD_WORK_RECHECK: Duration = Duration::from_secs(60);
const HELD_WORK_RECHECK_CAP: Duration = Duration::from_secs(5 * 60);

/// What the reconcile service is doing about a `disconnected` row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconcilePhase {
    /// Asking Blox about the workstation and the session's process.
    Checking,
    /// Cancelling the remote turn for a Stop.
    Stopping,
    /// The last attempt couldn't settle the row; the next is at
    /// `nextAttemptAt` (or sooner, on a successful Blox call for the
    /// workstation or a "Check now").
    Waiting,
    /// Nothing in progress or pending: the row has settled, or this process
    /// hasn't looked at it yet.
    #[default]
    Idle,
}

/// Why the last attempt left a row `disconnected`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileReasonKind {
    /// Blox didn't answer, or answered with an error.
    Unreachable,
    /// The `sq` CLI is signed out.
    NotAuthenticated,
    /// The workstation is suspended. Its processes are stopped while it is,
    /// so the turn can only be settled once it runs again.
    Suspended,
    /// The workstation is starting, stopping, stopped or degraded.
    WorkstationNotRunning,
    /// The workstation answered, but attaching to the session failed.
    AttachFailed,
    /// The turn is over, but the session is still working on the workstation:
    /// background tasks it started are live, or a continuation cycle they
    /// woke is running. Not a failure; the row settles once the work ends.
    BackgroundWork,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReconcileReason {
    pub kind: ReconcileReasonKind,
    /// Human-readable detail, e.g. the `sq` error.
    pub message: String,
}

impl ReconcileReason {
    fn new(kind: ReconcileReasonKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// The `session-reconcile-changed` event, and one entry of
/// [`get_session_reconcile_states`]: where reconciling one `disconnected`
/// session stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionReconcileEvent {
    pub session_id: String,
    pub branch_id: Option<String>,
    pub project_id: Option<String>,
    pub phase: ReconcilePhase,
    /// Why the last attempt couldn't settle the row; `None` before the first
    /// failure and once the row settles.
    pub reason: Option<ReconcileReason>,
    /// Failed attempts since the row last made progress.
    pub attempts: u32,
    /// When the sweep will next try, in ms since the epoch, while `Waiting`.
    pub next_attempt_at: Option<i64>,
}

/// What started an attempt; it decides whether backoff applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// App launch: every `disconnected` row, once.
    Launch,
    /// The periodic sweep: rows whose backoff has passed.
    Sweep,
    /// A Blox call for the row's workstation just succeeded: rows that have
    /// failed before, spaced by [`NUDGE_MIN_SPACING`].
    WorkspaceReachable,
    /// The user asked ("Check now"): regardless of backoff.
    CheckNow,
}

/// The reconcile service's memory of one row, for this process. Rows it has
/// never looked at have none.
#[derive(Debug, Default)]
struct RowState {
    /// An attempt or a Stop is running for the row; nothing else starts.
    in_flight: bool,
    /// A Stop arrived while an attempt was in flight, before it claimed the
    /// row: the attempt turns into the Stop, with this completion reason, at
    /// its next checkpoint.
    stop_requested: Option<CompletionReason>,
    phase: ReconcilePhase,
    failures: u32,
    reason: Option<ReconcileReason>,
    last_attempt: Option<Instant>,
    next_attempt: Option<Instant>,
    next_attempt_at: Option<i64>,
    /// How long the session has been working after its turn, as attempts
    /// found it ([`RowOutcome::Held`]), for [`Reconciler::held_work_ran_out`].
    held: Option<HeldClock>,
    /// Checks in a row that found the work still going, for the recheck
    /// spacing.
    held_checks: u32,
    /// Learnt on the first attempt, for [`note_workspace_reachable`] and the
    /// event's routing fields.
    workspace_name: Option<String>,
    branch_id: Option<String>,
    project_id: Option<String>,
}

/// The clock on a session still working after its turn, in ms since the
/// epoch.
#[derive(Debug, Clone)]
struct HeldClock {
    /// When the hold began: the turn's end, as far as Blox dates it.
    entered_at: i64,
    /// The background tasks last seen live, sorted.
    tasks: Vec<String>,
    /// Since when exactly those have been live.
    since: i64,
}

/// How an attempt left its row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RowOutcome {
    /// The row is settled, or no longer `disconnected` for some other reason:
    /// forget it.
    Done,
    /// Still `disconnected`, for `reason`: back off.
    Backoff(ReconcileReason),
    /// Still `disconnected` because the attempt stood down (the app is
    /// quitting), not because anything failed.
    Left,
    /// Still `disconnected` because the session is still working after its
    /// turn (a [`ReconcileReasonKind::BackgroundWork`] reason): check again
    /// soon, without counting a failure.
    Held(ReconcileReason),
}

/// The reconcile service: per-row state plus what it reconciles through.
pub struct Reconciler {
    store: Arc<Store>,
    registry: Arc<SessionRegistry>,
    blox: Arc<dyn ReconcileBlox>,
    host: Arc<dyn ReconcileHost>,
    rows: Mutex<HashMap<String, RowState>>,
    /// How long the same background tasks may keep a row held, and how long
    /// a row may stay held at all from its turn's end, before it settles
    /// regardless: the live post-turn hold's own cap and ceiling (see
    /// [`acp_client::BackgroundHoldConfig`]). A hung background shell must
    /// not hold the branch forever, and honest work never reaches either.
    held_work_cap: Duration,
    held_work_ceiling: Duration,
    /// The time, in ms since the epoch ([`now_timestamp`]; tests move it).
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl Reconciler {
    pub(crate) fn new(
        store: Arc<Store>,
        registry: Arc<SessionRegistry>,
        blox: Arc<dyn ReconcileBlox>,
        host: Arc<dyn ReconcileHost>,
    ) -> Self {
        Self {
            store,
            registry,
            blox,
            host,
            rows: Mutex::new(HashMap::new()),
            held_work_cap: acp_client::BackgroundHoldConfig::default().hold_cap,
            held_work_ceiling: acp_client::BackgroundHoldConfig::default().hold_ceiling,
            clock: Arc::new(now_timestamp),
        }
    }

    /// [`Self::new`] with a different [`Self::held_work_cap`] and
    /// [`Self::held_work_ceiling`] (tests).
    #[cfg(test)]
    pub(super) fn with_held_work_limits(mut self, cap: Duration, ceiling: Duration) -> Self {
        self.held_work_cap = cap;
        self.held_work_ceiling = ceiling;
        self
    }

    /// [`Self::new`] telling the time from `clock` (tests).
    #[cfg(test)]
    pub(super) fn with_clock(mut self, clock: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// Start an attempt for `session_id` on its own thread, if `trigger`
    /// allows one now. Returns whether one started.
    pub(crate) fn spawn_reconcile(self: &Arc<Self>, session_id: &str, trigger: Trigger) -> bool {
        if !self.begin(session_id, trigger, Instant::now()) {
            return false;
        }
        let this = Arc::clone(self);
        let session_id = session_id.to_string();
        std::thread::spawn(move || this.run_attempt(&session_id));
        true
    }

    /// [`Self::spawn_reconcile`] on the calling thread.
    #[cfg(test)]
    pub(super) fn reconcile_blocking(&self, session_id: &str, trigger: Trigger) -> bool {
        if !self.begin(session_id, trigger, Instant::now()) {
            return false;
        }
        self.run_attempt(session_id);
        true
    }

    /// Try every `disconnected` row `trigger` allows, after forgetting the
    /// rows that no longer are.
    pub(crate) fn reconcile_due(self: &Arc<Self>, trigger: Trigger) -> usize {
        let rows = match self.store.get_disconnected_sessions() {
            Ok(rows) => rows,
            Err(e) => {
                log::warn!("[reconcile] Failed to list disconnected sessions: {e}");
                return 0;
            }
        };
        self.forget_rows_not_in(rows.iter().map(|row| row.id.as_str()).collect());
        rows.iter()
            .filter(|row| self.spawn_reconcile(&row.id, trigger))
            .count()
    }

    /// Forget every row this process remembers that is not `disconnected`
    /// any more and has nothing in flight, announcing each `idle`. A row
    /// moves on without an attempt of this process's settling it — a resume
    /// or delete, another Staged instance's reconcile, a recovery sweep —
    /// and its last attempt's state (`Waiting`, a failure reason) would
    /// otherwise stay on in [`Self::states`] and in the UI.
    fn forget_rows_not_in(&self, disconnected: HashSet<&str>) {
        let events: Vec<SessionReconcileEvent> = {
            let mut rows = self.rows.lock().unwrap();
            let stale: Vec<String> = rows
                .iter()
                .filter(|(id, row)| !row.in_flight && !disconnected.contains(id.as_str()))
                .map(|(id, _)| id.clone())
                .collect();
            stale
                .into_iter()
                .filter_map(|id| rows.remove(&id).map(|row| forgotten(&id, row)))
                .collect()
        };
        for event in &events {
            log::info!(
                "[reconcile] Session {} is no longer disconnected; forgetting it",
                event.session_id
            );
            self.host.emit_reconcile(event);
        }
    }

    /// A Blox call for `workspace_name` succeeded: retry the rows on it that
    /// failed before, rather than leaving them to their backoff.
    pub(crate) fn workspace_reachable(self: &Arc<Self>, workspace_name: &str) {
        let candidates: Vec<String> = self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, row)| {
                row.failures > 0 && row.workspace_name.as_deref() == Some(workspace_name)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for session_id in candidates {
            self.spawn_reconcile(&session_id, Trigger::WorkspaceReachable);
        }
    }

    /// A Stop on a `disconnected` row: cancel the remote turn over a fresh
    /// connection, then end the row `cancelled` for `reason` (see [`stop`]).
    /// An attempt already in flight takes the Stop over instead.
    pub(crate) fn request_stop(self: &Arc<Self>, session_id: &str, reason: CompletionReason) {
        if !self.begin_stop(session_id, &reason) {
            return;
        }
        let this = Arc::clone(self);
        let session_id = session_id.to_string();
        std::thread::spawn(move || this.run_stop(&session_id, reason, HeldWork::Unknown));
    }

    /// [`Self::request_stop`] on the calling thread.
    #[cfg(test)]
    pub(super) fn stop_blocking(&self, session_id: &str, reason: CompletionReason) {
        if self.begin_stop(session_id, &reason) {
            self.run_stop(session_id, reason, HeldWork::Unknown);
        }
    }

    /// Where reconciling each row this process knows about stands.
    pub(crate) fn states(&self) -> Vec<SessionReconcileEvent> {
        let rows = self.rows.lock().unwrap();
        rows.iter().map(|(id, row)| event_for(id, row)).collect()
    }

    /// Claim the row for an attempt, if none is in flight and `trigger`
    /// allows one at `now`.
    fn begin(&self, session_id: &str, trigger: Trigger, now: Instant) -> bool {
        if self.host.is_quitting() {
            return false;
        }
        let mut rows = self.rows.lock().unwrap();
        let row = rows.entry(session_id.to_string()).or_default();
        if row.in_flight {
            return false;
        }
        let allowed = match trigger {
            Trigger::Launch | Trigger::CheckNow => true,
            Trigger::Sweep => row.next_attempt.map_or(true, |next| next <= now),
            Trigger::WorkspaceReachable => {
                row.failures > 0
                    && row
                        .last_attempt
                        .map_or(true, |last| now.duration_since(last) >= NUDGE_MIN_SPACING)
            }
        };
        if !allowed {
            return false;
        }
        row.in_flight = true;
        row.last_attempt = Some(now);
        true
    }

    /// Claim the row for a Stop, or hand the Stop to the attempt in flight.
    ///
    /// The attempt looks for the Stop at checkpoints up to the one just after
    /// it registers to claim the row; past that, the Stop reaches it through
    /// its registry entry. Both are done under the rows lock, and the attempt
    /// registers before that last checkpoint, so one of the two always lands.
    fn begin_stop(&self, session_id: &str, reason: &CompletionReason) -> bool {
        let mut rows = self.rows.lock().unwrap();
        let row = rows.entry(session_id.to_string()).or_default();
        if row.in_flight {
            // As in the registry, a project session's Stop wins over another.
            if row.stop_requested.is_none()
                || *reason == CompletionReason::ProjectSessionInterrupted
            {
                row.stop_requested = Some(reason.clone());
            }
            self.registry
                .cancel_with_completion_reason(session_id, reason.clone());
            return false;
        }
        row.in_flight = true;
        true
    }

    fn run_attempt(&self, session_id: &str) {
        self.set_phase(session_id, ReconcilePhase::Checking);
        let outcome = self.reconcile_row(session_id);
        let held = match outcome {
            RowOutcome::Held(_) => HeldWork::Held,
            _ => HeldWork::Unknown,
        };
        if let Some(reason) = self.finish(session_id, outcome) {
            self.run_stop(session_id, reason, held);
        }
    }

    fn run_stop(&self, session_id: &str, reason: CompletionReason, held: HeldWork) {
        let outcome = self.stop_disconnected(session_id, reason, held);
        // A Stop always ends the row, so nothing is handed back.
        self.finish(session_id, outcome);
    }

    /// Whether a Stop is waiting for the attempt in flight; taking it clears
    /// it.
    fn take_stop_request(&self, session_id: &str) -> Option<CompletionReason> {
        self.rows
            .lock()
            .unwrap()
            .get_mut(session_id)
            .and_then(|row| row.stop_requested.take())
    }

    /// Record where a resolved row lives, for nudges and event routing.
    fn note_target(&self, target: &ReconcileTarget) {
        if let Some(row) = self.rows.lock().unwrap().get_mut(&target.session_id) {
            row.workspace_name = Some(target.workspace_name.clone());
            row.branch_id = target.branch_id.clone();
            row.project_id = target.project_id.clone();
        }
    }

    fn set_phase(&self, session_id: &str, phase: ReconcilePhase) {
        let event = {
            let mut rows = self.rows.lock().unwrap();
            let Some(row) = rows.get_mut(session_id) else {
                return;
            };
            if row.phase == phase {
                return;
            }
            row.phase = phase;
            event_for(session_id, row)
        };
        self.host.emit_reconcile(&event);
    }

    /// Record how an attempt (or Stop) left its row, and announce it.
    /// Record how an attempt or Stop left the row. A Stop that arrived after
    /// the attempt's last look, on a row it left `disconnected`, is handed
    /// back with the row still in flight, for the caller to run next.
    fn finish(&self, session_id: &str, outcome: RowOutcome) -> Option<CompletionReason> {
        let event = {
            let mut rows = self.rows.lock().unwrap();
            if outcome != RowOutcome::Done {
                let pending_stop = rows
                    .get_mut(session_id)
                    .and_then(|row| row.stop_requested.take());
                if pending_stop.is_some() {
                    return pending_stop;
                }
            }
            match outcome {
                RowOutcome::Done => rows
                    .remove(session_id)
                    .map(|row| forgotten(session_id, row)),
                RowOutcome::Backoff(reason) => rows.get_mut(session_id).map(|row| {
                    row.in_flight = false;
                    row.failures += 1;
                    let delay = backoff(row.failures);
                    row.next_attempt = Some(Instant::now() + delay);
                    row.next_attempt_at = Some(now_timestamp() + delay.as_millis() as i64);
                    log::info!(
                        "[reconcile] Session {session_id} stays disconnected ({:?}: {}); \
                         retrying in {}s",
                        reason.kind,
                        reason.message,
                        delay.as_secs()
                    );
                    row.reason = Some(reason);
                    row.phase = ReconcilePhase::Waiting;
                    event_for(session_id, row)
                }),
                RowOutcome::Held(reason) => rows.get_mut(session_id).map(|row| {
                    let now = Instant::now();
                    row.in_flight = false;
                    // The workstation answered: whatever failed before has
                    // passed.
                    row.failures = 0;
                    row.held_checks += 1;
                    let delay = held_work_recheck(row.held_checks);
                    row.next_attempt = Some(now + delay);
                    row.next_attempt_at = Some(now_timestamp() + delay.as_millis() as i64);
                    log::info!(
                        "[reconcile] Session {session_id} stays disconnected: {}; checking \
                         again in {}s",
                        reason.message,
                        delay.as_secs()
                    );
                    row.reason = Some(reason);
                    row.phase = ReconcilePhase::Waiting;
                    event_for(session_id, row)
                }),
                RowOutcome::Left => rows.get_mut(session_id).map(|row| {
                    row.in_flight = false;
                    row.phase = if row.failures > 0 {
                        ReconcilePhase::Waiting
                    } else {
                        ReconcilePhase::Idle
                    };
                    event_for(session_id, row)
                }),
            }
        };
        if let Some(event) = event {
            self.host.emit_reconcile(&event);
        }
        None
    }
}

impl Reconciler {
    /// Whether the session's work, found still going with `tasks` live, has
    /// held the row as long as the live hold would have waited for it: the
    /// same tasks live for [`Self::held_work_cap`], or the row held for
    /// [`Self::held_work_ceiling`] from the turn's end. The attempt then
    /// settles the row with the work still going (`held_until_cap`).
    ///
    /// The live hold times each task from when it first sees it; a
    /// recording only says which tasks are live now, so the cap runs from
    /// when this set was first seen, and starts over when it changes (a task
    /// settled, or a continuation cycle started another). The first sighting
    /// after a launch dates the hold from `turn_end` — when Blox last saw the
    /// process go idle — rather than from now, so a relaunch doesn't hand a
    /// hung task a fresh cap. (A task a continuation cycle started after the
    /// turn's end gets that much less of its cap, after a relaunch only.)
    fn held_work_ran_out(
        &self,
        session_id: &str,
        tasks: Vec<String>,
        turn_end: Option<i64>,
    ) -> bool {
        let now = (self.clock)();
        let mut rows = self.rows.lock().unwrap();
        let Some(row) = rows.get_mut(session_id) else {
            return false;
        };
        let clock = match &mut row.held {
            Some(clock) => {
                if clock.tasks != tasks {
                    clock.tasks = tasks;
                    clock.since = now;
                }
                clock
            }
            None => {
                let entered_at = turn_end.map_or(now, |end| end.min(now));
                row.held.insert(HeldClock {
                    entered_at,
                    tasks,
                    since: entered_at,
                })
            }
        };
        let held_for = |since: i64| Duration::from_millis(now.saturating_sub(since).max(0) as u64);
        held_for(clock.since) >= self.held_work_cap
            || held_for(clock.entered_at) >= self.held_work_ceiling
    }
}

/// The `idle` event for a row the reconciler forgets.
fn forgotten(session_id: &str, mut row: RowState) -> SessionReconcileEvent {
    row.phase = ReconcilePhase::Idle;
    row.reason = None;
    row.failures = 0;
    row.next_attempt_at = None;
    event_for(session_id, &row)
}

fn event_for(session_id: &str, row: &RowState) -> SessionReconcileEvent {
    SessionReconcileEvent {
        session_id: session_id.to_string(),
        branch_id: row.branch_id.clone(),
        project_id: row.project_id.clone(),
        phase: row.phase,
        reason: row.reason.clone(),
        attempts: row.failures,
        next_attempt_at: row.next_attempt_at,
    }
}

/// The wait after a row's `failures`-th failed attempt in a row.
fn backoff(failures: u32) -> Duration {
    doubling(BACKOFF_BASE, failures, BACKOFF_CAP)
}

/// The wait after the `checks`-th check in a row that found a session still
/// working after its turn.
fn held_work_recheck(checks: u32) -> Duration {
    doubling(HELD_WORK_RECHECK, checks, HELD_WORK_RECHECK_CAP)
}

/// `base` doubled for each of `nth` after the first, up to `cap`.
fn doubling(base: Duration, nth: u32, cap: Duration) -> Duration {
    let doublings = nth.saturating_sub(1).min(16);
    base.saturating_mul(1 << doublings).min(cap)
}

// =============================================================================
// The process's reconciler, and its triggers
// =============================================================================

/// The reconciler for the current store. A process-wide slot rather than Tauri
/// state because the Blox wrappers that nudge it ([`note_workspace_reachable`])
/// have no app handle; replaced when the store is.
static ACTIVE: Mutex<Option<Arc<Reconciler>>> = Mutex::new(None);

pub(crate) fn active() -> Option<Arc<Reconciler>> {
    ACTIVE.lock().unwrap().clone()
}

/// Install the reconciler for a freshly opened (or reset) store, and start its
/// sweep. The launch pass is [`reconcile_all`], run once startup recovery has
/// left every orphaned Blox row `disconnected`.
pub(crate) fn install(
    store: Arc<Store>,
    registry: Arc<SessionRegistry>,
    app_handle: tauri::AppHandle,
) {
    let host = Arc::new(LiveHost::new(
        app_handle,
        Arc::clone(&store),
        Arc::clone(&registry),
    ));
    let reconciler = Arc::new(Reconciler::new(store, registry, Arc::new(LiveBlox), host));
    let first = ACTIVE.lock().unwrap().replace(reconciler).is_none();
    if first {
        spawn_sweep();
    }
}

/// Reconcile every `disconnected` row now; the launch pass.
pub(crate) fn reconcile_all() {
    if let Some(reconciler) = active() {
        let started = reconciler.reconcile_due(Trigger::Launch);
        if started > 0 {
            log::info!("[reconcile] Checking {started} disconnected Blox session(s)");
        }
    }
}

/// A Blox call for `workspace_name` just succeeded, so the workstation is
/// reachable: rows on it that failed to reconcile are worth another try.
/// Cheap when there are none.
pub(crate) fn note_workspace_reachable(workspace_name: &str) {
    if let Some(reconciler) = active() {
        reconciler.workspace_reachable(workspace_name);
    }
}

/// Re-run the sweep for the life of the process, always on the current
/// reconciler.
fn spawn_sweep() {
    tauri::async_runtime::spawn(async {
        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick completes immediately; the launch pass covers it.
        interval.tick().await;
        loop {
            interval.tick().await;
            let Some(reconciler) = active() else {
                continue;
            };
            // Listing the rows is a store read; the attempts run on their own
            // threads.
            if let Err(e) = tauri::async_runtime::spawn_blocking(move || {
                reconciler.reconcile_due(Trigger::Sweep);
            })
            .await
            {
                log::error!("[reconcile] Sweep panicked: {e}");
            }
        }
    });
}

// =============================================================================
// Commands
// =============================================================================

/// Check a `disconnected` session against its workstation now, ignoring its
/// backoff ("Check now"). Returns at once; progress arrives as
/// `session-reconcile-changed` and `session-status-changed` events. A no-op
/// when an attempt is already in flight.
#[tauri::command]
pub fn reconcile_session(session_id: String) -> Result<(), String> {
    let reconciler = active().ok_or_else(|| "Database not initialized".to_string())?;
    reconciler.spawn_reconcile(&session_id, Trigger::CheckNow);
    Ok(())
}

/// Where reconciling each `disconnected` session stands, for a client that
/// missed the `session-reconcile-changed` events (one mounting, or reloaded).
/// Rows this process hasn't looked at yet are absent.
#[tauri::command]
pub fn get_session_reconcile_states() -> Vec<SessionReconcileEvent> {
    active().map(|r| r.states()).unwrap_or_default()
}
