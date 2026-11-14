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
//! still quits there — but through the same confirmation gate as `Cmd+Q`, and
//! clicking the close button again asks again instead of forcing the quit (see
//! [`QuitTrigger`]).
//!
//! **Quitting with sessions running asks first, then stops them cleanly.**
//! [`request_quit`] gates on active sessions and asks; [`shutdown_cleanup`]
//! cancels sessions with [`CompletionReason::AppQuit`] and stops actions. That
//! cancel is the only thing that shuts an agent down: ACP children are spawned
//! with `process_group(0)` and `kill_on_drop`, and `process::exit` runs no
//! destructors, so a bare exit leaves the agent CLIs running. Blox sessions are
//! the exception: their agent runs on the workstation, so a quit *detaches*
//! them instead — it kills only the local `blox acp` proxy, sending no
//! `session/cancel`, and leaves the row `disconnected` with the turn still
//! running remotely (see [`SessionRegistry::detach`]). Since a quit stops
//! nothing of theirs, they don't gate it: with only Blox sessions running it
//! asks nothing, and when it does ask it counts only what it stops. Once a
//! shutdown is claimed the branch queue stops draining
//! ([`QuitState::is_quitting`]), so those cancels — terminal transitions like
//! any other — can't feed fresh agent children into an exit that would orphan
//! them.
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
//! most once and makes later callers wait for it to finish — a confirmed quit
//! calls it directly, `RunEvent::ExitRequested` covers programmatic exits, and
//! `RunEvent::Exit` is the only hook on the `NSApp terminate:` path (Dock ▸
//! Quit, logout), which never emits `ExitRequested`. That wait is what keeps an
//! impatient second quit gesture from cutting a cleanup already in flight short.

use std::collections::HashSet;
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

pub(crate) mod window_raise;
pub mod window_reveal;
pub(crate) mod window_use;

pub use window_raise::show_a_window;
pub use window_reveal::RevealedWindows;
pub use window_use::UsedWindows;

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
pub(crate) const MAIN_WINDOW_LABEL: &str = "main";

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
    /// Held for the duration of [`shutdown_cleanup`]'s work; `true` once it has
    /// completed. The lock is what makes a late caller *wait* rather than skip:
    /// see [`run_cleanup_once`](Self::run_cleanup_once).
    cleanup_done: Mutex<bool>,
    /// Cheap "a shutdown is under way" signal, published by the caller that
    /// claims the cleanup. Separate from `cleanup_done` because its readers
    /// ([`request_quit`] and [`on_close_requested`] on the main thread, the
    /// queue drain on the tokio runtime) run while a cleanup may be in flight
    /// and must not block on the lock. The mutex claims; this atomic publishes.
    quit_in_progress: AtomicBool,
    /// Set while a confirmation alert is unanswered. An
    /// [`Explicit`](QuitTrigger::Explicit) quit arriving while it is set forces
    /// the quit, so an alert that never appeared or never came back can't trap
    /// the app: a second `Cmd+Q` always gets out. A repeated window close does
    /// not force — it asks again. See [`QuitTrigger`].
    prompt_pending: AtomicBool,
}

impl QuitState {
    /// Whether a shutdown has been claimed. Non-blocking by construction — see
    /// [`quit_in_progress`](Self::quit_in_progress).
    ///
    /// Read by the queue drain to stop starting new work mid-shutdown, and by
    /// [`on_close_requested`] to stay out of the way of the exit's own closes.
    pub(crate) fn is_quitting(&self) -> bool {
        self.quit_in_progress.load(Ordering::SeqCst)
    }

    fn set_prompt_pending(&self) {
        self.prompt_pending.store(true, Ordering::SeqCst);
    }

    /// Clear any pending prompt, returning whether one was pending.
    fn take_prompt(&self) -> bool {
        self.prompt_pending.swap(false, Ordering::SeqCst)
    }

