//! Window close, the quit gate, and shutdown cleanup.
//!
//! Staged's work outlives its windows: agent sessions and long-running actions
//! are child processes this process owns. Two rules follow from that, and this
//! module owns both.
//!
//! **Closing a window is not quitting.** With peer windows still open, a close
//! is just a close — the process lives on in the others, so the window is
//! destroyed normally (`window_commands` owns that cleanup). Closing the *last*
//! window is where the rules bite: on macOS `CloseRequested` is prevented and
//! the window hidden, so sessions keep streaming; the Dock icon
//! (`RunEvent::Reopen`) or `Window ▸ Staged` brings it back. Other platforms
//! have no Dock/tray to recover a hidden window, so closing the last window
//! still quits there — but through the same confirmation gate as `Cmd+Q`.
//!
//! **Quitting with sessions running asks first, then stops them cleanly.**
//! [`request_quit`] gates on active sessions and hands the decision to the
//! frontend dialog, addressed to a single live window (revealed first if every
//! window is hidden); [`shutdown_cleanup`] cancels sessions with
//! [`CompletionReason::AppQuit`] and stops actions. That cancel is the only
//! thing that shuts an agent down: ACP children are spawned with
//! `process_group(0)` and `kill_on_drop`, and `process::exit` runs no
//! destructors, so a bare exit leaves the agent CLIs running.
//!
//! Every exit path funnels into [`shutdown_cleanup`], which runs its work at
//! most once — a confirmed quit calls it directly, `RunEvent::ExitRequested`
//! covers programmatic exits, and `RunEvent::Exit` is the only hook on the
//! `NSApp terminate:` path (Dock ▸ Quit, logout), which never emits
//! `ExitRequested`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow, Window, WindowEvent};

use crate::actions;
use crate::session_commands::{self, ActiveSessionInfo};
use crate::session_runner::SessionRegistry;
use crate::store::{CompletionReason, Session, SessionStatus, Store};

/// Event that raises the frontend's quit confirmation dialog.
const QUIT_REQUESTED_EVENT: &str = "app:quit-requested";

/// Menu id of the app-menu Quit item. Custom rather than
/// `PredefinedMenuItem::quit` so `Cmd+Q` is routable at all: the predefined item
/// maps to `NSApp terminate:`, which reaches no Tauri hook that can gate it.
pub(crate) const QUIT_MENU_ID: &str = "quit";

/// Menu id of `Window ▸ Staged`. The recovery path for `Cmd+Tab`-ing to an app
/// whose windows are all hidden — macOS sends no reopen event for that.
pub(crate) const SHOW_WINDOW_MENU_ID: &str = "show_window";

/// Label of the cold-start window (the `tauri.conf.json` entry). Secondary
/// windows are `win-N` peers — see `window_commands` — with nothing privileged
/// about `main` beyond being the one whose geometry is restored, which makes it
/// the nicest default to reveal.
const MAIN_WINDOW_LABEL: &str = "main";

/// Total budget for stopping sessions and actions. Sessions and actions are
/// signalled first and waited on against this one deadline, because the
/// `RunEvent::Exit` path runs inside `applicationWillTerminate:`, where the OS
/// gives us limited time before killing the process outright.
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(2);

/// Grace period before an action's process group is escalated to `SIGKILL`.
const ACTION_FORCE_KILL_AFTER: Duration = Duration::from_secs(1);

/// Quit bookkeeping, managed as Tauri state.
#[derive(Default)]
pub struct QuitState {
    /// Set by the first caller into [`shutdown_cleanup`], so the cleanup runs
    /// exactly once however many exit events follow it.
    quit_in_progress: AtomicBool,
    /// Label of the window showing an unanswered confirmation dialog. A quit
    /// request arriving while it is set forces the quit — a wedged webview must
    /// never be able to trap the app, so a second `Cmd+Q` always gets out. The
    /// label is what lets a destroyed host window clear the flag instead of
    /// leaving that force path armed with no dialog on screen.
    prompt_host: Mutex<Option<String>>,
}

impl QuitState {
    fn set_prompt_host(&self, label: &str) {
        *self.prompt_host.lock().unwrap() = Some(label.to_string());
    }

    /// Clear any pending prompt, returning whether one was pending.
    fn take_prompt(&self) -> bool {
        self.prompt_host.lock().unwrap().take().is_some()
    }

