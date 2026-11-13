//! Bounded storage for one execution's output.
//!
//! The executor keeps recent output so that a viewer opened part-way through a
//! run, or just after it, can show what it missed. Output is unbounded and a
//! progress bar redraws constantly, so the buffer keeps only the most recent
//! bytes and merges the small reads it is fed into larger blocks.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::models::OutputChunk;

/// Most recent output kept per execution, in bytes.
pub(crate) const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

/// Adjacent reads of one stream are merged into blocks of up to this size.
const MAX_BLOCK_BYTES: usize = 64 * 1024;

/// What a block costs beyond its text. Counting it keeps the bound honest when
/// two streams alternate and every block is tiny.
const BLOCK_OVERHEAD_BYTES: usize = std::mem::size_of::<OutputChunk>();

/// Stream the truncation notice is reported on, so that viewers which tell the
/// streams apart set it off from the command's own output.
const NOTICE_STREAM: &str = "stderr";

/// An output buffer shared between the reader threads that fill it and the
/// executor that hands it out.
pub(crate) type SharedOutputBuffer = Arc<Mutex<OutputBuffer>>;

/// The most recent output of one execution.
pub(crate) struct OutputBuffer {
    /// Oldest first. Each block is consecutive output of a single stream.
    blocks: VecDeque<OutputChunk>,
    /// Size of `blocks`: their text plus the per-block overhead.
    retained_bytes: usize,
    /// Bytes of output text evicted from the front so far.
    dropped_bytes: u64,
    /// Whether the oldest retained block starts part-way through a line,
    /// because the block evicted before it did not end on a line break.
    head_is_partial_line: bool,
    max_bytes: usize,
    block_bytes: usize,
}

impl OutputBuffer {
    /// A buffer that retains about `max_bytes` of the most recent output. The
    /// newest block always stays, so the bound is never below one block.
    pub(crate) fn new(max_bytes: usize) -> Self {
        Self {
            blocks: VecDeque::new(),
            retained_bytes: 0,
            dropped_bytes: 0,
            head_is_partial_line: false,
            max_bytes,
            block_bytes: MAX_BLOCK_BYTES.min(max_bytes / 4).max(1),
        }
    }

    pub(crate) fn shared(max_bytes: usize) -> SharedOutputBuffer {
        Arc::new(Mutex::new(Self::new(max_bytes)))
    }

    /// Append one read of `stream`, evicting the oldest output once the buffer
    /// is over its bound. `timestamp` is kept only for the first read of a
    /// block.
    pub(crate) fn push(&mut self, stream: &str, text: &str, timestamp: i64) {
        if text.is_empty() {
            return;
        }

        let block_bytes = self.block_bytes;
        let merged = match self.blocks.back_mut() {
            Some(last) if last.stream == stream && last.chunk.len() + text.len() <= block_bytes => {
                last.chunk.push_str(text);
                true
            }
            Some(last) => {
                // This block is finished: release the slack its growth left.
                last.chunk.shrink_to_fit();
                false
            }
            None => false,
        };
        if merged {
            self.retained_bytes += text.len();
        } else {
            let block = OutputChunk {
                chunk: text.to_string(),
                stream: stream.to_string(),
                timestamp,
            };
            self.retained_bytes += block_cost(&block);
            self.blocks.push_back(block);
        }

        while self.retained_bytes > self.max_bytes && self.blocks.len() > 1 {
            let Some(evicted) = self.blocks.pop_front() else {
                break;
            };
            self.retained_bytes -= block_cost(&evicted);
            self.dropped_bytes += evicted.chunk.len() as u64;
            self.head_is_partial_line = !evicted.chunk.ends_with(['\n', '\r']);
        }
    }

