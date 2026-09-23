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
//! pause).
//!
//! ## Per-client interest (Phase 2)
//!
//! Interest is tracked **per connected client** — each native Tauri window plus
//! each WebSocket browser session — keyed by a frontend-supplied `client_id`.
//! The cadence for a project is the union across all clients ([`PollState::any_focused`],
//! [`PollState::is_foreground`], [`PollState::project_has_pending`]), so a project
//! that any client cares about is polled at the appropriate tier; the *work*
//! bookkeeping (`last_polled_at`/`failures`/`stale`/`forced`) stays project-keyed
//! and shared, so N clients still trigger only one poll per project per tier.
//!
//! Clients are evicted on disconnect (clean WS close or native window destroyed
//! ⇒ [`PrPollScheduler::disconnect_client`]) and via a [`CLIENT_TTL_MS`] fallback
//! for dirty drops ([`PollState::evict_stale_clients`], swept each tick). Native
//! windows use `tauri-{window label}` ids ([`TAURI_CLIENT_PREFIX`]), which are
//! exempt from TTL eviction — they have no WS heartbeat; the first window's id
//! ([`TAURI_CLIENT_ID`]) is pre-seeded at launch, so single-window behaviour
//! stays byte-for-byte equivalent to Phase 1.
//!
//! The TTL exemption is only sound because the `tauri-*` namespace is
//! *reserved*: [`is_reserved_client_id`] names the invariant, and the web
//! boundaries in `web_server.rs` (the `/api/events` WS `clientId` and the
//! PR-poll `/api/dispatch` verbs) reject ids that claim it. An exempt entry
//! must have a window-`Destroyed` teardown behind it, so the exemption and the
//! rejection are one invariant split across two files.
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

/// Id prefix for native Tauri windows: `tauri-{window label}`. Native windows
/// have no WS heartbeat, so ids with this prefix are exempt from TTL eviction —
/// their teardown is the window being destroyed (the `on_window_event` hook in
/// `lib.rs` calls [`PrPollScheduler::disconnect_client`]) or the process dying.
/// Must match the prefix used in `prPollingService.ts`.
pub const TAURI_CLIENT_PREFIX: &str = "tauri-";

/// Well-known id for the first native window (label `main`). Pre-seeded at
/// launch as focused so the very first tick polls before any hint arrives.
const TAURI_CLIENT_ID: &str = "tauri-main";

/// How long a client's interest survives without a heartbeat before the tick
/// loop evicts it — the dirty-drop fallback for WS clients that vanish without a
/// clean close. Set to ≈3× the expected WS keepalive (≤~30s), so it tolerates
/// transient lag while bounding spurious pending-tier polls from a dead-but-
/// counted client to ≲6. The Tauri id is exempt.
const CLIENT_TTL_MS: i64 = 90_000;

/// Whether a caller-supplied client id claims the native-window namespace.
///
/// `tauri-*` ids are exempt from TTL eviction ([`PollState::evict_stale_clients`]),
/// which is only safe when the id was minted by native window code — teardown is
/// then guaranteed by the window-`Destroyed` hook in `lib.rs`. Web boundaries (the
/// `/api/events` WS `clientId`, the PR-poll `/api/dispatch` verbs) must reject
/// these: a web client claiming one would leak its interest forever on a dirty
/// drop (nothing evicts it, and no window exists to be destroyed), or spoof a real
/// window's entry. Legitimate web clients use a UUID, so rejecting the namespace
/// can never hit one.
pub fn is_reserved_client_id(id: &str) -> bool {
    id.starts_with(TAURI_CLIENT_PREFIX)
}

// ---------------------------------------------------------------------------
// Poll-state — pure decision logic, no clock / store / Tauri handles
// ---------------------------------------------------------------------------

