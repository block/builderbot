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
//! [`request_quit`] gates on active sessions and asks; [`shutdown_cleanup`]
//! cancels sessions with [`CompletionReason::AppQuit`] and stops actions. That
//! cancel is the only thing that shuts an agent down: ACP children are spawned
//! with `process_group(0)` and `kill_on_drop`, and `process::exit` runs no
//! destructors, so a bare exit leaves the agent CLIs running.
//!
//! The question is asked by a native alert with **no parent window**, not by a
//! dialog in a webview. Quitting is scoped to the application, and the case the
//! confirmation exists for is precisely the one where every window is hidden:
//! parenting the alert (which `tauri-plugin-dialog` renders as a window-modal
//! sheet) would drag a full window back on screen — restored geometry,
//! hydrating project tree and all — to host a two-button question, and
//! cancelling would leave it there. Unparented, rfd reaches for
//! `CFUserNotificationDisplayAlert` on macOS instead of `NSAlert`: system
//! chrome rather than the app's, and not modal to the app, in exchange for
//! needing no window at all. So quitting from a hidden state stays hidden,
//! cancelling returns the app to exactly the state the user left it in, and
//! there is no longer any state where a quit can't ask.
//!
//! Every exit path funnels into [`shutdown_cleanup`], which runs its work at
//! most once — a confirmed quit calls it directly, `RunEvent::ExitRequested`
//! covers programmatic exits, and `RunEvent::Exit` is the only hook on the
//! `NSApp terminate:` path (Dock ▸ Quit, logout), which never emits
//! `ExitRequested`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, Window, WindowEvent};
use tauri_plugin_dialog::{
    DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult,
};

use crate::actions;
use crate::session_commands::{self, ActiveSessionInfo};
use crate::session_runner::SessionRegistry;
use crate::store::{CompletionReason, Session, SessionStatus, Store};

/// Title of the quit confirmation alert.
const QUIT_PROMPT_TITLE: &str = "Quit Staged?";

/// Alert button that goes through with the quit.
///
/// It sits in `OkCancelCustom`'s *cancel* slot, and [`KEEP_RUNNING_BUTTON`] in
/// the ok slot, because the ok slot is the default (`Return`) button — a stray
/// Return must not be what kills a room full of running agents.
const QUIT_BUTTON: &str = "Quit & Stop Sessions";

/// Alert button that dismisses the prompt and leaves the sessions alone.
const KEEP_RUNNING_BUTTON: &str = "Keep Running";

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
    /// Set while a confirmation alert is unanswered. A quit request arriving
    /// while it is set forces the quit, so an alert that never appeared or never
    /// came back can't trap the app: a second `Cmd+Q` always gets out.
    prompt_pending: AtomicBool,
}

impl QuitState {
    fn set_prompt_pending(&self) {
        self.prompt_pending.store(true, Ordering::SeqCst);
    }

    /// Clear any pending prompt, returning whether one was pending.
    fn take_prompt(&self) -> bool {
        self.prompt_pending.swap(false, Ordering::SeqCst)
    }
}

/// What a quit would interrupt, as the alert describes it.
#[derive(Debug, Default)]
struct QuitBlockers {
    /// One label per running or queued session owned by this process, e.g.
    /// `review on fix-login` — the only thing that gates a quit.
    session_labels: Vec<String>,
    /// Running actions. Reported so the alert can say they stop too, but they
    /// don't gate the quit on their own: a dev server left running is the normal
    /// state of a workspace, and blocking `Cmd+Q` on it would be noise.
    running_action_count: usize,
}

/// Whether a quit should stop and ask first.
///
/// Queued sessions count: they're work the user asked for that a quit silently
/// drops, so they belong in the prompt.
fn should_prompt(blockers: &QuitBlockers) -> bool {
    !blockers.session_labels.is_empty()
}

// =============================================================================
// Window events
// =============================================================================

/// `Builder::on_window_event` hook — see the module docs for why closing the
/// last window doesn't end the process.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        on_close_requested(window, api);
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