    /// The retained output, oldest first.
    ///
    /// Once earlier output has been dropped, the first chunk is a synthetic
    /// notice saying how much, and the output after it starts on a whole line.
    pub(crate) fn snapshot(&self) -> Vec<OutputChunk> {
        if self.dropped_bytes == 0 {
            return self.blocks.iter().cloned().collect();
        }

        let head = self.blocks.front();
        let skipped = match head {
            Some(head) if self.head_is_partial_line => partial_line_len(&head.chunk),
            _ => 0,
        };

        let mut chunks = Vec::with_capacity(self.blocks.len() + 1);
        chunks.push(OutputChunk {
            chunk: truncation_notice(self.dropped_bytes + skipped as u64),
            stream: NOTICE_STREAM.to_string(),
            timestamp: head.map_or(0, |head| head.timestamp),
        });
        for (index, block) in self.blocks.iter().enumerate() {
            if index > 0 || skipped == 0 {
                chunks.push(block.clone());
            } else if skipped < block.chunk.len() {
                chunks.push(OutputChunk {
                    chunk: block.chunk[skipped..].to_string(),
                    stream: block.stream.clone(),
                    timestamp: block.timestamp,
                });
            }
        }
        chunks
    }
}

fn block_cost(block: &OutputChunk) -> usize {
    BLOCK_OVERHEAD_BYTES + block.stream.len() + block.chunk.len()
}

/// Length of the remainder of a line that `text` starts in the middle of,
/// including its line break. Zero when `text` holds no line break, in which
/// case there is no later line to start from.
fn partial_line_len(text: &str) -> usize {
    match text.find(['\n', '\r']) {
        Some(at) if text[at..].starts_with("\r\n") => at + 2,
        Some(at) => at + 1,
        None => 0,
    }
}

