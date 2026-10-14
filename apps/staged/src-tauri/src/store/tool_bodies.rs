//! Read-side deferral of large tool-call bodies.
//!
//! A settled tool row carries up to three JSON bodies — `acp_raw_input`,
//! `acp_raw_output` and `acp_content` — that the UI only renders once the
//! tool card is expanded: a `read` result, a `bash` transcript, an edit's
//! old/new text and its diff block. Across a long session those bodies are
//! most of the transcript payload, so the transcript queries replace each
//! body larger than [`DEFERRED_TOOL_BODY_MIN_BYTES`] with a marker:
//!
//! ```json
//! {"$deferred": {"messageId": 42, "field": "output", "bytes": 18231}}
//! ```
//!
//! and the client fetches the row's bodies through
//! [`Store::get_tool_call_body`] (`GET /api/tool-bodies/{message_id}` on the
//! web, the `get_tool_call_body` command under Tauri) when the card opens.
//!
//! Only rows whose `acp_tool_status` is terminal are projected. In-flight
//! rows are mutated in place by the message writer and re-read by the poll,
//! so they ship whole; once a row has settled its body never changes again,
//! which is what lets the client (and the browser) cache a fetched body for
//! as long as it likes. The table itself is untouched: replay, archive and
//! initialization readers still see the full bodies.
//!
//! The marker carries the two facts the *collapsed* card reads out of a body
//! (everything else — verb, detail, status, whether the card is expandable —
//! comes from the row's other columns or from the marker's mere presence):
//! `innerSessionId` for the diagram-session button of a `generate_pikchr`
//! call, and `pikchr` for the inline diagram of a completed `render_pikchr`
//! call. Both mirror the reads in `acpTranscript.ts`.

use std::io;

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::{Map, Value};

use super::models::SessionMessage;
use super::tool_images::{project_blocks, ToolImageField};
use super::{Store, StoreError};

/// Serialized size (after tool images have been projected to refs) above
/// which a body is deferred. Below this a body costs about one compressed
/// TCP segment to ship inline, less than the round trip it would take to
/// fetch; the sweep over the two profiled local sessions put the knee here
/// (see the commit that introduced this module).
pub const DEFERRED_TOOL_BODY_MIN_BYTES: usize = 2048;

/// Key under which a deferred body's marker lives.
pub const DEFERRED_TOOL_BODY_KEY: &str = "$deferred";

/// Which body of the row a marker stands in for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolBodyField {
    /// `acp_raw_input`
    Input,
    /// `acp_raw_output`
    Output,
    /// `acp_content`
    Content,
}

impl ToolBodyField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Content => "content",
        }
    }
}

/// The bodies of one tool row, served on demand. Field names match the
/// `SessionMessage` wire format so the client can drop them straight into the
/// row it holds; tool images inside are projected to refs exactly as the
/// transcript queries would have.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallBody {
    pub message_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acp_raw_input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acp_raw_output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acp_content: Option<Value>,
}

/// The terminal statuses, as `TRANSCRIPT_METADATA_FILTER` in `messages.rs`
/// and `isToolMetadataSettled` in `acpTranscript.ts` spell them.
pub(super) fn is_settled_tool_status(status: Option<&str>) -> bool {
    matches!(
        status,
        Some("completed" | "succeeded" | "success" | "failed" | "error" | "cancelled" | "canceled")
    )
}

/// Replace each large body of a settled tool row with its marker. Used by
/// the transcript queries only, after [`super::tool_images::project_tool_images`].
pub(super) fn project_tool_bodies(message: &mut SessionMessage) {
    project_tool_bodies_above(message, DEFERRED_TOOL_BODY_MIN_BYTES);
}

pub(super) fn project_tool_bodies_above(message: &mut SessionMessage, min_bytes: usize) {
    if !is_settled_tool_status(message.acp.acp_tool_status.as_deref()) {
        return;
    }
    let id = message.id;
    let acp = &mut message.acp;
    defer_body(&mut acp.acp_raw_input, id, ToolBodyField::Input, min_bytes);
    defer_body(
        &mut acp.acp_raw_output,
        id,
        ToolBodyField::Output,
        min_bytes,
    );
    defer_body(&mut acp.acp_content, id, ToolBodyField::Content, min_bytes);
}

