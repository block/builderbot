//! Resume the sessions the last app quit interrupted, at the next launch.
//!
//! A quit cancels every session this process owns with
//! `completion_reason = app_quit` (and a crash leaves rows the next launch's
//! dead-owner recovery ends the same way). Nothing restarted them: the user
//! had to find each one and press Resume. This module finds them once per
//! process, right after the startup queue recovery, hands the set to the main
//! window whenever it asks until it is answered (`get_interrupted_sessions`,
//! so a reload between the offer and the answer sees it again), and on the
//! user's say-so restarts them in two phases (`resume_interrupted_sessions`):
//! repo sessions first, project sessions second, so a resumed project agent
//! that polls its children finds them queued or running.
//!
//! - [`plan`] decides what each row gets — resume, re-queue, or left alone —
//!   and in what order, from the rows alone.
//! - [`run`] carries the plan out through `continue_resume_gate` and the
//!   branch queue.
//!
//! The set is not re-offered forever: the launch that handles it records the
//! newest `updated_at` it saw as an internal preferences value, and later
//! launches only look past that. A row the user resumes by hand and loses to
//! a later quit is newer again and is offered afresh.

pub(crate) mod plan;
pub(crate) mod run;
#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::Manager;

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::app_lifecycle::{quit_session_label, session_location};
use crate::session_commands::project_active_session;
use crate::session_runner::SessionRegistry;
use crate::store::{Session, Store};

use plan::{action_for, classify_candidate, CandidateKind};

/// Preferences-store key holding the `updated_at` through which interrupted
/// sessions have been offered. Internal: written here, never shown in
/// settings.
pub(crate) const HANDLED_THROUGH_KEY: &str = "launch-resume-handled-through";

/// One interrupted session as the dialog lists it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InterruptedSession {
    pub session_id: String,
    pub project_id: Option<String>,
    pub branch_id: Option<String>,
    pub session_type: Option<String>,
    /// `commit on fix-login`, as the quit prompt labels sessions.
    pub label: String,
    pub branch_name: Option<String>,
    pub project_name: Option<String>,
    /// `resume` (conversation restored), `restart` (re-queued; runs from the
    /// beginning), or `manual` (listed only; see `reason`).
    pub disposition: &'static str,
    pub reason: Option<&'static str>,
    /// When the quit ended it (`sessions.updated_at`).
    pub interrupted_at: i64,
}

/// The set offered at launch, repo sessions apart from project sessions.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRecovery {
    pub repo_sessions: Vec<InterruptedSession>,
    pub project_sessions: Vec<InterruptedSession>,
}

impl LaunchRecovery {
    pub(crate) fn is_empty(&self) -> bool {
        self.repo_sessions.is_empty() && self.project_sessions.is_empty()
    }

    /// Every session the dialog's "Resume all" sends back (the frontend
    /// applies the same filter on `disposition`).
    #[cfg(test)]
    pub(crate) fn resumable_ids(&self) -> Vec<String> {
        self.repo_sessions
            .iter()
            .chain(self.project_sessions.iter())
            .filter(|s| s.disposition != "manual")
            .map(|s| s.session_id.clone())
            .collect()
    }
}

/// What the last run did, for the Doctor's session-queue readout.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LaunchResumeSummary {
    pub(crate) requested: usize,
    pub(crate) resumed: usize,
    pub(crate) requeued: usize,
    pub(crate) skipped: usize,
    pub(crate) failed: usize,
}

/// Managed state: the offer, held until it is answered; the marker to record
/// when it is; and the last run's summary.
#[derive(Default)]
pub struct LaunchResumeState {
    offer: Mutex<Option<LaunchRecovery>>,
    offered_through: Mutex<Option<i64>>,
    last_run: Mutex<Option<LaunchResumeSummary>>,
    /// Branches with a deferred push the startup pass skipped because the
    /// offer names a session of theirs or of their project; kicked when the
    /// offer is answered (see [`kick_deferred_pushes_at_launch`]).
    deferred_push_holds: Mutex<Vec<String>>,
}

impl LaunchResumeState {
    pub(crate) fn last_run(&self) -> Option<LaunchResumeSummary> {
        self.last_run.lock().unwrap().clone()
    }

    /// The pending offer, left in place: a main-window reload between the
    /// offer and the answer asks again and must see the same set.
    pub(crate) fn offer(&self) -> Option<LaunchRecovery> {
        self.offer.lock().unwrap().clone()
    }

    /// Clear the offer and take the marker to record. The first answer gets
    /// the marker; a second answer (two windows, a reload mid-answer) gets
    /// `None` and records nothing — the rows it names have moved under the
    /// first answer, so `run_launch_resume` ignores them.
    pub(crate) fn take_handled(&self) -> Option<i64> {
        *self.offer.lock().unwrap() = None;
        self.offered_through.lock().unwrap().take()
    }

