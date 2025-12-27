//! Git status providers with different performance characteristics.
//!
//! This module provides the `StatusProvider` trait and implementations
//! for fetching git status. The `AdaptiveProvider` automatically switches
//! between git2 (fast for small repos) and CLI (uses fsmonitor for huge repos).
//!
//! Easy to swap out for different strategies if needed.

use super::status::{FileStatus, GitStatus};
use super::GitError;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Result of a status fetch, including timing information.
#[derive(Debug, Clone)]
pub struct StatusResult {
    pub status: GitStatus,
    pub duration: Duration,
    pub used_cli: bool,
}

/// Trait for git status providers.
/// Easy to swap implementations for different strategies.
pub trait StatusProvider: Send + Sync {
    /// Get the current git status for a repository.
    fn get_status(&self, repo_path: &Path) -> Result<StatusResult, GitError>;

    /// Reset any adaptive state (e.g., when switching repos).
    fn reset(&self);
}

/// Adaptive provider that uses git2 for small repos and CLI for large ones.
/// Automatically switches to CLI if git2 takes too long (>500ms).
pub struct AdaptiveProvider {
    /// Whether to use CLI instead of git2
    use_cli: AtomicBool,
    /// Threshold in ms above which we switch to CLI
    cli_threshold_ms: u64,
}

impl Default for AdaptiveProvider {
    fn default() -> Self {
        Self::new(500) // 500ms threshold
    }
}

impl AdaptiveProvider {
    pub fn new(cli_threshold_ms: u64) -> Self {
        Self {
            use_cli: AtomicBool::new(false),
            cli_threshold_ms,
        }
    }

    /// Get status using git2 (libgit2)
    fn get_status_git2(&self, repo_path: &Path) -> Result<GitStatus, GitError> {
        super::get_status(Some(repo_path.to_string_lossy().as_ref()))
    }

    /// Get status using git CLI (can leverage fsmonitor)
    fn get_status_cli(&self, repo_path: &Path) -> Result<GitStatus, GitError> {
        // Get branch name
        let branch = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(repo_path)
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout)
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                } else {
                    None
                }
            });

        // Get porcelain status
        // -uall lists individual untracked files (not collapsed directories)
        // to match git2's recurse_untracked_dirs(true) behavior
        let output = Command::new("git")
            .args(["status", "--porcelain", "-uall"])
            .current_dir(repo_path)
            .output()
            .map_err(|e| GitError {
                message: format!("Failed to run git status: {}", e),
            })?;

        if !output.status.success() {
            return Err(GitError {
                message: format!(
                    "git status failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let (staged, unstaged, untracked) = parse_porcelain_status(&stdout);

        Ok(GitStatus {
            staged,
            unstaged,
            untracked,
            branch,
            repo_path: repo_path.to_string_lossy().to_string(),
        })
    }
}

impl StatusProvider for AdaptiveProvider {
    fn get_status(&self, repo_path: &Path) -> Result<StatusResult, GitError> {
        let use_cli = self.use_cli.load(Ordering::Relaxed);
        let start = Instant::now();

        let status = if use_cli {
            self.get_status_cli(repo_path)?
        } else {
            let result = self.get_status_git2(repo_path)?;
            let duration = start.elapsed();

            // Switch to CLI if git2 is too slow
            if duration.as_millis() > self.cli_threshold_ms as u128 {
                log::info!(
                    "git2 took {}ms, switching to CLI for future calls",
                    duration.as_millis()
                );
                self.use_cli.store(true, Ordering::Relaxed);
            }

            result
        };

        let duration = start.elapsed();

        Ok(StatusResult {
            status,
            duration,
            used_cli: use_cli,
        })
    }

    fn reset(&self) {
        self.use_cli.store(false, Ordering::Relaxed);
    }
}

/// Parse git status --porcelain output into categorized file lists.
///
/// Porcelain format: XY PATH
/// - X = index status (staged)
/// - Y = worktree status (unstaged)
/// - ' ' = unmodified
/// - M = modified
/// - A = added
/// - D = deleted
/// - R = renamed
/// - ? = untracked
fn parse_porcelain_status(output: &str) -> (Vec<FileStatus>, Vec<FileStatus>, Vec<FileStatus>) {
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();

    for line in output.lines() {
        if line.len() < 3 {
            continue;
        }

        let index_status = line.chars().next().unwrap_or(' ');
        let worktree_status = line.chars().nth(1).unwrap_or(' ');
        let path = line[3..].to_string();

        // Handle renames (format: "R  old -> new" or "R  new\0old")
        let path = if path.contains(" -> ") {
            path.split(" -> ").last().unwrap_or(&path).to_string()
        } else {
            path
        };

        // Untracked files
        if index_status == '?' {
            untracked.push(FileStatus {
                path,
                status: "untracked".to_string(),
            });
            continue;
        }

        // Staged changes (index status)
        if index_status != ' ' && index_status != '?' {
            staged.push(FileStatus {
                path: path.clone(),
                status: porcelain_char_to_status(index_status),
            });
        }

        // Unstaged changes (worktree status)
        if worktree_status != ' ' && worktree_status != '?' {
            unstaged.push(FileStatus {
                path,
                status: porcelain_char_to_status(worktree_status),
            });
        }
    }

    (staged, unstaged, untracked)
}

fn porcelain_char_to_status(c: char) -> String {
    match c {
        'M' => "modified",
        'A' => "added",
        'D' => "deleted",
        'R' => "renamed",
        'C' => "copied",
        'T' => "typechange",
        'U' => "unmerged",
        _ => "unknown",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_porcelain_status() {
        let output = r#"M  src/modified_staged.rs
 M src/modified_unstaged.rs
MM src/both.rs
A  src/added.rs
 D src/deleted.rs
?? src/untracked.rs
"#;

        let (staged, unstaged, untracked) = parse_porcelain_status(output);

        assert_eq!(staged.len(), 3); // M, MM (index), A
        assert_eq!(unstaged.len(), 3); // M (worktree), MM (worktree), D
        assert_eq!(untracked.len(), 1);

        assert!(staged.iter().any(|f| f.path == "src/modified_staged.rs"));
        assert!(staged.iter().any(|f| f.path == "src/added.rs"));
        assert!(unstaged
            .iter()
            .any(|f| f.path == "src/modified_unstaged.rs"));
        assert!(untracked.iter().any(|f| f.path == "src/untracked.rs"));
    }

    #[test]
    fn test_porcelain_char_to_status() {
        assert_eq!(porcelain_char_to_status('M'), "modified");
        assert_eq!(porcelain_char_to_status('A'), "added");
        assert_eq!(porcelain_char_to_status('D'), "deleted");
        assert_eq!(porcelain_char_to_status('?'), "unknown");
    }
}
