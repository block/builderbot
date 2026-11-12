//! Stops that reach a run after it has decided how it ends.
//!
//! A run reads its cancel token once to choose its terminal state, but its
//! registry entry stays in place until after the write that records it (the
//! stranded-session sweep needs it there). A Stop landing in that stretch is
//! answered `true` by the entry and writes nothing, trusting the run to
//! record it. Two points honour that trust:
//!
//! - **Before the write** ([`terminal_decision_after_late_stop`]): the run
//!   reads the token once more and the Stop replaces whatever it had decided.
//! - **After the write** ([`deregister_paused_run`]): for a terminal row there
//!   is nothing left to stop. A row paused for rate-limit recovery is not
//!   terminal — the scheduler restarts it — so the run reads the Stop out as
//!   it deregisters and ends the row.

use super::{RateLimitRecovery, RegistrationEpoch, SessionRegistry};
use crate::store::{CompletionReason, SessionStatus, Store};

/// What a finishing run records: the status, the error message, the
/// completion reason, and the rate-limit recovery details when the status is
/// `paused`. The write that follows pauses the row exactly when the recovery
/// details are present.
pub(super) type TerminalDecision = (
    &'static str,
    Option<String>,
    CompletionReason,
    Option<RateLimitRecovery>,
);

/// The decision a run records given `late_stop`: the reason of a Stop that
/// landed after the run read its token to decide, or `None` when there was
/// none.
///
/// A late Stop replaces the whole decision, the recovery details included.
/// Dropping only the status, error and reason left a rate-limited decision
/// half-replaced: the write still paused the row with its retry bookkeeping
/// while the event said `cancelled`, so the UI showed a stopped session the
/// scheduler restarted at the reset time.
pub(super) fn terminal_decision_after_late_stop(
    decided: TerminalDecision,
    late_stop: Option<CompletionReason>,
) -> TerminalDecision {
    match late_stop {
        Some(reason) => ("cancelled", None, reason, None),
        None => decided,
    }
}

