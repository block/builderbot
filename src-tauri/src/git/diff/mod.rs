//! Diff operations for side-by-side viewing.
//!
//! This module generates diff data optimized for a two-pane diff viewer:
//! - Full file content for both sides (not just hunks)
//! - Range mappings for scroll synchronization
//! - Line-level change classification
//!
//! ## Module Structure
//! - `parse`: Extracts hunks from git2's callback-based diff API
//! - `side_by_side`: Transforms hunks into aligned pane content with ranges

mod parse;
mod side_by_side;

use super::repo::find_repo;
use super::GitError;
use git2::{DiffOptions, Repository};
use serde::{Deserialize, Serialize};

// Re-export for external use
pub use parse::DiffHunk;
pub use parse::HunkLine;

/// A single line in a diff pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffLine {
    /// "context", "added", or "removed"
    pub line_type: String,
    /// 1-indexed line number in the source file
    pub lineno: u32,
    /// Line content (without trailing newline)
    pub content: String,
}

/// Half-open interval [start, end) of row indices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// Maps corresponding regions between before/after panes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Range {
    pub before: Span,
    pub after: Span,
    /// true = region contains changes, false = identical lines
    pub changed: bool,
}

/// Content for one side of the diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffSide {
    pub path: Option<String>,
    pub lines: Vec<DiffLine>,
}

/// Complete diff for a file, ready for side-by-side display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    pub status: String,
    pub is_binary: bool,
    pub hunks: Vec<DiffHunk>,
    pub before: DiffSide,
    pub after: DiffSide,
    /// Range mappings for scroll sync and visual connectors
    pub ranges: Vec<Range>,
}

/// Get diff for a specific file.
///
/// # Arguments
/// * `repo_path` - Optional path to repository (uses discovery if None)
/// * `file_path` - Path to file relative to repo root
/// * `staged` - If true, compare HEAD→index; if false, compare index→workdir
pub fn get_file_diff(
    repo_path: Option<&str>,
    file_path: &str,
    staged: bool,
) -> Result<FileDiff, GitError> {
    let repo = find_repo(repo_path)?;

    let mut diff_opts = DiffOptions::new();
    diff_opts.pathspec(file_path);
    diff_opts.context_lines(0); // We show full file, don't need context from git

    let diff = if staged {
        // Staged: compare HEAD to index
        let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
        repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut diff_opts))?
    } else {
        // Unstaged: compare index to working directory
        repo.diff_index_to_workdir(None, Some(&mut diff_opts))?
    };

    // Get full file contents for both sides
    let before_content = get_before_content(&repo, file_path, staged)?;
    let after_content = get_after_content(&repo, file_path, staged)?;

    // Determine paths
    let before_path = if before_content.is_some() {
        Some(file_path.to_string())
    } else {
        None
    };
    let after_path = if after_content.is_some() {
        Some(file_path.to_string())
    } else {
        None
    };

    // Parse hunks from git2 diff
    let parse_result = parse::parse_diff(&diff, file_path)?;

    if parse_result.is_binary {
        return Ok(FileDiff {
            status: parse_result.status,
            is_binary: true,
            hunks: vec![],
            before: DiffSide {
                path: parse_result.renamed_from.or(before_path),
                lines: vec![],
            },
            after: DiffSide {
                path: after_path,
                lines: vec![],
            },
            ranges: vec![],
        });
    }

    // Build side-by-side content and ranges
    let (before_lines, after_lines, ranges) =
        side_by_side::build(&before_content, &after_content, &parse_result.hunks);

    Ok(FileDiff {
        status: parse_result.status,
        is_binary: false,
        hunks: parse_result.hunks,
        before: DiffSide {
            path: parse_result.renamed_from.or(before_path),
            lines: before_lines,
        },
        after: DiffSide {
            path: after_path,
            lines: after_lines,
        },
        ranges,
    })
}

