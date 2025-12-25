//! Diff operations
//!
//! Generates side-by-side diff data with range mappings for scroll synchronization.

use super::repo::find_repo;
use super::GitError;
use git2::{Diff, DiffOptions, Repository};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A single line in a diff pane
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffLine {
    /// "context", "added", or "removed"
    pub line_type: String,
    /// 1-indexed line number in the source file
    pub lineno: u32,
    /// Line content (without trailing newline)
    pub content: String,
}

/// A hunk from git's diff output (used internally, also exposed for potential future use)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub header: String,
    pub lines: Vec<HunkLine>,
}

/// A line within a hunk (internal representation with both line numbers)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HunkLine {
    pub line_type: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    pub content: String,
}

/// Half-open interval [start, end) of row indices
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// Maps corresponding regions between before/after panes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Range {
    pub before: Span,
    pub after: Span,
    /// true = region contains changes, false = identical lines
    pub changed: bool,
}

/// Content for one side of the diff
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffSide {
    pub path: Option<String>,
    pub lines: Vec<DiffLine>,
}

/// Complete diff for a file
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

/// Get diff for a specific file
/// `staged` parameter determines whether to get staged (index vs HEAD) or unstaged (working tree vs index) diff
pub fn get_file_diff(
    repo_path: Option<&str>,
    file_path: &str,
    staged: bool,
) -> Result<FileDiff, GitError> {
    let repo = find_repo(repo_path)?;

    let mut diff_opts = DiffOptions::new();
    diff_opts.pathspec(file_path);
    diff_opts.context_lines(0); // We'll show full file, don't need context from git

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

    parse_diff_for_file(
        &diff,
        file_path,
        before_path,
        after_path,
        &before_content,
        &after_content,
    )
}

