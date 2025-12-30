//! Refresh controller that orchestrates file watching and change notifications.
//!
//! This module ties together the watcher and event emission, handling:
//! - Throttling (don't notify too frequently)
//!
//! All policy decisions live here, making them easy to modify or remove.

use crate::watcher::{NotifyWatcher, WatcherManager};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// Event name for file change notifications sent to frontend.
/// Payload is empty - frontend decides what to refresh.
pub const EVENT_FILES_CHANGED: &str = "files-changed";

/// Minimum interval between notifications (1 second)
const MIN_THROTTLE_INTERVAL_MS: u64 = 1000;

/// State shared between the watcher callback and the controller
struct RefreshState {
    last_notify: Instant,
    repo_path: Option<PathBuf>,
}

impl Default for RefreshState {
    fn default() -> Self {
        Self {
            last_notify: Instant::now() - Duration::from_secs(10), // Allow immediate first notify
            repo_path: None,
        }
    }
}

/// Orchestrates file watching and change event emission.
pub struct RefreshController {
    watcher: Mutex<NotifyWatcher>,
    state: Arc<Mutex<RefreshState>>,
    app_handle: AppHandle,
}

impl RefreshController {
    /// Create a new refresh controller.
    pub fn new(app_handle: AppHandle) -> Self {
        Self {
            watcher: Mutex::new(NotifyWatcher::new()),
            state: Arc::new(Mutex::new(RefreshState::default())),
            app_handle,
        }
    }

    /// Start watching a repository for changes.
    /// Stops any existing watcher first.
    pub fn start(&self, repo_path: PathBuf) -> Result<(), String> {
        // Reset state for new repo
        {
            let mut state = self.state.lock().unwrap();
            *state = RefreshState::default();
            state.repo_path = Some(repo_path.clone());
        }

        // Set up the callback that will be called on FS changes
        let state = Arc::clone(&self.state);
        let app_handle = self.app_handle.clone();

        let on_change = Box::new(move || {
            Self::handle_change(&state, &app_handle);
        });

        // Start the watcher
        let mut watcher = self.watcher.lock().unwrap();
        watcher
            .start(&repo_path, on_change)
            .map_err(|e| e.message)?;

        // Do an initial notification immediately
        Self::handle_change(&self.state, &self.app_handle);

        Ok(())
    }

    /// Stop watching the current repository.
    pub fn stop(&self) {
        let mut watcher = self.watcher.lock().unwrap();
        watcher.stop();

        let mut state = self.state.lock().unwrap();
        state.repo_path = None;
    }

    /// Handle a file system change event.
    /// This is called by the watcher when relevant files change.
    fn handle_change(state: &Arc<Mutex<RefreshState>>, app_handle: &AppHandle) {
        // Check throttle
        {
            let state = state.lock().unwrap();
            if state.repo_path.is_none() {
                return; // No repo to watch
            }

            let throttle_interval = Duration::from_millis(MIN_THROTTLE_INTERVAL_MS);
            if state.last_notify.elapsed() < throttle_interval {
                log::debug!(
                    "Throttled: {}ms since last notify, need {}ms",
                    state.last_notify.elapsed().as_millis(),
                    throttle_interval.as_millis()
                );
                return;
            }
        }

        // Update state
        {
            let mut state = state.lock().unwrap();
            state.last_notify = Instant::now();
        }

        // Emit change notification to frontend (empty payload)
        if let Err(e) = app_handle.emit(EVENT_FILES_CHANGED, ()) {
            log::error!("Failed to emit files-changed event: {}", e);
        }
    }
}
