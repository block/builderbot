//! Side-by-side diff building.
//!
//! Transforms git hunks into aligned content for a two-pane diff viewer.
//! Produces:
//! - Two arrays of lines (before/after) with proper line numbers
//! - Range mappings that pair corresponding regions for scroll sync
//!
//! ## Algorithm
//! 1. Walk through hunks in order
//! 2. Fill unchanged lines between hunks (context ranges)
//! 3. For each hunk, group consecutive removed/added lines into change ranges
//! 4. Track positions in both panes to build accurate range mappings

use super::parse::{DiffHunk, HunkLine};
use super::{DiffLine, Range, Span};

/// Build side-by-side line arrays and range mappings from file contents and hunks.
///
/// # Arguments
/// * `before_content` - Full content of the "before" file (None if new file)
/// * `after_content` - Full content of the "after" file (None if deleted)
/// * `hunks` - Parsed diff hunks
///
/// # Returns
/// Tuple of (before_lines, after_lines, ranges)
pub fn build(
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

    let mut builder = SideBySideBuilder::new();

    // Process hunks in order, filling unchanged lines between them
    for hunk in hunks {
        let hunk_before_start = hunk.old_start as usize;
        let hunk_after_start = hunk.new_start as usize;

        // Add unchanged lines before this hunk
        builder.add_context_lines_until(&before_file_lines, hunk_before_start, hunk_after_start);

        // Process the hunk itself
        builder.process_hunk(hunk);
    }

    // Add remaining unchanged lines after last hunk
    builder.add_remaining_context(&before_file_lines, &after_file_lines);

    builder.finish()
}

/// Builder for constructing side-by-side diff output.
struct SideBySideBuilder {
    before_lines: Vec<DiffLine>,
    after_lines: Vec<DiffLine>,
    ranges: Vec<Range>,

    // Current position in source files (0-indexed)
    before_idx: usize,
    after_idx: usize,
}

impl SideBySideBuilder {
    fn new() -> Self {
        Self {
            before_lines: Vec::new(),
            after_lines: Vec::new(),
            ranges: Vec::new(),
            before_idx: 0,
            after_idx: 0,
        }
    }

    /// Add context lines from current position up to (but not including) the hunk start.
    ///
    /// Context lines are identical in both files, so we read from before_file
    /// (after_file would have the same content for these unchanged lines).
    fn add_context_lines_until(
        &mut self,
        before_file: &[&str],
        hunk_before_start: usize,
        hunk_after_start: usize,
    ) {
        // Only add if we haven't reached the hunk yet
        if (self.before_idx + 1 >= hunk_before_start && hunk_before_start > 0)
            || (self.after_idx + 1 >= hunk_after_start && hunk_after_start > 0)
        {
            return;
        }

        let range_before_start = self.before_lines.len();
        let range_after_start = self.after_lines.len();

        while self.before_idx + 1 < hunk_before_start && self.after_idx + 1 < hunk_after_start {
            let content = before_file.get(self.before_idx).unwrap_or(&"").to_string();

            self.before_lines.push(DiffLine {
                line_type: "context".to_string(),
                lineno: (self.before_idx + 1) as u32,
                content: content.clone(),
            });
            self.after_lines.push(DiffLine {
                line_type: "context".to_string(),
                lineno: (self.after_idx + 1) as u32,
                content,
            });

            self.before_idx += 1;
            self.after_idx += 1;
        }

        // Add context range if we added any lines
        if self.before_lines.len() > range_before_start {
            self.ranges.push(Range {
                before: Span {
                    start: range_before_start,
                    end: self.before_lines.len(),
                },
                after: Span {
                    start: range_after_start,
                    end: self.after_lines.len(),
                },
                changed: false,
            });
        }
    }

