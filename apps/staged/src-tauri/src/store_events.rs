//! Store change feed → frontend domain events.
//!
//! [`Store`](crate::store::Store) publishes a [`StoreChange`] from every
//! mutating method. This module is the piece above the Tauri boundary: a
//! task that drains the broadcast channel, coalesces bursts, and forwards
//! each distinct change through [`emit_to_all`] so every window and every
//! web client sees the same event on the same wire.
//!
//! Event names and payloads (all ids camelCase, `null` when unknown —
//! consumers treat a missing id as "refetch the whole surface"):
//!
//! | `StoreChange` | event            | payload                      |
//! |---------------|------------------|------------------------------|
//! | `Project`     | `project-changed`| `{ projectId }`              |
//! | `Branch`      | `branch-changed` | `{ branchId, projectId }`    |
//! | `Notes`       | `notes-changed`  | `{ branchId, projectId }`    |
//! | `Review`      | `review-changed` | `{ reviewId, branchId }`     |
//! | `Repos`       | `repos-changed`  | `{ githubRepo }`             |
//!
//! If the receiver falls behind the channel's capacity, the missed changes
//! are gone — so instead of dropping them silently, the task emits
//! [`lag_flush_events`]: every event with every id `null`, i.e. "the feed
//! lost track, refetch everything". See [`run`].

use crate::store::StoreChange;
use crate::web_server::emit_to_all;
use serde_json::json;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::broadcast;

/// How long to keep absorbing further changes after the first one arrives
/// before flushing the batch. Long enough that a bulk write (deleting a
/// project's branches, a multi-repo setup) collapses its duplicates into
/// one event each; short enough to be imperceptible after a single
/// interactive write.
const COALESCE_WINDOW: Duration = Duration::from_millis(50);

/// Spawn the forwarding task. Runs for the life of the app; survives a
/// store reset because the channel does.
pub fn spawn(app_handle: tauri::AppHandle, rx: broadcast::Receiver<StoreChange>) {
    tauri::async_runtime::spawn(async move {
        run(rx, move |event, payload| {
            emit_to_all(&app_handle, event, payload);
        })
        .await;
    });
}

