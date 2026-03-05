//! Git utilities for action execution
//!
//! Provides auto-commit functionality after successful action execution.

use anyhow::{Context, Result};
use std::path::PathBuf;
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

/// Auto-commit changes on a remote workspace via `sq blox ws exec`.
///
/// Runs the equivalent of `git diff --exit-code || (git add -A && git commit -m ...)`
/// inside the remote workspace.
///
/// # Arguments
/// * `sq_binary` - Path to the `sq` CLI binary
/// * `workspace_name` - Name of the Blox workspace
/// * `working_dir` - Resolved path inside the workspace (used as `cd` target)
/// * `action_name` - Name of the action (used in commit message)
///
/// # Returns
/// `Ok(true)` if a commit was created, `Ok(false)` if no changes existed, or an error
pub fn auto_commit_if_changes_remote(
    sq_binary: &PathBuf,
    workspace_name: &str,
    working_dir: &str,
    action_name: &str,
) -> Result<bool> {
    let commit_message = format!("chore: {}", action_name);
    // Shell command that checks for changes, stages, and commits in one shot.
    // Exit code 0 from `git diff --exit-code` means no changes — we echo
    // "no-changes" so we can distinguish that case from a commit.
    let shell_command = format!(
        "cd '{}' && if git diff --exit-code > /dev/null 2>&1; then echo no-changes; else git add -A && git commit -m '{}'; fi",
        working_dir.replace('\'', "'\\''"),
        commit_message.replace('\'', "'\\''"),
    );

    let output = Command::new(sq_binary)
        .args([
            "blox",
            "ws",
            "exec",
            workspace_name,
            "--",
            "sh",
            "-lc",
            &shell_command,
        ])
        .output()
        .context("Failed to run remote auto-commit via sq blox ws exec")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Remote auto-commit failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(!stdout.contains("no-changes"))
}
