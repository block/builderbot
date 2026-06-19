//! Slack event parsing.
//!
//! Pure functions that turn a raw Slack `app_mention` event payload into an
//! [`AgentTask`]. No network or side effects, so this is fully unit-testable
//! against captured JSON fixtures.

use serde::Deserialize;

/// A task derived from a Slack mention, ready to hand to the agent runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTask {
    /// Channel the mention arrived in.
    pub channel: String,
    /// Thread to reply into. For a root-level mention this is the mention's own
    /// `ts`, so progress lands in a fresh thread under the mention.
    pub thread_ts: String,
    /// The prompt text with the leading bot mention stripped.
    pub prompt: String,
    /// The user who mentioned the bot, if present.
    pub user: Option<String>,
}

/// Subset of a Slack `app_mention` event we care about.
///
/// See https://api.slack.com/events/app_mention
#[derive(Debug, Deserialize)]
struct AppMentionEvent {
    #[serde(rename = "type")]
    kind: String,
    channel: Option<String>,
    user: Option<String>,
    #[serde(default)]
    text: String,
    ts: Option<String>,
    thread_ts: Option<String>,
    /// Present when the message was posted by a bot. We ignore those to avoid
    /// the bridge replying to itself in a loop.
    bot_id: Option<String>,
}

/// Parse a raw `app_mention` event payload into an [`AgentTask`].
///
/// Returns `None` (the event should be ignored) when:
/// - the event is not an `app_mention`,
/// - it originated from a bot (`bot_id` set),
/// - it is missing the channel or timestamp, or
/// - the prompt is empty once the mention is stripped.
pub fn parse_app_mention(payload: &serde_json::Value) -> Option<AgentTask> {
    let event: AppMentionEvent = serde_json::from_value(payload.clone()).ok()?;

    if event.kind != "app_mention" {
        return None;
    }
    if event.bot_id.is_some() {
        return None;
    }

    let channel = event.channel?;
    let ts = event.ts?;

    let prompt = strip_leading_mention(&event.text).trim().to_string();
    if prompt.is_empty() {
        return None;
    }

    Some(AgentTask {
        channel,
        thread_ts: event.thread_ts.unwrap_or(ts),
        prompt,
        user: event.user,
    })
}

/// Remove a single leading Slack mention token (`<@U…>` or `<@W…|name>`) from
/// the start of `text`, along with any whitespace that follows it.
///
/// Slack always places the bot mention at the start of an `app_mention` event,
/// so stripping the first token is sufficient and avoids needing the bot's user
/// id at parse time. Mentions appearing later in the text are left intact.
pub fn strip_leading_mention(text: &str) -> &str {
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<@") {
        if let Some(end) = rest.find('>') {
            return rest[end + 1..].trim_start();
        }
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strips_leading_mention_token() {
        assert_eq!(strip_leading_mention("<@U123> hello world"), "hello world");
    }

    #[test]
    fn strips_mention_with_display_name_form() {
        assert_eq!(strip_leading_mention("<@W999|builderbot> go"), "go");
    }

    #[test]
    fn leaves_later_mentions_intact() {
        assert_eq!(
            strip_leading_mention("<@U123> ping <@U456> please"),
            "ping <@U456> please"
        );
    }

    #[test]
    fn passes_through_text_without_mention() {
        assert_eq!(strip_leading_mention("just text"), "just text");
    }

    #[test]
    fn parses_basic_mention_into_task() {
        let payload = json!({
            "type": "app_mention",
            "channel": "C123",
            "user": "U999",
            "text": "<@UBOT> run the tests",
            "ts": "1700000000.000100"
        });
        let task = parse_app_mention(&payload).expect("should parse");
        assert_eq!(
            task,
            AgentTask {
                channel: "C123".into(),
                thread_ts: "1700000000.000100".into(),
                prompt: "run the tests".into(),
                user: Some("U999".into()),
            }
        );
    }

    #[test]
    fn uses_thread_ts_when_replying_in_existing_thread() {
        let payload = json!({
            "type": "app_mention",
            "channel": "C123",
            "text": "<@UBOT> continue",
            "ts": "1700000000.000200",
            "thread_ts": "1700000000.000001"
        });
        let task = parse_app_mention(&payload).expect("should parse");
        assert_eq!(task.thread_ts, "1700000000.000001");
    }

    #[test]
    fn preserves_multiline_prompt() {
        let payload = json!({
            "type": "app_mention",
            "channel": "C1",
            "text": "<@UBOT> line one\nline two",
            "ts": "1.1"
        });
        let task = parse_app_mention(&payload).expect("should parse");
        assert_eq!(task.prompt, "line one\nline two");
    }

    #[test]
    fn ignores_bot_authored_mentions() {
        let payload = json!({
            "type": "app_mention",
            "channel": "C1",
            "text": "<@UBOT> hi",
            "ts": "1.1",
            "bot_id": "B123"
        });
        assert_eq!(parse_app_mention(&payload), None);
    }

    #[test]
    fn ignores_non_mention_events() {
        let payload = json!({
            "type": "message",
            "channel": "C1",
            "text": "<@UBOT> hi",
            "ts": "1.1"
        });
        assert_eq!(parse_app_mention(&payload), None);
    }

    #[test]
    fn ignores_mention_with_empty_prompt() {
        let payload = json!({
            "type": "app_mention",
            "channel": "C1",
            "text": "<@UBOT>   ",
            "ts": "1.1"
        });
        assert_eq!(parse_app_mention(&payload), None);
    }

    #[test]
    fn ignores_event_missing_channel() {
        let payload = json!({
            "type": "app_mention",
            "text": "<@UBOT> hi",
            "ts": "1.1"
        });
        assert_eq!(parse_app_mention(&payload), None);
    }
}
