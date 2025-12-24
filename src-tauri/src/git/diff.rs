//! Diff operations

use super::repo::find_repo;
use super::GitError;
use git2::{Diff, DiffOptions};
use serde::{Deserialize, Serialize};

/// Represents a single line in a diff
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffLine {
    pub line_type: String, // "context", "added", "removed", "empty"
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    pub content: String,
}

/// Represents a hunk (chunk) of changes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// Represents the complete diff for a file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>, // For renames
    pub status: String,
    pub hunks: Vec<DiffHunk>,
    pub is_binary: bool,
    pub old_content: Vec<DiffLine>, // Lines for left pane (original)
    pub new_content: Vec<DiffLine>, // Lines for right pane (modified)
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
    diff_opts.context_lines(3);

    let diff = if staged {
        // Staged: compare HEAD to index
        let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());

        repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut diff_opts))?
    } else {
        // Unstaged: compare index to working directory
        repo.diff_index_to_workdir(None, Some(&mut diff_opts))?
    };

    parse_diff_for_file(&diff, file_path)
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

    let lines: Vec<DiffLine> = content
        .lines()
        .enumerate()
        .map(|(i, line)| DiffLine {
            line_type: "added".to_string(),
            old_lineno: None,
            new_lineno: Some((i + 1) as u32),
            content: line.to_string(),
        })
        .collect();

    // For untracked files, old_content is empty, new_content has all lines
    Ok(FileDiff {
        path: file_path.to_string(),
        old_path: None,
        status: "untracked".to_string(),
        hunks: vec![DiffHunk {
            old_start: 0,
            old_lines: 0,
            new_start: 1,
            new_lines: lines.len() as u32,
            header: format!("@@ -0,0 +1,{} @@", lines.len()),
            lines: lines.clone(),
        }],
        is_binary: false,
        old_content: vec![],
        new_content: lines,
    })
}

/// Parse a git2 Diff object and extract information for a specific file
fn parse_diff_for_file(diff: &Diff, target_path: &str) -> Result<FileDiff, GitError> {
    use std::cell::RefCell;

    let hunks: RefCell<Vec<DiffHunk>> = RefCell::new(Vec::new());
    let is_binary: RefCell<bool> = RefCell::new(false);
    let file_status: RefCell<String> = RefCell::new("modified".to_string());
    let old_path: RefCell<Option<String>> = RefCell::new(None);
    let found_file: RefCell<bool> = RefCell::new(false);

    let current_hunk_lines: RefCell<Vec<DiffLine>> = RefCell::new(Vec::new());
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
                    *old_path.borrow_mut() = old_file_path.map(|s| s.to_string());
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

                current_hunk_lines.borrow_mut().push(DiffLine {
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

    if *is_binary.borrow() {
        return Ok(FileDiff {
            path: target_path.to_string(),
            old_path: old_path.into_inner(),
            status: file_status.into_inner(),
            hunks: vec![],
            is_binary: true,
            old_content: vec![],
            new_content: vec![],
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

    // Build side-by-side content from hunks
    let (old_content, new_content) = build_side_by_side_content(&hunks);

    Ok(FileDiff {
        path: target_path.to_string(),
        old_path: old_path.into_inner(),
        status: file_status.into_inner(),
        hunks,
        is_binary: false,
        old_content,
        new_content,
    })
}

/// Build side-by-side content arrays from hunks for the diff viewer
fn build_side_by_side_content(hunks: &[DiffHunk]) -> (Vec<DiffLine>, Vec<DiffLine>) {
    let mut old_content: Vec<DiffLine> = Vec::new();
    let mut new_content: Vec<DiffLine> = Vec::new();

    for hunk in hunks {
        for line in &hunk.lines {
            match line.line_type.as_str() {
                "context" => {
                    old_content.push(line.clone());
                    new_content.push(line.clone());
                }
                "removed" => {
                    old_content.push(line.clone());
                    // Add placeholder on new side to keep alignment
                    new_content.push(DiffLine {
                        line_type: "empty".to_string(),
                        old_lineno: None,
                        new_lineno: None,
                        content: String::new(),
                    });
                }
                "added" => {
                    // Add placeholder on old side to keep alignment
                    old_content.push(DiffLine {
                        line_type: "empty".to_string(),
                        old_lineno: None,
                        new_lineno: None,
                        content: String::new(),
                    });
                    new_content.push(line.clone());
                }
                _ => {}
            }
        }
    }

    (old_content, new_content)
}
