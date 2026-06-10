//! Backend-owned PR-status poll scheduler.
//!
//! Owns the *cadence and concurrency* of PR-status polling that used to live in
//! the frontend `prPollingService`. A single long-lived tick loop decides which
//! projects are due, dedups in-flight work, and drives every refresh through one
//! bounded pool (shared via [`crate::prs::refresh_project_pr_statuses`]).
//!
//! Frontends shrink to a thin *interest/hint* layer: they tell the backend which
//! project is foregrounded, which branches have pending checks, and whether a
//! window is focused, via the [`set_foreground_project`], [`set_branch_pending`],
//! and [`set_focus`] commands. The effective tier for a project is the union of
//! interest (any foregrounding ⇒ selected; any pending ⇒ fast; nothing focused ⇒
//! pause). For a single client this is just that client's state, but it is
//! structured as a union so Phase 2 can extend it across connected clients.
//!
//! Poll-state (last-polled timestamps, failure counts) is intentionally **not
//! persisted** — on restart everything is "due", matching the frontend's
//! previous behaviour where the service started fresh each app launch.
//!
//! ## Testing seam
//!
//! All due/dedup/backoff *decisions* live in [`PollState`], whose methods take an
//! explicit `now` (ms since epoch) and in-flight set. The tick loop is the only
//! place that touches the real clock ([`crate::store::now_timestamp`]) and the
//! real fetcher, so the decision logic is unit-testable without `gh` calls or
//! wall-clock sleeps (see the tests at the bottom of this file).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::store::Store;

// ---------------------------------------------------------------------------
// Interval tiers (milliseconds)
// ---------------------------------------------------------------------------
//
// Moved verbatim from the frontend `prPollingService` so the backend is the
// single owner of polling cadence.

/// Any project with a branch that has pending CI checks (fastest tier).
const PENDING_INTERVAL_MS: i64 = 15_000;
/// The foregrounded/selected project, no pending checks.
const SELECTED_INTERVAL_MS: i64 = 60_000;
/// Background (non-selected, no pending checks).
const BACKGROUND_INTERVAL_MS: i64 = 5 * 60_000;
/// Consecutive failures before a project is reported as stale to the frontend.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// How often the tick loop wakes up to re-evaluate what is due. Kept well below
/// the fastest tier (15s) so the loop's granularity adds only a small bounded
/// jitter to each tier; interest changes and `refresh_now` nudges additionally
/// wake the loop immediately, so this only bounds the *periodic* re-poll delay.
const TICK_INTERVAL_SECS: u64 = 5;

// ---------------------------------------------------------------------------
// Poll-state — pure decision logic, no clock / store / Tauri handles
// ---------------------------------------------------------------------------

/// The poll-state and interest the scheduler owns. Pure: every method that needs
/// the current time takes `now` (ms since epoch) as a parameter, so the
/// due/dedup/backoff logic can be unit-tested deterministically.
struct PollState {
    /// When each project was last polled (ms since epoch). Absent ⇒ never
    /// polled ⇒ immediately due. Not persisted across restarts.
    last_polled_at: HashMap<String, i64>,
    /// Consecutive failure count per project.
    failures: HashMap<String, u32>,
    /// Projects currently reported as stale (failures ≥ threshold). Tracked so a
    /// stale event is only emitted on transitions.
    stale: HashSet<String>,
    /// The foregrounded/selected project (→ selected tier). `Option` models a
    /// single client; Phase 2 will union a set across connected clients.
    foreground_project: Option<String>,
    /// Whether any client window is focused. No focus ⇒ polling pauses.
    focused: bool,
    /// branch_id → project_id for branches with pending CI checks (→ pending
    /// tier). Mirrors the granularity the frontend tracked via
    /// `updateChecksStatus`.
    pending_branches: HashMap<String, String>,
    /// Projects explicitly nudged via `refresh_now`; due on the next tick
    /// regardless of interval or focus.
    forced: HashSet<String>,
}

impl PollState {
    fn new() -> Self {
        Self {
            last_polled_at: HashMap::new(),
            failures: HashMap::new(),
            stale: HashSet::new(),
            foreground_project: None,
            // Start focused so the very first tick polls at launch, matching the
            // frontend which began with `windowFocused = true`.
            focused: true,
            pending_branches: HashMap::new(),
            forced: HashSet::new(),
        }
    }

