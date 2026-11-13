//! Which windows the user has used, and in what order.
//!
//! Nothing in tauri reports "the window the user was last in" once Staged is
//! not the frontmost app, and nothing reports a window going on screen. Both
//! are read off this record instead:
//!
//! - [`pick_window_to_show`](super::window_raise::pick_window_to_show) lands
//!   on the most recently used window when none holds focus, which is the
//!   normal state for a request that comes from a terminal.
//! - [`raise_window_once_revealed`](super::window_raise::raise_window_once_revealed)
//!   takes a window that is on the record as one its frontend has already
//!   revealed. Every window is created hidden and shown by its own frontend.
//!   It cannot be focused or closed before that, and one the backend raises
//!   is on screen from then on.
//!
//! A window goes on the record when it gains focus, when the last window is
//! closed to hide, and when the backend raises it. It comes off when it is
//! destroyed.

use std::sync::Mutex;

use tauri::{AppHandle, Manager, Runtime};

/// Labels of the windows the user has used, least recent first. Managed as
/// Tauri state, in memory only: window labels do not outlive the process.
#[derive(Default)]
pub struct UsedWindows {
    order: Mutex<Vec<String>>,
}

impl UsedWindows {
    /// Record `label` as the window used most recently.
    pub(crate) fn note(&self, label: &str) {
        let mut order = self.order.lock().unwrap();
        order.retain(|known| known != label);
        order.push(label.to_string());
    }

    /// Drop a destroyed window from the record.
    pub(crate) fn forget(&self, label: &str) {
        self.order.lock().unwrap().retain(|known| known != label);
    }

    /// Whether `label` has been used, and so has been on screen.
    pub(crate) fn contains(&self, label: &str) -> bool {
        self.order
            .lock()
            .unwrap()
            .iter()
            .any(|known| known == label)
    }

    /// The used windows' labels, most recent first.
    pub(crate) fn most_recent_first(&self) -> Vec<String> {
        self.order.lock().unwrap().iter().rev().cloned().collect()
    }
}

/// [`UsedWindows::note`] on the app's record. A no-op on an app that manages
/// none, as the mock apps in most tests do not.
pub(crate) fn note_window_used<R: Runtime>(app: &AppHandle<R>, label: &str) {
    if let Some(used) = app.try_state::<UsedWindows>() {
        used.note(label);
    }
}

/// [`UsedWindows::forget`] on the app's record.
pub(crate) fn forget_window<R: Runtime>(app: &AppHandle<R>, label: &str) {
    if let Some(used) = app.try_state::<UsedWindows>() {
        used.forget(label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_noted_last_comes_first() {
        let used = UsedWindows::default();
        used.note("main");
        used.note("win-2");
        used.note("win-3");

        assert_eq!(used.most_recent_first(), ["win-3", "win-2", "main"]);
    }

    /// Going back to a window moves it to the front rather than listing it
    /// twice.
    #[test]
    fn noting_a_window_again_moves_it_to_the_front() {
        let used = UsedWindows::default();
        used.note("main");
        used.note("win-2");
        used.note("main");

        assert_eq!(used.most_recent_first(), ["main", "win-2"]);
    }

    #[test]
    fn a_forgotten_window_is_off_the_record() {
        let used = UsedWindows::default();
        used.note("main");
        used.note("win-2");

        used.forget("win-2");

        assert!(used.contains("main"));
        assert!(!used.contains("win-2"));
        assert_eq!(used.most_recent_first(), ["main"]);
    }

    #[test]
    fn a_window_never_noted_is_not_on_the_record() {
        let used = UsedWindows::default();

        assert!(!used.contains("main"));
        assert!(used.most_recent_first().is_empty());
        // Forgetting it is harmless.
        used.forget("main");
    }
}
