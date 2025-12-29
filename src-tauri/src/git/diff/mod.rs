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

/// Special ref representing the working tree (uncommitted changes).
pub const WORKING_TREE_REF: &str = "@";

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

/// Source file line numbers for a changed region.
/// These are 1-indexed line numbers in the original files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceLines {
    /// Lines removed from the "before" file (1-indexed, inclusive range)
    /// None if this is a pure addition
    pub old_start: Option<u32>,
    pub old_end: Option<u32>,
    /// Lines added in the "after" file (1-indexed, inclusive range)
    /// None if this is a pure deletion
    pub new_start: Option<u32>,
    pub new_end: Option<u32>,
}

/// Maps corresponding regions between before/after panes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Range {
    pub before: Span,
    pub after: Span,
    /// true = region contains changes, false = identical lines
    pub changed: bool,
    /// Source file line numbers (only present for changed ranges)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_lines: Option<SourceLines>,
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

// =============================================================================
// Ref-based Diff (base..head)
// =============================================================================

/// Get diff for a file between two refs.
///
/// This is the primary diff function for the review model. It compares any two
/// refs (branches, tags, SHAs) or the working tree ("@").
///
/// # Arguments
/// * `repo_path` - Optional path to repository (uses discovery if None)
/// * `base` - Base ref (branch name, SHA, "HEAD", etc.)
/// * `head` - Head ref (same as base, or "@" for working tree)
/// * `file_path` - Path to file relative to repo root
///
/// # Examples
/// * `get_ref_diff(None, "main", "@", "src/lib.rs")` - Changes from main to working tree
/// * `get_ref_diff(None, "HEAD~1", "HEAD", "src/lib.rs")` - Last commit's changes
/// * `get_ref_diff(None, "v1.0", "v2.0", "src/lib.rs")` - Changes between tags
pub fn get_ref_diff(
    repo_path: Option<&str>,
    base: &str,
    head: &str,
    file_path: &str,
) -> Result<FileDiff, GitError> {
    let repo = find_repo(repo_path)?;

    // Get content from both sides
    let before_content = get_content_from_ref(&repo, base, file_path)?;
    let after_content = get_content_from_ref(&repo, head, file_path)?;

    // Handle case where file doesn't exist on either side
    if before_content.is_none() && after_content.is_none() {
        return Err(GitError {
            message: format!(
                "File '{}' not found in either {} or {}",
                file_path, base, head
            ),
        });
    }

    // Determine status based on presence in each ref
    let status = match (&before_content, &after_content) {
        (None, Some(_)) => "added",
        (Some(_), None) => "deleted",
        (Some(_), Some(_)) => "modified",
        (None, None) => unreachable!(), // Handled above
    };

    // Check for binary content
    if let Some(ref content) = before_content {
        if is_binary_content(content.as_bytes()) {
            return Ok(FileDiff {
                status: status.to_string(),
                is_binary: true,
                hunks: vec![],
                before: DiffSide {
                    path: Some(file_path.to_string()),
                    lines: vec![],
                },
                after: DiffSide {
                    path: Some(file_path.to_string()),
                    lines: vec![],
                },
                ranges: vec![],
            });
        }
    }
    if let Some(ref content) = after_content {
        if is_binary_content(content.as_bytes()) {
            return Ok(FileDiff {
                status: status.to_string(),
                is_binary: true,
                hunks: vec![],
                before: DiffSide {
                    path: Some(file_path.to_string()),
                    lines: vec![],
                },
                after: DiffSide {
                    path: Some(file_path.to_string()),
                    lines: vec![],
                },
                ranges: vec![],
            });
        }
    }

    // For purely added or deleted files, synthesize hunks directly from content
    // rather than using git2's diff (which may not include untracked files)
    let hunks = if before_content.is_none() || after_content.is_none() {
        // Synthesize hunks for added/deleted files
        synthesize_hunks(&before_content, &after_content)
    } else {
        // Use git2 for modified files (has proper rename detection, etc.)
        let mut diff_opts = DiffOptions::new();
        diff_opts.pathspec(file_path);
        diff_opts.context_lines(0);

        let diff = if head == WORKING_TREE_REF {
            // Diff from base tree to working directory (including staged changes)
            let base_tree = resolve_tree(&repo, base)?;
            repo.diff_tree_to_workdir_with_index(Some(&base_tree), Some(&mut diff_opts))?
        } else {
            // Diff between two trees
            let base_tree = resolve_tree(&repo, base)?;
            let head_tree = resolve_tree(&repo, head)?;
            repo.diff_tree_to_tree(Some(&base_tree), Some(&head_tree), Some(&mut diff_opts))?
        };

        // Parse hunks from git2 diff
        let parse_result = parse::parse_diff(&diff, file_path)?;
        parse_result.hunks
    };

    // Build side-by-side content and ranges
    let (before_lines, after_lines, ranges) =
        side_by_side::build(&before_content, &after_content, &hunks);

    Ok(FileDiff {
        status: status.to_string(),
        is_binary: false,
        hunks,
        before: DiffSide {
            path: if before_content.is_some() {
                Some(file_path.to_string())
            } else {
                None
            },
            lines: before_lines,
        },
        after: DiffSide {
            path: if after_content.is_some() {
                Some(file_path.to_string())
            } else {
                None
            },
            lines: after_lines,
        },
        ranges,
    })
}

