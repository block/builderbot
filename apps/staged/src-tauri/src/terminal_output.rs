//! Terminal output normalization shared by backend prompt/detection paths.
//!
//! Shell commands often emit progress by writing a line, then `\r`, then a
//! replacement line. This module renders those updates the way the frontend's
//! action output viewer does, and strips ANSI/control sequences before text is
//! used for AI prompts or regex matching.

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum EscapeState {
    #[default]
    Ground,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

/// How much a processor fed by a long-running command retains.
#[derive(Debug, Clone, Copy)]
struct RecentLines {
    max_lines: usize,
    /// Bound on the text of the finalized lines, and separately on the text
    /// of the in-progress line.
    max_bytes: usize,
}

/// Incrementally renders terminal text into plain lines.
///
/// Lines are numbered from zero in the order they were written. A processor
/// built with [`TerminalOutputProcessor::keeping_recent`] drops its oldest
/// lines once it is over its bound, and the numbers of the lines that remain
/// do not change.
#[derive(Debug, Default)]
pub(crate) struct TerminalOutputProcessor {
    /// Oldest first. The first one is line number `dropped_lines`.
    finalized_lines: VecDeque<String>,
    /// Bytes of text in `finalized_lines`.
    finalized_bytes: usize,
    /// How many finalized lines have been dropped from the front.
    dropped_lines: usize,
    current_text: String,
    pending_cr: bool,
    escape_state: EscapeState,
    /// `None` retains every line.
    recent: Option<RecentLines>,
}

impl TerminalOutputProcessor {
    /// A processor that retains only its most recent lines: at most
    /// `max_lines` finalized lines holding at most `max_bytes` of text. The
    /// in-progress line stops growing at `max_bytes` too, until it ends.
    pub(crate) fn keeping_recent(max_lines: usize, max_bytes: usize) -> Self {
        Self {
            recent: Some(RecentLines {
                max_lines,
                max_bytes,
            }),
            ..Self::default()
        }
    }

    pub(crate) fn process_chunk(&mut self, raw: &str) {
        for ch in raw.chars() {
            if let Some(clean) = self.consume_escape_or_control(ch) {
                self.process_visible_char(clean);
            }
        }
    }

    /// How many lines there have been so far: every finalized line, retained
    /// or not, plus the in-progress line if it is visible.
    pub(crate) fn line_count(&self) -> usize {
        self.dropped_lines + self.finalized_lines.len() + usize::from(!self.current_text.is_empty())
    }

    /// The lines numbered `from` and up: finalized lines, then the visible
    /// in-progress line. Even when a trailing `\r` is unresolved, live
    /// snapshots expose the current text so readiness detection matches the
    /// frontend incremental output view. Lines that are no longer retained
    /// are left out.
    pub(crate) fn lines_from(&self, from: usize) -> Vec<String> {
        let skip = from.saturating_sub(self.dropped_lines);
        self.finalized_lines
            .iter()
            .chain((!self.current_text.is_empty()).then_some(&self.current_text))
            .skip(skip)
            .cloned()
            .collect()
    }

    /// The last `count` lines, as [`Self::lines_from`] lists them.
    pub(crate) fn last_lines(&self, count: usize) -> Vec<String> {
        self.lines_from(self.line_count().saturating_sub(count))
    }

    fn finish(mut self) -> Vec<String> {
        if self.pending_cr {
            self.current_text.clear();
            self.pending_cr = false;
        }

        let mut lines = Vec::from(self.finalized_lines);
        if !self.current_text.is_empty() {
            lines.push(self.current_text);
        }
        lines
    }

    fn consume_escape_or_control(&mut self, ch: char) -> Option<char> {
        match self.escape_state {
            EscapeState::Ground => match ch {
                '\x1b' => {
                    self.escape_state = EscapeState::Esc;
                    None
                }
                '\u{009b}' => {
                    self.escape_state = EscapeState::Csi;
                    None
                }
                '\u{009d}' => {
                    self.escape_state = EscapeState::Osc;
                    None
                }
                '\n' | '\r' | '\t' => Some(ch),
                c if is_prompt_hostile_control(c) => None,
                c => Some(c),
            },
            EscapeState::Esc => {
                self.escape_state = match ch {
                    '[' => EscapeState::Csi,
                    ']' | 'P' | '_' | '^' | 'X' => EscapeState::Osc,
                    _ => EscapeState::Ground,
                };
                None
            }
            EscapeState::Csi => {
                if is_csi_final_byte(ch) {
                    self.escape_state = EscapeState::Ground;
                }
                None
            }
            EscapeState::Osc => match ch {
                '\x07' => {
                    self.escape_state = EscapeState::Ground;
                    None
                }
                '\x1b' => {
                    self.escape_state = EscapeState::OscEsc;
                    None
                }
                _ => None,
            },
            EscapeState::OscEsc => {
                self.escape_state = if ch == '\\' {
                    EscapeState::Ground
                } else {
                    EscapeState::Osc
                };
                None
            }
        }
    }

    fn process_visible_char(&mut self, ch: char) {
        if self.pending_cr {
            self.pending_cr = false;
            if ch == '\n' {
                self.finalize_current_line();
                return;
            }
            self.current_text.clear();
        }

        match ch {
            '\n' => self.finalize_current_line(),
            '\r' => {
                self.pending_cr = true;
            }
            c => {
                // A line with no end in sight must not grow without bound
                // either: past the bound, the rest of it is dropped.
                let full = self
                    .recent
                    .is_some_and(|recent| self.current_text.len() >= recent.max_bytes);
                if !full {
                    self.current_text.push(c);
                }
            }
        }
    }

    fn finalize_current_line(&mut self) {
        let line = std::mem::take(&mut self.current_text);
        self.finalized_bytes += line.len();
        self.finalized_lines.push_back(line);

        let Some(recent) = self.recent else {
            return;
        };
        while self.finalized_lines.len() > recent.max_lines
            || self.finalized_bytes > recent.max_bytes
        {
            let Some(dropped) = self.finalized_lines.pop_front() else {
                break;
            };
            self.finalized_bytes -= dropped.len();
            self.dropped_lines += 1;
        }
    }
}

/// Render terminal output into display-ready plain text.
pub(crate) fn normalize_display_output(raw: &str) -> String {
    let mut processor = TerminalOutputProcessor::default();
    processor.process_chunk(raw);
    processor.finish().join("\n")
}

/// Render bytes from a process pipe into display-ready plain text.
pub(crate) fn normalize_display_bytes(bytes: &[u8]) -> String {
    normalize_display_output(&String::from_utf8_lossy(bytes))
}

/// Clean text for prompt injection or regex matching.
pub(crate) fn normalize_for_prompt(raw: &str) -> String {
    normalize_display_output(raw)
        .chars()
        .filter_map(|ch| match ch {
            '\n' => Some('\n'),
            '\t' => Some(' '),
            c if is_prompt_hostile_control(c) => None,
            c => Some(c),
        })
        .collect()
}

/// Strip prompt-hostile control characters from already display-normalized text.
///
/// Use this instead of `normalize_for_prompt` when the input has already been
/// through `normalize_display_output`/`normalize_display_bytes` to avoid running
/// the CR/ANSI processing pass a second time.
pub(crate) fn strip_prompt_hostile_chars(display_normalized: &str) -> String {
    display_normalized
        .chars()
        .filter_map(|ch| match ch {
            '\n' => Some('\n'),
            '\t' => Some(' '),
            c if is_prompt_hostile_control(c) => None,
            c => Some(c),
        })
        .collect()
}

/// Truncate long prompt output while making the truncation explicit.
pub(crate) fn truncate_for_prompt(output: &str, max_chars: usize) -> String {
    let char_count = output.chars().count();
    if char_count <= max_chars {
        return output.to_string();
    }

    let marker = format!("[Output truncated: showing tail of {char_count} characters]\n");
    let marker_chars = marker.chars().count();
    if marker_chars >= max_chars {
        return marker;
    }

    let keep_chars = max_chars - marker_chars;
    let tail_start = output
        .char_indices()
        .rev()
        .nth(keep_chars - 1)
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    format!("{}{}", marker, &output[tail_start..])
}

fn is_prompt_hostile_control(ch: char) -> bool {
    matches!(ch, '\u{0000}'..='\u{0008}' | '\u{000b}' | '\u{000c}' | '\u{000e}'..='\u{001f}' | '\u{007f}'..='\u{009f}')
}

fn is_csi_final_byte(ch: char) -> bool {
    matches!(ch, '\u{0040}'..='\u{007e}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalize_chunks(chunks: &[&str]) -> String {
        let mut processor = TerminalOutputProcessor::default();
        for chunk in chunks {
            processor.process_chunk(chunk);
        }
        processor.finish().join("\n")
    }

    #[test]
    fn handles_crlf_split_across_chunks() {
        assert_eq!(normalize_chunks(&["hello\r", "\nworld"]), "hello\nworld");
    }

    #[test]
    fn bare_carriage_return_overwrites_current_line() {
        assert_eq!(
            normalize_display_output("progress 50%\rprogress 100%\n"),
            "progress 100%"
        );
    }

    #[test]
    fn repeated_progress_updates_collapse_to_final_line() {
        assert_eq!(normalize_display_output("10%\r20%\r30%\r40%\n"), "40%");
    }

    #[test]
    fn trailing_bare_carriage_return_drops_in_progress_line() {
        assert_eq!(normalize_display_output("in progress\r"), "");
    }

    #[test]
    fn strips_ansi_csi_and_osc_sequences() {
        assert_eq!(
            normalize_display_output("\x1b[31mred\x1b[0m \x1b]0;title\x07plain"),
            "red plain"
        );
    }

    #[test]
    fn strips_ansi_sequences_split_across_chunks() {
        assert_eq!(
            normalize_chunks(&["\x1b[31", "mred\x1b]", "0;title\x07 plain"]),
            "red plain"
        );
    }

    #[test]
    fn pipeline_progress_output_is_clean_for_prompts() {
        assert_eq!(normalize_for_prompt("10%\r20%\rdone\n"), "done");
    }

    #[test]
    fn snapshot_includes_unresolved_carriage_return_line() {
        let mut processor = TerminalOutputProcessor::default();
        processor.process_chunk("10%\r");
        assert_eq!(processor.lines_from(0), vec!["10%".to_string()]);

        processor.process_chunk("done");
        assert_eq!(processor.lines_from(0), vec!["done".to_string()]);
    }

    #[test]
    fn lines_are_numbered_from_the_start_of_the_output() {
        let mut processor = TerminalOutputProcessor::default();
        processor.process_chunk("zero\none\ntwo\nthr");

        assert_eq!(processor.line_count(), 4);
        assert_eq!(processor.lines_from(2), vec!["two", "thr"]);
        assert_eq!(processor.lines_from(4), Vec::<String>::new());
        assert_eq!(processor.lines_from(9), Vec::<String>::new());
        assert_eq!(processor.last_lines(2), vec!["two", "thr"]);
        assert_eq!(processor.last_lines(9), processor.lines_from(0));

        // An in-progress line that is cleared stops counting.
        processor.process_chunk("\rx\r");
        processor.process_chunk("\r");
        assert_eq!(processor.line_count(), 3);
        assert_eq!(processor.lines_from(2), vec!["two"]);
    }

    #[test]
    fn a_processor_keeping_recent_lines_drops_the_oldest() {
        let mut processor = TerminalOutputProcessor::keeping_recent(3, usize::MAX);
        for line in 0..10 {
            processor.process_chunk(&format!("line {line}\n"));
        }
        processor.process_chunk("partial");

        assert_eq!(
            processor.lines_from(0),
            vec!["line 7", "line 8", "line 9", "partial"]
        );
        // The lines that remain keep their numbers.
        assert_eq!(processor.line_count(), 11);
        assert_eq!(processor.lines_from(9), vec!["line 9", "partial"]);
        // Asking for a dropped line starts at the oldest one retained.
        assert_eq!(processor.lines_from(2), processor.lines_from(0));
        assert_eq!(processor.last_lines(2), vec!["line 9", "partial"]);
    }

    #[test]
    fn a_processor_keeping_recent_lines_is_bounded_in_bytes() {
        let max_bytes = 100;
        let mut processor = TerminalOutputProcessor::keeping_recent(usize::MAX, max_bytes);
        for line in 0..1_000 {
            processor.process_chunk(&format!("line {line:04}\n"));
            assert!(processor.finalized_bytes <= max_bytes);
        }
        assert_eq!(processor.lines_from(0).len(), max_bytes / "line 0000".len());
        assert_eq!(processor.lines_from(0).last().unwrap(), "line 0999");
        assert_eq!(processor.line_count(), 1_000);

        // A line that never ends stops growing, and the next line is whole.
        processor.process_chunk(&"x".repeat(10 * max_bytes));
        assert_eq!(processor.current_text.len(), max_bytes);
        processor.process_chunk("\nnext\n");
        assert_eq!(processor.lines_from(0), vec!["next"]);
        assert_eq!(processor.line_count(), 1_002);
    }

    #[test]
    fn a_default_processor_keeps_every_line() {
        let mut processor = TerminalOutputProcessor::default();
        for line in 0..30_000 {
            processor.process_chunk(&format!("{line}\n"));
        }
        assert_eq!(processor.lines_from(0).len(), 30_000);
        assert_eq!(processor.dropped_lines, 0);
    }

    #[test]
    fn truncation_keeps_tail_and_marks_output() {
        let input = format!("{}tail", "a".repeat(100));
        let output = truncate_for_prompt(&input, 80);
        assert!(output.starts_with("[Output truncated:"));
        assert!(output.ends_with("tail"));
        assert_eq!(output.chars().count(), 80);
    }
}
