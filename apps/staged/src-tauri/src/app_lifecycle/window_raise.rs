//! Bringing a window back on screen, in two steps a caller can take apart:
//! [`pick_window_to_show`] chooses the window and [`raise_window`] reveals it.
//!
//! [`show_a_window`] is the two run back to back, for callers that only want
//! *a* window on screen. A caller that has something to say to the window it
//! lands on picks first, so it knows which window that is before raising it.
//! One that may arrive while the window is still loading raises with
//! [`raise_window_once_revealed`](super::window_reveal::raise_window_once_revealed),
//! which leaves the first reveal to the window's own frontend.

use std::collections::HashMap;

use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

use super::window_reveal::note_window_shown;
use super::window_use::{note_window_used, UsedWindows};
use super::{set_native_focus, MAIN_WINDOW_LABEL};

/// Bring a window back on screen: the Dock-icon click and `Window ▸ Staged`
/// both funnel here. Deliberately *not* on the quit path — see the
/// [`app_lifecycle`](super) module docs.
///
/// Finds nothing only if every window has been destroyed, which no close path
/// produces — closing the last window hides it instead.
pub fn show_a_window(app: &AppHandle) {
    let Some(window) = pick_window_to_show(app) else {
        log::warn!("No window left to show");
        return;
    };
    raise_window(&window);
}

/// The window a reveal should land on, or `None` when every window has been
/// destroyed.
///
/// Prefers where the user already is: the focused window, then the one they
/// used most recently that is still open (see [`UsedWindows`]). Staged is
/// rarely frontmost when a request arrives from a terminal, so that second
/// tier is the usual one for `stagedctl project open`. With no window used
/// yet it falls back to a visible one, then to one that needs unhiding.
/// Those two tiers go in a fixed order: `main` first, for its restored
/// geometry, then the `win-N` peers, oldest first.
pub(crate) fn pick_window_to_show<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    let windows = app.webview_windows();
    let recently_used = app
        .try_state::<UsedWindows>()
        .map(|used| used.most_recent_first())
        .unwrap_or_default();
    pick_window(
        &windows,
        &recently_used,
        |window| window.is_focused().unwrap_or(false),
        |window| window.is_visible().unwrap_or(false),
    )
    .cloned()
}

/// [`pick_window_to_show`]'s order, with the focus and visibility probes passed
/// in: tauri's mock runtime reports every window as visible and none as
/// focused, so the tests supply their own to reach each tier.
///
/// `recently_used` is most recent first, and may name windows that are gone.
/// Each tier is probed only when the ones above it found nothing.
fn pick_window<'a, W>(
    windows: &'a HashMap<String, W>,
    recently_used: &[String],
    is_focused: impl Fn(&W) -> bool,
    is_visible: impl Fn(&W) -> bool,
) -> Option<&'a W> {
    // `webview_windows()` is a fresh `HashMap` per call, so its own iteration
    // order differs from one call to the next.
    let mut labels: Vec<&String> = windows.keys().collect();
    labels.sort_by(|a, b| fixed_rank(a).cmp(&fixed_rank(b)));
    let in_fixed_order = || labels.iter().map(|label| &windows[*label]);

    in_fixed_order()
        .find(|window| is_focused(window))
        .or_else(|| recently_used.iter().find_map(|label| windows.get(label)))
        .or_else(|| in_fixed_order().find(|window| is_visible(window)))
        .or_else(|| in_fixed_order().next())
}

/// Sort key for the tiers that have no better order: `main`, then `win-N` by
/// `N` (so `win-10` follows `win-9`), then anything else by name.
fn fixed_rank(label: &str) -> (u8, usize, &str) {
    if label == MAIN_WINDOW_LABEL {
        return (0, 0, label);
    }
    match label
        .strip_prefix("win-")
        .and_then(|index| index.parse().ok())
    {
        Some(index) => (1, index, label),
        None => (2, 0, label),
    }
}

