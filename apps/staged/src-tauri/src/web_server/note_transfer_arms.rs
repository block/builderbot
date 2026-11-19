//! The bodies of the four note-transfer arms of `dispatch`:
//! `move_note_to_project`, `copy_note_to_project` and their project-note
//! twins.
//!
//! They live here, not in the arm table, so they can be run. `dispatch`
//! takes the app's own handle, which no test can make, and these take the
//! one thing the handle is for: `add_repo`, the step that adds a repo to
//! the destination. `dispatch` passes
//! [`add_repo_to_project`](crate::note_transfer::add_repo_to_project), and
//! a test passes a stand-in without git. Everything else an arm does is
//! here, for every caller: the arguments, the transfer, and the answer.
//!
//! One transfer, whoever asks. The caller decides nothing until the
//! transfer has returned, and then only how much it is told. A window has
//! the note open and is answered with the note alone, or with the
//! transfer's error as it is. A control caller named the note by id, so it
//! is told where the note landed and which repos the destination gained
//! (see [`control`](crate::note_transfer::control)), on failure as well.

use std::future::Future;

use serde_json::Value;

use super::{arg, DispatchCaller};
use crate::note_transfer::control::{self, AddedRepos};
use crate::note_transfer::{transfer_branch_note, transfer_project_note, RepoKey, TransferMode};
use crate::store::Store;

/// The transfer a command asks for. `dispatch` routes only the four
/// commands here, a `move_` and a `copy_` for each kind of note.
fn mode_of(command: &str) -> TransferMode {
    if command.starts_with("move_") {
        TransferMode::Move
    } else {
        TransferMode::Copy
    }
}

/// `move_note_to_project` and `copy_note_to_project`. `add_repo` adds the
/// repo `RepoKey` names to the project whose id it is given.
pub(super) async fn branch_note<F, Fut>(
    store: &Store,
    command: &str,
    args: &Value,
    caller: &DispatchCaller,
    add_repo: F,
) -> Result<Value, String>
where
    F: Fn(String, RepoKey) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let note_id: String = arg(args, "noteId")?;
    let target_project_id: String = arg(args, "targetProjectId")?;
    let mode = mode_of(command);
    let added = AddedRepos::default();
    let note = transfer_branch_note(store, &note_id, &target_project_id, mode, |key| {
        added.watch(key.clone(), add_repo(target_project_id.clone(), key))
    })
    .await
    .map_err(|e| match caller {
        // The repo this transfer added stays in the destination. A control
        // caller cannot see that it did, so its error says so, as a
        // project note's does for every caller.
        DispatchCaller::Control(_) => added.explain(store, &target_project_id, e),
        DispatchCaller::Web => e,
    })?;
    let branch_id = note.branch_id.clone();
    let item = match mode {
        TransferMode::Move => crate::note_commands::note_to_timeline_item(store, note),
        TransferMode::Copy => crate::note_commands::standalone_note_to_timeline_item(note),
    };
    let item = serde_json::to_value(item).unwrap();
    Ok(match caller {
        // A window has the note open and knows where it sent it. A control
        // caller is told: the branch the note landed on and the repos the
        // transfer added.
        DispatchCaller::Control(_) => control::branch_note_answer(store, item, &branch_id, added),
        DispatchCaller::Web => item,
    })
}

/// `move_project_note_to_project` and `copy_project_note_to_project`.
/// `add_repo` is as in [`branch_note`], and runs once for each repo a moved
/// note's children need.
pub(super) async fn project_note<F, Fut>(
    store: &Store,
    command: &str,
    args: &Value,
    caller: &DispatchCaller,
    add_repo: F,
) -> Result<Value, String>
where
    F: Fn(String, RepoKey) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let project_note_id: String = arg(args, "projectNoteId")?;
    let target_project_id: String = arg(args, "targetProjectId")?;
    let added = AddedRepos::default();
    // A failure here already names the repos added before it, for every
    // caller: the transfer words that itself.
    let note = transfer_project_note(
        store,
        &project_note_id,
        &target_project_id,
        mode_of(command),
        |key| added.watch(key.clone(), add_repo(target_project_id.clone(), key)),
    )
    .await?;
    Ok(match caller {
        // As for a branch note: a control caller is told which child notes
        // came along, where each landed, and the repos they brought.
        DispatchCaller::Control(_) => control::project_note_answer(store, note, added),
        DispatchCaller::Web => serde_json::to_value(note).unwrap(),
    })
}

#[cfg(test)]
mod tests;