/// One connected client's interest. The effective cadence for a project is the
/// *union* of these across all clients (see [`PollState::any_focused`],
/// [`PollState::is_foreground`], [`PollState::project_has_pending`]).
#[derive(Default)]
struct ClientInterest {
    /// This client's foregrounded/selected project (→ selected tier).
    foreground_project: Option<String>,
    /// Whether this client's window is focused.
    focused: bool,
    /// branch_id → project_id for branches this client sees as having pending CI
    /// checks (→ pending tier). Mirrors the granularity the frontend tracks via
    /// `updateChecksStatus`.
    pending_branches: HashMap<String, String>,
    /// Last heartbeat (ms since epoch). Drives TTL eviction of dirty-dropped
    /// clients; the Tauri id is exempt regardless of this value.
    last_seen: i64,
}

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
    /// Projects explicitly nudged via `refresh_now`; due on the next tick
    /// regardless of interval or focus. Project-keyed and global (a nudge is
    /// about the *project*, not the observer), so it is shared across clients
    /// and survives the forcing client disconnecting.
    forced: HashSet<String>,
    /// Per-connected-client interest, keyed by `client_id`. The cadence for a
    /// project is the union across these (see the union helpers). Only the
    /// *interest* is per-client; the work bookkeeping above stays project-keyed
    /// so N clients still trigger only one poll per project per tier.
    clients: HashMap<String, ClientInterest>,
}

impl PollState {
    fn new() -> Self {
        let mut clients = HashMap::new();
        // Pre-seed the native client as focused so the very first immediate tick
        // polls at launch, matching Phase 1's `focused: true` default. The Tauri
        // frontend's later hints update this same entry, and `last_seen: 0` is
        // fine because the Tauri id is exempt from TTL eviction.
        clients.insert(
            TAURI_CLIENT_ID.to_string(),
            ClientInterest {
                focused: true,
                last_seen: 0,
                ..Default::default()
            },
        );
        Self {
            last_polled_at: HashMap::new(),
            failures: HashMap::new(),
            stale: HashSet::new(),
            forced: HashSet::new(),
            clients,
        }
    }

    /// Whether any connected client's window is focused. No focused client ⇒
    /// periodic polling pauses.
    fn any_focused(&self) -> bool {
        self.clients.values().any(|c| c.focused)
    }

    /// Whether any client has this project foregrounded/selected.
    fn is_foreground(&self, project_id: &str) -> bool {
        self.clients
            .values()
            .any(|c| c.foreground_project.as_deref() == Some(project_id))
    }

    /// Whether any client sees a pending branch in this project.
    fn project_has_pending(&self, project_id: &str) -> bool {
        self.clients
            .values()
            .any(|c| c.pending_branches.values().any(|p| p == project_id))
    }

    /// The polling interval for a project, as the union of current interest
    /// across all clients. Mirrors the frontend `getProjectInterval`.
    fn interval_for(&self, project_id: &str) -> i64 {
        if self.project_has_pending(project_id) {
            PENDING_INTERVAL_MS
        } else if self.is_foreground(project_id) {
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
            if !self.any_focused() {
                continue; // no focused client ⇒ pause periodic polling
            }
            let last = self.last_polled_at.get(id).copied().unwrap_or(0);
            if now.saturating_sub(last) >= self.interval_for(id) {
                due.push(id.clone());
            }
        }
        due
    }

    /// Drop tracking for projects/branches that no longer exist. Project-keyed
    /// work bookkeeping is pruned by project membership; each client's interest
    /// is pruned in place. Client *lifecycle* eviction (disconnect / TTL) is
    /// separate — that drops whole clients, this drops dead project/branch
    /// interest from surviving clients.
    fn prune(&mut self, known_projects: &HashSet<&str>, known_branches: &HashSet<&str>) {
        self.last_polled_at
            .retain(|k, _| known_projects.contains(k.as_str()));
        self.failures
            .retain(|k, _| known_projects.contains(k.as_str()));
        self.stale.retain(|k| known_projects.contains(k.as_str()));
        self.forced.retain(|k| known_projects.contains(k.as_str()));
        for client in self.clients.values_mut() {
            client.pending_branches.retain(|branch_id, project_id| {
                known_branches.contains(branch_id.as_str())
                    && known_projects.contains(project_id.as_str())
            });
            if let Some(fg) = &client.foreground_project {
                if !known_projects.contains(fg.as_str()) {
                    client.foreground_project = None;
                }
            }
        }
    }

