//! Starting a run's connection over when it failed before its prompt went out.
//!
//! Two failures leave nothing done that a second connection would do twice:
//!
//! - A setup step timed out (`acp_client::is_session_setup_timeout_error`).
//!   Setup comes before the prompt, so the agent never got this turn. The
//!   start is tried once more on a fresh agent process: on a machine where
//!   the session's thread was not scheduled for a minute and a half, the
//!   next attempt usually goes through, and otherwise the session ends in
//!   `error` exactly as before, one attempt later. Nothing new is written for
//!   the user: the turn's prompt row is stored before the loop and the retry
//!   sends the same prompt.
//! - The agent has no session by the stored id
//!   (`acp_client::is_session_not_found_error`) and nothing the agent said is
//!   recorded for the session (`acp_client::agent_output_recorded`). Earlier
//!   builds stored the id as soon as `session/new` answered, before setup
//!   finished, so a session whose setup failed kept an id for a conversation
//!   the agent never kept. Loading it can only fail, and starting fresh with
//!   `session/new` loses nothing. A session the agent has spoken in keeps
//!   the error: starting over there would silently drop its conversation.
//!
//! Each happens at most once per run, so a run makes at most three
//! connections before it gives up.

/// How to start a failed connection over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StartupRestart {
    /// Retry the same start on a fresh agent process.
    RetryTimedOutSetup,
    /// Start a fresh agent session with `session/new` instead of loading the
    /// stored id.
    StartFresh,
}

/// What a run has already restarted for.
#[derive(Debug, Default)]
pub(super) struct StartupRestarts {
    timeout_retried: bool,
    started_fresh: bool,
}

impl StartupRestarts {
    /// Whether to start over after a connection failed with `error`, and
    /// how. `resuming` is whether the connection was asked to load a stored
    /// agent session. `agent_output_recorded` is only asked when the agent
    /// refused that load as not found.
    ///
    /// The caller rules out a Stop, a quit and a reattached turn first:
    /// those are not startup failures to try again.
    pub(super) fn after_failed_connection(
        &mut self,
        error: &str,
        resuming: bool,
        agent_output_recorded: impl FnOnce() -> bool,
    ) -> Option<StartupRestart> {
        if acp_client::is_session_setup_timeout_error(error) {
            if self.timeout_retried {
                return None;
            }
            self.timeout_retried = true;
            return Some(StartupRestart::RetryTimedOutSetup);
        }
        if resuming
            && !self.started_fresh
            && acp_client::is_session_not_found_error(error)
            && !agent_output_recorded()
        {
            self.started_fresh = true;
            return Some(StartupRestart::StartFresh);
        }
        None
    }
}

/// [`acp_client::agent_output_recorded`] for a decision that must not start
/// a session over on a guess: a store that cannot answer counts as the agent
/// having spoken.
pub(super) fn agent_output_recorded_or_assumed(
    store: &crate::store::Store,
    session_id: &str,
) -> bool {
    acp_client::agent_output_recorded(store, session_id).unwrap_or_else(|e| {
        log::warn!(
            "Session {session_id}: could not read the transcript to decide whether a fresh \
             agent session loses anything ({e}); keeping the error"
        );
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{MessageRole, Store};

    const TIMEOUT: &str = "Timed out waiting for ACP protocol startup: session/new did not \
                           answer within 90s; completed: initialize 0.50s";
    const NOT_FOUND: &str = "The agent has no session by that id (failed to load ACP \
                             session): Error { code: -32002 }";

    #[test]
    fn a_setup_timeout_is_retried_once() {
        let mut restarts = StartupRestarts::default();
        assert_eq!(
            restarts.after_failed_connection(TIMEOUT, false, || unreachable!()),
            Some(StartupRestart::RetryTimedOutSetup)
        );
        // The second timeout is the session's error.
        assert_eq!(
            restarts.after_failed_connection(TIMEOUT, false, || unreachable!()),
            None
        );
    }

    #[test]
    fn a_resumed_setup_timeout_is_retried_once_too() {
        let mut restarts = StartupRestarts::default();
        assert_eq!(
            restarts.after_failed_connection(TIMEOUT, true, || unreachable!()),
            Some(StartupRestart::RetryTimedOutSetup)
        );
    }

    #[test]
    fn other_failures_are_not_retried() {
        let mut restarts = StartupRestarts::default();
        for error in [
            "Prompt failed: overloaded",
            "Failed to create ACP session: Error { code: -32603 }",
            "ACP authentication is required to load ACP session. Sign this agent in, then retry.",
            "Remote agent lost: bloxlet reports the process stopped",
        ] {
            assert_eq!(
                restarts.after_failed_connection(error, true, || false),
                None,
                "{error}"
            );
        }
    }

    #[test]
    fn an_unknown_session_the_agent_never_spoke_in_starts_fresh_once() {
        let mut restarts = StartupRestarts::default();
        assert_eq!(
            restarts.after_failed_connection(NOT_FOUND, true, || false),
            Some(StartupRestart::StartFresh)
        );
        assert_eq!(
            restarts.after_failed_connection(NOT_FOUND, true, || false),
            None
        );
    }

    #[test]
    fn an_unknown_session_with_a_conversation_keeps_the_error() {
        let mut restarts = StartupRestarts::default();
        assert_eq!(
            restarts.after_failed_connection(NOT_FOUND, true, || true),
            None
        );
    }

    #[test]
    fn a_fresh_start_that_times_out_still_gets_its_retry() {
        let mut restarts = StartupRestarts::default();
        assert_eq!(
            restarts.after_failed_connection(NOT_FOUND, true, || false),
            Some(StartupRestart::StartFresh)
        );
        assert_eq!(
            restarts.after_failed_connection(TIMEOUT, false, || unreachable!()),
            Some(StartupRestart::RetryTimedOutSetup)
        );
        assert_eq!(
            restarts.after_failed_connection(TIMEOUT, false, || unreachable!()),
            None
        );
    }

    /// The transcript of a session whose setup failed holds its prompt rows
    /// and the hidden rows setup wrote, none of which the agent said; a
    /// session it answered in holds its reply.
    #[test]
    fn only_the_agents_own_rows_count_as_its_output() {
        let store = Store::in_memory().expect("in-memory store");
        let session =
            crate::store::Session::new_running("do the thing", std::path::Path::new("/tmp"));
        store.create_session(&session).expect("create session");
        store
            .add_session_message(&session.id, MessageRole::User, "do the thing")
            .expect("prompt row");
        store
            .add_acp_metadata_message(
                &session.id,
                &crate::store::AcpMessageMetadata {
                    acp_event_kind: Some("initialize".to_string()),
                    ..Default::default()
                },
            )
            .expect("initialize row");
        store
            .add_session_message(&session.id, MessageRole::User, "please continue")
            .expect("continue row");
        assert!(!agent_output_recorded_or_assumed(&store, &session.id));

        store
            .add_session_message(&session.id, MessageRole::Assistant, "on it")
            .expect("reply row");
        assert!(agent_output_recorded_or_assumed(&store, &session.id));
    }
}