    /// Clear the pending prompt if `label` was hosting it.
    fn clear_prompt_if_host(&self, label: &str) {
        let mut host = self.prompt_host.lock().unwrap();
        if host.as_deref() == Some(label) {
            *host = None;
        }
    }
}

/// What a quit would interrupt, as sent to the confirmation dialog.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuitBlockers {
    /// Running and queued sessions owned by this process — the only thing that
    /// gates a quit.
    pub sessions: Vec<ActiveSessionInfo>,
    /// Running actions. Reported so the dialog can say they stop too, but they
    /// don't gate the quit on their own: a dev server left running is the normal
    /// state of a workspace, and blocking `Cmd+Q` on it would be noise.
    pub running_action_count: usize,
}

/// Whether a quit should stop and ask first.
///
/// Queued sessions count: they're work the user asked for that a quit silently
/// drops, so they belong in the prompt.
fn should_prompt(blockers: &QuitBlockers) -> bool {
    !blockers.sessions.is_empty()
}

// =============================================================================
// Window events
// =============================================================================

/// `Builder::on_window_event` hook — see the module docs for why closing the
/// last window doesn't end the process.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    match event {
        WindowEvent::CloseRequested { api, .. } => on_close_requested(window, api),
        // A destroyed window takes its webview — and any dialog in it — with
        // it. Left set, the pending flag would turn the next quit request into
        // a silent force-quit; cleared, that quit just asks again.
        WindowEvent::Destroyed => {
            if let Some(quit_state) = window.app_handle().try_state::<QuitState>() {
                quit_state.clear_prompt_if_host(window.label());
            }
        }
        _ => {}
    }
}

fn on_close_requested(window: &Window, api: &tauri::CloseRequestApi) {
    let app = window.app_handle();

    // Mid-shutdown, closes are the exit tearing windows down — stay out of the
    // way.
    if let Some(quit_state) = app.try_state::<QuitState>() {
        if quit_state.quit_in_progress.load(Ordering::SeqCst) {
            return;
        }
    }

    // With peer windows still live (visible or hidden), a close is just a
    // close: sessions belong to the process, not this window. The `Destroyed`
    // hook in lib.rs does the per-window cleanup.
    if app.webview_windows().len() > 1 {
        return;
    }

    // Last window: the window-state plugin has its own `CloseRequested` handler
    // and saves geometry there, so preventing the close still persists the
    // window's position and size.
    api.prevent_close();

    #[cfg(target_os = "macos")]
    hide_window(window);

    // No Dock or tray icon elsewhere, so a hidden window would be unreachable —
    // closing the last window still quits, with the confirmation gate in front
    // of it.
    #[cfg(not(target_os = "macos"))]
    request_quit(app, false);
}

/// Hide the window and drop its PR-poll client to the unfocused tier.
///
/// `prPollingService` derives focus from `document.hasFocus()` and the webview's
/// focus events, and hiding the native window does not reliably deliver a blur
/// to the webview — so tell the scheduler directly instead of leaving it polling
/// on behalf of a window nobody can see.
#[cfg(target_os = "macos")]
fn hide_window(window: &Window) {
    if let Err(e) = window.hide() {
        log::warn!("Failed to hide window on close: {e}");
        return;
    }
    set_native_focus(window.app_handle(), window.label(), false);
}

/// Bring a window back on screen: the Dock-icon click, `Window ▸ Staged`, and a
/// quit arriving with no visible window all funnel here.
pub fn show_a_window(app: &AppHandle) {
    if reveal_a_window(app).is_none() {
        log::warn!("No window left to show");
    }
}

/// Pick a window and make sure it is on screen and focused, returning it.
///
/// Prefers where the user already is (focused, then visible — reachable when a
/// quit request arrives from the store-incompatibility screen or the web
/// dispatch refusal path while windows are up), then falls back to unhiding one:
/// `main` for its restored geometry, else any. `None` only if every window has
/// been destroyed, which no close path produces — closing the last window hides
/// it instead.
fn reveal_a_window(app: &AppHandle) -> Option<WebviewWindow> {
    let windows = app.webview_windows();
    let window = windows
        .values()
        .find(|window| window.is_focused().unwrap_or(false))
        .or_else(|| {
            windows
                .values()
                .find(|window| window.is_visible().unwrap_or(false))
        })
        .or_else(|| windows.get(MAIN_WINDOW_LABEL))
        .or_else(|| windows.values().next())?;

    if let Err(e) = window.show() {
        log::warn!("Failed to show window: {e}");
    }
    if let Err(e) = window.unminimize() {
        log::warn!("Failed to unminimize window: {e}");
    }
    if let Err(e) = window.set_focus() {
        log::warn!("Failed to focus window: {e}");
    }
    set_native_focus(app, window.label(), true);
    Some(window.clone())
}

