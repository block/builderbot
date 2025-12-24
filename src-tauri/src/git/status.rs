//! Git status operations

use super::repo::{find_repo, get_branch_name};
use super::GitError;
use git2::{Status, StatusOptions};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStatus {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatus {
    pub staged: Vec<FileStatus>,
    pub unstaged: Vec<FileStatus>,
    pub untracked: Vec<FileStatus>,
    pub branch: Option<String>,
    pub repo_path: String,
}

/// Convert git2 status flags to a human-readable status string
fn status_to_string(status: Status, staged: bool) -> &'static str {
    if staged {
        if status.contains(Status::INDEX_NEW) {
            "added"
        } else if status.contains(Status::INDEX_MODIFIED) {
            "modified"
        } else if status.contains(Status::INDEX_DELETED) {
            "deleted"
        } else if status.contains(Status::INDEX_RENAMED) {
            "renamed"
        } else if status.contains(Status::INDEX_TYPECHANGE) {
            "typechange"
        } else {
            "unknown"
        }
    } else if status.contains(Status::WT_NEW) {
        "untracked"
    } else if status.contains(Status::WT_MODIFIED) {
        "modified"
    } else if status.contains(Status::WT_DELETED) {
        "deleted"
    } else if status.contains(Status::WT_RENAMED) {
        "renamed"
    } else if status.contains(Status::WT_TYPECHANGE) {
        "typechange"
    } else {
        "unknown"
    }
}

/// Get the full git status for a repository
pub fn get_status(repo_path: Option<&str>) -> Result<GitStatus, GitError> {
    let repo = find_repo(repo_path)?;
    let repo_root = repo
        .workdir()
        .ok_or_else(|| GitError {
            message: "Repository has no working directory".to_string(),
        })?
        .to_string_lossy()
        .to_string();

    let branch = get_branch_name(&repo);

    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);

    let statuses = repo.statuses(Some(&mut opts))?;

    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();

    for entry in statuses.iter() {
        let path = entry.path().unwrap_or("").to_string();
        let status = entry.status();

        // Check for staged changes (index)
        if status.intersects(
            Status::INDEX_NEW
                | Status::INDEX_MODIFIED
                | Status::INDEX_DELETED
                | Status::INDEX_RENAMED
                | Status::INDEX_TYPECHANGE,
        ) {
            staged.push(FileStatus {
                path: path.clone(),
                status: status_to_string(status, true).to_string(),
            });
        }

        // Check for unstaged changes (working tree)
        if status.intersects(
            Status::WT_MODIFIED | Status::WT_DELETED | Status::WT_RENAMED | Status::WT_TYPECHANGE,
        ) {
            unstaged.push(FileStatus {
                path: path.clone(),
                status: status_to_string(status, false).to_string(),
            });
        }

        // Check for untracked files
        if status.contains(Status::WT_NEW) {
            untracked.push(FileStatus {
                path,
                status: "untracked".to_string(),
            });
        }
    }

    Ok(GitStatus {
        staged,
        unstaged,
        untracked,
        branch,
        repo_path: repo_root,
    })
}