/// Bring a window back on screen: the Dock-icon click and `Window ▸ Staged`
/// both funnel here. Deliberately *not* on the quit path — see the module docs.
///
/// Prefers where the user already is (focused, then visible), then falls back to
/// unhiding one: `main` for its restored geometry, else any surviving `win-N`
/// peer. Finds nothing only if every window has been destroyed, which no close
/// path produces — closing the last window hides it instead.
pub fn show_a_window(app: &AppHandle) {
    let windows = app.webview_windows();
    let Some(window) = windows
        .values()
        .find(|window| window.is_focused().unwrap_or(false))
        .or_else(|| {
            windows
                .values()
                .find(|window| window.is_visible().unwrap_or(false))
        })
        .or_else(|| windows.get(MAIN_WINDOW_LABEL))
        .or_else(|| windows.values().next())
    else {
        log::warn!("No window left to show");
        return;
    };

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
/// either hands off to a background quit or raises the alert.
pub fn request_quit(app: &AppHandle, force: bool) {
    let quit_state = app.state::<QuitState>();

    // A quit arriving while the alert is unanswered (a second `Cmd+Q`) is the
    // escape hatch from a prompt that never appeared or never came back. The
    // system alert isn't app-modal, so that second `Cmd+Q` is still dispatchable
    // with the alert on screen.
    if force || quit_state.take_prompt() {
        spawn_quit(app);
        return;
    }

    // Already shutting down, and the alert dismissed on click while cleanup runs
    // out its budget — so there is nothing on screen saying so, and a `Cmd+Q`
    // here means "I already answered", not "ask me again".
    if quit_state.quit_in_progress.load(Ordering::SeqCst) {
        return;
    }

    let blockers = collect_quit_blockers(app);
    if !should_prompt(&blockers) {
        spawn_quit(app);
        return;
    }

    quit_state.set_prompt_pending();
    ask_before_quitting(app, &blockers);
}

/// Raise the confirmation alert and act on the answer.
///
/// No `.parent()`, which is what keeps this window-independent — see the module
/// docs. `tauri-plugin-dialog` hops to the main thread to start the alert and
/// then runs it on its own thread, so this returns immediately and the event
/// loop keeps turning underneath it.
fn ask_before_quitting(app: &AppHandle, blockers: &QuitBlockers) {
    let app = app.clone();
    app.dialog()
        .message(quit_prompt_message(blockers))
        .title(QUIT_PROMPT_TITLE)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            KEEP_RUNNING_BUTTON.to_string(),
            QUIT_BUTTON.to_string(),
        ))
        .show_with_result(move |result| {
            app.state::<QuitState>().take_prompt();
            // Anything that isn't the quit button — "Keep Running", or the
            // system dismissing the alert itself — leaves the sessions alone.
            // Nothing to undo on that path: no window was revealed to host the
            // question, so the app is already in the state the user left it in.
            if quit_confirmed(&result) {
                // The snapshot the message was built from may be stale by now:
                // the alert is not modal to the app, so a session could have
                // started or finished behind it. `shutdown_cleanup` re-queries,
                // so it stops what is actually running.
                spawn_quit(&app);
            }
        });
}

/// Whether the alert was answered with [`QUIT_BUTTON`].
fn quit_confirmed(result: &MessageDialogResult) -> bool {
    matches!(result, MessageDialogResult::Custom(label) if label == QUIT_BUTTON)
}

/// Quit from the UI, through the same gate as `Cmd+Q`.
///
/// Used by the store-incompatibility screens' "Close" button, which has to end
/// the app: closing the last window only hides it, and those screens have no
/// working database behind them to come back to.
///
/// Deliberately absent from the web-mode `dispatch` table — a browser client
/// must not be able to terminate the desktop host.
#[tauri::command]
pub fn quit_app(app_handle: AppHandle) {
    request_quit(&app_handle, false);
}

/// Run the quit sequence off the main thread so the bounded waits never freeze
/// the event loop — windows keep repainting while agents shut down, and the
/// close events the exit generates are still delivered.
fn spawn_quit(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        shutdown_cleanup(&app);
        app.exit(0);
    });
}

