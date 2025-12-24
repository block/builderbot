//! Commit operations

use super::repo::find_repo;
use super::GitError;
use serde::{Deserialize, Serialize};

/// Result of a commit operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitResult {
    pub oid: String,
    pub message: String,
}

/// Get the last commit message (for amend UI)
pub fn get_last_commit_message(repo_path: Option<&str>) -> Result<Option<String>, GitError> {
    let repo = find_repo(repo_path)?;

    let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());

    Ok(head.map(|c| c.message().unwrap_or("").to_string()))
}

/// Create a new commit with the staged changes
pub fn create_commit(repo_path: Option<&str>, message: &str) -> Result<CommitResult, GitError> {
    let repo = find_repo(repo_path)?;

    // Validate message
    let message = message.trim();
    if message.is_empty() {
        return Err(GitError {
            message: "Commit message cannot be empty".to_string(),
        });
    }

    // Get the index (staged changes)
    let mut index = repo.index()?;

    // Check if there are staged changes
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let diff = repo.diff_tree_to_index(head_tree.as_ref(), Some(&index), None)?;
    if diff.deltas().count() == 0 {
        return Err(GitError {
            message: "No staged changes to commit".to_string(),
        });
    }

    // Write the index as a tree
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;

    // Get signature (author and committer)
    let signature = repo.signature().map_err(|e| GitError {
        message: format!(
            "Failed to get git signature. Configure user.name and user.email: {}",
            e
        ),
    })?;

    // Get parent commit (if any)
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();

    // Create the commit
    let oid = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parents,
    )?;

    Ok(CommitResult {
        oid: oid.to_string(),
        message: message.to_string(),
    })
}

/// Amend the last commit with staged changes and/or new message
pub fn amend_commit(repo_path: Option<&str>, message: &str) -> Result<CommitResult, GitError> {
    let repo = find_repo(repo_path)?;

    // Validate message
    let message = message.trim();
    if message.is_empty() {
        return Err(GitError {
            message: "Commit message cannot be empty".to_string(),
        });
    }

    // Get HEAD commit to amend
    let head = repo
        .head()
        .map_err(|_| GitError {
            message: "No commits to amend".to_string(),
        })?
        .peel_to_commit()
        .map_err(|_| GitError {
            message: "HEAD is not a commit".to_string(),
        })?;

    // Get the index and write as tree
    let mut index = repo.index()?;
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;

    // Get signature
    let signature = repo.signature().map_err(|e| GitError {
        message: format!(
            "Failed to get git signature. Configure user.name and user.email: {}",
            e
        ),
    })?;

    // Amend the commit
    let oid = head.amend(
        Some("HEAD"),
        Some(&signature),
        Some(&signature),
        None, // encoding
        Some(message),
        Some(&tree),
    )?;

    Ok(CommitResult {
        oid: oid.to_string(),
        message: message.to_string(),
    })
}