/// Get the "before" file content (what we're comparing from)
/// - For staged diffs: content from HEAD
/// - For unstaged diffs: content from index
fn get_before_content(
    repo: &Repository,
    file_path: &str,
    staged: bool,
) -> Result<Option<String>, GitError> {
    if staged {
        // Get from HEAD
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
    } else {
        // Get from index
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
}

/// Get the "after" file content (what we're comparing to)
/// - For staged diffs: content from index
/// - For unstaged diffs: content from working directory
fn get_after_content(
    repo: &Repository,
    file_path: &str,
    staged: bool,
) -> Result<Option<String>, GitError> {
    if staged {
        // Get from index
        let index = repo.index().map_err(|e| GitError {
            message: format!("Failed to get index: {}", e),
        })?;
        let entry = match index.get_path(std::path::Path::new(file_path), 0) {
            Some(e) => e,
            None => return Ok(None), // File deleted from index
        };
        let blob = repo.find_blob(entry.id).map_err(|e| GitError {
            message: format!("Failed to get blob: {}", e),
        })?;
        if blob.is_binary() {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
    } else {
        // Get from working directory
        let workdir = repo.workdir().ok_or_else(|| GitError {
            message: "Repository has no working directory".to_string(),
        })?;
        let full_path = workdir.join(file_path);
        match std::fs::read_to_string(&full_path) {
            Ok(content) => Ok(Some(content)),
            Err(_) => Ok(None), // File deleted from working directory
        }
    }
}

/// Get diff for an untracked file (show entire file as added)
pub fn get_untracked_file_diff(
    repo_path: Option<&str>,
    file_path: &str,
) -> Result<FileDiff, GitError> {
    let repo = find_repo(repo_path)?;
    let workdir = repo.workdir().ok_or_else(|| GitError {
        message: "Repository has no working directory".to_string(),
    })?;

    let full_path = workdir.join(file_path);
    let content = std::fs::read_to_string(&full_path).map_err(|e| GitError {
        message: format!("Failed to read file: {}", e),
    })?;

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

/// Parse a git2 Diff object and extract information for a specific file
fn parse_diff_for_file(
    diff: &Diff,
    target_path: &str,
    before_path: Option<String>,
    after_path: Option<String>,
    before_content: &Option<String>,
    after_content: &Option<String>,
) -> Result<FileDiff, GitError> {
    use std::cell::RefCell;

    let hunks: RefCell<Vec<DiffHunk>> = RefCell::new(Vec::new());
    let is_binary: RefCell<bool> = RefCell::new(false);
    let file_status: RefCell<String> = RefCell::new("modified".to_string());
    let found_file: RefCell<bool> = RefCell::new(false);
    let renamed_from: RefCell<Option<String>> = RefCell::new(None);

    let current_hunk_lines: RefCell<Vec<HunkLine>> = RefCell::new(Vec::new());
    let current_hunk_header: RefCell<String> = RefCell::new(String::new());
    let current_hunk_old_start: RefCell<u32> = RefCell::new(0);
    let current_hunk_old_lines: RefCell<u32> = RefCell::new(0);
    let current_hunk_new_start: RefCell<u32> = RefCell::new(0);
    let current_hunk_new_lines: RefCell<u32> = RefCell::new(0);
    let in_target_file: RefCell<bool> = RefCell::new(false);

    diff.foreach(
        &mut |delta, _progress| {
            let new_file_path = delta.new_file().path().and_then(|p| p.to_str());
            let old_file_path = delta.old_file().path().and_then(|p| p.to_str());

            let is_target =
                new_file_path == Some(target_path) || old_file_path == Some(target_path);
            *in_target_file.borrow_mut() = is_target;

            if is_target {
                *found_file.borrow_mut() = true;
                *is_binary.borrow_mut() =
                    delta.new_file().is_binary() || delta.old_file().is_binary();

                *file_status.borrow_mut() = match delta.status() {
                    git2::Delta::Added => "added",
                    git2::Delta::Deleted => "deleted",
                    git2::Delta::Modified => "modified",
                    git2::Delta::Renamed => "renamed",
                    git2::Delta::Copied => "copied",
                    _ => "modified",
                }
                .to_string();

                if delta.status() == git2::Delta::Renamed {
                    *renamed_from.borrow_mut() = old_file_path.map(|s| s.to_string());
                }
            }
            true
        },
        None, // binary_cb
        Some(&mut |_delta, hunk| {
            if *in_target_file.borrow() {
                // Save previous hunk if exists
                let mut lines = current_hunk_lines.borrow_mut();
                if !lines.is_empty() {
                    hunks.borrow_mut().push(DiffHunk {
                        old_start: *current_hunk_old_start.borrow(),
                        old_lines: *current_hunk_old_lines.borrow(),
                        new_start: *current_hunk_new_start.borrow(),
                        new_lines: *current_hunk_new_lines.borrow(),
                        header: current_hunk_header.borrow().clone(),
                        lines: lines.clone(),
                    });
                    lines.clear();
                }

                *current_hunk_old_start.borrow_mut() = hunk.old_start();
                *current_hunk_old_lines.borrow_mut() = hunk.old_lines();
                *current_hunk_new_start.borrow_mut() = hunk.new_start();
                *current_hunk_new_lines.borrow_mut() = hunk.new_lines();
                *current_hunk_header.borrow_mut() =
                    String::from_utf8_lossy(hunk.header()).to_string();
            }
            true
        }),
        Some(&mut |_delta, _hunk, line| {
            if *in_target_file.borrow() {
                let line_type = match line.origin() {
                    '+' => "added",
                    '-' => "removed",
                    ' ' => "context",
                    _ => "context",
                }
                .to_string();

                let content = String::from_utf8_lossy(line.content())
                    .trim_end_matches('\n')
                    .trim_end_matches('\r')
                    .to_string();

                current_hunk_lines.borrow_mut().push(HunkLine {
                    line_type,
                    old_lineno: line.old_lineno(),
                    new_lineno: line.new_lineno(),
                    content,
                });
            }
            true
        }),
    )
    .map_err(|e| GitError {
        message: format!("Failed to parse diff: {}", e),
    })?;

    if !*found_file.borrow() {
        return Err(GitError {
            message: format!("File not found in diff: {}", target_path),
        });
    }

    let status = file_status.into_inner();
    let renamed_from = renamed_from.into_inner();

    if *is_binary.borrow() {
        return Ok(FileDiff {
            status,
            is_binary: true,
            hunks: vec![],
            before: DiffSide {
                path: renamed_from.or(before_path),
                lines: vec![],
            },
            after: DiffSide {
                path: after_path,
                lines: vec![],
            },
            ranges: vec![],
        });
    }

    // Don't forget the last hunk
    let lines = current_hunk_lines.borrow();
    if !lines.is_empty() {
        hunks.borrow_mut().push(DiffHunk {
            old_start: *current_hunk_old_start.borrow(),
            old_lines: *current_hunk_old_lines.borrow(),
            new_start: *current_hunk_new_start.borrow(),
            new_lines: *current_hunk_new_lines.borrow(),
            header: current_hunk_header.borrow().clone(),
            lines: lines.clone(),
        });
    }
    drop(lines);

    let hunks = hunks.into_inner();

    // Build side-by-side content and ranges
    let (before_lines, after_lines, ranges) =
        build_side_by_side(before_content, after_content, &hunks);

    Ok(FileDiff {
        status,
        is_binary: false,
        hunks,
        before: DiffSide {
            path: renamed_from.or(before_path),
            lines: before_lines,
        },
        after: DiffSide {
            path: after_path,
            lines: after_lines,
        },
        ranges,
    })
}

/// Build side-by-side line arrays and range mappings from file contents and hunks.
fn build_side_by_side(
    before_content: &Option<String>,
    after_content: &Option<String>,
    hunks: &[DiffHunk],
) -> (Vec<DiffLine>, Vec<DiffLine>, Vec<Range>) {
    let before_file_lines: Vec<&str> = before_content
        .as_ref()
        .map(|s| s.lines().collect())
        .unwrap_or_default();
    let after_file_lines: Vec<&str> = after_content
        .as_ref()
        .map(|s| s.lines().collect())
        .unwrap_or_default();

    // Build sets of changed line numbers from hunks
    let mut removed_lines: HashSet<u32> = HashSet::new();
    let mut added_lines: HashSet<u32> = HashSet::new();

    for hunk in hunks {
        for line in &hunk.lines {
            match line.line_type.as_str() {
                "removed" => {
                    if let Some(lineno) = line.old_lineno {
                        removed_lines.insert(lineno);
                    }
                }
                "added" => {
                    if let Some(lineno) = line.new_lineno {
                        added_lines.insert(lineno);
                    }
                }
                _ => {}
            }
        }
    }

    let mut before_lines: Vec<DiffLine> = Vec::new();
    let mut after_lines: Vec<DiffLine> = Vec::new();
    let mut ranges: Vec<Range> = Vec::new();

    // Track positions for range building
    let mut before_idx: usize = 0; // 0-indexed into before_file_lines
    let mut after_idx: usize = 0; // 0-indexed into after_file_lines

    // Process hunks in order, filling in unchanged lines between them
    for hunk in hunks {
        let hunk_before_start = hunk.old_start as usize;
        let hunk_after_start = hunk.new_start as usize;

        // Add unchanged lines before this hunk as a context range
        if (before_idx + 1 < hunk_before_start || hunk.old_start == 0)
            && (after_idx + 1 < hunk_after_start || hunk.new_start == 0)
        {
            let range_before_start = before_lines.len();
            let range_after_start = after_lines.len();

            while before_idx + 1 < hunk_before_start && after_idx + 1 < hunk_after_start {
                let content = before_file_lines.get(before_idx).unwrap_or(&"").to_string();

                before_lines.push(DiffLine {
                    line_type: "context".to_string(),
                    lineno: (before_idx + 1) as u32,
                    content: content.clone(),
                });
                after_lines.push(DiffLine {
                    line_type: "context".to_string(),
                    lineno: (after_idx + 1) as u32,
                    content,
                });

                before_idx += 1;
                after_idx += 1;
            }

            // Add context range if we added any lines
            if before_lines.len() > range_before_start {
                ranges.push(Range {
                    before: Span {
                        start: range_before_start,
                        end: before_lines.len(),
                    },
                    after: Span {
                        start: range_after_start,
                        end: after_lines.len(),
                    },
                    changed: false,
                });
            }
        }

        // Process the hunk
        process_hunk(
            hunk,
            &mut before_lines,
            &mut after_lines,
            &mut ranges,
            &mut before_idx,
            &mut after_idx,
        );
    }

    // Add any remaining unchanged lines after the last hunk
    if before_idx < before_file_lines.len() || after_idx < after_file_lines.len() {
        let range_before_start = before_lines.len();
        let range_after_start = after_lines.len();

        while before_idx < before_file_lines.len() && after_idx < after_file_lines.len() {
            let content = before_file_lines.get(before_idx).unwrap_or(&"").to_string();

            before_lines.push(DiffLine {
                line_type: "context".to_string(),
                lineno: (before_idx + 1) as u32,
                content: content.clone(),
            });
            after_lines.push(DiffLine {
                line_type: "context".to_string(),
                lineno: (after_idx + 1) as u32,
                content,
            });

            before_idx += 1;
            after_idx += 1;
        }

        // Handle remaining lines on either side (edge case)
        while before_idx < before_file_lines.len() {
            let content = before_file_lines.get(before_idx).unwrap_or(&"").to_string();
            before_lines.push(DiffLine {
                line_type: "context".to_string(),
                lineno: (before_idx + 1) as u32,
                content,
            });
            before_idx += 1;
        }

        while after_idx < after_file_lines.len() {
            let content = after_file_lines.get(after_idx).unwrap_or(&"").to_string();
            after_lines.push(DiffLine {
                line_type: "context".to_string(),
                lineno: (after_idx + 1) as u32,
                content,
            });
            after_idx += 1;
        }

        // Add final context range
        if before_lines.len() > range_before_start || after_lines.len() > range_after_start {
            ranges.push(Range {
                before: Span {
                    start: range_before_start,
                    end: before_lines.len(),
                },
                after: Span {
                    start: range_after_start,
                    end: after_lines.len(),
                },
                changed: false,
            });
        }
    }

    (before_lines, after_lines, ranges)
}

/// Process a single hunk, adding lines and ranges
fn process_hunk(
    hunk: &DiffHunk,
    before_lines: &mut Vec<DiffLine>,
    after_lines: &mut Vec<DiffLine>,
    ranges: &mut Vec<Range>,
    before_idx: &mut usize,
    after_idx: &mut usize,
) {
    let mut pending_removed: Vec<&HunkLine> = Vec::new();
    let mut pending_added: Vec<&HunkLine> = Vec::new();

    for line in &hunk.lines {
        match line.line_type.as_str() {
            "context" => {
                // Flush pending changes as a change range
                flush_changes(
                    &mut pending_removed,
                    &mut pending_added,
                    before_lines,
                    after_lines,
                    ranges,
                );

                // Add context line to both sides
                let range_before_start = before_lines.len();
                let range_after_start = after_lines.len();

                before_lines.push(DiffLine {
                    line_type: "context".to_string(),
                    lineno: line.old_lineno.unwrap_or(0),
                    content: line.content.clone(),
                });
                after_lines.push(DiffLine {
                    line_type: "context".to_string(),
                    lineno: line.new_lineno.unwrap_or(0),
                    content: line.content.clone(),
                });

                // Single-line context range
                ranges.push(Range {
                    before: Span {
                        start: range_before_start,
                        end: before_lines.len(),
                    },
                    after: Span {
                        start: range_after_start,
                        end: after_lines.len(),
                    },
                    changed: false,
                });

                if let Some(ln) = line.old_lineno {
                    *before_idx = ln as usize;
                }
                if let Some(ln) = line.new_lineno {
                    *after_idx = ln as usize;
                }
            }
            "removed" => {
                pending_removed.push(line);
                if let Some(ln) = line.old_lineno {
                    *before_idx = ln as usize;
                }
            }
            "added" => {
                pending_added.push(line);
                if let Some(ln) = line.new_lineno {
                    *after_idx = ln as usize;
                }
            }
            _ => {}
        }
    }

    // Flush any remaining changes
    flush_changes(
        &mut pending_removed,
        &mut pending_added,
        before_lines,
        after_lines,
        ranges,
    );
}

/// Flush pending removed/added lines as a single change range
fn flush_changes(
    pending_removed: &mut Vec<&HunkLine>,
    pending_added: &mut Vec<&HunkLine>,
    before_lines: &mut Vec<DiffLine>,
    after_lines: &mut Vec<DiffLine>,
    ranges: &mut Vec<Range>,
) {
    if pending_removed.is_empty() && pending_added.is_empty() {
        return;
    }

    let range_before_start = before_lines.len();
    let range_after_start = after_lines.len();

    // Add removed lines to before pane
    for line in pending_removed.drain(..) {
        before_lines.push(DiffLine {
            line_type: "removed".to_string(),
            lineno: line.old_lineno.unwrap_or(0),
            content: line.content.clone(),
        });
    }

    // Add added lines to after pane
    for line in pending_added.drain(..) {
        after_lines.push(DiffLine {
            line_type: "added".to_string(),
            lineno: line.new_lineno.unwrap_or(0),
            content: line.content.clone(),
        });
    }

    // Create change range
    ranges.push(Range {
        before: Span {
            start: range_before_start,
            end: before_lines.len(),
        },
        after: Span {
            start: range_after_start,
            end: after_lines.len(),
        },
        changed: true,
    });
}
