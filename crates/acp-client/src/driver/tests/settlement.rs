//! Settlement delivery through the real publishers and coalescing watch channel.

use super::*;
use std::future::{poll_fn, Future};
use std::task::Poll;

#[derive(Clone, Copy, Debug)]
enum TerminalKind {
    RawNotification,
    RawPatch,
    Typed,
}

impl TerminalKind {
    fn mode(self) -> TaskTrackingMode {
        match self {
            Self::RawNotification | Self::RawPatch => TaskTrackingMode::Raw,
            Self::Typed => TaskTrackingMode::Typed,
        }
    }

    async fn start(self, handler: &Arc<AcpNotificationHandler>, id: &str) {
        match self {
            Self::RawNotification | Self::RawPatch => {
                feed_sdk_frame(handler, task_started(id)).await;
            }
            Self::Typed => {
                feed_async_task_update(
                    handler,
                    serde_json::json!({
                        "sessionUpdate": "async_task_spawned",
                        "asyncTaskId": id,
                    }),
                )
                .await;
            }
        }
    }

    async fn settle(self, handler: &Arc<AcpNotificationHandler>, id: &str) {
        match self {
            Self::RawNotification | Self::RawPatch => {
                let frame = match self {
                    Self::RawNotification => serde_json::json!({
                        "type": "system",
                        "subtype": "task_notification",
                        "task_id": id,
                    }),
                    _ => serde_json::json!({
                        "type": "system",
                        "subtype": "task_updated",
                        "task_id": id,
                        "patch": { "status": "completed" },
                    }),
                };
                feed_sdk_frame(handler, frame).await;
            }
            Self::Typed => {
                feed_async_task_update(
                    handler,
                    serde_json::json!({
                        "sessionUpdate": "async_task_state_update",
                        "asyncTaskId": id,
                        "state": "completed",
                    }),
                )
                .await;
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Follower {
    Ordinary,
    RawState,
    TypedProgress,
}

impl Follower {
    async fn publish(self, handler: &Arc<AcpNotificationHandler>) {
        match self {
            Self::Ordinary => feed(handler, agent_text(None, "continuing")).await,
            Self::RawState => {
                feed_sdk_frame(
                    handler,
                    serde_json::json!({
                        "type": "system",
                        "subtype": "session_state_changed",
                        "state": "running",
                    }),
                )
                .await;
            }
            Self::TypedProgress => {
                feed_async_task_update(
                    handler,
                    serde_json::json!({
                        "sessionUpdate": "async_task_progress",
                        "asyncTaskId": "OLD",
                    }),
                )
                .await;
            }
        }
    }
}

async fn check_coalesced_settlement(kind: TerminalKind) {
    for publish_start in [false, true] {
        for follower in [
            Follower::Ordinary,
            Follower::RawState,
            Follower::TypedProgress,
        ] {
            let case = format!("{kind:?}, publish_start={publish_start}, {follower:?}");
            let handler = hold_test_handler();
            handler.set_task_tracking_mode(kind.mode());
            kind.start(&handler, "OLD").await;
            let mut rx = handler.subscribe_background_activity();
            let start = tokio::time::Instant::now();
            let initial = rx.borrow_and_update().clone();
            let mut state = HoldingState::new(test_hold_config(), kind.mode(), start, &initial);

            // No watch read between the start, terminal edge, and follower:
            // both missing starts and complete coalesced lifecycles must work.
            if publish_start {
                kind.start(&handler, "B").await;
            }
            kind.settle(&handler, "B").await;
            follower.publish(&handler).await;
            assert!(rx.has_changed().unwrap(), "{case}");
            let latest = rx.borrow_and_update().clone();
            assert_eq!(latest.live_task_ids, initial.live_task_ids, "{case}");
            assert_eq!(latest.live_tasks, 1, "{case}");
            assert!(latest.activity_seq > initial.activity_seq, "{case}");

            // OLD never leaves the observed set: ID removal cannot rescue a
            // lost settlement signal, and no new live task can extend the cap.
            state.observe(start + Duration::from_secs(599), &latest);
            assert_eq!(
                state.cap_deadline(),
                start + Duration::from_secs(1199),
                "{case}"
            );
            assert_eq!(
                state.poll_settle(start + Duration::from_secs(600)),
                None,
                "{case}"
            );
            assert_eq!(
                state.poll_settle(start + Duration::from_secs(1199)),
                Some(HoldSettle::HeldUntilCap),
                "{case}"
            );
        }
    }
}

#[tokio::test]
async fn raw_notifications_survive_watch_coalescing() {
    check_coalesced_settlement(TerminalKind::RawNotification).await;
}

#[tokio::test]
async fn raw_terminal_patches_survive_watch_coalescing() {
    check_coalesced_settlement(TerminalKind::RawPatch).await;
}

#[tokio::test]
async fn typed_terminals_survive_watch_coalescing() {
    check_coalesced_settlement(TerminalKind::Typed).await;
}

#[tokio::test(start_paused = true)]
async fn holding_wait_survives_the_old_cap_after_a_coalesced_settlement() {
    let handler = hold_test_handler();
    let (_prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
    let (_control_tx, mut control_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let child_exited = CancellationToken::new();
    let hold_active = AtomicBool::new(false);
    let config = test_hold_config();
    feed_sdk_frame(&handler, task_started("OLD")).await;

    let mut hold = std::pin::pin!(hold_for_background_quiescence(
        &config,
        TaskTrackingMode::Raw,
        &handler,
        &cancel,
        &child_exited,
        &mut prompt_rx,
        &mut control_rx,
        &hold_active,
        &|_| {},
        "sess-1",
        None,
    ));
    // Explicit polls establish entry and consumption barriers. Leaving the
    // future unspawned guarantees it cannot see any intermediate snapshot.
    assert!(poll_fn(|cx| Poll::Ready(hold.as_mut().poll(cx)))
        .await
        .is_pending());
    tokio::time::advance(Duration::from_secs(599)).await;
    TerminalKind::RawNotification.start(&handler, "B").await;
    TerminalKind::RawNotification.settle(&handler, "B").await;
    Follower::Ordinary.publish(&handler).await;
    assert!(poll_fn(|cx| Poll::Ready(hold.as_mut().poll(cx)))
        .await
        .is_pending());

    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(
        poll_fn(|cx| Poll::Ready(hold.as_mut().poll(cx)))
            .await
            .is_pending(),
        "the coalesced settlement must carry the hold past its original 600s cap"
    );
    tokio::time::advance(Duration::from_secs(598)).await;
    assert!(matches!(
        poll_fn(|cx| Poll::Ready(hold.as_mut().poll(cx))).await,
        Poll::Ready(HoldOutcome::HeldUntilCap)
    ));
}

#[tokio::test]
async fn settlements_are_consumed_once_not_replayed_by_ordinary_activity() {
    for kind in [
        TerminalKind::RawNotification,
        TerminalKind::RawPatch,
        TerminalKind::Typed,
    ] {
        let handler = hold_test_handler();
        handler.set_task_tracking_mode(kind.mode());
        kind.start(&handler, "OLD").await;
        let mut rx = handler.subscribe_background_activity();
        let start = tokio::time::Instant::now();
        let initial = rx.borrow_and_update().clone();
        let mut state = HoldingState::new(test_hold_config(), kind.mode(), start, &initial);

        // Multiple terminal events coalesced into one read need only one
        // extension, measured from the observation rather than each event.
        kind.settle(&handler, "B").await;
        kind.settle(&handler, "C").await;
        let settled = rx.borrow_and_update().clone();
        assert_eq!(settled.settlement_seq, 2, "{kind:?}");
        state.observe(start + Duration::from_secs(100), &settled);
        let cap = start + Duration::from_secs(700);
        assert_eq!(state.cap_deadline(), cap, "{kind:?}");

        state.observe(start + Duration::from_secs(150), &settled);
        assert_eq!(state.cap_deadline(), cap, "{kind:?}: repeated snapshot");
        for follower in [
            Follower::Ordinary,
            Follower::RawState,
            Follower::TypedProgress,
        ] {
            follower.publish(&handler).await;
            let latest = rx.borrow_and_update().clone();
            assert!(latest.activity_seq > settled.activity_seq);
            assert_eq!(latest.settlement_seq, settled.settlement_seq);
            state.observe(start + Duration::from_secs(200), &latest);
            assert_eq!(state.cap_deadline(), cap, "{kind:?}: {follower:?}");
        }

        kind.settle(&handler, "D").await;
        let latest = rx.borrow_and_update().clone();
        assert_eq!(latest.settlement_seq, 3, "{kind:?}");
        state.observe(start + Duration::from_secs(300), &latest);
        assert_eq!(
            state.cap_deadline(),
            start + Duration::from_secs(900),
            "{kind:?}"
        );
        assert_eq!(
            state.task_deadlines["OLD"],
            start + Duration::from_secs(600)
        );
    }
}

#[test]
fn hold_entry_and_reentry_baseline_historical_settlements() {
    for mode in [TaskTrackingMode::Raw, TaskTrackingMode::Typed] {
        let start = tokio::time::Instant::now();
        let mut activity = BackgroundActivity {
            settlement_seq: 7,
            ..seen_with_ids(["OLD"])
        };
        let mut state = HoldingState::new(test_hold_config(), mode, start, &activity);
        activity.activity_seq += 1;
        state.observe(start + Duration::from_secs(599), &activity);
        assert_eq!(state.cap_deadline(), start + Duration::from_secs(600));

        activity.settlement_seq += 1;
        activity.activity_seq += 1;
        state.observe(start + Duration::from_secs(599), &activity);
        assert_eq!(state.cap_deadline(), start + Duration::from_secs(1199));

        // A new turn can enter a new hold on the same connection. Settlements
        // consumed by the earlier hold must not be credited a second time.
        let reentry = start + Duration::from_secs(1500);
        let mut next = HoldingState::new(test_hold_config(), mode, reentry, &activity);
        activity.activity_seq += 1;
        next.observe(reentry + Duration::from_secs(599), &activity);
        assert_eq!(next.cap_deadline(), reentry + Duration::from_secs(600));
    }
}

#[tokio::test]
async fn settlement_counters_wrap_and_still_extend_the_floor() {
    for kind in [
        TerminalKind::RawNotification,
        TerminalKind::RawPatch,
        TerminalKind::Typed,
    ] {
        let handler = hold_test_handler();
        handler.set_task_tracking_mode(kind.mode());
        kind.start(&handler, "OLD").await;
        handler.background_activity_tx.send_modify(|activity| {
            activity.settlement_seq = u64::MAX;
        });
        let mut rx = handler.subscribe_background_activity();
        let start = tokio::time::Instant::now();
        let initial = rx.borrow_and_update().clone();
        let mut state = HoldingState::new(test_hold_config(), kind.mode(), start, &initial);

        kind.settle(&handler, "B").await;
        let latest = rx.borrow_and_update().clone();
        assert_eq!(latest.settlement_seq, 0, "{kind:?}");
        state.observe(start + Duration::from_secs(599), &latest);
        assert_eq!(
            state.cap_deadline(),
            start + Duration::from_secs(1199),
            "{kind:?}"
        );
    }
}

#[test]
fn settlement_sequence_extensions_cannot_exceed_the_absolute_ceiling() {
    let start = tokio::time::Instant::now();
    let mut activity = seen_with_ids(["OLD"]);
    let config = BackgroundHoldConfig {
        hold_ceiling: Duration::from_secs(650),
        ..test_hold_config()
    };
    let mut state = HoldingState::new(config, TaskTrackingMode::Raw, start, &activity);
    activity.settlement_seq += 1;
    state.observe(start + Duration::from_secs(599), &activity);
    assert_eq!(state.cap_deadline(), start + Duration::from_secs(650));
    assert_eq!(state.poll_settle(start + Duration::from_secs(600)), None);
    assert_eq!(
        state.poll_settle(start + Duration::from_secs(650)),
        Some(HoldSettle::HeldUntilCap)
    );
}