/// Deregister a run whose terminal write paused its session for rate-limit
/// recovery, ending the row as `cancelled` if a Stop reached the run's entry
/// after it decided. Returns whether it did; the caller then skips its own
/// `paused` announcement and queue drain, which the cancel has replaced.
///
/// The read and the removal happen under one lock
/// ([`SessionRegistry::deregister_reporting_cancellation_of`]), so a Stop
/// either reached the entry first and is reported here, or finds no entry
/// and takes `cancel_session_impl`'s fallback, which handles a paused row
/// itself. Either way it is recorded once.
///
/// A quit's cancel is no Stop: the row stays `paused`, like every other
/// paused session a quit leaves for the scheduler to resume after relaunch.
///
/// The row is read back first because the shared cancel's write is not
/// guarded on the status. A manual Resume that won the paused row since the
/// pause write owns it now, and the Stop was aimed at the turn that just
/// ended (see [`SessionRegistry::mark_finished`]). The rate-limit scheduler
/// cannot have claimed it: the earliest retry is seconds out.
pub(super) fn deregister_paused_run<R: tauri::Runtime>(
    store: &Store,
    registry: &SessionRegistry,
    app_handle: &tauri::AppHandle<R>,
    session_id: &str,
    epoch: RegistrationEpoch,
) -> bool {
    let Some(reason) = registry
        .deregister_reporting_cancellation_of(session_id, epoch)
        .filter(|reason| *reason != CompletionReason::AppQuit)
    else {
        return false;
    };
    let still_paused = store
        .get_session(session_id)
        .ok()
        .flatten()
        .is_some_and(|session| {
            session.status == SessionStatus::Paused
                && session.completion_reason == Some(CompletionReason::RateLimited)
        });
    if !still_paused {
        return false;
    }
    log::info!("Session {session_id} was stopped as it paused for a rate limit; cancelling it");
    // The kick is this run's terminal-state drain: the caller skips its own.
    crate::session_commands::cancel_session_without_runner(
        store,
        app_handle,
        session_id,
        reason,
        crate::session_queue::KickSource::TerminalState,
    );
    true
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use tauri::Listener;

    use super::*;
    use crate::store::{Branch, PipelineExecution, PipelineKind, Project, Session};

    /// 2026-11-12T08:00:00Z, an hour before [`RESETS_AT`].
    const NOW_MS: i64 = 1_794_470_400_000;
    const RESETS_AT: &str = "2026-11-12T09:00:00Z";

    fn paused_decision() -> TerminalDecision {
        (
            "paused",
            Some("rate limit".to_string()),
            CompletionReason::RateLimited,
            Some(RateLimitRecovery {
                resets_at: Some(RESETS_AT.to_string()),
                kind: Some("five_hour".to_string()),
            }),
        )
    }

    /// The first half of the fix. The rebinding used to cover the status,
    /// error and reason only, so a Stop that landed after a rate-limit error
    /// settled left the recovery set and the write paused the row.
    #[test]
    fn a_late_stop_replaces_a_rate_limited_decision_and_drops_its_recovery() {
        for reason in [
            CompletionReason::Interrupted,
            CompletionReason::ProjectSessionInterrupted,
        ] {
            assert_eq!(
                terminal_decision_after_late_stop(paused_decision(), Some(reason.clone())),
                ("cancelled", None, reason, None),
            );
        }
        // Unchanged for the decisions it already covered.
        assert_eq!(
            terminal_decision_after_late_stop(
                ("completed", None, CompletionReason::TurnComplete, None),
                Some(CompletionReason::Interrupted),
            ),
            ("cancelled", None, CompletionReason::Interrupted, None),
        );
    }

    #[test]
    fn no_late_stop_leaves_the_decision_unchanged() {
        assert_eq!(
            terminal_decision_after_late_stop(paused_decision(), None),
            paused_decision(),
        );
        let failed: TerminalDecision = (
            "error",
            Some("boom".to_string()),
            CompletionReason::Crashed,
            None,
        );
        assert_eq!(
            terminal_decision_after_late_stop(failed.clone(), None),
            failed
        );
    }

    fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app")
    }

    /// A run at the point `finish_session_run` deregisters it after a pause:
    /// registered, marked finished, and its row paused by the real pause
    /// write, bookkeeping included. A push is queued behind the session.
    struct PausedRun {
        store: Arc<Store>,
        registry: SessionRegistry,
        session_id: String,
        epoch: RegistrationEpoch,
        dependent_push_id: String,
    }

    fn paused_run() -> PausedRun {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();
        let session = Session::new_running("rebase", Path::new("/tmp")).with_branch(&branch.id);
        store.create_session(&session).unwrap();
        let mut push = Session::new_queued("push").with_branch(&branch.id);
        push.pipeline = Some(
            PipelineExecution::from_steps(&[])
                .with_kind(PipelineKind::Push)
                .with_depends_on_session_id(Some(session.id.clone())),
        );
        store.create_session(&push).unwrap();

        let registry = SessionRegistry::new();
        let (_token, epoch) = registry.register(&session.id);
        registry.mark_finished(&session.id, epoch);
        assert!(crate::rate_limit_scheduler::pause_for_rate_limit(
            &store,
            &session.id,
            Some("rate limit"),
            Some(RESETS_AT),
            Some("five_hour"),
            NOW_MS,
        )
        .unwrap());
        PausedRun {
            store,
            registry,
            session_id: session.id,
            epoch,
            dependent_push_id: push.id,
        }
    }

    impl PausedRun {
        fn row(&self) -> Session {
            self.store.get_session(&self.session_id).unwrap().unwrap()
        }

        fn deregister(&self, app: &tauri::App<tauri::test::MockRuntime>) -> bool {
            let cancelled = deregister_paused_run(
                &self.store,
                &self.registry,
                app.handle(),
                &self.session_id,
                self.epoch,
            );
            assert!(
                !self.registry.is_live(&self.session_id),
                "the run's entry is gone whether or not a Stop was recorded"
            );
            cancelled
        }

        fn assert_still_paused_for_auto_resume(&self) {
            let row = self.row();
            assert_eq!(row.status, SessionStatus::Paused);
            assert_eq!(row.completion_reason, Some(CompletionReason::RateLimited));
            assert_eq!(row.rate_limit_resets_at.as_deref(), Some(RESETS_AT));
            assert_eq!(
                self.store.get_rate_limited_paused_sessions().unwrap().len(),
                1,
                "the scheduler still lists the row"
            );
            assert_eq!(
                self.store
                    .get_session(&self.dependent_push_id)
                    .unwrap()
                    .unwrap()
                    .status,
                SessionStatus::Queued
            );
        }
    }

    /// The second half of the fix. The entry answers a Stop `true` until the
    /// deregister, so one landing after the late-Stop check wrote nothing and
    /// was dropped, and the scheduler restarted the stopped session at the
    /// reset time.
    #[test]
    fn a_stop_accepted_after_the_pause_write_cancels_the_paused_row() {
        for reason in [
            CompletionReason::Interrupted,
            CompletionReason::ProjectSessionInterrupted,
        ] {
            let run = paused_run();
            let paused_row = run.row();
            assert_eq!(paused_row.status, SessionStatus::Paused);
            assert!(
                paused_row.provider_metadata.is_some(),
                "the pause write lands with its retry bookkeeping"
            );
            // The Stop: accepted by the entry, so it writes nothing itself.
            assert!(run
                .registry
                .cancel_with_completion_reason(&run.session_id, reason.clone()));
            assert_eq!(run.row().status, SessionStatus::Paused);

            let app = mock_app();
            let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
            let seen = Arc::clone(&events);
            app.handle()
                .listen_any("session-status-changed", move |event| {
                    seen.lock()
                        .unwrap()
                        .push(serde_json::from_str(event.payload()).unwrap());
                });

            assert!(run.deregister(&app));

            let row = run.row();
            assert_eq!(row.status, SessionStatus::Cancelled);
            assert_eq!(row.completion_reason, Some(reason.clone()));
            assert_eq!(row.rate_limit_resets_at, None);
            assert_eq!(row.rate_limit_kind, None);
            // As for a UI Stop on a paused row, the retry bookkeeping stays
            // in the provider metadata; nothing reads it off a cancelled row.
            assert_eq!(row.provider_metadata, paused_row.provider_metadata);
            // The scheduler neither lists the row nor wins its claim.
            assert!(run
                .store
                .get_rate_limited_paused_sessions()
                .unwrap()
                .is_empty());
            assert!(!run
                .store
                .transition_to_running_with_provider_metadata(&run.session_id, None)
                .unwrap());
            // Work gated on the session is settled, as a UI Stop settles it.
            assert_eq!(
                run.store
                    .get_session(&run.dependent_push_id)
                    .unwrap()
                    .unwrap()
                    .status,
                SessionStatus::Cancelled
            );
            // The cancel announces itself, in place of the run's `paused`.
            let events = events.lock().unwrap().clone();
            let event = events
                .iter()
                .find(|event| event["sessionId"] == run.session_id.as_str())
                .unwrap_or_else(|| panic!("no status event for the stopped session: {events:?}"));
            assert_eq!(event["status"], "cancelled");
            assert_eq!(event["completionReason"], reason.as_str());
            assert!(event["rateLimitResetsAt"].is_null());
            assert!(event["rateLimitKind"].is_null());
        }
    }

    /// The ordinary pause: no Stop reached the entry, so the row is left for
    /// the scheduler and the caller goes on to announce `paused`.
    #[test]
    fn a_paused_run_no_stop_reached_stays_paused() {
        let run = paused_run();

        assert!(!run.deregister(&mock_app()));

        run.assert_still_paused_for_auto_resume();
    }

    /// A quit cancels every registered session. It is no Stop: the paused
    /// row waits for the scheduler after relaunch, like any other paused
    /// session a quit leaves behind.
    #[test]
    fn a_quit_after_the_pause_write_leaves_the_row_paused() {
        let run = paused_run();
        assert!(run
            .registry
            .cancel_with_completion_reason(&run.session_id, CompletionReason::AppQuit));

        assert!(!run.deregister(&mock_app()));

        run.assert_still_paused_for_auto_resume();
    }

    /// The shared cancel's write is not guarded on the status, so the row is
    /// read back first: a Resume that took the paused row since the pause
    /// write owns it, and the Stop was aimed at the turn that just ended.
    #[test]
    fn a_stop_does_not_cancel_a_row_a_resume_took_since_the_pause() {
        let run = paused_run();
        assert!(run.registry.cancel(&run.session_id));
        assert!(run
            .store
            .transition_to_running_with_provider_metadata(&run.session_id, None)
            .unwrap());

        assert!(!run.deregister(&mock_app()));

        assert_eq!(run.row().status, SessionStatus::Running);
        assert_eq!(
            run.store
                .get_session(&run.dependent_push_id)
                .unwrap()
                .unwrap()
                .status,
            SessionStatus::Queued
        );
    }

    /// `finish_session_run` needs a concrete app runtime and a live agent
    /// run to drive, so its use of the two functions above is pinned by
    /// source text, like `rate_limit_scheduler`'s
    /// `run_tick_holds_liveness_before_its_claim`: the late Stop is folded
    /// into the decision before the write, the pause branch deregisters
    /// through `deregister_paused_run` after it, and that comes before the
    /// `paused` announcement it replaces.
    #[test]
    fn finish_session_run_records_a_late_stop_on_both_sides_of_the_pause_write() {
        let source = include_str!("../session_runner.rs");
        let start = "fn finish_session_run(";
        let body = &source[source.find(start).expect(start)..];
        let body = &body[..body.find("\n}\n").expect("end of finish_session_run")];
        let position = |needle: &str| {
            body.find(needle)
                .unwrap_or_else(|| panic!("`finish_session_run` must contain `{needle}`"))
        };

        let late_stop = position("late_stop::terminal_decision_after_late_stop(");
        let pause_write = position("crate::rate_limit_scheduler::pause_for_rate_limit(");
        let deregister = position("late_stop::deregister_paused_run(");
        let announce = position("emit_status(");
        assert!(
            late_stop < pause_write,
            "a late Stop must replace the decision before the pause write reads it"
        );
        assert!(
            pause_write < deregister,
            "the Stop is read out with the deregister that follows the pause write"
        );
        assert!(
            deregister < announce,
            "a paused row a Stop ended must not then be announced as `paused`"
        );
        // The pause branch has no other way out of the registry: a plain
        // `deregister` there would drop the Stop its entry accepted.
        let pause_branch = &body[position("if transitioned && rate_limit_recovery.is_some() {")..];
        let plain_deregister = pause_branch
            .find("registry.deregister(")
            .expect("the non-pause branch deregisters plainly");
        assert!(
            pause_branch[..plain_deregister].contains("late_stop::deregister_paused_run("),
            "the pause branch must deregister through `deregister_paused_run`"
        );
    }
}
