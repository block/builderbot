//! Which windows have been revealed, and which are owed a raise once they are.
//!
//! Every window is created hidden. Its frontend shows it at the end of mount,
//! once the theme is applied, and then says so with [`note_window_revealed`].
//! Showing it sooner flashes an unpainted webview, so a caller that may be
//! ahead of the frontend raises with [`raise_window_once_revealed`]:
//!
//! - A window on the record is raised on the spot, whatever it looks like
//!   now: hidden, minimized, never focused, or in an app that is hidden.
//! - Any other window is still loading. It is left alone, and the raise it is
//!   owed is made when its frontend reports the reveal. Nothing polls for it.
//!
//! The record is told, not inferred. Whether a window is visible now, or has
//! held focus, does not say whether its frontend has revealed it: a window
//! revealed while Staged was in the background and hidden since is neither,
//! and would be waited on for a reveal that already happened.
//!
//! A window goes on the record when its frontend reports the reveal, and when
//! the backend shows it itself ([`raise_window`]): from then on there is no
//! first paint left to wait for. It comes off when it is destroyed.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

use super::window_raise::raise_window;

/// How long a raise stays owed to a window that was still loading. Far longer
/// than a cold start takes to reach its reveal. A reveal that comes later
/// than this raises nothing: whoever asked has long since moved on, and
/// pulling Staged in front of them then would be a surprise.
pub(crate) const REVEAL_WAIT: Duration = Duration::from_secs(60);

/// Labels of the windows that have been revealed, and of those owed a raise
/// once they are. Managed as Tauri state, in memory only: window labels do
/// not outlive the process.
#[derive(Default)]
pub struct RevealedWindows {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    revealed: HashSet<String>,
    /// Windows owed a raise once revealed, each with the moment the raise
    /// stops being wanted.
    owed: HashMap<String, Instant>,
}

impl RevealedWindows {
    /// Record `label` as revealed. Returns whether it was owed a raise that
    /// is still wanted at `now`; either way nothing is owed to it any more.
    pub(crate) fn note(&self, label: &str, now: Instant) -> bool {
        let mut inner = self.inner.lock().unwrap();
        inner.revealed.insert(label.to_string());
        inner.owed.remove(label).is_some_and(|until| now < until)
    }

    /// Whether `label` has been revealed. When it has not, it is owed a raise
    /// if the reveal comes before `until`.
    ///
    /// One step under one lock, so a reveal reported at the same moment is
    /// either seen here or finds the raise owed.
    pub(crate) fn contains_or_owe(&self, label: &str, until: Instant) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.revealed.contains(label) {
            return true;
        }
        inner.owed.insert(label.to_string(), until);
        false
    }

    /// Drop a destroyed window from the record, with any raise it was owed.
    pub(crate) fn forget(&self, label: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.revealed.remove(label);
        inner.owed.remove(label);
    }

    /// Whether `label` has been revealed.
    #[cfg(test)]
    pub(crate) fn contains(&self, label: &str) -> bool {
        self.inner.lock().unwrap().revealed.contains(label)
    }
}

/// The calling window's frontend has shown it: the theme is applied and the
/// window is on screen. Sent once per page load, right after the frontend's
/// own `show()`.
///
/// Records the window as revealed and makes the raise it is owed, if a
/// `stagedctl project open` arrived while it was loading. Acts on the calling
/// window only. Desktop only: `web_server::dispatch` refuses it, since a
/// browser client has no native window, and it stays out of
/// `control_server::ALLOWED_COMMANDS`, so no stagedctl caller can have a
/// loading window shown early.
///
/// Generic over the runtime so the tests below can call it with windows from
/// tauri's mock runtime; `generate_handler!` instantiates it with `Wry`.
#[tauri::command]
pub fn note_window_revealed<R: Runtime>(window: WebviewWindow<R>) {
    window_revealed_at(&window, Instant::now());
}