    /// Record a successful poll. Returns `true` if the project transitioned out
    /// of the stale state (so the caller should emit a stale-cleared event).
    fn record_success(&mut self, project_id: &str, now: i64) -> bool {
        self.last_polled_at.insert(project_id.to_string(), now);
        self.failures.remove(project_id);
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
        let count = self.failures.entry(project_id.to_string()).or_insert(0);
        *count += 1;
        if *count == MAX_CONSECUTIVE_FAILURES {
            self.stale.insert(project_id.to_string())
        } else {
            false
        }
    }

    fn set_foreground(&mut self, client_id: &str, project_id: Option<String>, now: i64) {
        let client = self.clients.entry(client_id.to_string()).or_default();
        client.foreground_project = project_id;
        client.last_seen = now;
    }

    fn set_focus(&mut self, client_id: &str, focused: bool, now: i64) {
        let client = self.clients.entry(client_id.to_string()).or_default();
        client.focused = focused;
        client.last_seen = now;
    }

    fn set_branch_pending(
        &mut self,
        client_id: &str,
        branch_id: String,
        project_id: String,
        pending: bool,
        now: i64,
    ) {
        let client = self.clients.entry(client_id.to_string()).or_default();
        if pending {
            client.pending_branches.insert(branch_id, project_id);
        } else {
            client.pending_branches.remove(&branch_id);
        }
        client.last_seen = now;
    }

    /// `refresh_now` nudge. Project-keyed and global — independent of which
    /// client asked, so it survives that client disconnecting.
    fn force(&mut self, project_id: String) {
        self.forced.insert(project_id);
    }

    // -- Client lifecycle -------------------------------------------------

    /// Heartbeat: create the client entry if absent and bump its `last_seen` so
    /// it survives the next TTL sweep. Called on WS connect and each WS ping.
    fn touch(&mut self, client_id: &str, now: i64) {
        self.clients
            .entry(client_id.to_string())
            .or_default()
            .last_seen = now;
    }

    /// Clean disconnect: drop the client and all its interest.
    fn disconnect_client(&mut self, client_id: &str) {
        self.clients.remove(client_id);
    }

