//! Git2 diff parsing.
//!
//! Extracts hunks from git2's callback-based diff API. This module isolates
//! the complexity of git2's callback pattern (requiring RefCell for state)
//! from the rest of the diff logic.
//!
//! ## Why RefCell?
//! Git2's `Diff::foreach` takes multiple callbacks that are called during
//! iteration. Rust's borrow checker can't verify the callbacks don't overlap,
//! so we use RefCell for interior mutability. This is safe because git2
//! calls the callbacks sequentially, never concurrently.

use super::super::GitError;
use git2::Diff;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

/// A hunk from git's diff output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub header: String,
    pub lines: Vec<HunkLine>,
}

/// A line within a hunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HunkLine {
    pub line_type: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    pub content: String,
}

/// Result of parsing a diff for a specific file.
pub struct ParseResult {
    pub hunks: Vec<DiffHunk>,
    pub is_binary: bool,
    pub status: String,
    pub renamed_from: Option<String>,
}

/// Parse a git2 Diff and extract hunks for a specific file.
///
/// This function handles git2's callback-based API, collecting all hunk
/// and line data into a structured result.
pub fn parse_diff(diff: &Diff, target_path: &str) -> Result<ParseResult, GitError> {
    // State collected during callbacks (RefCell needed for git2's callback API)
    let state = ParseState::new();

    diff.foreach(
        &mut |delta, _progress| state.on_file(delta, target_path),
        None, // binary_cb
        Some(&mut |_delta, hunk| state.on_hunk(hunk)),
        Some(&mut |_delta, _hunk, line| state.on_line(line)),
    )
    .map_err(|e| GitError {
        message: format!("Failed to parse diff: {}", e),
    })?;

    state.into_result(target_path)
}

/// Internal state for collecting diff data during git2 callbacks.
struct ParseState {
    hunks: RefCell<Vec<DiffHunk>>,
    is_binary: RefCell<bool>,
    file_status: RefCell<String>,
    found_file: RefCell<bool>,
    renamed_from: RefCell<Option<String>>,

    // Current hunk being built
    current_hunk: RefCell<Option<HunkBuilder>>,
    in_target_file: RefCell<bool>,
}

struct HunkBuilder {
    old_start: u32,
    old_lines: u32,
    new_start: u32,
    new_lines: u32,
    header: String,
    lines: Vec<HunkLine>,
}

impl ParseState {
    fn new() -> Self {
        Self {
            hunks: RefCell::new(Vec::new()),
            is_binary: RefCell::new(false),
            file_status: RefCell::new("modified".to_string()),
            found_file: RefCell::new(false),
            renamed_from: RefCell::new(None),
            current_hunk: RefCell::new(None),
            in_target_file: RefCell::new(false),
        }
    }

    fn on_file(&self, delta: git2::DiffDelta, target_path: &str) -> bool {
        let new_file_path = delta.new_file().path().and_then(|p| p.to_str());
        let old_file_path = delta.old_file().path().and_then(|p| p.to_str());

        let is_target = new_file_path == Some(target_path) || old_file_path == Some(target_path);
        *self.in_target_file.borrow_mut() = is_target;

        if is_target {
            *self.found_file.borrow_mut() = true;
            *self.is_binary.borrow_mut() =
                delta.new_file().is_binary() || delta.old_file().is_binary();

            *self.file_status.borrow_mut() = match delta.status() {
                git2::Delta::Added => "added",
                git2::Delta::Deleted => "deleted",
                git2::Delta::Modified => "modified",
                git2::Delta::Renamed => "renamed",
                git2::Delta::Copied => "copied",
                _ => "modified",
            }
            .to_string();

            if delta.status() == git2::Delta::Renamed {
                *self.renamed_from.borrow_mut() = old_file_path.map(|s| s.to_string());
            }
        }
        true
    }

    fn on_hunk(&self, hunk: git2::DiffHunk) -> bool {
        if !*self.in_target_file.borrow() {
            return true;
        }

        // Finalize previous hunk if exists
        self.finalize_current_hunk();

        // Start new hunk
        *self.current_hunk.borrow_mut() = Some(HunkBuilder {
            old_start: hunk.old_start(),
            old_lines: hunk.old_lines(),
            new_start: hunk.new_start(),
            new_lines: hunk.new_lines(),
            header: String::from_utf8_lossy(hunk.header()).to_string(),
            lines: Vec::new(),
        });

        true
    }

    fn on_line(&self, line: git2::DiffLine) -> bool {
        if !*self.in_target_file.borrow() {
            return true;
        }

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

        let hunk_line = HunkLine {
            line_type,
            old_lineno: line.old_lineno(),
            new_lineno: line.new_lineno(),
            content,
        };

        if let Some(ref mut hunk) = *self.current_hunk.borrow_mut() {
            hunk.lines.push(hunk_line);
        }

        true
    }

    fn finalize_current_hunk(&self) {
        let hunk = self.current_hunk.borrow_mut().take();
        if let Some(h) = hunk {
            if !h.lines.is_empty() {
                self.hunks.borrow_mut().push(DiffHunk {
                    old_start: h.old_start,
                    old_lines: h.old_lines,
                    new_start: h.new_start,
                    new_lines: h.new_lines,
                    header: h.header,
                    lines: h.lines,
                });
            }
        }
    }

    fn into_result(self, target_path: &str) -> Result<ParseResult, GitError> {
        // Finalize any remaining hunk
        self.finalize_current_hunk();

        if !*self.found_file.borrow() {
            return Err(GitError {
                message: format!("File not found in diff: {}", target_path),
            });
        }

        Ok(ParseResult {
            hunks: self.hunks.into_inner(),
            is_binary: self.is_binary.into_inner(),
            status: self.file_status.into_inner(),
            renamed_from: self.renamed_from.into_inner(),
        })
    }
}