    /// Process a single hunk, adding its lines and ranges.
    fn process_hunk(&mut self, hunk: &DiffHunk) {
        let mut pending_removed: Vec<&HunkLine> = Vec::new();
        let mut pending_added: Vec<&HunkLine> = Vec::new();

        for line in &hunk.lines {
            match line.line_type.as_str() {
                "context" => {
                    // Flush pending changes
                    self.flush_changes(&mut pending_removed, &mut pending_added);

                    // Add context line to both sides
                    let range_before_start = self.before_lines.len();
                    let range_after_start = self.after_lines.len();

                    self.before_lines.push(DiffLine {
                        line_type: "context".to_string(),
                        lineno: line.old_lineno.unwrap_or(0),
                        content: line.content.clone(),
                    });
                    self.after_lines.push(DiffLine {
                        line_type: "context".to_string(),
                        lineno: line.new_lineno.unwrap_or(0),
                        content: line.content.clone(),
                    });

                    // Single-line context range
                    self.ranges.push(Range {
                        before: Span {
                            start: range_before_start,
                            end: self.before_lines.len(),
                        },
                        after: Span {
                            start: range_after_start,
                            end: self.after_lines.len(),
                        },
                        changed: false,
                    });

                    if let Some(ln) = line.old_lineno {
                        self.before_idx = ln as usize;
                    }
                    if let Some(ln) = line.new_lineno {
                        self.after_idx = ln as usize;
                    }
                }
                "removed" => {
                    pending_removed.push(line);
                    if let Some(ln) = line.old_lineno {
                        self.before_idx = ln as usize;
                    }
                }
                "added" => {
                    pending_added.push(line);
                    if let Some(ln) = line.new_lineno {
                        self.after_idx = ln as usize;
                    }
                }
                _ => {}
            }
        }

        // Flush any remaining changes
        self.flush_changes(&mut pending_removed, &mut pending_added);
    }

    /// Flush pending removed/added lines as a single change range.
    fn flush_changes(
        &mut self,
        pending_removed: &mut Vec<&HunkLine>,
        pending_added: &mut Vec<&HunkLine>,
    ) {
        if pending_removed.is_empty() && pending_added.is_empty() {
            return;
        }

        let range_before_start = self.before_lines.len();
        let range_after_start = self.after_lines.len();

        // Add removed lines to before pane
        for line in pending_removed.drain(..) {
            self.before_lines.push(DiffLine {
                line_type: "removed".to_string(),
                lineno: line.old_lineno.unwrap_or(0),
                content: line.content.clone(),
            });
        }

        // Add added lines to after pane
        for line in pending_added.drain(..) {
            self.after_lines.push(DiffLine {
                line_type: "added".to_string(),
                lineno: line.new_lineno.unwrap_or(0),
                content: line.content.clone(),
            });
        }

        // Create change range
        self.ranges.push(Range {
            before: Span {
                start: range_before_start,
                end: self.before_lines.len(),
            },
            after: Span {
                start: range_after_start,
                end: self.after_lines.len(),
            },
            changed: true,
        });
    }

    /// Add any remaining unchanged lines after the last hunk.
    fn add_remaining_context(&mut self, before_file: &[&str], after_file: &[&str]) {
        if self.before_idx >= before_file.len() && self.after_idx >= after_file.len() {
            return;
        }

        let range_before_start = self.before_lines.len();
        let range_after_start = self.after_lines.len();

        // Add matching lines from both sides
        while self.before_idx < before_file.len() && self.after_idx < after_file.len() {
            let content = before_file.get(self.before_idx).unwrap_or(&"").to_string();

            self.before_lines.push(DiffLine {
                line_type: "context".to_string(),
                lineno: (self.before_idx + 1) as u32,
                content: content.clone(),
            });
            self.after_lines.push(DiffLine {
                line_type: "context".to_string(),
                lineno: (self.after_idx + 1) as u32,
                content,
            });

            self.before_idx += 1;
            self.after_idx += 1;
        }

        // Handle remaining lines on either side (edge case)
        while self.before_idx < before_file.len() {
            let content = before_file.get(self.before_idx).unwrap_or(&"").to_string();
            self.before_lines.push(DiffLine {
                line_type: "context".to_string(),
                lineno: (self.before_idx + 1) as u32,
                content,
            });
            self.before_idx += 1;
        }

        while self.after_idx < after_file.len() {
            let content = after_file.get(self.after_idx).unwrap_or(&"").to_string();
            self.after_lines.push(DiffLine {
                line_type: "context".to_string(),
                lineno: (self.after_idx + 1) as u32,
                content,
            });
            self.after_idx += 1;
        }

        // Add final context range
        if self.before_lines.len() > range_before_start
            || self.after_lines.len() > range_after_start
        {
            self.ranges.push(Range {
                before: Span {
                    start: range_before_start,
                    end: self.before_lines.len(),
                },
                after: Span {
                    start: range_after_start,
                    end: self.after_lines.len(),
                },
                changed: false,
            });
        }
    }

    fn finish(self) -> (Vec<DiffLine>, Vec<DiffLine>, Vec<Range>) {
        (self.before_lines, self.after_lines, self.ranges)
    }
}