/// [`note_window_revealed`] with the clock passed in.
fn window_revealed_at<R: Runtime>(window: &WebviewWindow<R>, now: Instant) {
    let owed = window
        .app_handle()
        .try_state::<RevealedWindows>()
        .is_some_and(|revealed| revealed.note(window.label(), now));
    if owed {
        // The frontend's `show()` put the window on screen. It did not bring
        // Staged forward if the user had gone back to another app meanwhile.
        raise_window(window);
    }
}

/// [`raise_window`] for a caller that may be ahead of the window's frontend.
///
/// A window that has been revealed is raised now. One that is still loading
/// is left alone and raised when its frontend reports the reveal
/// ([`note_window_revealed`]), so the frontend's own reveal is the first and
/// the window still ends up in front and focused. That raise is dropped if
/// the window is destroyed first, or is revealed more than [`REVEAL_WAIT`]
/// from now.
///
/// On an app that manages no record there is nothing to defer against, and
/// the window is raised now.
pub(crate) fn raise_window_once_revealed<R: Runtime>(window: &WebviewWindow<R>) {
    raise_window_once_revealed_at(window, Instant::now());
}

/// [`raise_window_once_revealed`] with the clock passed in.
fn raise_window_once_revealed_at<R: Runtime>(window: &WebviewWindow<R>, now: Instant) {
    let revealed = match window.app_handle().try_state::<RevealedWindows>() {
        Some(revealed) => revealed.contains_or_owe(window.label(), now + REVEAL_WAIT),
        None => true,
    };
    if revealed {
        raise_window(window);
    }
}

/// Record a window the backend has shown itself as revealed. A raise that was
/// owed to it has just been made, so nothing is owed to it any more. A no-op
/// on an app that manages no record, as the mock apps in most tests do not.
pub(crate) fn note_window_shown<R: Runtime>(app: &AppHandle<R>, label: &str) {
    if let Some(revealed) = app.try_state::<RevealedWindows>() {
        revealed.note(label, Instant::now());
    }
}