/// Snapshot what a quit would interrupt.
fn collect_quit_blockers(app: &AppHandle) -> QuitBlockers {
    let session_labels = match app_store(app) {
        Some(store) => owned_active_sessions(&store)
            .iter()
            .map(|session| {
                let session = session_commands::project_active_session(&store, session);
                let location = session_location(&store, &session);
                quit_session_label(&session, location.as_deref())
            })
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
        session_labels,
        running_action_count,
    }
}

// =============================================================================
// Prompt copy
// =============================================================================

/// How each session type reads in the alert.
fn session_type_label(session_type: &str) -> Option<&'static str> {
    match session_type {
        "note" => Some("note"),
        "commit" => Some("commit"),
        "review" => Some("review"),
        "pr" => Some("PR"),
        "push" => Some("push"),
        "pull" => Some("pull"),
        _ => None,
    }
}

/// Where a session is running, as the user knows it: its branch name, or its
/// project name for project-level sessions (a note on a project has no branch).
fn session_location(store: &Store, session: &ActiveSessionInfo) -> Option<String> {
    let branch_name = session
        .branch_id
        .as_deref()
        .and_then(|id| store.get_branch(id).ok().flatten())
        .map(|branch| branch.branch_name);

    branch_name.or_else(|| {
        session
            .project_id
            .as_deref()
            .and_then(|id| store.get_project(id).ok().flatten())
            .map(|project| project.name)
    })
}

/// Label for one session a quit would stop, e.g. `review on fix-login` or
/// `commit on fix-login (queued)`.
///
/// Both halves can be missing — an unrecognised session type, or a row whose
/// branch and project have already been deleted — so each falls back rather than
/// dropping the session from the list.
fn quit_session_label(session: &ActiveSessionInfo, location: Option<&str>) -> String {
    let kind = session
        .session_type
        .as_deref()
        .and_then(session_type_label)
        .unwrap_or("session");
    let base = match location {
        Some(location) => format!("{kind} on {location}"),
        None => kind.to_string(),
    };

    if session.status == SessionStatus::Queued.as_str() {
        format!("{base} (queued)")
    } else {
        base
    }
}