    fn project_has_pending(&self, project_id: &str) -> bool {
        self.pending_branches.values().any(|p| p == project_id)
    }

    /// The polling interval for a project, as the union of current interest.
    /// Mirrors the frontend `getProjectInterval`.
    fn interval_for(&self, project_id: &str) -> i64 {
        if self.project_has_pending(project_id) {
            PENDING_INTERVAL_MS
        } else if self.foreground_project.as_deref() == Some(project_id) {
            SELECTED_INTERVAL_MS
        } else {
            BACKGROUND_INTERVAL_MS
        }
    }

    /// Compute which of `project_ids` should be polled right now.
    ///
    /// A project is due when it is not already in flight (dedup) and either it
    /// has been explicitly forced (`refresh_now` — bypasses focus and interval),
    /// or a client is focused and its tier interval has elapsed.
    fn due(&self, project_ids: &[String], now: i64, in_flight: &HashSet<String>) -> Vec<String> {
        let mut due = Vec::new();
        for id in project_ids {
            if in_flight.contains(id) {
                continue; // dedup: a refresh for this project is already running
            }
            if self.forced.contains(id) {
                due.push(id.clone());
                continue;
            }
            if !self.focused {
                continue; // no focused client ⇒ pause periodic polling
            }
            let last = self.last_polled_at.get(id).copied().unwrap_or(0);
            if now.saturating_sub(last) >= self.interval_for(id) {
                due.push(id.clone());
            }
        }
        due
    }

    /// Drop tracking for projects (and pending branches) that no longer exist.
    fn prune(&mut self, known: &HashSet<&str>) {
        self.last_polled_at
            .retain(|k, _| known.contains(k.as_str()));
        self.failures.retain(|k, _| known.contains(k.as_str()));
        self.stale.retain(|k| known.contains(k.as_str()));
        self.forced.retain(|k| known.contains(k.as_str()));
        self.pending_branches
            .retain(|_, p| known.contains(p.as_str()));
        if let Some(fg) = &self.foreground_project {
            if !known.contains(fg.as_str()) {
                self.foreground_project = None;
            }
        }
    }

    /// Record a successful poll. Returns `true` if the project transitioned out
    /// of the stale state (so the caller should emit a stale-cleared event).
    fn record_success(&mut self, project_id: &str, now: i64) -> bool {
        self.last_polled_at.insert(project_id.to_string(), now);
        self.failures.remove(project_id);
        self.forced.remove(project_id);
        self.stale.remove(project_id)
    }

    /// Record a failed poll. Returns `true` if the project just transitioned
    /// into the stale state (so the caller should emit a stale event).
    ///
    /// `last_polled_at` is advanced so a persistently failing project retries on
    /// its normal tier cadence rather than every tick. (The old frontend left
    /// `lastPolledAt` untouched on failure, which retried failing projects about
    /// once a second — exactly the kind of churn the backend should damp.)
    fn record_failure(&mut self, project_id: &str, now: i64) -> bool {
        self.last_polled_at.insert(project_id.to_string(), now);
        self.forced.remove(project_id);
        let count = self.failures.entry(project_id.to_string()).or_insert(0);
        *count += 1;
        if *count == MAX_CONSECUTIVE_FAILURES {
            self.stale.insert(project_id.to_string())
        } else {
            false
        }
    }

    fn set_foreground(&mut self, project_id: Option<String>) {
        self.foreground_project = project_id;
    }

    fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn set_branch_pending(&mut self, branch_id: String, project_id: String, pending: bool) {
        if pending {
            self.pending_branches.insert(branch_id, project_id);
        } else {
            self.pending_branches.remove(&branch_id);
        }
    }

    fn force(&mut self, project_id: String) {
        self.forced.insert(project_id);
    }
}

// ---------------------------------------------------------------------------
// Scheduler — managed state shared between the tick loop and the hint commands
// ---------------------------------------------------------------------------

/// Long-lived scheduler state, stored in Tauri managed state as
/// `Arc<PrPollScheduler>`. The interest/hint commands mutate [`PollState`] and
/// wake the loop; the loop reads it to decide what to poll.
pub struct PrPollScheduler {
    state: Mutex<PollState>,
    /// Projects with an in-flight refresh, for dedup across overlapping ticks
    /// and `refresh_now` nudges.
    in_flight: Mutex<HashSet<String>>,
    /// Wakes the tick loop when interest changes or a `refresh_now` arrives so
    /// it re-evaluates promptly instead of waiting out the periodic tick.
    notify: tokio::sync::Notify,
}

