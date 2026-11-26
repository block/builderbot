//! The wait between a message reaching a running session's agent and the
//! delivery being reported.
//!
//! A message delivered over a session's live connection (steered into the
//! turn, sent as a new turn, or sent after an interrupt) has its user row
//! written by the connection, through the session's `MessageWriter`. That
//! writer queues the write for the store's writer thread and returns (see
//! `store::write_queue`). The connection queues the row and then answers the
//! delivery. It does not wait for the row to be stored: it runs on the
//! session's thread, which must not wait on the store.
//!
//! The caller is then told the message was delivered and reads the
//! transcript next. The `continue_repo_session` tool builds its reply from
//! it, including the time of the session's last user message, which the
//! calling agent is told to check to confirm the message landed. The chat
//! pane polls at once. A read made while the row is only queued misses it.
//!
//! So the delivery waits here before it is reported, on the caller's
//! runtime. The wait is a barrier on the write queue: it is queued after the
//! connection answered, so it is behind the row. It is bounded, because the
//! queue is shared by every session and the store can be held up for longer
//! than a caller should wait. When the bound passes, the delivery is
//! reported anyway: the message did reach the agent, and the row appears
//! once the store catches up.

use std::time::Duration;

use super::ContinueOutcome;
use crate::store::Store;

/// Whether the session's connection wrote a user row for the message before
/// it answered with `outcome`. Only a message that reached the agent over
/// the live connection has one. A resume's row is written by
/// `start_session`, and a refused or blocked delivery writes none (an
/// unacknowledged steer included: its late answer persists nothing).
pub(super) fn connection_wrote_user_row(outcome: &ContinueOutcome) -> bool {
    match outcome {
        ContinueOutcome::Injected
        | ContinueOutcome::InterruptedAndReprompted
        | ContinueOutcome::StartedNewTurn => true,
        ContinueOutcome::Resumed
        | ContinueOutcome::RejectedQueued
        | ContinueOutcome::Blocked { .. }
        | ContinueOutcome::NotResumable { .. }
        | ContinueOutcome::NotRunning { .. } => false,
    }
}