/// Alert body: how much stops, what it is, and whether actions go with it.
///
/// Actions never gate the quit (see [`should_prompt`]), so they are mentioned
/// only as a consequence of one.
fn quit_prompt_message(blockers: &QuitBlockers) -> String {
    let labels = blockers.session_labels.join(", ");
    let mut message = if blockers.session_labels.len() == 1 {
        format!("1 session is still running: {labels}. Quitting will stop it.")
    } else {
        format!(
            "{} sessions are still running: {labels}. Quitting will stop them.",
            blockers.session_labels.len()
        )
    };

    match blockers.running_action_count {
        0 => {}
        1 => message.push_str(" 1 running action will also stop."),
        count => message.push_str(&format!(" {count} running actions will also stop.")),
    }

    message
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

    fn active_session(session_type: Option<&str>, status: SessionStatus) -> ActiveSessionInfo {
        ActiveSessionInfo {
            session_id: "s1".to_string(),
            project_id: Some("p1".to_string()),
            branch_id: Some("b1".to_string()),
            session_type: session_type.map(str::to_string),
            status: status.as_str().to_string(),
        }
    }

    fn blockers(session_labels: &[&str], running_action_count: usize) -> QuitBlockers {
        QuitBlockers {
            session_labels: session_labels.iter().map(|s| s.to_string()).collect(),
            running_action_count,
        }
    }

    #[test]
    fn active_sessions_prompt() {
        assert!(should_prompt(&blockers(&["review on fix-login"], 0)));
    }

    #[test]
    fn running_actions_alone_do_not_prompt() {
        assert!(!should_prompt(&blockers(&[], 3)));
    }

    #[test]
    fn nothing_active_does_not_prompt() {
        assert!(!should_prompt(&QuitBlockers::default()));
    }

    /// The pending flag turns the next quit into a force-quit, so answering the
    /// alert has to disarm it — otherwise the next `Cmd+Q` quits without asking.
    #[test]
    fn answering_the_prompt_disarms_the_force_path() {
        let state = QuitState::default();

        assert!(!state.take_prompt(), "nothing pending, nothing to force");

        state.set_prompt_pending();
        assert!(state.take_prompt(), "pending prompt did not arm the force");
        assert!(!state.take_prompt(), "prompt stayed armed after answering");
    }

    /// The ok slot is the default (`Return`) button, so it holds "Keep Running"
    /// and the cancel slot holds the destructive answer.
    #[test]
    fn only_the_quit_button_confirms() {
        assert!(quit_confirmed(&MessageDialogResult::Custom(
            QUIT_BUTTON.to_string()
        )));
        assert!(!quit_confirmed(&MessageDialogResult::Custom(
            KEEP_RUNNING_BUTTON.to_string()
        )));
        // What a system-dismissed alert reports.
        assert!(!quit_confirmed(&MessageDialogResult::Cancel));
        assert!(!quit_confirmed(&MessageDialogResult::Ok));
    }

    #[test]
    fn session_label_names_the_type_and_where_it_runs() {
        assert_eq!(
            quit_session_label(
                &active_session(Some("review"), SessionStatus::Running),
                Some("fix-login")
            ),
            "review on fix-login"
        );
    }

    #[test]
    fn session_label_marks_queued_sessions() {
        assert_eq!(
            quit_session_label(
                &active_session(Some("review"), SessionStatus::Queued),
                Some("fix-login")
            ),
            "review on fix-login (queued)"
        );
    }

    #[test]
    fn session_label_falls_back_for_unknown_type_or_missing_location() {
        assert_eq!(
            quit_session_label(&active_session(None, SessionStatus::Running), Some("docs")),
            "session on docs"
        );
        assert_eq!(
            quit_session_label(
                &active_session(Some("mystery"), SessionStatus::Running),
                Some("docs")
            ),
            "session on docs"
        );
        assert_eq!(
            quit_session_label(&active_session(Some("note"), SessionStatus::Running), None),
            "note"
        );
    }

    /// A branch session reads as its branch; a project-level session (a note on
    /// a project) has no branch, so it reads as its project.
    #[test]
    fn session_location_prefers_the_branch_then_the_project() {
        let store = Store::in_memory().unwrap();
        let mut project = crate::store::Project::new("owner/repo");
        project.name = "Widgets".to_string();
        store.create_project(&project).unwrap();
        let branch = crate::store::Branch::new(&project.id, "fix-login", "main");
        store.create_branch(&branch).unwrap();

        let mut session = active_session(Some("note"), SessionStatus::Running);
        session.project_id = Some(project.id.clone());
        session.branch_id = Some(branch.id.clone());
        assert_eq!(
            session_location(&store, &session).as_deref(),
            Some("fix-login")
        );

        session.branch_id = None;
        assert_eq!(
            session_location(&store, &session).as_deref(),
            Some("Widgets")
        );

        session.project_id = None;
        assert_eq!(session_location(&store, &session), None);
    }

    #[test]
    fn prompt_message_reads_singular_for_one_session() {
        assert_eq!(
            quit_prompt_message(&blockers(&["commit on fix-login"], 0)),
            "1 session is still running: commit on fix-login. Quitting will stop it."
        );
    }

    #[test]
    fn prompt_message_lists_every_session_for_a_plural_count() {
        assert_eq!(
            quit_prompt_message(&blockers(&["commit on fix-login", "note on docs"], 0)),
            "2 sessions are still running: commit on fix-login, note on docs. \
             Quitting will stop them."
        );
    }

    #[test]
    fn prompt_message_mentions_actions_only_when_there_are_some() {
        assert!(quit_prompt_message(&blockers(&["commit on fix-login"], 1))
            .ends_with(" 1 running action will also stop."));
        assert!(quit_prompt_message(&blockers(&["commit on fix-login"], 3))
            .ends_with(" 3 running actions will also stop."));
        assert!(!quit_prompt_message(&blockers(&["commit on fix-login"], 0)).contains("action"));
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
