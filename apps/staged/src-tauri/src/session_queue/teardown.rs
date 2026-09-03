//! Stopping a branch's sessions before the branch is deleted.
//!
//! `sessions.branch_id` is `ON DELETE CASCADE`, and every branch-scoped
//! session carries it, so deleting a branch deletes its session rows — and
//! their transcripts — whatever state they are in. Left to the cascade, a
//! running agent kept working in a worktree the delete had just removed, its
//! thread's terminal write matched no row, and the client saw the session
//! vanish rather than end. Every branch-removing path therefore runs
//! [`stop_branch_sessions`] first: queued rows are cancelled, running ones are
//! signalled and waited out, so each row reaches a terminal state (and the
//! client hears about it) before the cascade takes it.

use std::sync::Arc;
use std::time::Duration;

use crate::session_runner::{self, SessionRegistry};
use crate::store::{self, CompletionReason, SessionStatus, Store};

/// How long a branch delete waits for its running sessions to stop. A session
/// that outlasts it has still been signalled, and its thread's terminal write
/// tolerates the row being gone, so the delete goes ahead rather than leaving
/// the branch half torn down.
pub(crate) const BRANCH_TEARDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Stop every session on `branches` and wait, up to
/// [`BRANCH_TEARDOWN_TIMEOUT`], for the ones this process owns to finish.
pub(crate) async fn stop_branch_sessions<R: tauri::Runtime>(
    store: &Store,
    registry: &Arc<SessionRegistry>,
    app_handle: &tauri::AppHandle<R>,
    branches: &[store::Branch],
) -> Result<(), String> {
    let owned = signal_branch_sessions(store, registry, app_handle, branches)?;
    if owned.is_empty() {
        return Ok(());
    }

    let released = tauri::async_runtime::spawn_blocking({
        let registry = Arc::clone(registry);
        let owned = owned.clone();
        move || registry.wait_until_released(&owned, BRANCH_TEARDOWN_TIMEOUT)
    })
    .await
    .map_err(|e| format!("Failed to wait for branch sessions to stop: {e}"))?;
    if !released {
        log::warn!(
            "[session_queue] branch teardown: sessions {owned:?} did not stop within {}s; \
             deleting the branch anyway",
            BRANCH_TEARDOWN_TIMEOUT.as_secs()
        );
    }
    Ok(())
}

/// The non-blocking half of [`stop_branch_sessions`]: end or signal every
/// session on `branches`, and return the ids whose owner in this process has
/// to be waited out.
///
/// - Queued rows are cancelled first, so a terminal-state kick from one of the
///   running sessions below drains an empty queue instead of claiming a row
///   into a branch that is going away.
/// - A running row with a registered token is cancelled through it; its thread
///   writes `cancelled` and deregisters.
/// - A running row with only a liveness hold is between a claim and the
///   runner's `register` (or is an immediate pull): the cancel is parked for
///   the `register` to apply, and the hold is waited out either way.
/// - A running row nothing here owns (another process's, or one the sweep has
///   not reached yet) is written `cancelled` directly, as `cancel_session_impl`'s
///   fallback does, so the client sees it end. A `disconnected` row is never
///   owned, so it always takes this path, and its remote turn is cancelled
///   first, best effort, since nothing could stop it once the row is gone.
///   That is so whether or not the teardown is expected to delete the
///   workstation: cleanup can leave it standing (two branches removed
///   together that share it each see the other still using it; a failed
///   `ws_delete`), and a cancel against one being deleted does no harm.
pub(crate) fn signal_branch_sessions<R: tauri::Runtime>(
    store: &Store,
    registry: &SessionRegistry,
    app_handle: &tauri::AppHandle<R>,
    branches: &[store::Branch],
) -> Result<Vec<String>, String> {
    signal_branch_sessions_with(store, registry, app_handle, branches, &|store, session| {
        session_runner::reconcile::cancel_abandoned_turn(store, session);
    })
}

/// [`signal_branch_sessions`], with how a `disconnected` row's remote turn is
/// cancelled as a parameter (tests).
pub(super) fn signal_branch_sessions_with<R: tauri::Runtime>(
    store: &Store,
    registry: &SessionRegistry,
    app_handle: &tauri::AppHandle<R>,
    branches: &[store::Branch],
    cancel_abandoned_turn: &dyn Fn(&Store, &store::Session),
) -> Result<Vec<String>, String> {
    let mut owned = Vec::new();

    for branch in branches {
        let queued = store
            .get_queued_sessions_for_branch(&branch.id)
            .map_err(|e| e.to_string())?;
        for session in queued {
            let cancelled = store
                .transition_from_queued(
                    &session.id,
                    SessionStatus::Cancelled,
                    None,
                    Some(&CompletionReason::Interrupted),
                )
                .map_err(|e| e.to_string())?;
            if cancelled {
                emit_cancelled(app_handle, &session.id, branch);
            }
        }

        let running = store
            .get_busy_sessions_for_branch(&branch.id)
            .map_err(|e| e.to_string())?;
        for session in running {
            if registry.cancel_with_completion_reason(&session.id, CompletionReason::Interrupted) {
                owned.push(session.id);
            } else if registry.is_live(&session.id) {
                registry.cancel_or_defer(&session.id, CompletionReason::Interrupted);
                owned.push(session.id);
            } else {
                if session.status == SessionStatus::Disconnected {
                    cancel_abandoned_turn(store, &session);
                }
                store
                    .update_session_status(
                        &session.id,
                        SessionStatus::Cancelled,
                        None,
                        Some(&CompletionReason::Interrupted),
                    )
                    .map_err(|e| e.to_string())?;
                emit_cancelled(app_handle, &session.id, branch);
            }
        }
    }

    Ok(owned)
}

fn emit_cancelled<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    session_id: &str,
    branch: &store::Branch,
) {
    session_runner::emit_status(
        app_handle,
        session_id,
        SessionStatus::Cancelled.as_str(),
        None,
        Some(&CompletionReason::Interrupted),
        None,
        Some(branch.id.clone()),
        Some(branch.project_id.clone()),
    );
}
