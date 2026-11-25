//! Starting a run's connection over when it failed before its prompt went out.
//!
//! Two failures leave nothing done that a second connection would do twice:
//!
//! - A setup step timed out (`acp_client::SetupFailure::TimedOut`).
//!   Setup comes before the prompt, so the agent never got this turn. The
//!   start is tried once more on a fresh agent process: on a machine where
//!   the session's thread was not scheduled for a minute and a half, the
//!   next attempt usually goes through, and otherwise the session ends in
//!   `error` exactly as before, one attempt later. The user sees nothing
//!   twice: the turn's prompt row is stored before the loop and the retry
//!   sends the same prompt, and a config fallback notice the first attempt
//!   wrote (its setup got as far as a substituted model) is not written
//!   again when the retry applies the same selections
//!   (`MessageWriter::with_shown_config_fallbacks`): the notices one
//!   connection showed are handed to the connection that starts it over
//!   ([`StartupRestarts::shown_config_fallbacks`]), and to no other. The
//!   hidden rows setup records (initialize, modes, config options) are
//!   written by each attempt.
//! - The agent has no session by the stored id
//!   (`acp_client::SetupFailure::SessionNotFound`) and nothing the agent said is
//!   recorded for the session (`acp_client::agent_output_recorded`). Earlier
//!   builds stored the id as soon as `session/new` answered, before setup
//!   finished, so a session whose setup failed kept an id for a conversation
//!   the agent never kept. Loading it can only fail, and starting fresh with
//!   `session/new` loses nothing. A session the agent has spoken in keeps
//!   the error: starting over there would silently drop its conversation.
//!
//! Both are read off the connection's settle
//! (`acp_client::SessionSettled::setup_failure`), never off the error text:
//! that reaches the runner wrapped by the SDK ("ACP protocol failed: Error {
//! … }"), and matching its start is what kept the timeout retry from ever
//! firing.
//!
//! Each happens at most once per run, so a run makes at most three
//! connections before it gives up.

use acp_client::SetupFailure;

use crate::agent::ShownConfigFallbacks;

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
    /// The config fallback notices shown by the connection last handed a
    /// set ([`Self::shown_config_fallbacks`]).
    config_fallbacks: ShownConfigFallbacks,
    /// Whether that connection was just started over, so the next one
    /// carries its set on instead of starting an empty one.
    carry_config_fallbacks: bool,
}

impl StartupRestarts {
    /// The set of config fallback notices the next connection's writer
    /// starts from (`MessageWriter::with_shown_config_fallbacks`), taken at
    /// the top of each pass of the turn loop. It holds what the last
    /// connection showed when [`Self::after_turn`] just started that one
    /// over, and is empty for every other connection (a first one, a note
    /// follow-up, a Pikchr correction), which announces a fallback as usual.
    pub(super) fn shown_config_fallbacks(&mut self) -> ShownConfigFallbacks {
        if !std::mem::take(&mut self.carry_config_fallbacks) {
            self.config_fallbacks = ShownConfigFallbacks::default();
        }
        std::sync::Arc::clone(&self.config_fallbacks)
    }

    /// Whether to start over after a connection failed, and how.
    /// `setup_failure` is what its settle said about setup. `resuming` is
    /// whether the connection was asked to load a stored agent session.
    /// `agent_output_recorded` is only asked when the agent refused that load
    /// as not found.
    ///
    /// The caller rules out a Stop, a quit and a reattached turn first:
    /// those are not startup failures to try again.
    pub(super) fn after_failed_connection(
        &mut self,
        setup_failure: Option<SetupFailure>,
        resuming: bool,
        agent_output_recorded: impl FnOnce() -> bool,
    ) -> Option<StartupRestart> {
        if setup_failure == Some(SetupFailure::TimedOut) {
            if self.timeout_retried {
                return None;
            }
            self.timeout_retried = true;
            return Some(StartupRestart::RetryTimedOutSetup);
        }
        if resuming
            && !self.started_fresh
            && setup_failure == Some(SetupFailure::SessionNotFound)
            && !agent_output_recorded()
        {
            self.started_fresh = true;
            return Some(StartupRestart::StartFresh);
        }
        None
    }

    /// The runner's start-over decision once a connection's turn has ended
    /// with `turn_result`: [`Self::after_failed_connection`] for a failed
    /// one, unless `ruled_out` (a Stop, a quit or a reattached turn). Logs
    /// the restart and, for [`StartupRestart::StartFresh`], clears
    /// `agent_session_id` so the next connection sends `session/new`. The
    /// caller sends the same prompt again on a fresh connection, whose
    /// writer gets the failed connection's set from the next
    /// [`Self::shown_config_fallbacks`].
    pub(super) fn after_turn(
        &mut self,
        session_id: &str,
        turn_result: &Result<acp_client::AgentRunOutcome, String>,
        setup_failure: Option<SetupFailure>,
        ruled_out: bool,
        agent_session_id: &mut Option<String>,
        store: &crate::store::Store,
    ) -> Option<StartupRestart> {
        let Err(error) = turn_result else {
            return None;
        };
        if ruled_out {
            return None;
        }
        let restart =
            self.after_failed_connection(setup_failure, agent_session_id.is_some(), || {
                agent_output_recorded_or_assumed(store, session_id)
            })?;
        match restart {
            StartupRestart::RetryTimedOutSetup => log::warn!(
                "Session {session_id}: ACP setup timed out before the prompt was sent; retrying \
                 once on a fresh agent process: {error}"
            ),
            StartupRestart::StartFresh => {
                log::warn!(
                    "Session {session_id}: the agent has no session {} and never answered in \
                     this one; starting a fresh agent session: {error}",
                    agent_session_id.as_deref().unwrap_or_default()
                );
                *agent_session_id = None;
            }
        }
        self.carry_config_fallbacks = true;
        Some(restart)
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

    const TIMEOUT: Option<SetupFailure> = Some(SetupFailure::TimedOut);
    const NOT_FOUND: Option<SetupFailure> = Some(SetupFailure::SessionNotFound);

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
        // A connection that failed any other way (the prompt, authentication,
        // a lost remote agent) settles without a setup failure.
        assert_eq!(restarts.after_failed_connection(None, true, || false), None);
        assert_eq!(
            restarts.after_failed_connection(None, false, || false),
            None
        );
    }

    #[test]
    fn a_new_session_the_agent_does_not_know_is_not_started_fresh() {
        // Only a load can be answered by starting fresh instead.
        let mut restarts = StartupRestarts::default();
        assert_eq!(
            restarts.after_failed_connection(NOT_FOUND, false, || unreachable!()),
            None
        );
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
