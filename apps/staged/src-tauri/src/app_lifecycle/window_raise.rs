//! Bringing a window back on screen, in two steps a caller can take apart:
//! [`pick_window_to_show`] chooses the window and [`raise_window`] reveals it.
//!
//! [`show_a_window`] is the two run back to back, for callers that only want
//! *a* window on screen. A caller that has something to say to the window it
//! lands on picks first, so it knows which window that is before raising it.

use std::collections::HashMap;

use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

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
/// Prefers where the user already is (focused, then visible), then falls back to
/// one that needs unhiding: `main` for its restored geometry, else any surviving
/// `win-N` peer.
pub(crate) fn pick_window_to_show<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    let windows = app.webview_windows();
    pick_window(
        &windows,
        |window| window.is_focused().unwrap_or(false),
        |window| window.is_visible().unwrap_or(false),
    )
    .cloned()
}

/// [`pick_window_to_show`]'s order, with the focus and visibility probes passed
/// in: tauri's mock runtime reports every window as visible and none as
/// focused, so the tests supply their own to reach each tier.
///
/// Each tier is probed only when the ones above it found nothing.
fn pick_window<W>(
    windows: &HashMap<String, W>,
    is_focused: impl Fn(&W) -> bool,
    is_visible: impl Fn(&W) -> bool,
) -> Option<&W> {
    windows
        .values()
        .find(|window| is_focused(window))
        .or_else(|| windows.values().find(|window| is_visible(window)))
        .or_else(|| windows.get(MAIN_WINDOW_LABEL))
        .or_else(|| windows.values().next())
}

/// Show, unminimize and focus `window`, and put its PR-poll client back on the
/// focused tier.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};

    /// A mock app with one window per label.
    fn mock_app(labels: &[&str]) -> tauri::App<MockRuntime> {
        let app = mock_builder()
            .build(mock_context(noop_assets()))
            .expect("failed to build mock app");
        for label in labels {
            tauri::WebviewWindowBuilder::new(&app, *label, Default::default())
                .build()
                .expect("failed to create mock window");
        }
        app
    }

    /// Label [`pick_window`] lands on when exactly the `focused` and `visible`
    /// windows report themselves as such.
    fn picked(app: &tauri::App<MockRuntime>, focused: &[&str], visible: &[&str]) -> Option<String> {
        let windows = app.webview_windows();
        pick_window(
            &windows,
            |window| focused.contains(&window.label()),
            |window| visible.contains(&window.label()),
        )
        .map(|window| window.label().to_string())
    }

    #[test]
    fn the_focused_window_wins_over_visible_ones_and_main() {
        let app = mock_app(&["main", "win-2", "win-3"]);

        assert_eq!(
            picked(&app, &["win-3"], &["main", "win-2", "win-3"]).as_deref(),
            Some("win-3")
        );
    }

    #[test]
    fn a_visible_window_wins_over_a_hidden_main() {
        let app = mock_app(&["main", "win-2", "win-3"]);

        assert_eq!(picked(&app, &[], &["win-2"]).as_deref(), Some("win-2"));
    }

    #[test]
    fn main_wins_when_every_window_is_hidden() {
        let app = mock_app(&["win-2", "main", "win-3"]);

        assert_eq!(picked(&app, &[], &[]).as_deref(), Some("main"));
    }

    #[test]
    fn a_hidden_peer_is_picked_once_main_is_gone() {
        let app = mock_app(&["win-2"]);

        assert_eq!(picked(&app, &[], &[]).as_deref(), Some("win-2"));
    }

    #[test]
    fn nothing_is_picked_with_no_windows() {
        let app = mock_app(&[]);

        assert_eq!(picked(&app, &[], &[]), None);
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

    /// Smoke test for the raise half on the mock runtime. Raising needs no
    /// managed state: with no PR-poll scheduler on the app there is simply no
    /// focus hint to update.
    #[test]
    fn raising_a_window_needs_no_scheduler() {
        let app = mock_app(&["main"]);
        let window = pick_window_to_show(app.handle()).expect("expected a window");

        raise_window(&window);
    }
}