fn defer_body(body: &mut Option<Value>, message_id: i64, field: ToolBodyField, min_bytes: usize) {
    let Some(value) = body.as_ref() else {
        return;
    };
    let bytes = serialized_len(value);
    if bytes <= min_bytes {
        return;
    }
    let mut marker = Map::new();
    marker.insert("messageId".into(), Value::from(message_id));
    marker.insert("field".into(), Value::String(field.as_str().into()));
    marker.insert("bytes".into(), Value::from(bytes));
    if let Some(session_id) = inner_session_id(value) {
        marker.insert("innerSessionId".into(), Value::String(session_id));
    }
    if field == ToolBodyField::Input {
        // `pikchrSourceFromInput`: the source of a `render_pikchr` call is
        // what the collapsed card draws, so it rides along even though it is
        // the bulk of that input — a diagram's source is small next to the
        // bodies this module exists for.
        if let Some(source) = value
            .get("pikchr")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            marker.insert("pikchr".into(), Value::String(source.into()));
        }
    }
    let mut wrapper = Map::with_capacity(1);
    wrapper.insert(DEFERRED_TOOL_BODY_KEY.into(), Value::Object(marker));
    *body = Some(Value::Object(wrapper));
}

/// Bytes `serde_json` would write for the value, without buffering them.
fn serialized_len(value: &Value) -> usize {
    struct Count(usize);
    impl io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    // Serializing a `Value` cannot fail; a failure would only under-count.
    let _ = serde_json::to_writer(&mut count, value);
    count.0
}

/// The child session id a `generate_pikchr` result names, searched the way
/// `innerSessionIdFromValue` in `acpTranscript.ts` searches: a direct
/// `innerSessionId`/`inner_session_id`, else recursively under the
/// structured-content keys, through arrays, and into strings that hold JSON.
fn inner_session_id(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
                return None;
            }
            serde_json::from_str::<Value>(trimmed)
                .ok()
                .and_then(|parsed| inner_session_id(&parsed))
        }
        Value::Array(items) => items.iter().find_map(inner_session_id),
        Value::Object(object) => {
            let direct = object
                .get("innerSessionId")
                .or_else(|| object.get("inner_session_id"))
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty());
            if let Some(direct) = direct {
                return Some(direct.to_string());
            }
            [
                "structuredContent",
                "structured_content",
                "meta",
                "metadata",
                "data",
                "result",
            ]
            .iter()
            .filter_map(|key| object.get(*key))
            .find_map(inner_session_id)
        }
        _ => None,
    }
}

impl Store {
    /// The bodies a transcript marker stands in for, with tool images
    /// projected to refs. `None` when no such row exists or when the row has
    /// not settled: the transcript only emits markers for settled rows, and
    /// serving an in-flight one would let the route's `immutable` response
    /// cache bodies the writer may still replace. Same rule as
    /// [`Store::get_tool_image`].
    pub fn get_tool_call_body(&self, message_id: i64) -> Result<Option<ToolCallBody>, StoreError> {
        // input, output, content, status
        type BodyRow = (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let conn = self.conn.lock().unwrap();
        let row: Option<BodyRow> = conn
            .query_row(
                "SELECT acp_raw_input, acp_raw_output, acp_content, acp_tool_status
                 FROM session_messages WHERE id = ?1",
                params![message_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((input, output, content, status)) = row else {
            return Ok(None);
        };
        if !is_settled_tool_status(status.as_deref()) {
            return Ok(None);
        }
        let parse = |raw: Option<String>| raw.and_then(|s| serde_json::from_str::<Value>(&s).ok());
        let mut body = ToolCallBody {
            message_id,
            acp_raw_input: parse(input),
            acp_raw_output: parse(output),
            acp_content: parse(content),
        };
        if let Some(output) = body.acp_raw_output.as_mut() {
            project_blocks(output, message_id, ToolImageField::Output);
        }
        if let Some(content) = body.acp_content.as_mut() {
            project_blocks(content, message_id, ToolImageField::Content);
        }
        Ok(Some(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inner_session_id_mirrors_the_client_search() {
        assert_eq!(
            inner_session_id(&json!({"innerSessionId": "a"})).as_deref(),
            Some("a")
        );
        assert_eq!(
            inner_session_id(&json!({"result": {"structured_content": {"inner_session_id": "b"}}}))
                .as_deref(),
            Some("b")
        );
        assert_eq!(
            inner_session_id(&json!(["noise", "{\"meta\":{\"innerSessionId\":\"c\"}}"])).as_deref(),
            Some("c")
        );
        assert_eq!(inner_session_id(&json!({"innerSessionId": "  "})), None);
        assert_eq!(
            inner_session_id(&json!({"other": {"innerSessionId": "d"}})),
            None
        );
        // Only the structured-content keys are searched, as on the client.
        assert_eq!(
            inner_session_id(&json!([{"type": "text", "text": "{\"innerSessionId\":\"e\"}"}])),
            None
        );
        assert_eq!(inner_session_id(&json!("plain text")), None);
    }

    #[test]
    fn serialized_len_matches_serde_output() {
        for value in [
            json!(null),
            json!("héllo \"quoted\""),
            json!({"a": [1, 2.5, true, {"b": "c"}]}),
        ] {
            assert_eq!(
                serialized_len(&value),
                serde_json::to_vec(&value).unwrap().len()
            );
        }
    }
}