    /// Run `cleanup` at most once. A caller that arrives while it is already
    /// running **blocks until it has finished**, then returns without re-running
    /// it.
    ///
    /// Waiting, rather than returning early, is the point. A confirmed quit runs
    /// the cleanup on a background thread with nothing on screen for up to
    /// [`SHUTDOWN_BUDGET`], and a `terminate:` arriving in that window (an
    /// impatient Dock ▸ Quit) reaches [`shutdown_cleanup`] via `RunEvent::Exit`,
    /// on the main thread, inside `applicationWillTerminate:`. Returning there
    /// would let that method return and the OS kill the process mid-cancel —
    /// orphaning the agent CLIs (own process groups, `kill_on_drop` destructors
    /// an OS kill never runs) and skipping the DB sweep. Holding it open until
    /// the work is done is what those two guarantees need.
    ///
    /// A poisoned lock is taken over rather than propagated: if the first caller
    /// panicked mid-cleanup, `done` is still `false` and the late caller re-runs
    /// the work, which is the right recovery given every step is idempotent
    /// (cancelling a cancelled session is a no-op; the sweep is a guarded CAS
    /// per row). That's also why this isn't a [`std::sync::Once`] despite the
    /// matching blocking semantics — a panicked `call_once` poisons the `Once`
    /// and makes every later caller panic, and a panic inside
    /// `applicationWillTerminate:` aborts with no cleanup at all.
    fn run_cleanup_once(&self, cleanup: impl FnOnce()) {
        let mut done = self
            .cleanup_done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *done {
            return;
        }
        self.quit_in_progress.store(true, Ordering::SeqCst);
        cleanup();
        *done = true;
    }
}

/// [`QuitState::is_quitting`] from anywhere that holds an `AppHandle`. `false`
/// when the state isn't managed (mock apps in tests). Generic over the runtime
/// so the queue kicker, which mock-app tests drive, can read it too.
pub(crate) fn is_quitting<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<QuitState>()
        .is_some_and(|quit_state| quit_state.is_quitting())
}

/// What a quit would interrupt, as the alert describes it.
#[derive(Debug, Default)]
struct QuitBlockers {
    /// One label per running or queued session owned by this process that a
    /// quit stops, e.g. `review on fix-login`. These are the only thing that
    /// gates a quit. A Blox session the quit detaches is left out (see
    /// [`sessions_a_quit_stops`]): it keeps running on its workstation.
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
///
/// Also keeps the record of used windows ([`UsedWindows`]) in step with focus,
/// which is how a reveal finds the window the user was last in once Staged is
/// no longer the frontmost app, and drops a destroyed window from it and from
/// the record of revealed windows ([`RevealedWindows`]).
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    match event {
        WindowEvent::CloseRequested { api, .. } => on_close_requested(window, api),
        WindowEvent::Focused(true) => {
            window_use::note_window_used(window.app_handle(), window.label());
        }
        WindowEvent::Destroyed => {
            window_use::forget_window(window.app_handle(), window.label());
            window_reveal::forget_window(window.app_handle(), window.label());
        }
        _ => {}
    }
}

