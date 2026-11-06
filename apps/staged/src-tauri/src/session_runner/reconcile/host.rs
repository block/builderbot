//! What reconcile asks of the app around a settled row — events, completion
//! side effects, the branch queue — behind [`ReconcileHost`], so tests can
//! record it. [`LiveHost`] is the Tauri implementation.

use std::sync::{Arc, Mutex};

use super::SessionReconcileEvent;
use crate::session_runner::{SessionRegistry, SessionStatusEvent};
use crate::store::Store;

pub(crate) trait ReconcileHost: Send + Sync {
    /// Emit `session-status-changed`.
    fn emit_status(&self, event: SessionStatusEvent);

    /// Emit `session-reconcile-changed`.
    fn emit_reconcile(&self, event: &SessionReconcileEvent);

    /// The pr/push outcome effects for a row this process just settled, as
    /// every runner runs them before its terminal status event.
    fn completion_side_effects(&self, session_id: &str, branch_id: Option<&str>, status: &str);

    /// After the terminal status event: start the session's queued follow-up
    /// when `drain_message`, and kick the branch queue, which the row held.
    fn drain_and_kick(&self, session_id: &str, branch_id: Option<&str>, drain_message: bool);

    /// Settle what waits on a session that was cancelled without a runner's
    /// drain (see `pipeline_dependency::skip_dependents_of_cancelled`).
    fn skip_dependents_of_cancelled(&self, session_id: &str);

    /// Stop `session_id` the way a user's Stop does. For a Stop that landed
    /// on an attempt's registry entry after the row had gone to someone
    /// else, so the Stop reaches whoever has it now.
    fn restop(&self, session_id: &str);

    fn is_quitting(&self) -> bool;
}

/// [`ReconcileHost`] over the app.
pub(crate) struct LiveHost {
    app: tauri::AppHandle,
    store: Arc<Store>,
    registry: Arc<SessionRegistry>,
}

impl LiveHost {
    pub(crate) fn new(
        app: tauri::AppHandle,
        store: Arc<Store>,
        registry: Arc<SessionRegistry>,
    ) -> Self {
        Self {
            app,
            store,
            registry,
        }
    }
}

impl ReconcileHost for LiveHost {
    fn emit_status(&self, event: SessionStatusEvent) {
        crate::web_server::emit_to_all(&self.app, "session-status-changed", &event);
    }

    fn emit_reconcile(&self, event: &SessionReconcileEvent) {
        crate::web_server::emit_to_all(&self.app, "session-reconcile-changed", event);
    }

    fn completion_side_effects(&self, session_id: &str, branch_id: Option<&str>, status: &str) {
        crate::session_completion::run_completion_side_effects(
            &self.store,
            &self.app,
            session_id,
            branch_id,
            status,
        );
    }

    fn drain_and_kick(&self, session_id: &str, branch_id: Option<&str>, drain_message: bool) {
        crate::session_runner::drain_queued_after_terminal_state(
            Arc::clone(&self.store),
            Arc::clone(&self.registry),
            self.app.clone(),
            session_id.to_string(),
            branch_id.map(str::to_string),
            drain_message,
            crate::session_queue::KickSource::Reconciled,
        );
    }

    fn skip_dependents_of_cancelled(&self, session_id: &str) {
        if let Err(e) = crate::pipeline_dependency::skip_dependents_of_cancelled(
            &self.store,
            &self.app,
            session_id,
        ) {
            log::warn!("Failed to skip sessions waiting on cancelled {session_id}: {e}");
        }
    }

    fn restop(&self, session_id: &str) {
        use tauri::Manager;
        let Some(slot) = self.app.try_state::<Mutex<Option<Arc<Store>>>>() else {
            return;
        };
        if let Err(e) = crate::session_commands::cancel_session_impl(
            &self.registry,
            &slot,
            &self.app,
            session_id,
        ) {
            log::warn!("Failed to pass a Stop on to session {session_id}: {e}");
        }
    }

    fn is_quitting(&self) -> bool {
        crate::app_lifecycle::is_quitting(&self.app)
    }
}
