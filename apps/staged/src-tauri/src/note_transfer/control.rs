//! What a control caller (`stagedctl note move`, `stagedctl note copy`) is
//! told about a transfer.
//!
//! The app's windows open a note where it is and know its surroundings, so
//! the transfer commands answer them with the note alone. A control caller
//! named a note by id and a project, and is told what the transfer did with
//! them: the branch the note landed on, and the repos the transfer added to
//! the destination to make room for it. Only the transfer knows the second.
//! Once added, a repo reads like any other the project tracks, so each one
//! is noted as its add succeeds ([`AddedRepos`]).
//!
//! Nothing here decides anything about a transfer. Every caller runs the
//! same [`transfer_branch_note`](super::transfer_branch_note) or
//! [`transfer_project_note`](super::transfer_project_note) with the same
//! repo-adding step. This module watches that step and words the answer.

use std::future::Future;
use std::sync::{Mutex, PoisonError};

use serde_json::{json, Value};

use super::RepoKey;
use crate::store::Store;

/// The repos one transfer added to its destination, in the order it added
/// them.
#[derive(Debug, Default)]
pub(crate) struct AddedRepos(Mutex<Vec<RepoKey>>);

impl AddedRepos {
    /// Run `add`, the transfer's repo-adding step for `key`, and note the
    /// repo once it is added. A failed add is the transfer's to report and
    /// is not noted.
    pub(crate) async fn watch<Fut>(&self, key: RepoKey, add: Fut) -> Result<(), String>
    where
        Fut: Future<Output = Result<(), String>>,
    {
        add.await?;
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(key);
        Ok(())
    }

    /// `[{ githubRepo, subpath }]`, the names `list_project_repos` uses.
    fn into_json(self) -> Value {
        self.0
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
            .into_iter()
            .map(|key| json!({ "githubRepo": key.github_repo, "subpath": key.subpath }))
            .collect()
    }
}

/// A control caller's answer for a branch note: `note`, the value the UI
/// gets; `branch`, the branch it landed on, as `list_branches_for_project`
/// lists one (without the commit count only that list fills in); `repo`,
/// the repo that branch tracks, as `list_project_repos` lists one; and
/// `addedRepos`.
///
/// The transfer has committed by the time this runs. A row that cannot be
/// read back is answered as `null`: the note is where it was sent, and an
/// error here would say it is not.
pub(crate) fn branch_note_answer(
    store: &Store,
    note: Value,
    branch_id: &str,
    added: AddedRepos,
) -> Value {
    let branch = store.get_branch(branch_id).unwrap_or_else(|e| {
        log::warn!("Note transfer: cannot read back branch {branch_id}: {e}");
        None
    });
    let repo = branch
        .as_ref()
        .and_then(|branch| branch.project_repo_id.as_deref())
        .and_then(|repo_id| {
            store.get_project_repo(repo_id).unwrap_or_else(|e| {
                log::warn!("Note transfer: cannot read back repo {repo_id}: {e}");
                None
            })
        });
    let branch = branch.map(|branch| {
        let workdir = store
            .get_workdir_for_branch(&branch.id)
            .unwrap_or_else(|e| {
                log::warn!(
                    "Note transfer: cannot read back the workdir of branch {branch_id}: {e}"
                );
                None
            });
        crate::branches::to_branch_with_workdir(branch, workdir.map(|workdir| workdir.path))
    });
    json!({
        "note": note,
        "branch": branch,
        "repo": repo,
        "addedRepos": added.into_json(),
    })
}

/// A control caller's answer for a project note: `note`, the value the UI
/// gets, which names the project it is in now, and `addedRepos`, the repos
/// its child notes brought along.
pub(crate) fn project_note_answer(note: Value, added: AddedRepos) -> Value {
    json!({ "note": note, "addedRepos": added.into_json() })
}

#[cfg(test)]
mod tests;
