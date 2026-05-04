//! Terminal output normalization shared by backend prompt/detection paths.
//!
//! Shell commands often emit progress by writing a line, then `\r`, then a
//! replacement line. This module renders those updates the way the frontend's
//! action output viewer does, and strips ANSI/control sequences before text is
//! used for AI prompts or regex matching.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum EscapeState {
    #[default]
    Ground,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

/// Incrementally renders terminal text into plain lines.
#[derive(Debug, Default)]
pub(crate) struct TerminalOutputProcessor {
    finalized_lines: Vec<String>,
    current_text: String,
    pending_cr: bool,
    escape_state: EscapeState,
}

impl TerminalOutputProcessor {
    pub(crate) fn process_chunk(&mut self, raw: &str) {
        for ch in raw.chars() {
            if let Some(clean) = self.consume_escape_or_control(ch) {
                self.process_visible_char(clean);
            }
        }
    }

    /// Return finalized lines plus the visible in-progress line. Even when a
    /// trailing `\r` is unresolved, live snapshots expose the current text so
    /// readiness detection matches the frontend incremental output view.
    pub(crate) fn snapshot_lines(&self) -> Vec<String> {
        let mut lines = self.finalized_lines.clone();
        if !self.current_text.is_empty() {
            lines.push(self.current_text.clone());
        }
        lines
    }

    fn finish(mut self) -> Vec<String> {
        if self.pending_cr {
            self.current_text.clear();
            self.pending_cr = false;
        }

        let mut lines = self.finalized_lines;
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
                self.finalized_lines
                    .push(std::mem::take(&mut self.current_text));
                return;
            }
            self.current_text.clear();
        }

        match ch {
            '\n' => {
                self.finalized_lines
                    .push(std::mem::take(&mut self.current_text));
            }
            '\r' => {
                self.pending_cr = true;
            }
            c => self.current_text.push(c),
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
        assert_eq!(processor.snapshot_lines(), vec!["10%".to_string()]);

        processor.process_chunk("done");
        assert_eq!(processor.snapshot_lines(), vec!["done".to_string()]);
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