impl PrPollScheduler {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(PollState::new()),
            in_flight: Mutex::new(HashSet::new()),
            notify: tokio::sync::Notify::new(),
        }
    }

    fn set_foreground(&self, project_id: Option<String>) {
        self.state.lock().unwrap().set_foreground(project_id);
        self.notify.notify_one();
    }

    fn set_focus(&self, focused: bool) {
        self.state.lock().unwrap().set_focus(focused);
        self.notify.notify_one();
    }

    fn set_branch_pending(&self, branch_id: String, project_id: String, pending: bool) {
        self.state
            .lock()
            .unwrap()
            .set_branch_pending(branch_id, project_id, pending);
        self.notify.notify_one();
    }

    fn force(&self, project_id: String) {
        self.state.lock().unwrap().force(project_id);
        self.notify.notify_one();
    }
}

impl Default for PrPollScheduler {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tick loop
// ---------------------------------------------------------------------------

/// Spawn the PR-poll loop. Call once during app setup, after the store is
/// initialised. Takes the store directly (like `background_sync::spawn`); a DB
/// reset only happens via a restart-gated flow, so the loop's store handle stays
/// valid for the process lifetime.
pub fn spawn(scheduler: Arc<PrPollScheduler>, store: Arc<Store>, app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        poll_loop(scheduler, store, app_handle).await;
    });
}