/// Drain, coalesce, and forward until the channel closes.
///
/// Split out of [`spawn`] so tests can drive it with a plain closure instead
/// of an `AppHandle`.
async fn run(
    mut rx: broadcast::Receiver<StoreChange>,
    mut emit: impl FnMut(&'static str, serde_json::Value),
) {
    loop {
        // Set by either receive arm: the batch can no longer describe what
        // changed, so the window flushes the all-null recovery events instead.
        let mut lagged = false;

        // Idle until something changes, then open the coalescing window.
        let first = match rx.recv().await {
            Ok(change) => Some(change),
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                log::warn!(
                    "Store change feed lagged; {missed} change(s) dropped — \
                     flushing a full invalidation"
                );
                // Fall through into the window rather than looping: it lets
                // the burst that caused the lag finish, so one flush covers it.
                lagged = true;
                None
            }
            Err(broadcast::error::RecvError::Closed) => return,
        };

        // First-seen order, deduplicated on the full change (variant + ids).
        let mut batch = Vec::new();
        let mut seen = HashSet::new();
        if let Some(change) = first {
            seen.insert(change.clone());
            batch.push(change);
        }
        let mut closed = false;

        let window = tokio::time::sleep(COALESCE_WINDOW);
        tokio::pin!(window);
        loop {
            tokio::select! {
                _ = &mut window => break,
                recv = rx.recv() => match recv {
                    Ok(change) => {
                        // Once lagged, individual changes are subsumed by the
                        // null flush — keep draining (which keeps the receiver
                        // caught up) without accumulating them.
                        if !lagged && seen.insert(change.clone()) {
                            batch.push(change);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        log::warn!(
                            "Store change feed lagged; {missed} change(s) dropped — \
                             flushing a full invalidation"
                        );
                        lagged = true;
                        batch.clear();
                        seen.clear();
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        closed = true;
                        break;
                    }
                },
            }
        }

        // Every change the burst dropped had already committed before it was
        // published, so the refetches these events trigger read post-burst
        // state.
        if lagged {
            for (event, payload) in lag_flush_events() {
                emit(event, payload);
            }
        } else {
            for change in batch {
                let (event, payload) = event_for(&change);
                emit(event, payload);
            }
        }
        if closed {
            return;
        }
    }
}

/// The recovery flush: one event per aggregate carrying only nulls, which
/// consumers read as "refetch the whole surface".
///
/// Synthesized directly as wire payloads because [`StoreChange`] can't
/// represent them — a real mutation always knows its aggregate id, and
/// loosening the enum would weaken that invariant across every publish site.
/// So an all-null payload means exactly one thing: the feed lost track.
fn lag_flush_events() -> [(&'static str, serde_json::Value); 5] {
    [
        ("project-changed", json!({ "projectId": null })),
        (
            "branch-changed",
            json!({ "branchId": null, "projectId": null }),
        ),
        (
            "notes-changed",
            json!({ "branchId": null, "projectId": null }),
        ),
        (
            "review-changed",
            json!({ "reviewId": null, "branchId": null }),
        ),
        ("repos-changed", json!({ "githubRepo": null })),
    ]
}

/// Map a change to its wire event name and payload.
fn event_for(change: &StoreChange) -> (&'static str, serde_json::Value) {
    match change {
        StoreChange::Project { project_id } => {
            ("project-changed", json!({ "projectId": project_id }))
        }
        StoreChange::Branch {
            branch_id,
            project_id,
        } => (
            "branch-changed",
            json!({ "branchId": branch_id, "projectId": project_id }),
        ),
        StoreChange::Notes {
            branch_id,
            project_id,
        } => (
            "notes-changed",
            json!({ "branchId": branch_id, "projectId": project_id }),
        ),
        StoreChange::Review {
            review_id,
            branch_id,
        } => (
            "review-changed",
            json!({ "reviewId": review_id, "branchId": branch_id }),
        ),
        StoreChange::Repos { github_repo } => {
            ("repos-changed", json!({ "githubRepo": github_repo }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    type Emitted = Arc<Mutex<Vec<(&'static str, Value)>>>;

    /// A recording `emit` closure plus the handle to read it back.
    fn collector() -> (Emitted, impl FnMut(&'static str, Value)) {
        let emitted: Emitted = Arc::new(Mutex::new(Vec::new()));
        let sink = emitted.clone();
        (emitted, move |event, payload| {
            sink.lock().unwrap().push((event, payload));
        })
    }

    fn branch(id: &str) -> StoreChange {
        StoreChange::Branch {
            branch_id: id.to_string(),
            project_id: Some("p1".to_string()),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn lag_before_the_window_flushes_null_events_instead_of_the_batch() {
        let (tx, rx) = broadcast::channel(4);
        // Six changes into a capacity-4 channel: the receiver's first recv
        // reports the two it missed.
        for i in 0..6 {
            tx.send(branch(&format!("b{i}"))).unwrap();
        }
        drop(tx);

        let (emitted, sink) = collector();
        run(rx, sink).await;

        // Nothing from the four changes still buffered — just the flush.
        assert_eq!(*emitted.lock().unwrap(), lag_flush_events().to_vec());
    }

    #[tokio::test(start_paused = true)]
    async fn the_unlagged_path_still_dedupes_and_passes_changes_through() {
        let (tx, rx) = broadcast::channel(16);
        tx.send(branch("b1")).unwrap();
        tx.send(branch("b1")).unwrap();
        tx.send(StoreChange::Repos { github_repo: None }).unwrap();
        drop(tx);

        let (emitted, sink) = collector();
        run(rx, sink).await;

        assert_eq!(
            *emitted.lock().unwrap(),
            vec![
                (
                    "branch-changed",
                    json!({ "branchId": "b1", "projectId": "p1" })
                ),
                ("repos-changed", json!({ "githubRepo": null })),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn lag_inside_the_window_discards_the_batch_accumulated_before_it() {
        let (tx, rx) = broadcast::channel(4);
        tx.send(branch("before-the-burst")).unwrap();

        let (emitted, sink) = collector();
        let task = tokio::spawn(run(rx, sink));
        // Let the task take that first change and open the coalescing window.
        tokio::task::yield_now().await;

        for i in 0..6 {
            tx.send(branch(&format!("burst-{i}"))).unwrap();
        }
        drop(tx);
        task.await.unwrap();

        assert_eq!(*emitted.lock().unwrap(), lag_flush_events().to_vec());
    }
}