fn truncation_notice(dropped_bytes: u64) -> String {
    format!(
        "[earlier output truncated: {} dropped]\n",
        format_bytes(dropped_bytes)
    )
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let bytes_f = bytes as f64;
    if bytes_f < KB {
        format!("{bytes} B")
    } else if bytes_f < KB * KB {
        format!("{:.1} KB", bytes_f / KB)
    } else if bytes_f < KB * KB * KB {
        format!("{:.1} MB", bytes_f / (KB * KB))
    } else {
        format!("{:.1} GB", bytes_f / (KB * KB * KB))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(chunks: &[OutputChunk]) -> String {
        chunks.iter().map(|c| c.chunk.as_str()).collect()
    }

    #[test]
    fn adjacent_reads_of_one_stream_merge_into_one_block() {
        let mut buffer = OutputBuffer::new(MAX_OUTPUT_BYTES);
        buffer.push("stdout", "one ", 10);
        buffer.push("stdout", "two ", 20);
        buffer.push("stdout", "three", 30);

        let chunks = buffer.snapshot();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].chunk, "one two three");
        assert_eq!(chunks[0].stream, "stdout");
        // The block carries the time of its first read.
        assert_eq!(chunks[0].timestamp, 10);
    }

    #[test]
    fn a_change_of_stream_starts_a_new_block() {
        let mut buffer = OutputBuffer::new(MAX_OUTPUT_BYTES);
        buffer.push("stdout", "out1", 1);
        buffer.push("stderr", "err", 2);
        buffer.push("stdout", "out2", 3);
        buffer.push("stdout", "out3", 4);

        let chunks = buffer.snapshot();
        let seen: Vec<(&str, &str)> = chunks
            .iter()
            .map(|c| (c.stream.as_str(), c.chunk.as_str()))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("stdout", "out1"),
                ("stderr", "err"),
                ("stdout", "out2out3")
            ]
        );
    }

    #[test]
    fn blocks_stop_growing_at_the_block_size() {
        let mut buffer = OutputBuffer::new(MAX_OUTPUT_BYTES);
        let read = "x".repeat(1024);
        for _ in 0..200 {
            buffer.push("stdout", &read, 0);
        }

        let chunks = buffer.snapshot();
        assert_eq!(text_of(&chunks).len(), 200 * 1024);
        assert_eq!(chunks.len(), 200 * 1024 / MAX_BLOCK_BYTES + 1);
        assert!(chunks.iter().all(|c| c.chunk.len() <= MAX_BLOCK_BYTES));
    }

    #[test]
    fn empty_reads_are_ignored() {
        let mut buffer = OutputBuffer::new(MAX_OUTPUT_BYTES);
        buffer.push("stdout", "", 0);
        assert!(buffer.snapshot().is_empty());
    }

    #[test]
    fn only_the_most_recent_output_is_retained() {
        let max_bytes = 8 * 1024;
        let mut buffer = OutputBuffer::new(max_bytes);
        let mut written = String::new();
        for line in 0..5_000 {
            let text = format!("line {line}\n");
            buffer.push("stdout", &text, line);
            written.push_str(&text);
            assert!(buffer.retained_bytes <= max_bytes);
        }

        let chunks = buffer.snapshot();
        let retained = text_of(&chunks[1..]);
        assert!(retained.len() <= max_bytes);
        // Nearly the whole budget is used, and what is kept is the end.
        assert!(retained.len() > max_bytes / 2);
        assert!(written.ends_with(&retained));
        assert!(retained.ends_with("line 4999\n"));
    }

    #[test]
    fn dropped_output_is_reported_in_a_leading_notice() {
        let mut buffer = OutputBuffer::new(4 * 1024);
        let line = format!("{}\n", "y".repeat(99));
        for _ in 0..1_000 {
            buffer.push("stdout", &line, 7);
        }

        let chunks = buffer.snapshot();
        let retained = text_of(&chunks[1..]);
        let dropped = 1_000 * line.len() - retained.len();
        assert_eq!(chunks[0].stream, NOTICE_STREAM);
        assert_eq!(chunks[0].chunk, truncation_notice(dropped as u64));
        assert!(chunks[0].chunk.ends_with('\n'));
    }

    #[test]
    fn no_notice_while_nothing_has_been_dropped() {
        let mut buffer = OutputBuffer::new(4 * 1024);
        buffer.push("stdout", "short\n", 0);
        assert_eq!(text_of(&buffer.snapshot()), "short\n");
    }

    #[test]
    fn retained_output_starts_on_a_whole_line() {
        // Reads that end mid-line, as a full pipe produces them.
        let mut buffer = OutputBuffer::new(2 * 1024);
        let mut written = String::new();
        for line in 0..2_000 {
            written.push_str(&format!("line {line}\n"));
        }
        for read in written.as_bytes().chunks(100) {
            buffer.push("stdout", std::str::from_utf8(read).unwrap(), 0);
        }

        let chunks = buffer.snapshot();
        let retained = text_of(&chunks[1..]);
        assert!(retained.starts_with("line "));
        assert!(written.ends_with(&retained));
        // The cut-off line counts as dropped.
        let dropped = (written.len() - retained.len()) as u64;
        assert_eq!(chunks[0].chunk, truncation_notice(dropped));
    }

    #[test]
    fn a_whole_line_at_the_cut_is_kept() {
        // Reads that are whole lines, as a terminal produces them.
        let mut buffer = OutputBuffer::new(2 * 1024);
        let mut written = String::new();
        for line in 0..2_000 {
            let text = format!("line {line}\n");
            buffer.push("stdout", &text, 0);
            written.push_str(&text);
        }

        let chunks = buffer.snapshot();
        let retained = text_of(&chunks[1..]);
        let dropped = (written.len() - retained.len()) as u64;
        // Nothing beyond the evicted blocks is skipped.
        assert_eq!(dropped, buffer.dropped_bytes);
        assert!(retained.starts_with("line "));
    }

    #[test]
    fn carriage_return_redraws_are_capped_too() {
        let max_bytes = 4 * 1024;
        let mut buffer = OutputBuffer::new(max_bytes);
        for frame in 0..10_000 {
            buffer.push("stdout", &format!("\rprogress {frame}"), 0);
        }

        let chunks = buffer.snapshot();
        let retained = text_of(&chunks[1..]);
        assert!(retained.len() <= max_bytes);
        assert!(retained.starts_with("progress "));
        assert!(retained.ends_with("\rprogress 9999"));
    }

    #[test]
    fn alternating_streams_stay_within_the_bound() {
        let max_bytes = 4 * 1024;
        let mut buffer = OutputBuffer::new(max_bytes);
        for i in 0..10_000 {
            let stream = if i % 2 == 0 { "stdout" } else { "stderr" };
            buffer.push(stream, "x", 0);
            assert!(buffer.retained_bytes <= max_bytes);
        }
        // Every block is one byte, so the overhead dominates the budget.
        assert!(buffer.blocks.len() < max_bytes / BLOCK_OVERHEAD_BYTES);
    }

    #[test]
    fn sizes_are_formatted_in_readable_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(124 * 1024 * 1024), "124.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