    /// The branches whose deferred push waits on the offer, taken once.
    pub(crate) fn take_deferred_push_holds(&self) -> Vec<String> {
        std::mem::take(&mut *self.deferred_push_holds.lock().unwrap())
    }

    /// The branches whose deferred push is waiting on the offer.
    #[cfg(test)]
    pub(crate) fn deferred_push_holds(&self) -> Vec<String> {
        self.deferred_push_holds.lock().unwrap().clone()
    }
}

// =============================================================================
// Handled marker
// =============================================================================

/// The marker as stored in `preferences.json`; missing or malformed reads as
/// "nothing handled yet".
pub(crate) fn handled_through_from_preferences(json: &serde_json::Value) -> i64 {
    json.get(HANDLED_THROUGH_KEY)
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .unwrap_or(0)
}

fn read_handled_through() -> i64 {
    let Some(path) = crate::preferences_store_path_buf() else {
        return 0;
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return 0;
    };
    serde_json::from_str::<serde_json::Value>(&contents)
        .map(|json| handled_through_from_preferences(&json))
        .unwrap_or(0)
}

fn write_handled_through(app_handle: &tauri::AppHandle, value: i64) -> Result<(), String> {
    let store = crate::web_server::preferences_store(app_handle)?;
    store.set(HANDLED_THROUGH_KEY, serde_json::json!(value));
    store
        .save()
        .map_err(|e| format!("Failed to save preferences: {e}"))
}

// =============================================================================
// Detection
// =============================================================================

fn describe(store: &Store, session: &Session) -> Option<InterruptedSession> {
    let candidate = classify_candidate(store, session);
    if candidate.kind == CandidateKind::Internal {
        return None;
    }
    let action = action_for(&candidate);
    let info = project_active_session(store, session);
    let location = session_location(store, &info);
    let label = quit_session_label(&info, location.as_deref());
    let branch_name = info
        .branch_id
        .as_deref()
        .and_then(|id| store.get_branch(id).ok().flatten())
        .map(|branch| branch.branch_name);
    let project_name = info
        .project_id
        .as_deref()
        .and_then(|id| store.get_project(id).ok().flatten())
        .map(|project| project.name);
    Some(InterruptedSession {
        session_id: session.id.clone(),
        project_id: info.project_id,
        branch_id: info.branch_id,
        session_type: info.session_type,
        label,
        branch_name,
        project_name,
        disposition: action.as_str(),
        reason: action.skip_reason(),
        interrupted_at: session.updated_at,
    })
}

/// The interrupted set newer than `handled_through`, and the marker a launch
/// that handles it should record. `None` when there is nothing to offer.
pub(crate) fn build_recovery(
    store: &Store,
    handled_through: i64,
) -> Result<Option<(LaunchRecovery, i64)>, String> {
    let rows = store
        .get_app_quit_sessions_since(handled_through)
        .map_err(|e| e.to_string())?;
    let mut recovery = LaunchRecovery::default();
    let mut newest = handled_through;
    for session in &rows {
        let Some(described) = describe(store, session) else {
            continue;
        };
        newest = newest.max(session.updated_at);
        if described.branch_id.is_some() {
            recovery.repo_sessions.push(described);
        } else {
            recovery.project_sessions.push(described);
        }
    }
    Ok((!recovery.is_empty()).then_some((recovery, newest)))
}

/// Compute the offer once per process and park it in managed state. Runs in
/// the setup closure after `recover_dead_sessions`, so crash-orphaned rows
/// already carry the quit's reason.
pub(crate) fn detect_at_launch<R: tauri::Runtime>(app_handle: &tauri::AppHandle<R>, store: &Store) {
    let state = app_handle.state::<LaunchResumeState>();
    match build_recovery(store, read_handled_through()) {
        Ok(Some((recovery, newest))) => {
            log::info!(
                "[launch_resume] {} repo and {} project session(s) were interrupted by the last quit",
                recovery.repo_sessions.len(),
                recovery.project_sessions.len()
            );
            *state.offer.lock().unwrap() = Some(recovery);
            *state.offered_through.lock().unwrap() = Some(newest);
        }
        Ok(None) => {}
        Err(e) => log::warn!("[launch_resume] could not look for interrupted sessions: {e}"),
    }
}

