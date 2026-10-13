//! Read-side projection of base64 tool-result images.
//!
//! Agents return screenshots and `read` results for image files as base64
//! blocks inside `acp_raw_output` (Claude's `{"type":"image","source":
//! {"type":"base64","data":…}}`) and again inside `acp_content` (ACP's
//! `{"type":"content","content":{"type":"image","data":…,"mimeType":…}}`).
//! A session with a couple of dozen screenshots carries ~12 MB of base64 that
//! the client only ever looks at once a tool card is expanded. The transcript
//! queries therefore replace each block's payload with a reference:
//!
//! ```json
//! {"type":"image","ref":"<message_id>/<output|content>/<index>","mimeType":"image/png"}
//! ```
//!
//! and the bytes are fetched on demand through [`Store::get_tool_image`]
//! (`GET /api/tool-images/{ref}` on the web, the `get_tool_image` command
//! under Tauri). The table itself is untouched: `session_messages` keeps the
//! base64, and the replay/archive readers still see it.

use std::fmt;

use rusqlite::params;
use serde_json::{Map, Value};

use super::models::SessionMessage;
use super::{Store, StoreError};

/// Which column of the row a reference points into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolImageField {
    /// `acp_raw_output`
    Output,
    /// `acp_content`
    Content,
}

impl ToolImageField {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "output" => Some(Self::Output),
            "content" => Some(Self::Content),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Output => "output",
            Self::Content => "content",
        }
    }

    fn column(self) -> &'static str {
        match self {
            Self::Output => "acp_raw_output",
            Self::Content => "acp_content",
        }
    }
}

impl fmt::Display for ToolImageField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A decoded tool-result image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolImage {
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

/// Rewrite every base64 image block in the message's ACP output and content
/// into reference form. Used by the transcript queries only.
pub(super) fn project_tool_images(message: &mut SessionMessage) {
    let id = message.id;
    if let Some(output) = message.acp.acp_raw_output.as_mut() {
        project_blocks(output, id, ToolImageField::Output);
    }
    if let Some(content) = message.acp.acp_content.as_mut() {
        project_blocks(content, id, ToolImageField::Content);
    }
}

fn project_blocks(value: &mut Value, message_id: i64, field: ToolImageField) {
    let Some(blocks) = blocks_mut(value) else {
        return;
    };
    for (index, entry) in blocks.iter_mut().enumerate() {
        let Some(block) = image_block_mut(entry) else {
            continue;
        };
        let Some((mime_type, _)) = base64_image(block) else {
            continue;
        };
        let mut projected = Map::new();
        if let Some(object) = block.as_object() {
            for (key, value) in object {
                // `source` only ever holds the payload and its media type.
                if key != "source" && key != "data" && key != "mimeType" && key != "media_type" {
                    projected.insert(key.clone(), value.clone());
                }
            }
        }
        projected.insert("type".into(), Value::String("image".into()));
        projected.insert(
            "ref".into(),
            Value::String(format!("{message_id}/{field}/{index}")),
        );
        projected.insert("mimeType".into(), Value::String(mime_type));
        *block = Value::Object(projected);
    }
}

/// The block list a tool result carries: either the value itself (ACP and
/// Claude) or its `content` array (MCP results).
fn blocks_mut(value: &mut Value) -> Option<&mut Vec<Value>> {
    match value {
        Value::Array(blocks) => Some(blocks),
        Value::Object(object) => object.get_mut("content").and_then(Value::as_array_mut),
        _ => None,
    }
}

fn blocks(value: &Value) -> Option<&Vec<Value>> {
    match value {
        Value::Array(blocks) => Some(blocks),
        Value::Object(object) => object.get("content").and_then(Value::as_array),
        _ => None,
    }
}

/// Unwrap ACP's `{"type":"content","content":{…}}` envelope, then accept the
/// entry only if it is an image block.
fn image_block_mut(entry: &mut Value) -> Option<&mut Value> {
    let block = if entry.get("type").and_then(Value::as_str) == Some("content")
        && entry.get("content").is_some_and(Value::is_object)
    {
        entry.get_mut("content")?
    } else {
        entry
    };
    (block.get("type").and_then(Value::as_str) == Some("image")).then_some(block)
}

fn image_block(entry: &Value) -> Option<&Value> {
    let block = if entry.get("type").and_then(Value::as_str) == Some("content")
        && entry.get("content").is_some_and(Value::is_object)
    {
        entry.get("content")?
    } else {
        entry
    };
    (block.get("type").and_then(Value::as_str) == Some("image")).then_some(block)
}

/// `(mime type, base64 payload)` of an image block that embeds its data.
/// ACP/MCP put `data`/`mimeType` on the block; Claude nests them in `source`
/// with `media_type`. Mirrors `toolCallImageContent` in `toolCallImages.ts`,
/// so the server projects exactly the blocks the client would have rendered
/// inline.
fn base64_image(block: &Value) -> Option<(String, &str)> {
    let source = match block.get("source") {
        Some(source) if source.is_object() => source,
        _ => block,
    };
    let data = source.get("data")?.as_str()?;
    if data.is_empty() {
        return None;
    }
    let mime_type = source
        .get("mimeType")
        .or_else(|| source.get("media_type"))?
        .as_str()?;
    is_image_mime_type(mime_type).then(|| (mime_type.to_string(), data))
}

/// The client's `/^image\/[a-z0-9.+-]+$/i`; also what the route is willing
/// to put in a `Content-Type` header.
pub fn is_image_mime_type(value: &str) -> bool {
    let Some(subtype) = value
        .get(..6)
        .filter(|prefix| prefix.eq_ignore_ascii_case("image/"))
        .and(value.get(6..))
    else {
        return false;
    };
    !subtype.is_empty()
        && subtype
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
}

impl Store {
    /// Decode the image block a transcript reference points at. `None` when
    /// the row, field or index does not resolve to a base64 image block.
    pub fn get_tool_image(
        &self,
        message_id: i64,
        field: ToolImageField,
        index: usize,
    ) -> Result<Option<ToolImage>, StoreError> {
        let raw: Option<String> = {
            let conn = self.conn.lock().unwrap();
            let sql = format!(
                "SELECT {} FROM session_messages WHERE id = ?1",
                field.column()
            );
            match conn.query_row(&sql, params![message_id], |row| row.get(0)) {
                Ok(raw) => raw,
                Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
                Err(e) => return Err(e.into()),
            }
        };
        let Some(value) = raw.and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
            return Ok(None);
        };
        let Some((mime_type, data)) = blocks(&value)
            .and_then(|blocks| blocks.get(index))
            .and_then(image_block)
            .and_then(base64_image)
        else {
            return Ok(None);
        };
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .map_err(|e| {
                StoreError(format!("Invalid base64 image in message {message_id}: {e}"))
            })?;
        Ok(Some(ToolImage { mime_type, bytes }))
    }
}
