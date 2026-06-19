//! Progress message formatting.
//!
//! [`ProgressState`] accumulates streamed agent output (text chunks and tool
//! activity) and renders a single Slack-safe message body. Keeping this pure
//! lets the live bridge simply edit one thread message as state evolves, and
//! lets us unit-test formatting without touching Slack.

/// Conservative ceiling for a single Slack message. The hard limit is ~4000
/// characters; we stay under it to leave room for headers and ellipsis.
pub const SLACK_MAX_MESSAGE_CHARS: usize = 3900;

/// Per-tool-result preview cap, so a single noisy command can't dominate the
/// message.
pub const TOOL_RESULT_PREVIEW_CHARS: usize = 300;

const TRUNCATION_SUFFIX: &str = "\n…(truncated)";

/// Accumulated progress for one agent task, rendered into a thread message.
#[derive(Debug, Clone, Default)]
pub struct ProgressState {
    text: String,
    activity: Vec<String>,
    done: bool,
    failed: Option<String>,
}

impl ProgressState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a streamed text chunk from the agent's message.
    pub fn append_text(&mut self, chunk: &str) {
        self.text.push_str(chunk);
    }

    /// Record that the agent started a tool call.
    pub fn record_tool_call(&mut self, title: &str) {
        self.activity.push(format_tool_call(title));
    }

    /// Record a tool result, stored as a short preview line.
    pub fn record_tool_result(&mut self, content: &str) {
        let preview = collapse_whitespace(content);
        if preview.is_empty() {
            return;
        }
        self.activity.push(format!(
            "   ↳ {}",
            truncate(&preview, TOOL_RESULT_PREVIEW_CHARS)
        ));
    }

    /// Mark the task as finished successfully.
    pub fn mark_done(&mut self) {
        self.done = true;
    }

    /// Mark the task as failed with an error message.
    pub fn mark_failed(&mut self, error: &str) {
        self.failed = Some(error.to_string());
        self.done = true;
    }

    /// True when nothing meaningful has been produced yet.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.activity.is_empty()
    }

    /// Render the current state into a Slack-safe message body.
    pub fn render(&self) -> String {
        let mut body = String::new();
        body.push_str(self.header());

        if !self.activity.is_empty() {
            body.push_str("\n\n");
            body.push_str(&self.activity.join("\n"));
        }

        let text = self.text.trim();
        if !text.is_empty() {
            body.push_str("\n\n");
            body.push_str(text);
        }

        if let Some(err) = &self.failed {
            body.push_str("\n\n> ");
            body.push_str(&collapse_whitespace(err));
        }

        truncate(&body, SLACK_MAX_MESSAGE_CHARS)
    }

    fn header(&self) -> &str {
        if self.failed.is_some() {
            "❌ builderbot failed"
        } else if self.done {
            "✅ builderbot finished"
        } else {
            "🤖 builderbot is working…"
        }
    }

    /// The raw failure message, if any (used by callers that want detail).
    pub fn failure(&self) -> Option<&str> {
        self.failed.as_deref()
    }
}

/// Format a tool call title into an activity line.
pub fn format_tool_call(title: &str) -> String {
    let title = collapse_whitespace(title);
    if title.is_empty() {
        "🔧 (tool call)".to_string()
    } else {
        format!("🔧 {title}")
    }
}

/// Truncate `s` to at most `max` characters, appending a marker when cut.
///
/// Operates on `char` boundaries so multibyte text is never split.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let suffix_len = TRUNCATION_SUFFIX.chars().count();
    // If `max` is too small to hold the marker, hard-cut to `max` chars.
    if max <= suffix_len {
        return s.chars().take(max).collect();
    }
    let head: String = s.chars().take(max - suffix_len).collect();
    format!("{head}{TRUNCATION_SUFFIX}")
}

/// Collapse runs of whitespace (including newlines) into single spaces and trim.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_tool_call_with_title() {
        assert_eq!(format_tool_call("cargo test"), "🔧 cargo test");
    }

    #[test]
    fn formats_tool_call_collapsing_whitespace() {
        assert_eq!(format_tool_call("run\n  the   tests"), "🔧 run the tests");
    }

    #[test]
    fn formats_empty_tool_call() {
        assert_eq!(format_tool_call("   "), "🔧 (tool call)");
    }

    #[test]
    fn truncate_leaves_short_strings_untouched() {
        assert_eq!(truncate("hello", 100), "hello");
    }

    #[test]
    fn truncate_cuts_long_strings_and_marks_them() {
        let out = truncate(&"a".repeat(50), 20);
        assert!(out.chars().count() <= 20);
        assert!(out.ends_with("…(truncated)"));
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        // 10 multibyte chars; cap at 5 must not panic or split a char.
        let out = truncate(&"é".repeat(10), 5);
        assert!(out.chars().count() <= 5);
    }

    #[test]
    fn renders_working_header_for_in_progress_task() {
        let mut state = ProgressState::new();
        state.append_text("looking into it");
        let out = state.render();
        assert!(out.starts_with("🤖 builderbot is working…"));
        assert!(out.contains("looking into it"));
    }

    #[test]
    fn renders_done_header_after_finish() {
        let mut state = ProgressState::new();
        state.append_text("all set");
        state.mark_done();
        assert!(state.render().starts_with("✅ builderbot finished"));
    }

    #[test]
    fn renders_failed_header_and_exposes_detail() {
        let mut state = ProgressState::new();
        state.mark_failed("agent not found");
        let out = state.render();
        assert!(out.starts_with("❌ builderbot failed"));
        assert!(out.contains("agent not found"));
        assert_eq!(state.failure(), Some("agent not found"));
    }

    #[test]
    fn renders_tool_activity_before_text() {
        let mut state = ProgressState::new();
        state.record_tool_call("cargo test");
        state.record_tool_result("ok, 12 passed");
        state.append_text("done");
        let out = state.render();
        let tool_idx = out.find("🔧 cargo test").unwrap();
        let result_idx = out.find("↳ ok, 12 passed").unwrap();
        let text_idx = out.find("done").unwrap();
        assert!(tool_idx < result_idx);
        assert!(result_idx < text_idx);
    }

    #[test]
    fn coalesces_streamed_text_chunks() {
        let mut state = ProgressState::new();
        state.append_text("hel");
        state.append_text("lo ");
        state.append_text("world");
        assert!(state.render().contains("hello world"));
    }

    #[test]
    fn detects_empty_state() {
        let mut state = ProgressState::new();
        assert!(state.is_empty());
        state.append_text("   ");
        assert!(state.is_empty());
        state.append_text("x");
        assert!(!state.is_empty());
    }

    #[test]
    fn render_is_within_slack_limit() {
        let mut state = ProgressState::new();
        state.append_text(&"x".repeat(10_000));
        let out = state.render();
        assert!(out.chars().count() <= SLACK_MAX_MESSAGE_CHARS);
    }

    #[test]
    fn tool_result_preview_is_truncated() {
        let mut state = ProgressState::new();
        state.record_tool_result(&"y".repeat(1000));
        let out = state.render();
        assert!(out.contains("…(truncated)"));
    }
}