/// Kick the branches left with a deferred push ("Push after current work")
/// armed by the last run, except those the offer holds.
///
/// A branch whose own session, or whose project's session, the last quit
/// interrupted is not kicked yet: the push would fire before the user
/// resumes, and the resumed session's commits would go unpushed. Those
/// branches are parked in the state and kicked when the offer is answered —
/// after the resumed rows are back in place, so the drain's idle check sees
/// them (`resume_interrupted_sessions`). Runs right after [`detect_at_launch`].
pub(crate) fn kick_deferred_pushes_at_launch<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Store,
) {
    let armed = match store.branch_ids_with_deferred_push(None) {
        Ok(ids) => ids,
        Err(e) => {
            log::warn!("[launch_resume] could not list branches with a deferred push: {e}");
            return;
        }
    };
    if armed.is_empty() {
        return;
    }
    let state = app_handle.state::<LaunchResumeState>();
    let offer = state.offer();
    let mut held = Vec::new();
    for branch_id in armed {
        if offer
            .as_ref()
            .is_some_and(|offer| offer_holds_branch(offer, store, &branch_id))
        {
            log::info!(
                "[launch_resume] deferred push on branch {branch_id} waits for the launch offer"
            );
            held.push(branch_id);
        } else {
            crate::session_queue::kick(
                app_handle,
                &branch_id,
                crate::session_queue::KickSource::Startup,
            );
        }
    }
    *state.deferred_push_holds.lock().unwrap() = held;
}

/// Whether the offer names a session on `branch_id`, or a project session of
/// its project.
fn offer_holds_branch(offer: &LaunchRecovery, store: &Store, branch_id: &str) -> bool {
    if offer
        .repo_sessions
        .iter()
        .any(|s| s.branch_id.as_deref() == Some(branch_id))
    {
        return true;
    }
    let Ok(Some(branch)) = store.get_branch(branch_id) else {
        return false;
    };
    offer
        .project_sessions
        .iter()
        .any(|s| s.project_id.as_deref() == Some(branch.project_id.as_str()))
}

// =============================================================================
// Commands
// =============================================================================

/// The launch's interrupted set, until it is answered: a reload between the
/// offer and the answer sees it again; after `resume_interrupted_sessions`
/// (and on a launch with nothing to offer) this is `None`.
#[tauri::command]
pub fn get_interrupted_sessions(
    state: tauri::State<'_, LaunchResumeState>,
) -> Option<LaunchRecovery> {
    state.offer()
}

/// Record the offer as handled and clear it, then restart `session_ids` on a
/// task of its own (progress arrives as `session-status-changed` events). An
/// empty list is a dismissal: the marker is recorded and nothing is
/// restarted. A second answer is harmless: the marker is already recorded,
/// and `run_launch_resume` ignores rows that are no longer terminal with the
/// quit's reason.
///
/// Runs on the blocking pool: recording the marker saves the preferences
/// file, which as a synchronous command held the main thread. The store
/// handle is looked up inline but only checked where the run needs it, so a
/// dismissal still succeeds without a store.
#[tauri::command]
pub async fn resume_interrupted_sessions(
    app_handle: tauri::AppHandle,
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<SessionRegistry>>,
    action_executor: tauri::State<'_, Arc<ActionExecutor>>,
    action_registry: tauri::State<'_, Arc<ActionRegistry>>,
    session_ids: Vec<String>,
) -> Result<(), String> {
    let store = crate::get_store(&store);
    let registry = Arc::clone(&registry);
    let action_executor = Arc::clone(&action_executor);
    let action_registry = Arc::clone(&action_registry);
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<LaunchResumeState>();
        if let Some(through) = state.take_handled() {
            if let Err(e) = write_handled_through(&app_handle, through) {
                log::warn!("[launch_resume] could not record the handled marker: {e}");
            }
        }
        // The deferred pushes the startup pass held for this offer. On a
        // dismissal nothing comes back, so they go now; otherwise after the run,
        // once the resumed rows are in place for the drain's idle check.
        let held_deferred_pushes = state.take_deferred_push_holds();
        if session_ids.is_empty() {
            kick_held_deferred_pushes(&app_handle, held_deferred_pushes);
            return Ok(());
        }
        let deps = run::ResumeDeps {
            store: store?,
            registry,
            action_executor,
            action_registry,
            app_handle: app_handle.clone(),
        };
        tauri::async_runtime::spawn(async move {
            let summary = run::run_launch_resume(deps, session_ids).await;
            kick_held_deferred_pushes(&app_handle, held_deferred_pushes);
            if let Some(state) = app_handle.try_state::<LaunchResumeState>() {
                *state.last_run.lock().unwrap() = Some(summary);
            }
        });
        Ok(())
    })
    .await
    .map_err(|e| format!("resume_interrupted_sessions task failed: {e}"))?
}

fn kick_held_deferred_pushes<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    branch_ids: Vec<String>,
) {
    for branch_id in branch_ids {
        crate::session_queue::kick(
            app_handle,
            &branch_id,
            crate::session_queue::KickSource::LaunchResume,
        );
    }
}
