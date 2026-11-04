//! One transfer per project, and the check every conflicting path makes.
//!
//! A transfer runs for minutes against a project whose rows still describe
//! the source, so anything that would start work on those branches, or
//! change which branches and repos the project has, refuses while it runs:
//! session starts, resumes and queue drains, actions and pipelines, worktree
//! and workspace setup, resets and discards, workspace resumes and deletes,
//! and branch and repo edits. Each of those calls
//! [`ensure_branch_not_transferring`], [`ensure_workspace_not_transferring`]
//! or [`ensure_project_not_transferring`]; session starts do it under the
//! branch's launch lock, which the transfer takes after registering here, so
//! a start either landed before the transfer looked or sees it.
//!
//! Once the database has switched, [`TransferGuard::release_branches`] lets
//! the branches go — their rows describe the destination, which setup may
//! need to provision — while the project stays registered until the
//! teardown is done.
//!
//! In-memory and per process: a second Staged instance on the same database
//! cannot see it (the store's running-session check is the backstop there).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// What a refused path reports.
pub const TRANSFER_IN_PROGRESS: &str =
    "Project is being transferred; try again once the transfer finishes.";

struct Entry {
    transfer_id: String,
    branch_ids: Vec<String>,
    /// Source workspaces, for the paths that name a workspace, not a branch.
    workspaces: Vec<String>,
    cancel: Arc<AtomicBool>,
}

fn transfers() -> &'static Mutex<HashMap<String, Entry>> {
    static TRANSFERS: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    TRANSFERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A project's registration, released on drop.
pub struct TransferGuard {
    project_id: String,
    transfer_id: String,
    cancel: Arc<AtomicBool>,
}

impl TransferGuard {
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn transfer_id(&self) -> &str {
        &self.transfer_id
    }

    /// Whether [`cancel`] has been asked for this transfer.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn with_entry(&self, f: impl FnOnce(&mut Entry)) {
        let mut transfers = transfers().lock().unwrap();
        if let Some(entry) = transfers
            .get_mut(&self.project_id)
            .filter(|entry| entry.transfer_id == self.transfer_id)
        {
            f(entry);
        }
    }

    /// Add `branch_ids` to the branches this transfer guards: those the
    /// transfer found, which a branch created just before it registered
    /// may have joined.
    pub fn cover_branches<'a>(&self, branch_ids: impl IntoIterator<Item = &'a str>) {
        self.with_entry(|entry| add_missing(&mut entry.branch_ids, branch_ids));
    }

    /// Add `workspaces` to the workspaces this transfer guards.
    pub fn cover_workspaces<'a>(&self, workspaces: impl IntoIterator<Item = &'a str>) {
        self.with_entry(|entry| add_missing(&mut entry.workspaces, workspaces));
    }

    /// Stop guarding the branches and workspaces, once the database has
    /// switched: the rows now describe the destination, and nothing may
    /// refuse to provision them. The project stays registered until drop.
    pub fn release_branches(&self) {
        self.with_entry(|entry| {
            entry.branch_ids.clear();
            entry.workspaces.clear();
        });
    }
}

fn add_missing<'a>(known: &mut Vec<String>, items: impl IntoIterator<Item = &'a str>) {
    for item in items {
        if !known.iter().any(|k| k == item) {
            known.push(item.to_string());
        }
    }
}

impl Drop for TransferGuard {
    fn drop(&mut self) {
        let mut transfers = transfers().lock().unwrap();
        if transfers
            .get(&self.project_id)
            .is_some_and(|entry| entry.transfer_id == self.transfer_id)
        {
            transfers.remove(&self.project_id);
        }
    }
}

