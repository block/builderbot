//! Work held while a project is transferred.
//!
//! A transfer covers every branch of its project from the moment it is
//! registered until the database has switched. In that window nothing may
//! start on those branches, since the transfer snapshots and removes their
//! checkouts; but the user need not wait:
//!
//! - A **new session** is inserted as a `queued` row like any start the
//!   branch can't take yet (`start_or_queue_branch_session_for_store`), and
//!   `drain_branch_queue` waits while the branch is covered. The transfer
//!   kicks the branch once the project has moved.
//! - A **reply** to one of the project's sessions, which `resume_session`
//!   would otherwise refuse, is inserted as a queued follow-up message
//!   (`resume_or_queue_session_for_store`), on a session that is not
//!   running — which the follow-up queue is otherwise never in. The
//!   transfer drains those with [`flush_follow_ups_for_branch`].
//!
//! Both are rows in the store, so a Staged that stops mid-transfer loses
//! nothing: at the next start, once the transfer has been rolled back or
//! marked, the branch queue is kicked and the follow-ups flushed. A transfer
//! that fails or is cancelled sets them going too, where the project still
//! is ([`release_held_work`]), and says so in its error ([`held_work_note`]).
//!
//! A flush waits, as the branch queue's drain does, while the branch has
//! nowhere to run: a transfer that carried nothing lands the branch with its
//! workspace still starting (to remote) or no worktree yet (to local), and
//! a reply sent into either would only fail. The kick provisioning sends
//! when the workspace is running or the worktree is linked asks for the
//! flush again ([`super::kick::flush_follow_ups`]).

use std::sync::Arc;

use tauri::Manager;

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::session_runner::SessionRegistry;
use crate::store::Store;

/// The sessions on `branch_id` whose held follow-ups a flush can send now:
/// none while the branch is gone or still provisioning — its rows wait for
/// the kick provisioning sends, which flushes too — otherwise every session
/// not running that has one, read under the branch's launch lock.
pub(crate) fn held_follow_ups_ready(store: &Store, branch_id: &str) -> Result<Vec<String>, String> {
    let Some(branch) = store.get_branch(branch_id).map_err(|e| e.to_string())? else {
        log::debug!("[session_queue] branch {branch_id} is gone; no follow-ups to flush");
        return Ok(Vec::new());
    };
    if crate::session_commands::branch_waits_for_provisioning(store, &branch)? {
        log::debug!(
            "[session_queue] branch {branch_id} is still provisioning; its held follow-ups wait"
        );
        return Ok(Vec::new());
    }
    // Under the branch's launch lock, which a reply queued for the transfer
    // is inserted under too: one queued as the transfer ends is either in
    // this read or, finding the branch no longer covered, sent directly.
    let lock = crate::session_commands::branch_session_launch_lock_for(branch_id);
    let _guard = lock.lock().unwrap();
    store
        .session_ids_with_queued_messages_for_branch(branch_id)
        .map_err(|e| e.to_string())
}

/// Send the queued follow-ups of every session on `branch_id` that is not
/// running, each on its own task, once the branch can take them (see
/// [`held_follow_ups_ready`]); failures are logged and leave the follow-up
/// queued with its error, where the user can send it by hand.
pub(crate) fn flush_follow_ups_for_branch(
    app_handle: &tauri::AppHandle,
    store: &Arc<Store>,
    branch_id: &str,
) {
    let session_ids = match held_follow_ups_ready(store, branch_id) {
        Ok(ids) => ids,
        Err(e) => {
            log::warn!(
                "[session_queue] cannot list the queued follow-ups on branch {branch_id}: {e}"
            );
            return;
        }
    };
    if session_ids.is_empty() {
        return;
    }
    let Some(registry) = app_handle.try_state::<Arc<SessionRegistry>>() else {
        log::debug!("[session_queue] no session registry managed; leaving follow-ups queued");
        return;
    };
    log::info!(
        "[session_queue] flushing the queued follow-ups of {} session(s) on branch {branch_id}",
        session_ids.len()
    );
    for session_id in session_ids {
        let store = Arc::clone(store);
        let registry = Arc::clone(&registry);
        let app_handle = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            match crate::session_commands::drain_queued_message_for_session(
                store,
                registry,
                Arc::new(ActionExecutor::new()),
                Arc::new(ActionRegistry::new()),
                app_handle,
                session_id.clone(),
            )
            .await
            {
                Ok(true) => log::info!(
                    "[session_queue] sent the follow-up held for session {session_id}"
                ),
                Ok(false) => {}
                Err(e) => log::error!(
                    "[session_queue] failed to send the follow-up held for session {session_id}: {e}"
                ),
            }
        });
    }
}

/// [`flush_follow_ups_for_branch`] for every branch of `project_id`.
pub(crate) fn flush_follow_ups_for_project(
    app_handle: &tauri::AppHandle,
    store: &Arc<Store>,
    project_id: &str,
) {
    let branches = match store.list_branches_for_project(project_id) {
        Ok(branches) => branches,
        Err(e) => {
            log::warn!("[session_queue] cannot list the branches of project {project_id}: {e}");
            return;
        }
    };
    for branch in branches {
        flush_follow_ups_for_branch(app_handle, store, &branch.id);
    }
}

/// Set the work held for a transfer that did not move the project going
/// where the project still is: each of `branch_ids` has its held replies
/// flushed and its queue kicked. Called once the transfer's registry entry
/// has gone, which is what the drain and the flush wait on.
pub(crate) fn release_held_work(
    app_handle: &tauri::AppHandle,
    store: &Arc<Store>,
    branch_ids: &[String],
) {
    for branch_id in branch_ids {
        flush_follow_ups_for_branch(app_handle, store, branch_id);
        super::kick::kick(
            app_handle,
            branch_id,
            super::kick::KickSource::TransferFailed,
        );
    }
}

/// What is queued on `project_id`'s branches waiting for a transfer, as a
/// sentence for a failed transfer's error — or nothing, when nothing is.
/// Said with the project where it was: the work goes ahead there.
pub(crate) fn held_work_note(store: &Store, project_id: &str) -> Option<String> {
    let branches = store.list_branches_for_project(project_id).ok()?;
    let mut sessions = 0usize;
    let mut replies = 0usize;
    for branch in &branches {
        sessions += store.get_queued_sessions_for_branch(&branch.id).ok()?.len();
        replies += store
            .count_queued_session_messages_for_branch(&branch.id)
            .ok()?;
    }
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    let held = match (sessions, replies) {
        (0, 0) => return None,
        (s, 0) => plural(s, "session", "sessions"),
        (0, r) => plural(r, "reply", "replies"),
        (s, r) => format!(
            "{} and {}",
            plural(s, "session", "sessions"),
            plural(r, "reply", "replies")
        ),
    };
    let verb = if sessions + replies == 1 { "is" } else { "are" };
    Some(format!(
        "{held} queued for the transfer {verb} going ahead where the project is."
    ))
}