/// Mirror a native window's visibility onto its PR-poll client's focus hint.
/// Paired with the webview's own focus events, which report the same value once
/// the window is back on screen.
fn set_native_focus(app: &AppHandle, window_label: &str, focused: bool) {
    if let Some(scheduler) = app.try_state::<Arc<crate::pr_poll_scheduler::PrPollScheduler>>() {
        crate::pr_poll_scheduler::set_tauri_client_focus(&scheduler, window_label, focused);
    }
}

// =============================================================================
// Quit gate
// =============================================================================

/// Handle a quit request from the app menu, `Cmd+Q`, or (off macOS) the last
/// window's close. Cheap enough for the main thread: it snapshots blockers and
/// either hands off to a background quit or raises the dialog.
pub fn request_quit(app: &AppHandle, force: bool) {
    let quit_state = app.state::<QuitState>();

    // A quit arriving while the dialog is unanswered (a second `Cmd+Q`) is the
    // escape hatch from a webview that never rendered or answered it.
    if force || quit_state.take_prompt() {
        spawn_quit(app);
        return;
    }

    let blockers = collect_quit_blockers(app);
    if !should_prompt(&blockers) {
        spawn_quit(app);
        return;
    }

    // The dialog goes to exactly one window — where the user is, or a window
    // revealed for the purpose if the quit arrived with everything hidden. A
    // broadcast would raise one dialog per window, each unaware of the others'
    // answers. No window at all means nobody to ask, so the quit proceeds.
    let Some(host) = reveal_a_window(app) else {
        spawn_quit(app);
        return;
    };
    quit_state.set_prompt_host(host.label());

    if let Err(e) = app.emit_to(host.label(), QUIT_REQUESTED_EVENT, &blockers) {
        log::warn!("Failed to ask for quit confirmation, quitting anyway: {e}");
        quit_state.take_prompt();
        spawn_quit(app);
    }
}

/// Quit from the UI, through the same gate as `Cmd+Q`.
///
/// Used by the store-incompatibility screens' "Close" button, which has to end
/// the app: closing the last window only hides it, and those screens have no
/// working database behind them to come back to.
#[tauri::command]
pub fn quit_app(app_handle: AppHandle) {
    request_quit(&app_handle, false);
}

/// Quit confirmed in the dialog: stop sessions and actions, then exit.
///
/// Deliberately absent from the web-mode `dispatch` table — a browser client
/// must not be able to terminate the desktop host.
#[tauri::command]
pub fn confirm_quit(app_handle: AppHandle) {
    app_handle.state::<QuitState>().take_prompt();
    spawn_quit(&app_handle);
}

/// Quit declined in the dialog: sessions keep running.
#[tauri::command]
pub fn cancel_quit(app_handle: AppHandle) {
    app_handle.state::<QuitState>().take_prompt();
}

/// Run the quit sequence off the main thread so the bounded waits never freeze
/// the event loop — the dialog stays interactive and can render its
/// "Stopping sessions…" state while agents shut down.
fn spawn_quit(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        shutdown_cleanup(&app);
        app.exit(0);
    });
}

/// Snapshot what a quit would interrupt.
fn collect_quit_blockers(app: &AppHandle) -> QuitBlockers {
    let sessions = match app_store(app) {
        Some(store) => owned_active_sessions(&store)
            .iter()
            .map(|session| session_commands::project_active_session(&store, session))
            .collect(),
        None => Vec::new(),
    };

    let running_action_count = match (
        app.try_state::<Arc<actions::ActionExecutor>>(),
        app.try_state::<Arc<actions::ActionRegistry>>(),
    ) {
        (Some(executor), Some(registry)) => {
            actions::commands::get_all_running_actions_impl(&executor, &registry)
                .map(|running| running.len())
                .unwrap_or(0)
        }
        _ => 0,
    };

    QuitBlockers {
        sessions,
        running_action_count,
    }
}

// =============================================================================
// Shutdown cleanup
// =============================================================================