async fn poll_loop(
    scheduler: Arc<PrPollScheduler>,
    store: Arc<Store>,
    app_handle: tauri::AppHandle,
) {
    // One bounded pool shared across every project the scheduler refreshes, so a
    // tick that finds many projects due still caps total concurrent `gh`
    // subprocesses — this is what tames the focus-regain / launch herd.
    let semaphore = Arc::new(tokio::sync::Semaphore::new(
        crate::prs::PR_REFRESH_CONCURRENCY,
    ));

    let mut interval = tokio::time::interval(Duration::from_secs(TICK_INTERVAL_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        // The first `interval.tick()` resolves immediately, so the loop runs an
        // initial tick at startup (everything is due ⇒ initial poll).
        tokio::select! {
            _ = interval.tick() => {}
            _ = scheduler.notify.notified() => {}
        }
        tick(&scheduler, &store, &app_handle, &semaphore).await;
    }
}

async fn tick(
    scheduler: &Arc<PrPollScheduler>,
    store: &Arc<Store>,
    app_handle: &tauri::AppHandle,
    semaphore: &Arc<tokio::sync::Semaphore>,
) {
    // Re-derive the project list from the DB each tick (cheap indexed read) so
    // the backend owns the set of projects to poll without a frontend hint —
    // this is what replaces the old `prPollingService.setProjects`.
    let project_ids: Vec<String> = match store.list_projects() {
        Ok(projects) => projects.into_iter().map(|p| p.id).collect(),
        Err(e) => {
            log::warn!("[pr_poll] failed to list projects: {e}");
            return;
        }
    };

    let now = crate::store::now_timestamp();

    // Decide what to poll while holding the locks (no `.await` inside), then
    // mark those projects in flight. Lock order here (in_flight → state) is the
    // only nesting site; completion handlers below take the two locks
    // sequentially, never nested, so there is no ordering hazard.
    let due = {
        let known: HashSet<&str> = project_ids.iter().map(|s| s.as_str()).collect();
        let mut in_flight = scheduler.in_flight.lock().unwrap();
        let mut state = scheduler.state.lock().unwrap();
        state.prune(&known);
        let due = state.due(&project_ids, now, &in_flight);
        for id in &due {
            in_flight.insert(id.clone());
            // Consume the explicit nudge now that we're acting on it.
            state.forced.remove(id);
        }
        due
    };

    for project_id in due {
        emit_refresh_state(app_handle, &project_id, true);

        let scheduler = Arc::clone(scheduler);
        let store = Arc::clone(store);
        let app_handle = app_handle.clone();
        let semaphore = Arc::clone(semaphore);

        tauri::async_runtime::spawn(async move {
            let result = crate::prs::refresh_project_pr_statuses(
                &store,
                &app_handle,
                &project_id,
                semaphore,
            )
            .await;
            let now = crate::store::now_timestamp();

            // Update poll-state, then clear the in-flight marker. The two locks
            // are taken sequentially (never nested) to stay deadlock-free.
            let stale_change = {
                let mut state = scheduler.state.lock().unwrap();
                match &result {
                    Ok(_) => state.record_success(&project_id, now).then_some(false),
                    Err(e) => {
                        log::warn!("[pr_poll] refresh failed for project {project_id}: {e}");
                        state.record_failure(&project_id, now).then_some(true)
                    }
                }
            };
            scheduler.in_flight.lock().unwrap().remove(&project_id);

            emit_refresh_state(&app_handle, &project_id, false);
            if let Some(stale) = stale_change {
                emit_stale(&app_handle, &project_id, stale);
            }

            // A refresh just finished — wake the loop so any project forced
            // while this one was in flight is picked up without waiting out the
            // periodic tick.
            scheduler.notify.notify_one();
        });
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Per-project refresh lifecycle, so the frontend can show "checking right now".
/// Replaces the refresh-state the frontend used to track around its own poll
/// loop, which now lives here.
fn emit_refresh_state(app_handle: &tauri::AppHandle, project_id: &str, refreshing: bool) {
    #[derive(Clone, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct PrRefreshState {
        project_id: String,
        refreshing: bool,
    }

    crate::web_server::emit_to_all(
        app_handle,
        "pr-refresh-state",
        PrRefreshState {
            project_id: project_id.to_string(),
            refreshing,
        },
    );
}

/// Project crossed (or recovered from) the consecutive-failure threshold, so the
/// frontend can show a stale-data indicator.
fn emit_stale(app_handle: &tauri::AppHandle, project_id: &str, stale: bool) {
    #[derive(Clone, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct PrStale {
        project_id: String,
        stale: bool,
    }

    crate::web_server::emit_to_all(
        app_handle,
        "pr-status-stale",
        PrStale {
            project_id: project_id.to_string(),
            stale,
        },
    );
}

// ---------------------------------------------------------------------------
// Interest / hint commands
// ---------------------------------------------------------------------------

/// Set the foregrounded/selected project (→ selected tier). `None` clears it.
#[tauri::command(rename_all = "camelCase")]
pub fn set_foreground_project(
    scheduler: tauri::State<'_, Arc<PrPollScheduler>>,
    project_id: Option<String>,
) {
    scheduler.set_foreground(project_id);
}

/// Report window focus. With no focused client, periodic polling pauses (an
/// explicit `refresh_now` still fetches).
#[tauri::command]
pub fn set_focus(scheduler: tauri::State<'_, Arc<PrPollScheduler>>, focused: bool) {
    scheduler.set_focus(focused);
}

/// Mark whether a branch has pending CI checks (→ pending tier for its project).
#[tauri::command(rename_all = "camelCase")]
pub fn set_branch_pending(
    scheduler: tauri::State<'_, Arc<PrPollScheduler>>,
    branch_id: String,
    project_id: String,
    pending: bool,
) {
    scheduler.set_branch_pending(branch_id, project_id, pending);
}

/// Explicitly nudge the scheduler to refresh a project now (e.g. just created or
/// pushed a PR). Folded into the scheduler's dedup rather than fetching directly.
#[tauri::command(rename_all = "camelCase")]
pub fn refresh_now(scheduler: tauri::State<'_, Arc<PrPollScheduler>>, project_id: String) {
    scheduler.force(project_id);
}

// ---------------------------------------------------------------------------
// Tests — pure due/dedup/backoff logic, no clock or `gh`
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn set(v: &[&str]) -> HashSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn due_respects_the_three_tiers() {
        let mut st = PollState::new();
        st.set_foreground(Some("sel".into()));
        st.set_branch_pending("b1".into(), "pend".into(), true);
        for id in ["sel", "pend", "bg"] {
            st.last_polled_at.insert(id.into(), 0);
        }
        let projects = ids(&["sel", "pend", "bg"]);
        let none = HashSet::new();

        // Just after the pending interval: only the pending project is due.
        assert_eq!(
            st.due(&projects, PENDING_INTERVAL_MS, &none),
            ids(&["pend"])
        );

        // Past the selected interval: pending + selected (order follows input).
        assert_eq!(
            st.due(&projects, SELECTED_INTERVAL_MS, &none),
            ids(&["sel", "pend"])
        );

        // Past the background interval: all three.
        assert_eq!(
            st.due(&projects, BACKGROUND_INTERVAL_MS, &none),
            ids(&["sel", "pend", "bg"])
        );
    }

    #[test]
    fn unfocused_pauses_periodic_polling_but_not_forced() {
        let mut st = PollState::new();
        st.last_polled_at.insert("p".into(), 0);
        st.set_focus(false);
        let none = HashSet::new();

        // Long past every interval, but no focused client ⇒ nothing due.
        assert!(st
            .due(&ids(&["p"]), BACKGROUND_INTERVAL_MS * 10, &none)
            .is_empty());

        // A forced project still polls while unfocused.
        st.force("p".into());
        assert_eq!(
            st.due(&ids(&["p"]), BACKGROUND_INTERVAL_MS * 10, &none),
            ids(&["p"])
        );
    }

    #[test]
    fn in_flight_projects_are_not_re_enqueued() {
        let mut st = PollState::new();
        st.last_polled_at.insert("p".into(), 0);
        let now = BACKGROUND_INTERVAL_MS * 10;

        // Due when nothing is in flight.
        assert_eq!(st.due(&ids(&["p"]), now, &HashSet::new()), ids(&["p"]));
        // Deduped when already in flight.
        assert!(st.due(&ids(&["p"]), now, &set(&["p"])).is_empty());

        // Even an explicit nudge does not double-fetch an in-flight project.
        st.force("p".into());
        assert!(st.due(&ids(&["p"]), now, &set(&["p"])).is_empty());
    }

    #[test]
    fn refresh_now_forces_a_poll_before_the_interval_elapses() {
        let mut st = PollState::new();
        st.last_polled_at.insert("p".into(), 0);

        // Just polled ⇒ not yet interval-due.
        assert!(st.due(&ids(&["p"]), 1_000, &HashSet::new()).is_empty());

        // refresh_now folds in: due immediately regardless of interval.
        st.force("p".into());
        assert_eq!(st.due(&ids(&["p"]), 1_000, &HashSet::new()), ids(&["p"]));
    }

    #[test]
    fn failures_trip_stale_after_threshold_then_clear_on_success() {
        let mut st = PollState::new();
        assert!(!st.record_failure("p", 1));
        assert!(!st.record_failure("p", 2));
        // Third consecutive failure crosses the threshold ⇒ transition to stale.
        assert!(st.record_failure("p", 3));
        // Further failures stay stale (no new transition).
        assert!(!st.record_failure("p", 4));
        // A success transitions back out of stale exactly once.
        assert!(st.record_success("p", 5));
        assert!(!st.record_success("p", 6));
    }

    #[test]
    fn failure_advances_last_polled_so_retries_use_the_tier_cadence() {
        let mut st = PollState::new();
        st.last_polled_at.insert("p".into(), 0);
        // Fails at now=1000; last_polled advances to 1000.
        st.record_failure("p", 1_000);
        // Not due again until a full background interval after the failure.
        assert!(st
            .due(
                &ids(&["p"]),
                1_000 + BACKGROUND_INTERVAL_MS - 1,
                &HashSet::new()
            )
            .is_empty());
        assert_eq!(
            st.due(
                &ids(&["p"]),
                1_000 + BACKGROUND_INTERVAL_MS,
                &HashSet::new()
            ),
            ids(&["p"])
        );
    }

    #[test]
    fn prune_drops_unknown_projects_and_their_interest() {
        let mut st = PollState::new();
        st.last_polled_at.insert("gone".into(), 0);
        st.failures.insert("gone".into(), 2);
        st.stale.insert("gone".into());
        st.force("gone".into());
        st.set_foreground(Some("gone".into()));
        st.set_branch_pending("b".into(), "gone".into(), true);

        let known: HashSet<&str> = ["alive"].into_iter().collect();
        st.prune(&known);

        assert!(st.last_polled_at.is_empty());
        assert!(st.failures.is_empty());
        assert!(st.stale.is_empty());
        assert!(st.forced.is_empty());
        assert!(st.pending_branches.is_empty());
        assert!(st.foreground_project.is_none());
    }
}
