//! One transfer per project, and the check every conflicting path makes.
//!
//! A transfer runs for minutes against a project whose rows still describe
//! the source, so anything that would start work on those branches, or
//! change which branches and repos the project has, refuses while it runs:
//! session starts, resumes and queue drains, pipelines, worktree and
//! workspace setup, and branch and repo edits. Each of those calls
//! [`ensure_branch_not_transferring`] or [`ensure_project_not_transferring`];
//! session starts do it under the branch's launch lock, which the transfer
//! takes after registering here, so a start either landed before the
//! transfer looked or sees it.
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

    /// Add `branch_ids` to the branches this transfer guards: those the
    /// transfer found, which a branch created just before it registered
    /// may have joined.
    pub fn cover_branches<'a>(&self, branch_ids: impl IntoIterator<Item = &'a str>) {
        let mut transfers = transfers().lock().unwrap();
        let Some(entry) = transfers
            .get_mut(&self.project_id)
            .filter(|entry| entry.transfer_id == self.transfer_id)
        else {
            return;
        };
        for id in branch_ids {
            if !entry.branch_ids.iter().any(|known| known == id) {
                entry.branch_ids.push(id.to_string());
            }
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

        drop(guard);
        assert!(!is_project_transferring(&project));
        assert!(!is_branch_transferring(&branch));
        assert!(ensure_project_not_transferring(&project).is_ok());
        assert!(!cancel(&project));
    }
}