/// Stop everything this process owns. Idempotent — the first caller does the
/// work, later ones return immediately.
pub fn shutdown_cleanup(app: &AppHandle) {
    let Some(quit_state) = app.try_state::<QuitState>() else {
        return;
    };
    if quit_state.quit_in_progress.swap(true, Ordering::SeqCst) {
        return;
    }

    // Signal both kinds of work before waiting on either, so they shut down in
    // parallel inside one shared budget instead of one after the other.
    let session_ids = cancel_owned_sessions(app);
    let execution_ids = stop_running_actions(app);

    let deadline = Instant::now() + SHUTDOWN_BUDGET;
    if !session_ids.is_empty() && !wait_for_sessions(app, &session_ids, deadline) {
        log::warn!(
            "Timed out waiting for {} session(s) to stop during app shutdown",
            session_ids.len()
        );
    }
    if !execution_ids.is_empty() && !wait_for_actions(app, &execution_ids, deadline) {
        log::warn!(
            "Timed out waiting for {} action(s) to stop during app shutdown",
            execution_ids.len()
        );
    }

    // Last, so the rows reflect whatever the session threads managed to write
    // for themselves first.
    sweep_active_sessions(app);
}

/// Cancel every session this process is running, recording `AppQuit` as the
/// reason. Returns the ids that were signalled.
fn cancel_owned_sessions(app: &AppHandle) -> Vec<String> {
    let Some(registry) = app.try_state::<Arc<SessionRegistry>>() else {
        return Vec::new();
    };

    let session_ids = registry.running_session_ids();
    for session_id in &session_ids {
        registry.cancel_with_completion_reason(session_id, CompletionReason::AppQuit);
    }
    session_ids
}

/// Send every running action's process group a hangup, escalating to `SIGKILL`
/// after a grace period. Returns the execution ids that were signalled.
fn stop_running_actions(app: &AppHandle) -> Vec<String> {
    let (Some(executor), Some(registry)) = (
        app.try_state::<Arc<actions::ActionExecutor>>(),
        app.try_state::<Arc<actions::ActionRegistry>>(),
    ) else {
        return Vec::new();
    };

    actions::commands::stop_all_actions(
        &executor,
        &registry,
        actions::StopOptions {
            force_kill_after: Some(ACTION_FORCE_KILL_AFTER),
        },
    )
}

fn wait_for_sessions(app: &AppHandle, session_ids: &[String], deadline: Instant) -> bool {
    let Some(registry) = app.try_state::<Arc<SessionRegistry>>() else {
        return true;
    };
    registry.wait_for_sessions(session_ids, remaining_until(deadline))
}

fn wait_for_actions(app: &AppHandle, execution_ids: &[String], deadline: Instant) -> bool {
    let Some(executor) = app.try_state::<Arc<actions::ActionExecutor>>() else {
        return true;
    };
    executor.wait_for_executions(execution_ids, remaining_until(deadline))
}