fn on_close_requested(window: &Window, api: &tauri::CloseRequestApi) {
    let app = window.app_handle();

    // Mid-shutdown, closes are the exit tearing windows down — stay out of the
    // way.
    if let Some(quit_state) = app.try_state::<QuitState>() {
        if quit_state.is_quitting() {
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
    // of it. Clicking the X again re-raises that question rather than forcing
    // the quit — see `QuitTrigger::may_force`.
    #[cfg(not(target_os = "macos"))]
    request_quit(app, QuitTrigger::WindowClose);
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

/// Mirror a native window's visibility onto its PR-poll client's focus hint.
/// Paired with the webview's own focus events, which report the same value once
/// the window is back on screen.
///
/// Generic over the runtime so [`window_raise`]'s mock-app tests can raise a
/// window too.
fn set_native_focus<R: tauri::Runtime>(app: &AppHandle<R>, window_label: &str, focused: bool) {
    if let Some(scheduler) = app.try_state::<Arc<crate::pr_poll_scheduler::PrPollScheduler>>() {
        crate::pr_poll_scheduler::set_tauri_client_focus(&scheduler, window_label, focused);
    }
}

// =============================================================================
// Quit gate
// =============================================================================

/// What asked the app to quit.
///
/// A first request is a first request whatever raised it; the trigger decides
/// what a *repeat* means while the confirmation alert is still unanswered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitTrigger {
    /// A quit aimed at the application: the app menu's Quit item (`Cmd+Q`), or
    /// the store-incompatibility screens' Close button.
    Explicit,
    /// The last window's close button — or `Alt+F4`, which takes the same
    /// `CloseRequested` path — off macOS, where closing the last window quits.
    WindowClose,
}

impl QuitTrigger {
    /// Whether repeating this trigger while the alert is unanswered quits
    /// without an answer.
    ///
    /// Only an [`Explicit`](Self::Explicit) quit may: a second `Cmd+Q` is a
    /// deliberate second attempt at quitting, and it has to get out in case the
    /// alert never appeared or never came back. A close button carries no such
    /// intent. Off macOS it is the only interactive quit trigger there is, and
    /// it is the thing users click twice in a second when a window won't shut —
    /// reflex, aimed at the window, not an answer to a question they may not
    /// have noticed. Killing running agents on that reflex is the opposite of
    /// what the gate exists for.
    ///
    /// Asking again covers the trapped case just as well, without the damage:
    /// clicking the X puts an answerable question back on screen — a second
    /// copy of it if the first is still up, which is a cheap thing to dismiss
    /// next to a forced quit — and if the sessions have finished in the
    /// meantime, the repeat quits with nothing left to warn about.
    fn may_force(self) -> bool {
        matches!(self, Self::Explicit)
    }
}

/// Handle a quit request from the app menu, `Cmd+Q`, or (off macOS) the last
/// window's close. Cheap enough for the main thread: it snapshots blockers and
/// either hands off to a background quit or raises the alert.
pub fn request_quit(app: &AppHandle, trigger: QuitTrigger) {
    let quit_state = app.state::<QuitState>();

    // An explicit quit arriving while the alert is unanswered (a second
    // `Cmd+Q`) is the escape hatch from a prompt that never appeared or never
    // came back. The system alert isn't app-modal, so that second `Cmd+Q` is
    // still dispatchable with the alert on screen. A repeated window close
    // deliberately doesn't take this path and falls through to the gate below,
    // which asks again.
    if trigger.may_force() && quit_state.take_prompt() {
        spawn_quit(app);
        return;
    }

    // Already shutting down, and the alert dismissed on click while cleanup runs
    // out its budget — so there is nothing on screen saying so, and a `Cmd+Q`
    // here means "I already answered", not "ask me again".
    if quit_state.is_quitting() {
        return;
    }

    let blockers = collect_quit_blockers(app);
    if !should_prompt(&blockers) {
        spawn_quit(app);
        return;
    }

    // A prompt already pending here means a repeated window close: the flag is
    // already set, so setting it again is a no-op, and the alert goes back up
    // describing whatever is running *now*.
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
    request_quit(&app_handle, QuitTrigger::Explicit);
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
        Some(store) => {
            let registry = app.try_state::<Arc<SessionRegistry>>();
            sessions_a_quit_stops(&store, registry.as_deref().map(Arc::as_ref))
        }
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

/// Labels of the owned sessions a quit would stop.
///
/// That is every one except a running session the quit would detach. The
/// registry makes that call ([`SessionRegistry::would_detach`]), and the quit
/// acts on the same call ([`signal_owned_sessions`], [`sweep_sessions`]). That
/// leaves out only a connected or reattaching Blox session. A Blox session
/// still starting its workstation, before its connection is up, is cancelled
/// by a quit, so it counts, as does a queued one, which has no remote turn
/// yet. With no registry nothing would be detached, so everything counts.
fn sessions_a_quit_stops(store: &Store, registry: Option<&SessionRegistry>) -> Vec<String> {
    owned_active_sessions(store)
        .iter()
        .filter(|session| {
            let detaches = session.status == SessionStatus::Running
                && registry.is_some_and(|registry| registry.would_detach(&session.id));
            !detaches
        })
        .map(|session| {
            let info = session_commands::project_active_session(store, session);
            let location = session_location(store, &info);
            quit_session_label(&info, location.as_deref())
        })
        .collect()
}

// =============================================================================
// Prompt copy
// =============================================================================

/// How each session type reads in the alert.
pub(crate) fn session_type_label(session_type: &str) -> Option<&'static str> {
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
pub(crate) fn session_location(store: &Store, session: &ActiveSessionInfo) -> Option<String> {
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
pub(crate) fn quit_session_label(session: &ActiveSessionInfo, location: Option<&str>) -> String {
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
/// Only what the quit stops: a Blox session it detaches keeps running on its
/// workstation, so it is neither counted nor named.
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

/// Stop everything this process owns. Runs its work at most once — the first
/// caller does it, and a caller arriving while it runs waits for it to finish
/// (see [`QuitState::run_cleanup_once`]) rather than returning to an exit that
/// would kill the process mid-cleanup.
pub fn shutdown_cleanup(app: &AppHandle) {
    let Some(quit_state) = app.try_state::<QuitState>() else {
        return;
    };

    quit_state.run_cleanup_once(|| {
        // Retire the stagedctl discovery file and PATH shim dir first so CLI
        // callers stop finding an instance that is going away.
        crate::control_server::remove_instance_files();

        // Signal both kinds of work before waiting on either, so they shut down
        // in parallel inside one shared budget instead of one after the other.
        let OwnedSessionSignals {
            session_ids,
            detached,
        } = signal_owned_sessions(app);
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

        // Last, so the rows reflect whatever the session threads managed to
        // write for themselves first.
        sweep_active_sessions(app, &detached);
    });
}

/// What [`signal_owned_sessions`] did.
#[derive(Default)]
struct OwnedSessionSignals {
    /// Every session signalled, to wait on.
    session_ids: Vec<String>,
    /// The Blox sessions among them that were detached rather than cancelled.
    detached: HashSet<String>,
}

/// Stop every session this process is running, recording `AppQuit` as the
/// reason — except a connected Blox session, which is detached and keeps
/// running on its workstation (see [`SessionRegistry::detach`]). So does one
/// reattaching to its remote turn with no connection up to detach: waiting
/// between attempts, or before its first is up. Cancelling it would end the
/// row `cancelled` with the turn still running on the workstation, out of
/// reconcile's reach.
///
/// Detach is SIGKILL-only by design. A SIGINT to the `blox acp` proxy that
/// started the remote agent stops that agent and every turn on it, even with
/// no `session/cancel`, so the graceful stop a cancel ends in would end the
/// very turn the detach means to keep.
fn signal_owned_sessions(app: &AppHandle) -> OwnedSessionSignals {
    let Some(registry) = app.try_state::<Arc<SessionRegistry>>() else {
        return OwnedSessionSignals::default();
    };

    let session_ids = registry.running_session_ids();
    let mut detached = HashSet::new();
    for session_id in &session_ids {
        if registry.detach(session_id, CompletionReason::AppQuit) {
            detached.insert(session_id.clone());
        } else {
            registry.cancel_with_completion_reason(session_id, CompletionReason::AppQuit);
        }
    }
    OwnedSessionSignals {
        session_ids,
        detached,
    }
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

/// Mark whatever is still active in the DB as cancelled by the quit, or as
/// `disconnected` for a detached Blox session.
///
/// Covers sessions whose thread didn't finish its own terminal write inside the
/// budget, plus queued sessions that never started. Without this the next launch
/// finds them owned by a dead process and reports them as errored sessions.
fn sweep_active_sessions(app: &AppHandle, detached: &HashSet<String>) {
    let Some(store) = app_store(app) else {
        return;
    };

    let swept = sweep_sessions(&store, detached);
    if swept > 0 {
        log::info!("Settled {swept} session(s) the quit left active");
    }
}

/// The sweep itself: snapshot what we own, then CAS each row to cancelled —
/// or to `disconnected` for a session in `detached`, whose turn is still
/// running on its workstation. Returns how many rows it actually moved.
fn sweep_sessions(store: &Store, detached: &HashSet<String>) -> usize {
    let owner_pid = std::process::id();

    owned_active_sessions(store)
        .iter()
        .filter(|session| {
            let (status, reason) =
                if session.status == SessionStatus::Running && detached.contains(&session.id) {
                    (SessionStatus::Disconnected, None)
                } else {
                    (SessionStatus::Cancelled, Some(CompletionReason::AppQuit))
                };
            // Guarded CAS per row, on liveness *and* ownership. A session thread
            // that wrote its own terminal status while we were waiting keeps
            // that status; so does a row another instance claimed since the
            // snapshot, which stamped its pid in the same statement that took
            // the row off `queued`.
            store
                .transition_from_owned_active(&session.id, status, None, reason.as_ref(), owner_pid)
                .unwrap_or_else(|e| {
                    log::warn!("Failed to cancel session {} on quit: {e}", session.id);
                    false
                })
        })
        .count()
}

/// Running and queued sessions **this process owns**.
///
/// The store is shared with any other Staged instance pointed at the same data
/// dir — that's what `owner_pid` is for — so a quit must neither prompt about
/// nor cancel another instance's work. Queued rows carry no owner yet, so they
/// count as ours: claiming one (`transition_queued_to_running`) stamps a pid
/// atomically, which is what takes another instance's claim out of this set.
///
/// A claim can also land *after* this snapshot, which is why the sweep's CAS
/// (`transition_from_owned_active`) re-checks the same ownership rule at write
/// time rather than trusting the list this returns.
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
    use std::sync::Barrier;

    fn active_session(session_type: Option<&str>, status: SessionStatus) -> ActiveSessionInfo {
        ActiveSessionInfo {
            session_id: "s1".to_string(),
            project_id: Some("p1".to_string()),
            branch_id: Some("b1".to_string()),
            session_type: session_type.map(str::to_string),
            status: status.as_str().to_string(),
            holding: false,
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

    /// A session on `branch`, created in `store`.
    fn session_on(store: &Store, branch: &crate::store::Branch, status: SessionStatus) -> Session {
        let mut session = match status {
            SessionStatus::Queued => Session::new_queued("work"),
            _ => Session::new_running("work", Path::new("/tmp")),
        };
        session.branch_id = Some(branch.id.clone());
        store.create_session(&session).unwrap();
        session
    }

    /// A store with a local branch (`local-fix`) and a Blox one (`blox-fix`).
    fn store_with_branches() -> (Store, crate::store::Branch, crate::store::Branch) {
        let store = Store::in_memory().unwrap();
        let project = crate::store::Project::new("owner/repo");
        store.create_project(&project).unwrap();
        let local = crate::store::Branch::new(&project.id, "local-fix", "main");
        store.create_branch(&local).unwrap();
        let blox = crate::store::Branch::new_remote(&project.id, "blox-fix", "main", "ws-blox");
        store.create_branch(&blox).unwrap();
        (store, local, blox)
    }

    /// The quit prompt's count. A running Blox session the quit detaches
    /// (here one reattaching to its remote turn, which `detach` lets go of
    /// with or without a connection up) keeps running on its workstation and
    /// is not counted. A quit cancels the rest, so they count: a running
    /// local session, a Blox session the registry can't detach yet (still
    /// starting its workstation, no connection up) and a queued Blox session.
    #[test]
    fn only_sessions_a_quit_stops_are_counted() {
        let (store, local_branch, blox_branch) = store_with_branches();
        let local = session_on(&store, &local_branch, SessionStatus::Running);
        let blox = session_on(&store, &blox_branch, SessionStatus::Running);
        let connecting = session_on(&store, &blox_branch, SessionStatus::Running);
        session_on(&store, &blox_branch, SessionStatus::Queued);

        let registry = Arc::new(SessionRegistry::new());
        let _local = registry.register_external(&local.id);
        let _blox = registry.register_external(&blox.id);
        let reattach = registry.reattaching_for_test(&blox.id);
        let _connecting = registry.register_external(&connecting.id);

        let mut counted = sessions_a_quit_stops(&store, Some(&registry));
        counted.sort();
        assert_eq!(
            counted,
            [
                "session on blox-fix",
                "session on blox-fix (queued)",
                "session on local-fix",
            ]
        );
        assert!(!reattach.is_cancelled(), "counting detaches nothing");
        assert_eq!(
            quit_prompt_message(&QuitBlockers {
                session_labels: counted,
                running_action_count: 0,
            }),
            "3 sessions are still running: session on blox-fix, session on blox-fix (queued), \
             session on local-fix. Quitting will stop them."
        );
    }

    /// With every running session on Blox and detachable, nothing gates the
    /// quit, whatever actions are running. Without a registry nothing would
    /// be detached, so the same session counts.
    #[test]
    fn only_detachable_blox_sessions_running_does_not_prompt() {
        let (store, _, blox_branch) = store_with_branches();
        let blox = session_on(&store, &blox_branch, SessionStatus::Running);
        let registry = Arc::new(SessionRegistry::new());
        let _blox = registry.register_external(&blox.id);
        let _reattach = registry.reattaching_for_test(&blox.id);

        let session_labels = sessions_a_quit_stops(&store, Some(&registry));
        assert!(session_labels.is_empty(), "{session_labels:?}");
        assert!(!should_prompt(&QuitBlockers {
            session_labels,
            running_action_count: 1,
        }));

        assert_eq!(sessions_a_quit_stops(&store, None), ["session on blox-fix"]);
    }

    #[test]
    fn running_actions_alone_do_not_prompt() {
        assert!(!should_prompt(&blockers(&[], 3)));
    }

    #[test]
    fn nothing_active_does_not_prompt() {
        assert!(!should_prompt(&QuitBlockers::default()));
    }

    /// A second `Cmd+Q` forces the quit; a second click on a close button asks
    /// again, because that click is aimed at the window and is exactly the one
    /// users repeat by reflex.
    #[test]
    fn only_an_explicit_quit_can_force_past_the_prompt() {
        assert!(QuitTrigger::Explicit.may_force());
        assert!(!QuitTrigger::WindowClose.may_force());
    }

    /// The pending flag turns the next explicit quit into a force-quit, so
    /// answering the alert has to disarm it — otherwise the next `Cmd+Q` quits
    /// without asking.
    #[test]
    fn answering_the_prompt_disarms_the_force_path() {
        let state = QuitState::default();

        assert!(!state.take_prompt(), "nothing pending, nothing to force");

        state.set_prompt_pending();
        assert!(state.take_prompt(), "pending prompt did not arm the force");
        assert!(!state.take_prompt(), "prompt stayed armed after answering");
    }

    /// The `RunEvent::Exit` case: a `terminate:` arriving while a confirmed
    /// quit's cleanup is still running must hold `applicationWillTerminate:`
    /// open until that cleanup finishes, not return to an exit that kills the
    /// process mid-cancel.
    #[test]
    fn a_late_caller_waits_for_the_running_cleanup() {
        let state = Arc::new(QuitState::default());
        // Trips inside the first closure, so passing it proves the first caller
        // holds the lock before the test thread tries to take it.
        let entered = Arc::new(Barrier::new(2));
        let finished = Arc::new(AtomicBool::new(false));

        let first = {
            let state = Arc::clone(&state);
            let entered = Arc::clone(&entered);
            let finished = Arc::clone(&finished);
            std::thread::spawn(move || {
                state.run_cleanup_once(|| {
                    entered.wait();
                    // Stands in for the bounded waits, so the second caller has
                    // to block rather than happening to arrive after the fact.
                    std::thread::sleep(Duration::from_millis(50));
                    finished.store(true, Ordering::SeqCst);
                });
            })
        };

        entered.wait();
        let second_ran = AtomicBool::new(false);
        state.run_cleanup_once(|| second_ran.store(true, Ordering::SeqCst));

        assert!(
            finished.load(Ordering::SeqCst),
            "second call returned before the running cleanup had finished"
        );
        assert!(
            !second_ran.load(Ordering::SeqCst),
            "second call re-ran the cleanup instead of waiting for it"
        );
        first.join().unwrap();
    }

    #[test]
    fn a_sequential_second_call_does_not_run_the_cleanup_again() {
        let state = QuitState::default();
        let runs = AtomicBool::new(false);

        state.run_cleanup_once(|| runs.store(true, Ordering::SeqCst));
        runs.store(false, Ordering::SeqCst);
        state.run_cleanup_once(|| runs.store(true, Ordering::SeqCst));

        assert!(!runs.load(Ordering::SeqCst));
    }

    /// `request_quit`'s "already shutting down" early return,
    /// `on_close_requested`'s stay-out-of-the-way check, and the queue drain's
    /// gate all read this while a cleanup is in flight, so it has to be
    /// published before the work starts rather than after it.
    #[test]
    fn quit_in_progress_is_published_while_the_cleanup_runs() {
        let state = QuitState::default();
        assert!(!state.is_quitting());

        state.run_cleanup_once(|| {
            assert!(
                state.is_quitting(),
                "shutdown was under way but the flag still read false"
            );
        });

        assert!(state.is_quitting());
    }

    /// Poisoning is taken over rather than propagated, so a cleanup that
    /// panicked half-done gets re-run by the next exit event — every step of it
    /// is idempotent.
    #[test]
    fn a_panicked_cleanup_is_retried() {
        let state = Arc::new(QuitState::default());

        let panicked = {
            let state = Arc::clone(&state);
            std::thread::spawn(move || state.run_cleanup_once(|| panic!("cleanup blew up")))
        };
        assert!(panicked.join().is_err(), "expected the panic to propagate");

        let retried = AtomicBool::new(false);
        state.run_cleanup_once(|| retried.store(true, Ordering::SeqCst));
        assert!(retried.load(Ordering::SeqCst));
    }

    /// The ok slot is the default (`Return`) button, so it holds the dismissing
    /// answer and the cancel slot holds the quit.
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

        assert_eq!(sweep_sessions(&store, &HashSet::new()), 2);

        for id in [&running.id, &queued.id] {
            let session = store.get_session(id).unwrap().unwrap();
            assert_eq!(session.status, SessionStatus::Cancelled);
            assert_eq!(session.completion_reason, Some(CompletionReason::AppQuit));
        }
    }

    /// A detached Blox session whose thread did not write its own state in
    /// time is left `disconnected` and unowned, not cancelled: its turn is
    /// still running on the workstation. A queued row in the same set has no
    /// turn to keep, and a disconnected row is not the sweep's to touch.
    #[test]
    fn sweep_leaves_detached_sessions_disconnected() {
        let store = Store::in_memory().unwrap();

        let detached = Session::new_running("detached", Path::new("/tmp"));
        store.create_session(&detached).unwrap();
        let queued = Session::new_queued("queued");
        store.create_session(&queued).unwrap();
        let already = Session::new_running("already", Path::new("/tmp"));
        store.create_session(&already).unwrap();
        store
            .update_session_status(&already.id, SessionStatus::Disconnected, None, None)
            .unwrap();

        let ids = HashSet::from([detached.id.clone(), queued.id.clone(), already.id.clone()]);
        assert_eq!(sweep_sessions(&store, &ids), 2);

        let session = store.get_session(&detached.id).unwrap().unwrap();
        assert_eq!(session.status, SessionStatus::Disconnected);
        assert_eq!(session.completion_reason, None);
        assert_eq!(session.owner_pid, None);
        let session = store.get_session(&queued.id).unwrap().unwrap();
        assert_eq!(session.status, SessionStatus::Cancelled);
        assert_eq!(session.completion_reason, Some(CompletionReason::AppQuit));
        let session = store.get_session(&already.id).unwrap().unwrap();
        assert_eq!(session.status, SessionStatus::Disconnected);
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
        assert_eq!(sweep_sessions(&store, &HashSet::new()), 0);

        let session = store.get_session(&completed.id).unwrap().unwrap();
        assert_eq!(session.status, SessionStatus::Completed);
        assert_eq!(
            session.completion_reason,
            Some(CompletionReason::TurnComplete)
        );
    }

    /// Another instance's live session survives our quit. The snapshot already
    /// filters it out; this asserts the CAS does too, which is what covers a
    /// claim landing *after* the snapshot — the interleaving the sweep can't
    /// otherwise see.
    #[test]
    fn sweep_leaves_another_instances_running_session_alone() {
        let store = Store::in_memory().unwrap();

        let mut theirs = Session::new_running("theirs", Path::new("/tmp"));
        theirs.owner_pid = Some(std::process::id().wrapping_add(1));
        store.create_session(&theirs).unwrap();

        assert_eq!(sweep_sessions(&store, &HashSet::new()), 0);
        assert!(!store
            .transition_from_owned_active(
                &theirs.id,
                SessionStatus::Cancelled,
                None,
                Some(&CompletionReason::AppQuit),
                std::process::id(),
            )
            .unwrap());

        let session = store.get_session(&theirs.id).unwrap().unwrap();
        assert_eq!(session.status, SessionStatus::Running);
        assert_eq!(session.completion_reason, None);
    }
}
