//! Git utilities for action execution
//!
//! Provides auto-commit functionality after successful action execution.

use anyhow::{Context, Result};
use std::process::Command;

/// Auto-commit changes if any exist in the working directory
///
/// # Arguments
/// * `worktree_path` - Path to the git worktree
/// * `action_name` - Name of the action (used in commit message)
///
/// # Returns
/// `Ok(true)` if a commit was created, `Ok(false)` if no changes existed, or an error
pub fn auto_commit_if_changes(worktree_path: &str, action_name: &str) -> Result<bool> {
    // Check if there are any changes
    let status = Command::new("git")
        .arg("diff")
        .arg("--exit-code")
        .current_dir(worktree_path)
        .status()
        .context("Failed to check git status")?;

    // If exit code is 0, no changes exist
    if status.success() {
        return Ok(false);
    }

    // Stage all changes
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(worktree_path)
        .status()
        .context("Failed to stage changes")?;

    // Commit with action name
    let commit_message = format!("chore: {}", action_name);
    Command::new("git")
        .args(["commit", "-m", &commit_message])
        .current_dir(worktree_path)
        .status()
        .context("Failed to commit changes")?;

    Ok(true)
}