fn remaining_until(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// Mark whatever is still active in the DB as cancelled by the quit.
///
/// Covers sessions whose thread didn't finish its own terminal write inside the
/// budget, plus queued sessions that never started. Without this the next launch
/// finds them owned by a dead process and reports them as errored sessions.
fn sweep_active_sessions(app: &AppHandle) {
    let Some(store) = app_store(app) else {
        return;
    };

    let swept = owned_active_sessions(&store)
        .iter()
        .filter(|session| {
            // Guarded CAS per row: a session thread that wrote its own terminal
            // status while we were waiting keeps that status.
            store
                .transition_from_active(
                    &session.id,
                    SessionStatus::Cancelled,
                    None,
                    Some(&CompletionReason::AppQuit),
                )
                .unwrap_or_else(|e| {
                    log::warn!("Failed to cancel session {} on quit: {e}", session.id);
                    false
                })
        })
        .count();

    if swept > 0 {
        log::info!("Marked {swept} session(s) cancelled (app_quit) during shutdown");
    }
}

/// Running and queued sessions **this process owns**.
///
/// The store is shared with any other Staged instance pointed at the same data
/// dir — that's what `owner_pid` is for — so a quit must neither prompt about
/// nor cancel another instance's work. Queued rows carry no owner yet, so they
/// count as ours: claiming one (`transition_queued_to_running`) stamps a pid
/// atomically, which is what takes another instance's claim out of this set.
fn owned_active_sessions(store: &Store) -> Vec<Session> {
    let sessions = match store.get_active_sessions() {
        Ok(sessions) => sessions,
        Err(e) => {
            log::warn!("Failed to query active sessions during quit: {e}");
            return Vec::new();
        }
    };

    sessions
        .into_iter()
        .filter(|session| {
            session.status == SessionStatus::Queued || session.owner_pid == Some(std::process::id())
        })
        .collect()
}

fn app_store(app: &AppHandle) -> Option<Arc<Store>> {
    app.try_state::<Mutex<Option<Arc<Store>>>>()
        .and_then(|slot| slot.lock().unwrap().clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn active_session(status: &str) -> ActiveSessionInfo {
        ActiveSessionInfo {
            session_id: "s1".to_string(),
            project_id: None,
            branch_id: None,
            session_type: None,
            status: status.to_string(),
        }
    }

    #[test]
    fn running_sessions_prompt() {
        let blockers = QuitBlockers {
            sessions: vec![active_session("running")],
            running_action_count: 0,
        };
        assert!(should_prompt(&blockers));
    }

    #[test]
    fn queued_sessions_prompt() {
        let blockers = QuitBlockers {
            sessions: vec![active_session("queued")],
            running_action_count: 0,
        };
        assert!(should_prompt(&blockers));
    }

    #[test]
    fn running_actions_alone_do_not_prompt() {
        let blockers = QuitBlockers {
            sessions: Vec::new(),
            running_action_count: 3,
        };
        assert!(!should_prompt(&blockers));
    }

    #[test]
    fn nothing_active_does_not_prompt() {
        assert!(!should_prompt(&QuitBlockers::default()));
    }

    /// The pending flag turns the next quit into a force-quit, so it must not
    /// outlive the window whose dialog it stands for — but a *peer* window
    /// closing must not answer a dialog it isn't showing.
    #[test]
    fn prompt_clears_only_when_its_host_window_is_destroyed() {
        let state = QuitState::default();

        state.set_prompt_host("win-2");
        state.clear_prompt_if_host("main");
        assert!(state.take_prompt(), "peer destruction dropped the prompt");

        state.set_prompt_host("win-2");
        state.clear_prompt_if_host("win-2");
        assert!(
            !state.take_prompt(),
            "host destruction left the prompt armed"
        );
    }

    #[test]
    fn owned_active_sessions_skips_other_instances_running_sessions() {
        let store = Store::in_memory().unwrap();

        let ours = Session::new_running("ours", Path::new("/tmp"));
        store.create_session(&ours).unwrap();
        let queued = Session::new_queued("queued");
        store.create_session(&queued).unwrap();
        let mut theirs = Session::new_running("theirs", Path::new("/tmp"));
        theirs.owner_pid = Some(std::process::id().wrapping_add(1));
        store.create_session(&theirs).unwrap();

        let owned = owned_active_sessions(&store);
        assert_eq!(owned.len(), 2);
        assert!(owned.iter().any(|session| session.id == ours.id));
        assert!(owned.iter().any(|session| session.id == queued.id));
    }

    /// The DB sweep is what keeps the next launch from reporting these sessions
    /// as errors recovered from a dead process.
    #[test]
    fn sweep_cancels_running_and_queued_sessions() {
        let store = Store::in_memory().unwrap();

        let running = Session::new_running("running", Path::new("/tmp"));
        store.create_session(&running).unwrap();
        let queued = Session::new_queued("queued");
        store.create_session(&queued).unwrap();

        for session in owned_active_sessions(&store) {
            assert!(store
                .transition_from_active(
                    &session.id,
                    SessionStatus::Cancelled,
                    None,
                    Some(&CompletionReason::AppQuit),
                )
                .unwrap());
        }

        for id in [&running.id, &queued.id] {
            let session = store.get_session(id).unwrap().unwrap();
            assert_eq!(session.status, SessionStatus::Cancelled);
            assert_eq!(session.completion_reason, Some(CompletionReason::AppQuit));
        }
    }

    #[test]
    fn sweep_leaves_terminal_sessions_alone() {
        let store = Store::in_memory().unwrap();

        let completed = Session::new_running("completed", Path::new("/tmp"));
        store.create_session(&completed).unwrap();
        store
            .update_session_status(
                &completed.id,
                SessionStatus::Completed,
                None,
                Some(&CompletionReason::TurnComplete),
            )
            .unwrap();

        assert!(owned_active_sessions(&store).is_empty());
        assert!(!store
            .transition_from_active(
                &completed.id,
                SessionStatus::Cancelled,
                None,
                Some(&CompletionReason::AppQuit),
            )
            .unwrap());

        let session = store.get_session(&completed.id).unwrap().unwrap();
        assert_eq!(session.status, SessionStatus::Completed);
        assert_eq!(
            session.completion_reason,
            Some(CompletionReason::TurnComplete)
        );
    }
}