/// Synthesize hunks for purely added or deleted files.
///
/// When a file is entirely new (before=None) or entirely deleted (after=None),
/// we create a single hunk covering all lines rather than using git2's diff.
fn synthesize_hunks(
    before_content: &Option<String>,
    after_content: &Option<String>,
) -> Vec<DiffHunk> {
    match (before_content, after_content) {
        (None, Some(content)) => {
            // New file - all lines are added
            let lines: Vec<HunkLine> = content
                .lines()
                .enumerate()
                .map(|(i, line)| HunkLine {
                    line_type: "added".to_string(),
                    old_lineno: None,
                    new_lineno: Some((i + 1) as u32),
                    content: line.to_string(),
                })
                .collect();

            let line_count = lines.len() as u32;
            if line_count == 0 {
                return vec![];
            }

            vec![DiffHunk {
                old_start: 0,
                old_lines: 0,
                new_start: 1,
                new_lines: line_count,
                header: format!("@@ -0,0 +1,{} @@", line_count),
                lines,
            }]
        }
        (Some(content), None) => {
            // Deleted file - all lines are removed
            let lines: Vec<HunkLine> = content
                .lines()
                .enumerate()
                .map(|(i, line)| HunkLine {
                    line_type: "removed".to_string(),
                    old_lineno: Some((i + 1) as u32),
                    new_lineno: None,
                    content: line.to_string(),
                })
                .collect();

            let line_count = lines.len() as u32;
            if line_count == 0 {
                return vec![];
            }

            vec![DiffHunk {
                old_start: 1,
                old_lines: line_count,
                new_start: 0,
                new_lines: 0,
                header: format!("@@ -1,{} +0,0 @@", line_count),
                lines,
            }]
        }
        _ => vec![], // Both present or both absent - shouldn't happen
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Resolve a ref string to a tree.
///
/// Handles branch names, tag names, SHAs, and special refs like HEAD.
fn resolve_tree<'a>(repo: &'a Repository, ref_str: &str) -> Result<git2::Tree<'a>, GitError> {
    let obj = repo.revparse_single(ref_str).map_err(|e| GitError {
        message: format!("Failed to resolve ref '{}': {}", ref_str, e),
    })?;

    obj.peel_to_tree().map_err(|e| GitError {
        message: format!("Failed to get tree for '{}': {}", ref_str, e),
    })
}

/// Get file content from a ref (branch, tag, SHA, or "@" for working tree).
fn get_content_from_ref(
    repo: &Repository,
    ref_str: &str,
    file_path: &str,
) -> Result<Option<String>, GitError> {
    if ref_str == WORKING_TREE_REF {
        // Working tree - read from disk
        get_content_from_workdir(repo, file_path)
    } else {
        // Resolve ref to tree and get blob
        let tree = match resolve_tree(repo, ref_str) {
            Ok(t) => t,
            Err(_) => return Ok(None), // Ref doesn't exist
        };

        let entry = match tree.get_path(std::path::Path::new(file_path)) {
            Ok(e) => e,
            Err(_) => return Ok(None), // File doesn't exist in this tree
        };

        let blob = repo.find_blob(entry.id()).map_err(|e| GitError {
            message: format!("Failed to get blob: {}", e),
        })?;

        if blob.is_binary() {
            return Ok(None);
        }

        Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
    }
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

// =============================================================================
// Changed Files List
// =============================================================================

/// A file that changed between two refs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
}

/// A git reference (branch or tag) for autocomplete.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRef {
    pub name: String,
    pub ref_type: String, // "branch", "tag", or "special"
}

/// Get list of refs for autocomplete (branches and tags).
pub fn get_refs(repo_path: Option<&str>) -> Result<Vec<GitRef>, GitError> {
    let repo = find_repo(repo_path)?;
    let mut refs = Vec::new();

    // Add special refs first
    refs.push(GitRef {
        name: "@".to_string(),
        ref_type: "special".to_string(),
    });
    refs.push(GitRef {
        name: "HEAD".to_string(),
        ref_type: "special".to_string(),
    });
    refs.push(GitRef {
        name: "HEAD~1".to_string(),
        ref_type: "special".to_string(),
    });

    // Add branches
    for branch in repo.branches(Some(git2::BranchType::Local))? {
        let (branch, _) = branch?;
        if let Some(name) = branch.name()? {
            refs.push(GitRef {
                name: name.to_string(),
                ref_type: "branch".to_string(),
            });
        }
    }

    // Add tags
    repo.tag_foreach(|_oid, name| {
        if let Ok(name_str) = std::str::from_utf8(name) {
            // Strip "refs/tags/" prefix
            let short_name = name_str.strip_prefix("refs/tags/").unwrap_or(name_str);
            refs.push(GitRef {
                name: short_name.to_string(),
                ref_type: "tag".to_string(),
            });
        }
        true
    })?;

    Ok(refs)
}

