//! What a control caller (`stagedctl note move`, `stagedctl note copy`) is
//! told about a transfer.
//!
//! The app's windows open a note where it is and know its surroundings, so
//! the transfer commands answer them with the note alone. A control caller
//! named a note by id and a project, and is told what the transfer did with
//! them: the branch the note landed on, the child notes a moved project
//! note took along and where each of those landed, and the repos the
//! transfer added to the destination to make room. Only the transfer knows
//! the last. Once added, a repo reads like any other the project tracks, so
//! each one is noted as its add succeeds ([`AddedRepos`]). The same list
//! lets a failure say which repos the destination gained before it
//! ([`AddedRepos::explain`]).
//!
//! Nothing here decides anything about a transfer. Every caller runs the
//! same [`transfer_branch_note`](super::transfer_branch_note) or
//! [`transfer_project_note`](super::transfer_project_note) with the same
//! repo-adding step. This module watches that step and words the answer,
//! from rows it reads back once the transfer has committed.

use std::future::Future;
use std::sync::{Mutex, PoisonError};

use serde_json::{json, Value};

use super::RepoKey;
use crate::store::models::ProjectNote;
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

    /// A branch-note transfer failed with `err`. When it had already added
    /// a repo to `target_project_id`, the repo stays tracked there, and the
    /// error says so in the words a project-note failure uses
    /// ([`with_added_repos`](super::with_added_repos)). The transfer words
    /// that sentence itself for a project note, so this is not for one.
    ///
    /// A failed add is not noted, so its error comes back as it is.
    pub(crate) fn explain(&self, store: &Store, target_project_id: &str, err: String) -> String {
        let added = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if added.is_empty() {
            return err;
        }
        match store.get_project(target_project_id) {
            Ok(Some(target)) => super::with_added_repos(err, &added, &target),
            Ok(None) => err,
            Err(e) => {
                log::warn!("Note transfer: cannot read back project {target_project_id}: {e}");
                err
            }
        }
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
    let (branch, repo) = landing(store, branch_id);
    json!({
        "note": note,
        "branch": branch,
        "repo": repo,
        "addedRepos": added.into_json(),
    })
}

/// A control caller's answer for a project note: `note`, the note as
/// `list_project_notes` lists one, which names the project it is in now;
/// `children`, the child notes that came with it; and `addedRepos`, the
/// repos those children brought along.
///
/// `note` is the transfer's own value, which a window is answered with as
/// it is. A moved note's is the row the move's transaction returned, with
/// no session status resolved: `sessionStatus` would read `null` for a note
/// whose session finished. So the note is read again here, the way the
/// project's note list reads it. If that read fails the transfer's value
/// stands, as the rows of a branch note's answer do.
///
/// Each of `children` is `{ reference, title, branch, repo }`: the child's
/// hashtag reference (`#note:<id>`, unchanged by the move), and the branch
/// and repo it is on now, as in a branch note's answer. A move takes every
/// child or is refused, so these are the note's children as they read
/// after it. A copy has none.
pub(crate) fn project_note_answer(store: &Store, note: ProjectNote, added: AddedRepos) -> Value {
    let note = match store.get_project_note_with_status(&note.id) {
        Ok(Some(resolved)) => resolved,
        Ok(None) => note,
        Err(e) => {
            log::warn!(
                "Note transfer: cannot read back project note {}: {e}",
                note.id
            );
            note
        }
    };
    let children: Vec<Value> = store
        .list_child_notes(&note.id)
        .unwrap_or_else(|e| {
            log::warn!(
                "Note transfer: cannot read back the child notes of {}: {e}",
                note.id
            );
            Vec::new()
        })
        .into_iter()
        .map(|child| {
            let (branch, repo) = landing(store, &child.branch_id);
            json!({
                "reference": format!("#note:{}", child.id),
                "title": child.title,
                "branch": branch,
                "repo": repo,
            })
        })
        .collect();
    json!({
        "note": note,
        "children": children,
        "addedRepos": added.into_json(),
    })
}

/// The branch a note is on and the repo that branch tracks, as the answers
/// give them. Either is `null` when its row cannot be read back.
fn landing(store: &Store, branch_id: &str) -> (Value, Value) {
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
    (json!(branch), json!(repo))
}

#[cfg(test)]
mod tests;