/// [`RevealedWindows::forget`] on the app's record.
pub(crate) fn forget_window<R: Runtime>(app: &AppHandle<R>, label: &str) {
    if let Some(revealed) = app.try_state::<RevealedWindows>() {
        revealed.forget(label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_lifecycle::window_use::UsedWindows;
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};

    /// A mock app with one window per label, none of them revealed or used.
    ///
    /// The mock runtime calls every window visible, so a test here that
    /// leaves a window unraised also shows that visibility is not consulted.
    fn mock_app(labels: &[&str]) -> tauri::App<MockRuntime> {
        let app = mock_builder()
            .build(mock_context(noop_assets()))
            .expect("failed to build mock app");
        app.manage(RevealedWindows::default());
        app.manage(UsedWindows::default());
        for label in labels {
            tauri::WebviewWindowBuilder::new(&app, *label, Default::default())
                .build()
                .expect("failed to create mock window");
        }
        app
    }

    fn window(app: &tauri::App<MockRuntime>, label: &str) -> WebviewWindow<MockRuntime> {
        app.get_webview_window(label).expect("window should exist")
    }

    /// The windows raised so far, most recent first: a raise puts its window
    /// at the head of the record of used windows, and nothing else here does.
    fn raised(app: &tauri::App<MockRuntime>) -> Vec<String> {
        app.state::<UsedWindows>().most_recent_first()
    }

    #[test]
    fn a_window_is_revealed_only_once_noted() {
        let record = RevealedWindows::default();
        assert!(!record.contains("main"));

        record.note("main", Instant::now());

        assert!(record.contains("main"));
        assert!(!record.contains("win-2"));
    }

    #[test]
    fn a_reveal_with_nothing_owed_asks_for_no_raise() {
        let record = RevealedWindows::default();

        assert!(!record.note("main", Instant::now()));
    }

    #[test]
    fn a_reveal_in_time_asks_for_the_raise_it_was_owed_once() {
        let record = RevealedWindows::default();
        let now = Instant::now();

        assert!(!record.contains_or_owe("main", now + REVEAL_WAIT));
        assert!(record.note("main", now + Duration::from_secs(5)));
        assert!(
            !record.note("main", now + Duration::from_secs(6)),
            "the raise is owed once, not on every reveal"
        );
    }

    #[test]
    fn a_reveal_that_comes_too_late_asks_for_no_raise() {
        let record = RevealedWindows::default();
        let now = Instant::now();

        assert!(!record.contains_or_owe("main", now + REVEAL_WAIT));
        assert!(!record.note("main", now + REVEAL_WAIT));
        assert!(record.contains("main"), "late, but revealed all the same");
    }

    /// A second request for a window still loading moves the deadline rather
    /// than owing two raises.
    #[test]
    fn a_newer_request_extends_the_wait() {
        let record = RevealedWindows::default();
        let now = Instant::now();
        let later = now + Duration::from_secs(30);

        assert!(!record.contains_or_owe("main", now + REVEAL_WAIT));
        assert!(!record.contains_or_owe("main", later + REVEAL_WAIT));
        assert!(record.note("main", now + REVEAL_WAIT + Duration::from_secs(1)));
        assert!(!record.note("main", now + REVEAL_WAIT + Duration::from_secs(2)));
    }

    #[test]
    fn a_revealed_window_is_owed_nothing() {
        let record = RevealedWindows::default();
        let now = Instant::now();
        record.note("main", now);

        assert!(record.contains_or_owe("main", now + REVEAL_WAIT));
        assert!(!record.note("main", now), "nothing was left owed");
    }

    #[test]
    fn a_forgotten_window_is_off_the_record_and_owed_nothing() {
        let record = RevealedWindows::default();
        let now = Instant::now();
        record.note("main", now);
        assert!(!record.contains_or_owe("win-2", now + REVEAL_WAIT));

        record.forget("main");
        record.forget("win-2");
        // Forgetting a window never seen is harmless.
        record.forget("win-3");

        assert!(!record.contains("main"));
        assert!(!record.note("win-2", now), "the raise went with the window");
    }

    #[test]
    fn a_revealed_window_is_raised_at_once() {
        let app = mock_app(&["main"]);
        note_window_revealed(window(&app, "main"));
        assert!(raised(&app).is_empty(), "a reveal alone raises nothing");

        raise_window_once_revealed(&window(&app, "main"));

        assert_eq!(raised(&app), ["main"]);
    }

    /// The regression this record closes. A window revealed while Staged was
    /// in the background has never held focus, so it is not on the record of
    /// used windows, and once hidden it is not on screen either. Its frontend
    /// revealed it all the same, so it is raised now, not waited on for a
    /// reveal that will never come again.
    #[test]
    fn a_revealed_window_that_was_never_used_is_raised_at_once() {
        let app = mock_app(&["main", "win-2"]);
        note_window_revealed(window(&app, "win-2"));
        assert!(!app.state::<UsedWindows>().contains("win-2"));

        raise_window_once_revealed(&window(&app, "win-2"));

        assert_eq!(raised(&app), ["win-2"]);
    }

    /// The cold start: the window is hidden until its frontend shows it, and
    /// the backend must not get there first.
    #[test]
    fn a_window_still_loading_is_raised_only_once_its_frontend_reveals_it() {
        let app = mock_app(&["main"]);

        raise_window_once_revealed(&window(&app, "main"));
        assert!(raised(&app).is_empty(), "not raised while it is loading");

        note_window_revealed(window(&app, "main"));
        assert_eq!(raised(&app), ["main"], "raised as its frontend revealed it");
    }

    /// Having held focus is not a reveal. Nothing but the record decides.
    #[test]
    fn a_used_window_that_is_not_revealed_is_still_waited_on() {
        let app = mock_app(&["main", "win-2"]);
        let used = app.state::<UsedWindows>();
        used.note("main");
        used.note("win-2");

        raise_window_once_revealed(&window(&app, "main"));
        assert_eq!(raised(&app), ["win-2", "main"], "main was not raised");

        note_window_revealed(window(&app, "main"));
        assert_eq!(raised(&app), ["main", "win-2"]);
    }

    #[test]
    fn a_window_revealed_too_late_is_not_raised() {
        let app = mock_app(&["main"]);
        let now = Instant::now();

        raise_window_once_revealed_at(&window(&app, "main"), now);
        window_revealed_at(&window(&app, "main"), now + REVEAL_WAIT);

        assert!(raised(&app).is_empty());
        // It is revealed, though, so the next request raises it at once.
        raise_window_once_revealed(&window(&app, "main"));
        assert_eq!(raised(&app), ["main"]);
    }

    #[test]
    fn a_window_revealed_just_in_time_is_raised() {
        let app = mock_app(&["main"]);
        let now = Instant::now();

        raise_window_once_revealed_at(&window(&app, "main"), now);
        window_revealed_at(
            &window(&app, "main"),
            now + REVEAL_WAIT - Duration::from_millis(1),
        );

        assert_eq!(raised(&app), ["main"]);
    }

    /// The raise is owed to one window. Another window's reveal does not make
    /// it, and is not raised itself.
    #[test]
    fn a_reveal_raises_only_its_own_window() {
        let app = mock_app(&["main", "win-2"]);
        raise_window_once_revealed(&window(&app, "main"));

        note_window_revealed(window(&app, "win-2"));
        assert!(raised(&app).is_empty());

        note_window_revealed(window(&app, "main"));
        assert_eq!(raised(&app), ["main"]);
    }

    /// A page reload runs the frontend's mount again, and with it the reveal.
    #[test]
    fn a_repeated_reveal_raises_nothing_more() {
        let app = mock_app(&["main", "win-2"]);
        raise_window_once_revealed(&window(&app, "main"));
        note_window_revealed(window(&app, "main"));
        raise_window(&window(&app, "win-2"));

        note_window_revealed(window(&app, "main"));

        assert_eq!(raised(&app), ["win-2", "main"]);
    }

    /// A Dock click raises a window without waiting for its frontend. There
    /// is then no first paint left to protect, and the raise owed has been
    /// made: the frontend's reveal must not pull the window forward again.
    #[test]
    fn a_window_the_backend_showed_is_revealed_and_owed_nothing() {
        let app = mock_app(&["main", "win-2"]);
        raise_window_once_revealed(&window(&app, "main"));

        raise_window(&window(&app, "main"));
        assert!(app.state::<RevealedWindows>().contains("main"));
        raise_window(&window(&app, "win-2"));

        note_window_revealed(window(&app, "main"));
        assert_eq!(raised(&app), ["win-2", "main"], "main was not raised twice");

        // And a later request raises it at once.
        raise_window_once_revealed(&window(&app, "main"));
        assert_eq!(raised(&app), ["main", "win-2"]);
    }

    #[test]
    fn a_destroyed_window_is_owed_nothing_and_is_no_longer_revealed() {
        let app = mock_app(&["main", "win-2"]);
        note_window_revealed(window(&app, "win-2"));
        raise_window_once_revealed(&window(&app, "main"));

        forget_window(app.handle(), "main");
        forget_window(app.handle(), "win-2");

        // The mock runtime keeps a destroyed window, so its reveal can still
        // be sent: nothing is owed to it now.
        note_window_revealed(window(&app, "main"));
        assert!(raised(&app).is_empty());
        assert!(!app.state::<RevealedWindows>().contains("win-2"));
    }

    /// Without a record there is nothing to defer against. Raising is the
    /// behaviour every other caller of [`raise_window`] has.
    #[test]
    fn with_no_record_the_window_is_raised_at_once() {
        let app = mock_builder()
            .build(mock_context(noop_assets()))
            .expect("failed to build mock app");
        app.manage(UsedWindows::default());
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("failed to create mock window");

        raise_window_once_revealed(&window);
        assert_eq!(raised(&app), ["main"]);

        // And the reveal has nowhere to go, which is harmless.
        note_window_revealed(window);
        forget_window(app.handle(), "main");
    }
}