/// Register a transfer of `project_id` covering `branch_ids`, or refuse if
/// one is already running.
pub fn begin(
    project_id: &str,
    transfer_id: &str,
    branch_ids: Vec<String>,
) -> Result<TransferGuard, String> {
    let mut transfers = transfers().lock().unwrap();
    if transfers.contains_key(project_id) {
        return Err("This project is already being transferred.".to_string());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    transfers.insert(
        project_id.to_string(),
        Entry {
            transfer_id: transfer_id.to_string(),
            branch_ids,
            workspaces: Vec::new(),
            cancel: Arc::clone(&cancel),
        },
    );
    Ok(TransferGuard {
        project_id: project_id.to_string(),
        transfer_id: transfer_id.to_string(),
        cancel,
    })
}

pub fn is_project_transferring(project_id: &str) -> bool {
    transfers().lock().unwrap().contains_key(project_id)
}

pub fn is_branch_transferring(branch_id: &str) -> bool {
    transfers()
        .lock()
        .unwrap()
        .values()
        .any(|entry| entry.branch_ids.iter().any(|id| id == branch_id))
}

pub fn is_workspace_transferring(workspace: &str) -> bool {
    transfers()
        .lock()
        .unwrap()
        .values()
        .any(|entry| entry.workspaces.iter().any(|w| w == workspace))
}

/// Ask a running transfer to stop at its next checkpoint. Only the steps
/// before the database switches can be cancelled; returns whether a
/// transfer was running.
pub fn cancel(project_id: &str) -> bool {
    match transfers().lock().unwrap().get(project_id) {
        Some(entry) => {
            entry.cancel.store(true, Ordering::SeqCst);
            true
        }
        None => false,
    }
}

pub fn ensure_project_not_transferring(project_id: &str) -> Result<(), String> {
    if is_project_transferring(project_id) {
        Err(TRANSFER_IN_PROGRESS.to_string())
    } else {
        Ok(())
    }
}

pub fn ensure_branch_not_transferring(branch_id: &str) -> Result<(), String> {
    if is_branch_transferring(branch_id) {
        Err(TRANSFER_IN_PROGRESS.to_string())
    } else {
        Ok(())
    }
}

pub fn ensure_workspace_not_transferring(workspace: &str) -> Result<(), String> {
    if is_workspace_transferring(workspace) {
        Err(TRANSFER_IN_PROGRESS.to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_transfer_per_project_released_on_drop() {
        let project = uuid::Uuid::new_v4().to_string();
        let branch = uuid::Uuid::new_v4().to_string();
        let guard = begin(&project, "t1", vec![branch.clone()]).unwrap();
        assert!(is_project_transferring(&project));
        assert!(is_branch_transferring(&branch));
        assert!(begin(&project, "t2", vec![]).is_err());
        assert_eq!(
            ensure_branch_not_transferring(&branch).unwrap_err(),
            TRANSFER_IN_PROGRESS
        );

        let late = uuid::Uuid::new_v4().to_string();
        assert!(!is_branch_transferring(&late));
        guard.cover_branches([late.as_str(), branch.as_str()]);
        assert!(is_branch_transferring(&late));

        assert!(!guard.is_cancelled());
        assert!(cancel(&project));
        assert!(guard.is_cancelled());

        let workspace = format!("ws-{project}");
        assert!(ensure_workspace_not_transferring(&workspace).is_ok());
        guard.cover_workspaces([workspace.as_str()]);
        assert_eq!(
            ensure_workspace_not_transferring(&workspace).unwrap_err(),
            TRANSFER_IN_PROGRESS
        );

        drop(guard);
        assert!(!is_project_transferring(&project));
        assert!(!is_workspace_transferring(&workspace));
        assert!(!is_branch_transferring(&branch));
        assert!(ensure_project_not_transferring(&project).is_ok());
        assert!(!cancel(&project));
    }

    #[test]
    fn released_branches_are_free_while_the_project_stays_registered() {
        let project = uuid::Uuid::new_v4().to_string();
        let branch = uuid::Uuid::new_v4().to_string();
        let workspace = format!("ws-{project}");
        let guard = begin(&project, "t1", vec![branch.clone()]).unwrap();
        guard.cover_workspaces([workspace.as_str()]);

        guard.release_branches();

        assert!(ensure_branch_not_transferring(&branch).is_ok());
        assert!(ensure_workspace_not_transferring(&workspace).is_ok());
        assert!(is_project_transferring(&project));
        assert!(begin(&project, "t2", vec![]).is_err());
        drop(guard);
        assert!(!is_project_transferring(&project));
    }
}