/// Resolve a ref to its SHA (for display purposes).
pub fn resolve_ref_to_sha(repo_path: Option<&str>, ref_str: &str) -> Result<String, GitError> {
    if ref_str == WORKING_TREE_REF {
        return Ok("working tree".to_string());
    }

    let repo = find_repo(repo_path)?;
    let obj = repo.revparse_single(ref_str).map_err(|e| GitError {
        message: format!("Failed to resolve ref '{}': {}", ref_str, e),
    })?;

    Ok(obj.id().to_string()[..8].to_string()) // Short SHA
}

/// Get list of files changed between two refs.
///
/// This is used to populate the sidebar when viewing a diff.
/// For working tree diffs (head="@"), this combines staged, unstaged, and untracked.
pub fn get_changed_files(
    repo_path: Option<&str>,
    base: &str,
    head: &str,
) -> Result<Vec<ChangedFile>, GitError> {
    let repo = find_repo(repo_path)?;

    if head == WORKING_TREE_REF {
        // For working tree, we need to combine:
        // 1. Changes from base to index (staged)
        // 2. Changes from index to workdir (unstaged)
        // 3. Untracked files
        get_working_tree_changes(&repo, base)
    } else {
        // Diff between two trees
        get_tree_diff_files(&repo, base, head)
    }
}

/// Get files changed in working tree relative to a base ref.
fn get_working_tree_changes(repo: &Repository, base: &str) -> Result<Vec<ChangedFile>, GitError> {
    use git2::{Status, StatusOptions};
    use std::collections::HashMap;

    let mut files: HashMap<String, String> = HashMap::new();

    // First, get changes from base to HEAD (committed since base)
    // This handles the case where base is "main" and we want to see all changes
    if base != "HEAD" {
        if let (Ok(base_tree), Ok(head_tree)) =
            (resolve_tree(repo, base), resolve_tree(repo, "HEAD"))
        {
            let diff = repo.diff_tree_to_tree(Some(&base_tree), Some(&head_tree), None)?;

            diff.foreach(
                &mut |delta, _| {
                    if let Some(path) = delta.new_file().path().or(delta.old_file().path()) {
                        let path_str = path.to_string_lossy().to_string();
                        let status = match delta.status() {
                            git2::Delta::Added => "added",
                            git2::Delta::Deleted => "deleted",
                            git2::Delta::Modified => "modified",
                            git2::Delta::Renamed => "renamed",
                            git2::Delta::Copied => "added",
                            _ => "modified",
                        };
                        files.insert(path_str, status.to_string());
                    }
                    true
                },
                None,
                None,
                None,
            )?;
        }
    }

    // Then overlay with working tree status (staged + unstaged + untracked)
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);

    let statuses = repo.statuses(Some(&mut opts))?;

    for entry in statuses.iter() {
        let path = entry.path().unwrap_or("").to_string();
        let status = entry.status();

        // Determine the display status
        let status_str = if status.contains(Status::WT_NEW) {
            "untracked"
        } else if status.contains(Status::INDEX_NEW) || status.contains(Status::WT_NEW) {
            "added"
        } else if status.contains(Status::INDEX_DELETED) || status.contains(Status::WT_DELETED) {
            "deleted"
        } else if status.intersects(
            Status::INDEX_MODIFIED
                | Status::WT_MODIFIED
                | Status::INDEX_RENAMED
                | Status::WT_RENAMED,
        ) {
            "modified"
        } else {
            continue; // Skip unchanged files
        };

        files.insert(path, status_str.to_string());
    }

    let mut result: Vec<ChangedFile> = files
        .into_iter()
        .map(|(path, status)| ChangedFile { path, status })
        .collect();

    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

/// Get files changed between two tree refs.
fn get_tree_diff_files(
    repo: &Repository,
    base: &str,
    head: &str,
) -> Result<Vec<ChangedFile>, GitError> {
    let base_tree = resolve_tree(repo, base)?;
    let head_tree = resolve_tree(repo, head)?;

    let diff = repo.diff_tree_to_tree(Some(&base_tree), Some(&head_tree), None)?;

    let mut files = Vec::new();

    diff.foreach(
        &mut |delta, _| {
            if let Some(path) = delta.new_file().path().or(delta.old_file().path()) {
                let status = match delta.status() {
                    git2::Delta::Added => "added",
                    git2::Delta::Deleted => "deleted",
                    git2::Delta::Modified => "modified",
                    git2::Delta::Renamed => "renamed",
                    git2::Delta::Copied => "added",
                    git2::Delta::Typechange => "typechange",
                    _ => "modified",
                };
                files.push(ChangedFile {
                    path: path.to_string_lossy().to_string(),
                    status: status.to_string(),
                });
            }
            true
        },
        None,
        None,
        None,
    )?;

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}