    /// Dirty-drop fallback: evict clients not heard from within `ttl_ms`.
    /// Native window ids ([`is_reserved_client_id`]) are exempt — they have no WS
    /// heartbeat; window destruction or process death tears them down. That
    /// guarantee holds only because the web boundaries reject the reserved
    /// namespace, so an exempt id here is always a real window's.
    fn evict_stale_clients(&mut self, now: i64, ttl_ms: i64) {
        self.clients
            .retain(|id, c| is_reserved_client_id(id) || now.saturating_sub(c.last_seen) <= ttl_ms);
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

    // These wrappers are the only place that reads the real clock for interest
    // updates: they stamp `now` and delegate to the pure [`PollState`] methods,
    // then wake the loop so the union is recomputed promptly. `pub` so the web
    // server's `dispatch` / `handle_ws` can drive them via the managed
    // `Arc<PrPollScheduler>` for WebSocket clients.

    pub fn set_foreground(&self, client_id: String, project_id: Option<String>) {
        let now = crate::store::now_timestamp();
        self.state
            .lock()
            .unwrap()
            .set_foreground(&client_id, project_id, now);
        self.notify.notify_one();
    }

    pub fn set_focus(&self, client_id: String, focused: bool) {
        let now = crate::store::now_timestamp();
        self.state
            .lock()
            .unwrap()
            .set_focus(&client_id, focused, now);
        self.notify.notify_one();
    }

    pub fn set_branch_pending(
        &self,
        client_id: String,
        branch_id: String,
        project_id: String,
        pending: bool,
    ) {
        let now = crate::store::now_timestamp();
        self.state
            .lock()
            .unwrap()
            .set_branch_pending(&client_id, branch_id, project_id, pending, now);
        self.notify.notify_one();
    }

    pub fn force(&self, project_id: String) {
        self.state.lock().unwrap().force(project_id);
        self.notify.notify_one();
    }

    /// Heartbeat for a client (WS connect / ping). Keeps it alive past the TTL.
    pub fn touch(&self, client_id: String) {
        let now = crate::store::now_timestamp();
        self.state.lock().unwrap().touch(&client_id, now);
        self.notify.notify_one();
    }

    /// Clean disconnect for a client (WS close). Wakes the loop so a vanished
    /// focus/foreground/pending recomputes the union promptly (it may now pause
    /// or slow polling).
    pub fn disconnect_client(&self, client_id: String) {
        self.state.lock().unwrap().disconnect_client(&client_id);
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
    let branch_ids: Vec<String> = match store.list_branch_ids() {
        Ok(branch_ids) => branch_ids,
        Err(e) => {
            log::warn!("[pr_poll] failed to list branches: {e}");
            return;
        }
    };

    let now = crate::store::now_timestamp();

    // Decide what to poll while holding the locks (no `.await` inside), then
    // mark those projects in flight. Lock order here (in_flight → state) is the
    // only nesting site; completion handlers below take the two locks
    // sequentially, never nested, so there is no ordering hazard.
    let due = {
        let known_projects: HashSet<&str> = project_ids.iter().map(|s| s.as_str()).collect();
        let known_branches: HashSet<&str> = branch_ids.iter().map(|s| s.as_str()).collect();
        let mut in_flight = scheduler.in_flight.lock().unwrap();
        let mut state = scheduler.state.lock().unwrap();
        // Evict clients that dropped without a clean WS close before deciding
        // what is due, so their stale interest stops inflating the union.
        state.evict_stale_clients(now, CLIENT_TTL_MS);
        state.prune(&known_projects, &known_branches);
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

/// Set a client's foregrounded/selected project (→ selected tier). `None`
/// clears it. The effective tier unions this across all connected clients.
#[tauri::command(rename_all = "camelCase")]
pub fn set_foreground_project(
    scheduler: tauri::State<'_, Arc<PrPollScheduler>>,
    client_id: String,
    project_id: Option<String>,
) {
    scheduler.set_foreground(client_id, project_id);
}

/// Report a native window's focus from the backend, bypassing the frontend.
///
/// `app_lifecycle` hides and shows windows itself, and a hidden native window
/// does not reliably deliver a blur to its webview — so without this the
/// scheduler would keep polling on the focused tier for a window nobody can
/// see. The id mirrors the frontend's own `tauri-{label}` scheme, so both sides
/// address the same per-window client.
pub(crate) fn set_tauri_client_focus(
    scheduler: &PrPollScheduler,
    window_label: &str,
    focused: bool,
) {
    scheduler.set_focus(format!("{TAURI_CLIENT_PREFIX}{window_label}"), focused);
}

/// Report a client's window focus. With no client focused, periodic polling
/// pauses (an explicit `refresh_now` still fetches).
#[tauri::command(rename_all = "camelCase")]
pub fn set_focus(
    scheduler: tauri::State<'_, Arc<PrPollScheduler>>,
    client_id: String,
    focused: bool,
) {
    scheduler.set_focus(client_id, focused);
}

/// Mark whether a branch has pending CI checks for a client (→ pending tier for
/// its project, unioned across clients).
#[tauri::command(rename_all = "camelCase")]
pub fn set_branch_pending(
    scheduler: tauri::State<'_, Arc<PrPollScheduler>>,
    client_id: String,
    branch_id: String,
    project_id: String,
    pending: bool,
) {
    scheduler.set_branch_pending(client_id, branch_id, project_id, pending);
}

/// Explicitly nudge the scheduler to refresh a project now (e.g. just created or
/// pushed a PR). Folded into the scheduler's dedup rather than fetching directly.
/// The `client_id` is carried only to keep that client's heartbeat fresh; the
/// force itself is project-keyed and global.
#[tauri::command(rename_all = "camelCase")]
pub fn refresh_now(
    scheduler: tauri::State<'_, Arc<PrPollScheduler>>,
    client_id: String,
    project_id: String,
) {
    scheduler.touch(client_id);
    scheduler.force(project_id);
}

/// Drop a client's interest on clean disconnect. For the native app this fires
/// from `prPollingService.dispose()`; for web it fires on WS close.
#[tauri::command(rename_all = "camelCase")]
pub fn disconnect_client(scheduler: tauri::State<'_, Arc<PrPollScheduler>>, client_id: String) {
    scheduler.disconnect_client(client_id);
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

    /// A `PollState` with the launch-seeded Tauri client removed, for
    /// multi-client tests that want a clean slate of explicitly-added clients.
    fn empty_state() -> PollState {
        let mut st = PollState::new();
        st.disconnect_client(TAURI_CLIENT_ID);
        st
    }

    #[test]
    fn due_respects_the_three_tiers() {
        let mut st = PollState::new();
        st.set_foreground(TAURI_CLIENT_ID, Some("sel".into()), 0);
        st.set_branch_pending(TAURI_CLIENT_ID, "b1".into(), "pend".into(), true, 0);
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
        st.set_focus(TAURI_CLIENT_ID, false, 0);
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
    fn refresh_now_during_in_flight_refresh_survives_completion() {
        for completion in [
            PollState::record_success as fn(&mut PollState, &str, i64) -> bool,
            PollState::record_failure,
        ] {
            let mut st = PollState::new();
            st.last_polled_at.insert("p".into(), 0);

            // First nudge is consumed when the tick schedules the refresh.
            st.force("p".into());
            assert_eq!(st.due(&ids(&["p"]), 1_000, &HashSet::new()), ids(&["p"]));
            st.forced.remove("p");

            // A second nudge arrives while that refresh is still in flight, so
            // it is deduped for now...
            st.force("p".into());
            assert!(st.due(&ids(&["p"]), 1_000, &set(&["p"])).is_empty());

            // ...but completing the original refresh must not clear the fresh
            // nudge. Once in-flight clears, the project is due immediately.
            completion(&mut st, "p", 2_000);
            assert_eq!(st.due(&ids(&["p"]), 2_000, &HashSet::new()), ids(&["p"]));
        }
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
    fn prune_clears_per_client_foreground_and_pending() {
        let mut st = PollState::new();
        st.last_polled_at.insert("gone".into(), 0);
        st.failures.insert("gone".into(), 2);
        st.stale.insert("gone".into());
        st.force("gone".into());
        st.set_foreground(TAURI_CLIENT_ID, Some("gone".into()), 0);
        st.set_branch_pending(TAURI_CLIENT_ID, "b".into(), "gone".into(), true, 0);

        let known_projects: HashSet<&str> = ["alive"].into_iter().collect();
        let known_branches: HashSet<&str> = HashSet::new();
        st.prune(&known_projects, &known_branches);

        assert!(st.last_polled_at.is_empty());
        assert!(st.failures.is_empty());
        assert!(st.stale.is_empty());
        assert!(st.forced.is_empty());
        // The client itself survives prune (lifecycle is separate); only its
        // interest in the now-gone project is cleared.
        let client = &st.clients[TAURI_CLIENT_ID];
        assert!(client.pending_branches.is_empty());
        assert!(client.foreground_project.is_none());
    }

    #[test]
    fn prune_clears_pending_for_deleted_branch_in_surviving_project() {
        let mut st = PollState::new();
        st.set_branch_pending(
            TAURI_CLIENT_ID,
            "deleted-branch".into(),
            "alive".into(),
            true,
            0,
        );
        assert!(st.project_has_pending("alive"));
        assert_eq!(st.interval_for("alive"), PENDING_INTERVAL_MS);

        let known_projects: HashSet<&str> = ["alive"].into_iter().collect();
        let known_branches: HashSet<&str> = ["other-branch"].into_iter().collect();
        st.prune(&known_projects, &known_branches);

        assert!(!st.project_has_pending("alive"));
        assert_eq!(st.interval_for("alive"), BACKGROUND_INTERVAL_MS);
    }

    // -- Phase 2: per-client interest / union / lifecycle ------------------

    #[test]
    fn union_foreground_across_two_clients() {
        let mut st = empty_state();
        st.set_focus("a", true, 0);
        st.set_focus("b", true, 0);
        st.set_foreground("a", Some("p1".into()), 0);
        st.set_foreground("b", Some("p2".into()), 0);
        for id in ["p1", "p2", "bg"] {
            st.last_polled_at.insert(id.into(), 0);
        }
        // Each client's foreground reaches the selected tier; bg stays slow.
        assert_eq!(st.interval_for("p1"), SELECTED_INTERVAL_MS);
        assert_eq!(st.interval_for("p2"), SELECTED_INTERVAL_MS);
        assert_eq!(st.interval_for("bg"), BACKGROUND_INTERVAL_MS);
        // Both foregrounded projects are due at the selected interval; bg isn't.
        assert_eq!(
            st.due(
                &ids(&["p1", "p2", "bg"]),
                SELECTED_INTERVAL_MS,
                &HashSet::new()
            ),
            ids(&["p1", "p2"])
        );
    }

    #[test]
    fn union_pending_beats_foreground() {
        let mut st = empty_state();
        // Client A sees a pending branch in p1; client B merely foregrounds p1.
        st.set_branch_pending("a", "b1".into(), "p1".into(), true, 0);
        st.set_foreground("b", Some("p1".into()), 0);
        // Pending wins the union precedence.
        assert_eq!(st.interval_for("p1"), PENDING_INTERVAL_MS);
    }

    #[test]
    fn union_focus() {
        let mut st = empty_state();
        st.set_focus("a", false, 0);
        st.set_focus("b", true, 0);
        st.last_polled_at.insert("p".into(), 0);
        let none = HashSet::new();

        // Any client focused ⇒ active.
        assert!(st.any_focused());
        assert_eq!(
            st.due(&ids(&["p"]), BACKGROUND_INTERVAL_MS, &none),
            ids(&["p"])
        );

        // All clients unfocused ⇒ paused.
        st.set_focus("b", false, 0);
        assert!(!st.any_focused());
        assert!(st
            .due(&ids(&["p"]), BACKGROUND_INTERVAL_MS * 10, &none)
            .is_empty());
    }

    #[test]
    fn disconnect_recomputes_union() {
        let mut st = empty_state();
        st.set_foreground("a", Some("p1".into()), 0);
        st.set_foreground("b", Some("p1".into()), 0);
        assert_eq!(st.interval_for("p1"), SELECTED_INTERVAL_MS);

        // One client leaves: still selected (B still holds it).
        st.disconnect_client("a");
        assert!(st.is_foreground("p1"));
        assert_eq!(st.interval_for("p1"), SELECTED_INTERVAL_MS);

        // Last client holding it leaves: falls back to the background tier.
        st.disconnect_client("b");
        assert!(!st.is_foreground("p1"));
        assert_eq!(st.interval_for("p1"), BACKGROUND_INTERVAL_MS);
    }

    #[test]
    fn disconnect_drops_pending() {
        let mut st = empty_state();
        st.set_branch_pending("a", "b1".into(), "p".into(), true, 0);
        assert!(st.project_has_pending("p"));
        assert_eq!(st.interval_for("p"), PENDING_INTERVAL_MS);

        st.disconnect_client("a");
        assert!(!st.project_has_pending("p"));
        assert_eq!(st.interval_for("p"), BACKGROUND_INTERVAL_MS);
    }

    #[test]
    fn ttl_evicts_stale_clients_but_exempts_tauri() {
        // Keep the launch-seeded Tauri client (last_seen = 0).
        let mut st = PollState::new();
        st.set_focus("web", true, 0);
        st.set_foreground("web", Some("p".into()), 0);
        assert!(st.is_foreground("p"));
        // A second native window: no heartbeat, idle since launch.
        st.set_foreground("tauri-win-2", Some("q".into()), 0);

        // Sweep well past the TTL relative to last_seen = 0.
        st.evict_stale_clients(CLIENT_TTL_MS + 1, CLIENT_TTL_MS);

        // The stale web client is gone; its interest no longer counts.
        assert!(!st.clients.contains_key("web"));
        assert!(!st.is_foreground("p"));
        // Native window ids are exempt despite last_seen = 0: the first window
        // stays focused and the idle second window keeps its foreground.
        assert!(st.clients.contains_key(TAURI_CLIENT_ID));
        assert!(st.any_focused());
        assert!(st.is_foreground("q"));

        // A destroyed native window is torn down via explicit disconnect.
        st.disconnect_client("tauri-win-2");
        assert!(!st.is_foreground("q"));
    }

    #[test]
    fn reserved_client_ids_are_the_tauri_namespace() {
        // Exactly the ids the native windows mint (see `prPollingService.ts`).
        assert!(is_reserved_client_id(TAURI_CLIENT_ID));
        assert!(is_reserved_client_id("tauri-win-2"));
        // Web ids never claim the namespace; the match is an exact, case-
        // sensitive prefix, matching the frontend's lowercase minting.
        assert!(!is_reserved_client_id("3f1a-uuid"));
        assert!(!is_reserved_client_id(""));
        assert!(!is_reserved_client_id("TAURI-main"));
        assert!(!is_reserved_client_id("tauri"));
        assert!(!is_reserved_client_id(" tauri-main"));
        assert!(!is_reserved_client_id("web-tauri-main"));
    }

    #[test]
    fn touch_keeps_client_alive() {
        let mut st = empty_state();
        st.set_focus("web", true, 0);

        // A heartbeat at the TTL boundary keeps the client alive through a sweep
        // at the same instant.
        let now = CLIENT_TTL_MS + 10;
        st.touch("web", now);
        st.evict_stale_clients(now, CLIENT_TTL_MS);
        assert!(st.clients.contains_key("web"));
        assert!(st.any_focused());

        // Without a further touch, it is evicted once the TTL elapses past the
        // last heartbeat.
        st.evict_stale_clients(now + CLIENT_TTL_MS + 1, CLIENT_TTL_MS);
        assert!(!st.clients.contains_key("web"));
    }

    #[test]
    fn forced_is_global_and_survives_disconnect() {
        let mut st = empty_state();
        st.last_polled_at.insert("p".into(), 0);

        // Client A nudges a refresh, then disconnects.
        st.force("p".into());
        st.disconnect_client("a");

        // `forced` is project-keyed and global, so it outlives the forcing
        // client and still bypasses the focus pause.
        assert!(!st.any_focused());
        assert_eq!(st.due(&ids(&["p"]), 1_000, &HashSet::new()), ids(&["p"]));
    }

    #[test]
    fn single_client_equivalence_to_phase1() {
        // One seeded Tauri client reproduces Phase 1's tier and pause behaviour.
        let mut st = PollState::new();
        st.set_foreground(TAURI_CLIENT_ID, Some("sel".into()), 0);
        st.set_branch_pending(TAURI_CLIENT_ID, "b1".into(), "pend".into(), true, 0);
        for id in ["sel", "pend", "bg"] {
            st.last_polled_at.insert(id.into(), 0);
        }
        let projects = ids(&["sel", "pend", "bg"]);
        let none = HashSet::new();

        assert_eq!(
            st.due(&projects, PENDING_INTERVAL_MS, &none),
            ids(&["pend"])
        );
        assert_eq!(
            st.due(&projects, SELECTED_INTERVAL_MS, &none),
            ids(&["sel", "pend"])
        );
        assert_eq!(
            st.due(&projects, BACKGROUND_INTERVAL_MS, &none),
            ids(&["sel", "pend", "bg"])
        );

        // Unfocusing the lone client pauses periodic polling.
        st.set_focus(TAURI_CLIENT_ID, false, 0);
        assert!(st
            .due(&projects, BACKGROUND_INTERVAL_MS * 10, &none)
            .is_empty());
    }
}