/// Show, unminimize and focus `window`, put its PR-poll client back on the
/// focused tier, and record it as revealed and as the window used most
/// recently.
///
/// Best effort: each step that fails is logged and the rest still run, so a
/// window that can't take focus is at least left on screen.
pub(crate) fn raise_window<R: Runtime>(window: &WebviewWindow<R>) {
    if let Err(e) = window.show() {
        log::warn!("Failed to show window: {e}");
    }
    if let Err(e) = window.unminimize() {
        log::warn!("Failed to unminimize window: {e}");
    }
    if let Err(e) = window.set_focus() {
        log::warn!("Failed to focus window: {e}");
    }
    set_native_focus(window.app_handle(), window.label(), true);
    // Shown by the backend, so there is no reveal left to wait for, whether
    // or not its frontend has reported one.
    note_window_shown(window.app_handle(), window.label());
    // Recorded here as well as on the focus event, which does not come when
    // the window could not take focus.
    note_window_used(window.app_handle(), window.label());
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};

    /// A mock app with one window per label and an empty record of used
    /// windows.
    fn mock_app(labels: &[&str]) -> tauri::App<MockRuntime> {
        let app = mock_builder()
            .build(mock_context(noop_assets()))
            .expect("failed to build mock app");
        app.manage(UsedWindows::default());
        for label in labels {
            tauri::WebviewWindowBuilder::new(&app, *label, Default::default())
                .build()
                .expect("failed to create mock window");
        }
        app
    }

    /// Label [`pick_window`] lands on when exactly the `focused` and `visible`
    /// windows report themselves as such, and `used` (most recent first) is
    /// the record of used windows.
    fn picked(
        app: &tauri::App<MockRuntime>,
        focused: &[&str],
        used: &[&str],
        visible: &[&str],
    ) -> Option<String> {
        let windows = app.webview_windows();
        let used: Vec<String> = used.iter().map(|label| label.to_string()).collect();
        pick_window(
            &windows,
            &used,
            |window| focused.contains(&window.label()),
            |window| visible.contains(&window.label()),
        )
        .map(|window| window.label().to_string())
    }

    /// The app's record of used windows, most recent first.
    fn used(app: &tauri::App<MockRuntime>) -> Vec<String> {
        app.state::<UsedWindows>().most_recent_first()
    }

    #[test]
    fn the_focused_window_wins_over_the_last_used_and_visible_ones() {
        let app = mock_app(&["main", "win-2", "win-3"]);

        assert_eq!(
            picked(&app, &["win-3"], &["win-2"], &["main", "win-2", "win-3"]).as_deref(),
            Some("win-3")
        );
    }

    /// The state `stagedctl project open` arrives in: a terminal is frontmost,
    /// so several windows are visible and none is focused.
    #[test]
    fn the_last_used_window_wins_when_none_is_focused() {
        let app = mock_app(&["main", "win-2", "win-3"]);

        assert_eq!(
            picked(&app, &[], &["win-2", "main"], &["main", "win-2", "win-3"]).as_deref(),
            Some("win-2")
        );
    }

    #[test]
    fn the_window_used_before_it_wins_once_the_last_used_is_gone() {
        let app = mock_app(&["main", "win-2", "win-3"]);

        assert_eq!(
            picked(
                &app,
                &[],
                &["win-9", "win-3", "main"],
                &["main", "win-2", "win-3"]
            )
            .as_deref(),
            Some("win-3")
        );
    }

    /// Where the user was beats what happens to be showing: every window of
    /// an app hidden with `Cmd+H` reports itself as not visible.
    #[test]
    fn the_last_used_window_wins_even_when_no_window_is_visible() {
        let app = mock_app(&["main", "win-2"]);

        assert_eq!(picked(&app, &[], &["win-2"], &[]).as_deref(), Some("win-2"));
    }

    /// With no window used, the pick among visible ones is still the same one
    /// every time: `main`, then the `win-N` peers by number, not by name.
    #[test]
    fn visible_windows_are_tried_in_a_fixed_order() {
        let app = mock_app(&["win-10", "win-3", "main", "win-2"]);

        for _ in 0..20 {
            assert_eq!(
                picked(&app, &[], &[], &["main", "win-2", "win-3", "win-10"]).as_deref(),
                Some("main")
            );
            assert_eq!(
                picked(&app, &[], &[], &["win-2", "win-3", "win-10"]).as_deref(),
                Some("win-2")
            );
            assert_eq!(
                picked(&app, &[], &[], &["win-3", "win-10"]).as_deref(),
                Some("win-3")
            );
        }
    }

    #[test]
    fn a_visible_window_wins_over_a_hidden_main() {
        let app = mock_app(&["main", "win-2", "win-3"]);

        assert_eq!(picked(&app, &[], &[], &["win-2"]).as_deref(), Some("win-2"));
    }

    #[test]
    fn main_wins_when_every_window_is_hidden() {
        let app = mock_app(&["win-2", "main", "win-3"]);

        assert_eq!(picked(&app, &[], &[], &[]).as_deref(), Some("main"));
    }

    #[test]
    fn the_oldest_hidden_peer_is_picked_once_main_is_gone() {
        let app = mock_app(&["win-10", "win-3", "win-2"]);

        for _ in 0..20 {
            assert_eq!(picked(&app, &[], &[], &[]).as_deref(), Some("win-2"));
        }
    }

    #[test]
    fn nothing_is_picked_with_no_windows() {
        let app = mock_app(&[]);

        assert_eq!(picked(&app, &[], &["main"], &[]), None);
        assert!(pick_window_to_show(app.handle()).is_none());
    }

    /// The probes are wired to the real window state: the mock runtime reports
    /// `win-2` as visible and unfocused, so it is found on the visible tier
    /// rather than by falling through to "any".
    #[test]
    fn the_app_level_pick_reads_the_windows_own_state() {
        let app = mock_app(&["win-2"]);

        let window = pick_window_to_show(app.handle()).expect("expected a window");

        assert_eq!(window.label(), "win-2");
        assert!(window.is_visible().unwrap());
        assert!(!window.is_focused().unwrap());
    }

    /// The record is wired in too: every mock window is visible and `main`
    /// leads the fixed order, so only the record can land the pick on `win-3`.
    #[test]
    fn the_app_level_pick_reads_the_record_of_used_windows() {
        let app = mock_app(&["main", "win-2", "win-3"]);
        let record = app.state::<UsedWindows>();
        record.note("win-2");
        record.note("win-3");

        let window = pick_window_to_show(app.handle()).expect("expected a window");
        assert_eq!(window.label(), "win-3");

        // A destroyed window comes off the record, and the one before it wins.
        record.forget("win-3");
        let window = pick_window_to_show(app.handle()).expect("expected a window");
        assert_eq!(window.label(), "win-2");
    }

    /// Smoke test for the raise half on the mock runtime. Raising needs no
    /// managed state: with no PR-poll scheduler on the app there is simply no
    /// focus hint to update, and with no record nothing is noted.
    #[test]
    fn raising_a_window_needs_no_managed_state() {
        let app = mock_builder()
            .build(mock_context(noop_assets()))
            .expect("failed to build mock app");
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("failed to create mock window");

        raise_window(&window);
    }

    #[test]
    fn a_raised_window_becomes_the_last_used() {
        let app = mock_app(&["main", "win-2"]);
        app.state::<UsedWindows>().note("win-2");

        raise_window(&app.get_webview_window("main").unwrap());

        assert_eq!(used(&app), ["main", "win-2"]);
    }
}