/// Wait until every write queued so far is stored, or `budget` has passed.
/// Whether it was all stored.
///
/// Call it after the connection has answered: the barrier is queued when
/// this is called, and it must be behind the delivered message's row.
pub(super) async fn wait_until_stored(store: &Store, budget: Duration) -> bool {
    let stored = store.flush_writes();
    if tokio::time::timeout(budget, stored).await.is_ok() {
        return true;
    }
    log::warn!(
        "continue_session: the delivered message's row was not stored within {:.1}s (the store \
         is behind); reporting the delivery without it",
        budget.as_secs_f64()
    );
    false
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::super::{
        route_continue_session, ContinueOutcome, ContinueRoute, ContinueWaits, StoppedSessionPolicy,
    };
    use crate::session_runner::{ExternalSessionRegistration, SessionRegistry};
    use crate::store::{MessageRole, Session, Store};

    const MESSAGE: &str = "also check the tests";
    /// A budget no test here is meant to reach.
    const AMPLE: Duration = Duration::from_secs(60);

    type Steer = Result<acp_client::SteerOutcome, acp_client::SteerError>;

    /// A `running` session whose connection is a stand-in that persists a
    /// delivered message through the session's real `MessageWriter` before
    /// it answers, as a real connection does.
    struct Fixture {
        store: Arc<Store>,
        registry: Arc<SessionRegistry>,
        _registration: ExternalSessionRegistration,
        session_id: String,
    }

    impl Fixture {
        fn new(steer: Steer, interrupt: Result<(), String>) -> Self {
            let store = Arc::new(Store::in_memory().unwrap());
            let session = Session::new_running("prompt", std::path::Path::new("/tmp"));
            store.create_session(&session).unwrap();
            let registry = Arc::new(SessionRegistry::new());
            let registration = registry.register_external(&session.id);
            let writer: Arc<dyn acp_client::MessageWriter> = Arc::new(
                crate::agent::writer::MessageWriter::new(session.id.clone(), Arc::clone(&store)),
            );
            let (handle, _served) =
                acp_client::TurnControlHandle::scripted_recording(steer, interrupt, writer);
            registry.set_turn_control_handle(&session.id, handle);
            Self {
                store,
                registry,
                _registration: registration,
                session_id: session.id,
            }
        }

        /// Deliver [`MESSAGE`] on another task. It resolves to the outcome
        /// and to the user rows a caller reading the transcript right after
        /// being told finds.
        fn deliver(
            &self,
            stored: Duration,
        ) -> tokio::task::JoinHandle<(ContinueOutcome, Vec<String>)> {
            let store = Arc::clone(&self.store);
            let registry = Arc::clone(&self.registry);
            let session_id = self.session_id.clone();
            tokio::spawn(async move {
                let route = route_continue_session(
                    &store,
                    &registry,
                    &session_id,
                    MESSAGE,
                    &[],
                    ContinueWaits {
                        stored,
                        ..ContinueWaits::uniform(Duration::from_secs(10))
                    },
                    StoppedSessionPolicy::Refuse,
                )
                .await
                .unwrap();
                let rows = user_rows(&store, &session_id);
                match route {
                    ContinueRoute::Delivered(outcome) => (outcome, rows),
                    ContinueRoute::Resume(_) => panic!("a running session is not resumed"),
                }
            })
        }

        fn user_rows(&self) -> Vec<String> {
            user_rows(&self.store, &self.session_id)
        }

        /// Wait until the connection has queued the user row. It answers
        /// right after, with nothing awaited in between.
        async fn row_queued(&self) {
            let started = Instant::now();
            while self.store.writes_idle() {
                assert!(
                    started.elapsed() < AMPLE,
                    "the connection never queued the user row"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    }

    fn user_rows(store: &Store, session_id: &str) -> Vec<String> {
        store
            .get_session_messages(session_id)
            .unwrap()
            .into_iter()
            .filter(|message| message.role == MessageRole::User)
            .map(|message| message.content)
            .collect()
    }

    fn refused() -> Steer {
        Err(acp_client::SteerError::Refused(
            "agent does not advertise steering".to_string(),
        ))
    }

    /// With the write connection held, the connection queues the user row
    /// and answers, and the row cannot be stored. The delivery must not be
    /// reported until it can: once the hold ends, the caller is told, and
    /// the read it makes next finds the row.
    async fn reported_once_the_row_is_readable(
        steer: Steer,
        interrupt: Result<(), String>,
        expected: ContinueOutcome,
    ) {
        let f = Fixture::new(steer, interrupt);
        let held = f.store.hold_connection();
        let delivery = f.deliver(AMPLE);

        f.row_queued().await;
        // Time for the answer to reach the caller and for the caller to
        // report it, which is what it did before it waited for the store.
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            !delivery.is_finished(),
            "the delivery was reported while its user row was only queued"
        );
        assert!(f.user_rows().is_empty());

        drop(held);
        let (outcome, rows_read_on_report) = tokio::time::timeout(AMPLE, delivery)
            .await
            .expect("the delivery is reported once the row is stored")
            .unwrap();
        assert_eq!(outcome, expected);
        assert_eq!(rows_read_on_report, vec![MESSAGE.to_string()]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_steered_message_is_reported_once_its_row_is_readable() {
        reported_once_the_row_is_readable(
            Ok(acp_client::SteerOutcome::Injected),
            Ok(()),
            ContinueOutcome::Injected,
        )
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_message_sent_as_a_new_turn_is_reported_once_its_row_is_readable() {
        reported_once_the_row_is_readable(
            Ok(acp_client::SteerOutcome::NewTurnStarted),
            Ok(()),
            ContinueOutcome::StartedNewTurn,
        )
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_message_sent_after_an_interrupt_is_reported_once_its_row_is_readable() {
        reported_once_the_row_is_readable(
            refused(),
            Ok(()),
            ContinueOutcome::InterruptedAndReprompted,
        )
        .await;
    }

    /// The wait is bounded. With the store held for longer than the budget,
    /// the delivery is still reported, after the budget and without the row,
    /// and the row is stored once the store is free.
    #[tokio::test(flavor = "current_thread")]
    async fn a_store_that_stays_blocked_delays_the_report_only_by_the_budget() {
        let budget = Duration::from_millis(300);
        let f = Fixture::new(Ok(acp_client::SteerOutcome::Injected), Ok(()));
        let held = f.store.hold_connection();
        let started = Instant::now();

        let (outcome, rows_read_on_report) = tokio::time::timeout(AMPLE, f.deliver(budget))
            .await
            .expect("the delivery is reported with the store still held")
            .unwrap();

        assert_eq!(outcome, ContinueOutcome::Injected);
        assert!(
            started.elapsed() >= budget,
            "the delivery waited {:?}, less than its {budget:?} budget",
            started.elapsed()
        );
        assert!(rows_read_on_report.is_empty());

        drop(held);
        f.store.flush_writes().await;
        assert_eq!(f.user_rows(), vec![MESSAGE.to_string()]);
    }

    /// A delivery that wrote no user row does not wait on the store. Here
    /// another session's write is stuck behind the held connection, and the
    /// blocked answer comes back regardless.
    #[tokio::test(flavor = "current_thread")]
    async fn a_delivery_that_wrote_no_row_does_not_wait_for_the_store() {
        let f = Fixture::new(
            Err(acp_client::SteerError::Unacknowledged {
                timeout: Duration::from_secs(30),
            }),
            Ok(()),
        );
        let held = f.store.hold_connection();
        f.store.queue_write(|_| {});

        let (outcome, rows_read_on_report) =
            tokio::time::timeout(Duration::from_secs(20), f.deliver(AMPLE))
                .await
                .expect("a blocked delivery is answered with the store still held")
                .unwrap();

        assert!(
            matches!(outcome, ContinueOutcome::Blocked { .. }),
            "{outcome:?}"
        );
        assert!(rows_read_on_report.is_empty());
        drop(held);
    }

    /// The outcomes whose user row the connection wrote are the three live
    /// deliveries, and no other.
    #[test]
    fn only_a_live_delivery_has_a_row_to_wait_for() {
        use super::connection_wrote_user_row as wrote;
        assert!(wrote(&ContinueOutcome::Injected));
        assert!(wrote(&ContinueOutcome::StartedNewTurn));
        assert!(wrote(&ContinueOutcome::InterruptedAndReprompted));
        assert!(!wrote(&ContinueOutcome::Resumed));
        assert!(!wrote(&ContinueOutcome::RejectedQueued));
        assert!(!wrote(&ContinueOutcome::blocked("no")));
        assert!(!wrote(&ContinueOutcome::not_resumable("no")));
        assert!(!wrote(&ContinueOutcome::not_running("no")));
    }
}