/// Get diff for an untracked file (show entire file as added).
pub fn get_untracked_file_diff(
    repo_path: Option<&str>,
    file_path: &str,
) -> Result<FileDiff, GitError> {
    let repo = find_repo(repo_path)?;
    let workdir = repo.workdir().ok_or_else(|| GitError {
        message: "Repository has no working directory".to_string(),
    })?;

    let full_path = workdir.join(file_path);

    // Read as bytes first to detect binary files
    let bytes = std::fs::read(&full_path).map_err(|e| GitError {
        message: format!("Failed to read file: {}", e),
    })?;

    // Check if binary (contains null bytes)
    if is_binary_content(&bytes) {
        return Ok(FileDiff {
            status: "untracked".to_string(),
            is_binary: true,
            hunks: vec![],
            before: DiffSide {
                path: None,
                lines: vec![],
            },
            after: DiffSide {
                path: Some(file_path.to_string()),
                lines: vec![],
            },
            ranges: vec![],
        });
    }

    // Try to convert to UTF-8
    let content = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => {
            // Not valid UTF-8, treat as binary
            return Ok(FileDiff {
                status: "untracked".to_string(),
                is_binary: true,
                hunks: vec![],
                before: DiffSide {
                    path: None,
                    lines: vec![],
                },
                after: DiffSide {
                    path: Some(file_path.to_string()),
                    lines: vec![],
                },
                ranges: vec![],
            });
        }
    };

    let after_lines: Vec<DiffLine> = content
        .lines()
        .enumerate()
        .map(|(i, line)| DiffLine {
            line_type: "added".to_string(),
            lineno: (i + 1) as u32,
            content: line.to_string(),
        })
        .collect();

    let line_count = after_lines.len();

    // Single range: empty before, all lines in after
    let ranges = vec![Range {
        before: Span { start: 0, end: 0 },
        after: Span {
            start: 0,
            end: line_count,
        },
        changed: true,
    }];

    Ok(FileDiff {
        status: "untracked".to_string(),
        is_binary: false,
        hunks: vec![DiffHunk {
            old_start: 0,
            old_lines: 0,
            new_start: 1,
            new_lines: line_count as u32,
            header: format!("@@ -0,0 +1,{} @@", line_count),
            lines: after_lines
                .iter()
                .map(|l| HunkLine {
                    line_type: l.line_type.clone(),
                    old_lineno: None,
                    new_lineno: Some(l.lineno),
                    content: l.content.clone(),
                })
                .collect(),
        }],
        before: DiffSide {
            path: None,
            lines: vec![],
        },
        after: DiffSide {
            path: Some(file_path.to_string()),
            lines: after_lines,
        },
        ranges,
    })
}

// =============================================================================
// File Content Helpers
// =============================================================================

/// Get the "before" file content (what we're comparing from).
/// - For staged diffs: content from HEAD
/// - For unstaged diffs: content from index
fn get_before_content(
    repo: &Repository,
    file_path: &str,
    staged: bool,
) -> Result<Option<String>, GitError> {
    if staged {
        get_content_from_head(repo, file_path)
    } else {
        get_content_from_index(repo, file_path)
    }
}

/// Get the "after" file content (what we're comparing to).
/// - For staged diffs: content from index
/// - For unstaged diffs: content from working directory
fn get_after_content(
    repo: &Repository,
    file_path: &str,
    staged: bool,
) -> Result<Option<String>, GitError> {
    if staged {
        get_content_from_index(repo, file_path)
    } else {
        get_content_from_workdir(repo, file_path)
    }
}

fn get_content_from_head(repo: &Repository, file_path: &str) -> Result<Option<String>, GitError> {
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return Ok(None), // No HEAD (initial commit)
    };
    let tree = head.peel_to_tree().map_err(|e| GitError {
        message: format!("Failed to get HEAD tree: {}", e),
    })?;
    let entry = match tree.get_path(std::path::Path::new(file_path)) {
        Ok(e) => e,
        Err(_) => return Ok(None), // File doesn't exist in HEAD (new file)
    };
    let blob = repo.find_blob(entry.id()).map_err(|e| GitError {
        message: format!("Failed to get blob: {}", e),
    })?;
    if blob.is_binary() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
}

fn get_content_from_index(repo: &Repository, file_path: &str) -> Result<Option<String>, GitError> {
    let index = repo.index().map_err(|e| GitError {
        message: format!("Failed to get index: {}", e),
    })?;
    let entry = match index.get_path(std::path::Path::new(file_path), 0) {
        Some(e) => e,
        None => return Ok(None), // File not in index
    };
    let blob = repo.find_blob(entry.id).map_err(|e| GitError {
        message: format!("Failed to get blob: {}", e),
    })?;
    if blob.is_binary() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
}

fn get_content_from_workdir(
    repo: &Repository,
    file_path: &str,
) -> Result<Option<String>, GitError> {
    let workdir = repo.workdir().ok_or_else(|| GitError {
        message: "Repository has no working directory".to_string(),
    })?;
    let full_path = workdir.join(file_path);
    match std::fs::read_to_string(&full_path) {
        Ok(content) => Ok(Some(content)),
        Err(_) => Ok(None), // File deleted from working directory
    }
}

/// Check if bytes appear to be binary content (contains null bytes).
fn is_binary_content(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}
