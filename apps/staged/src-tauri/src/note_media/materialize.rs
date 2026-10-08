//! Agent-facing paths for durable note references.

use super::{files, refs};
use crate::store::Store;
use std::collections::HashMap;

/// Workspace-side file operations, so tests can observe transfers without a
/// Blox CLI. A transfer is dozens of chunked `ws_exec` round trips, while a
/// size check is one, so callers check before re-sending a stable `/tmp` copy.
pub(crate) trait RemoteMedia {
    /// Size of an existing remote file, or `None` when it is missing.
    fn size(&self, workspace: &str, path: &str) -> Option<i64>;
    fn write(&self, workspace: &str, path: &str, bytes: &[u8]) -> Result<(), String>;
}

pub(crate) struct BloxRemote;

impl RemoteMedia for BloxRemote {
    fn size(&self, workspace: &str, path: &str) -> Option<i64> {
        crate::blox::ws_exec(
            workspace,
            &["sh", "-c", "wc -c < \"$1\"", "staged-note-media", path],
        )
        .ok()?
        .trim()
        .parse()
        .ok()
    }

    fn write(&self, workspace: &str, path: &str, bytes: &[u8]) -> Result<(), String> {
        crate::session_commands::write_bytes_to_remote(workspace, bytes, path)
    }
}

pub(crate) fn materialize_note_media(
    store: &Store,
    content: &str,
    workspace_name: Option<&str>,
) -> String {
    materialize_note_media_with(store, content, workspace_name, &BloxRemote)
}

pub(crate) fn materialize_note_media_with(
    store: &Store,
    content: &str,
    workspace_name: Option<&str>,
    remote: &dyn RemoteMedia,
) -> String {
    let mut result = content.to_owned();
    let mut materialized = HashMap::new();
    let mut edits = Vec::new();
    for reference in refs::extract_media_refs(content) {
        if !reference.target.starts_with("staged-media:") {
            continue;
        }
        let replacement = materialized
            .entry(reference.target.clone())
            .or_insert_with(|| {
                let (id, ext) = refs::stored_ref(&reference.target)
                    .ok_or_else(|| "invalid media reference".to_string())?;
                let image = store
                    .get_image(id)
                    .map_err(|e| e.to_string())?
                    .ok_or("media unavailable")?;
                if files::mime_for_extension(ext) != Some(image.mime_type.as_str()) {
                    return Err("media format mismatch".into());
                }
                if workspace_name.is_some()
                    && image.mime_type.starts_with("video/")
                    && image.size_bytes > files::MAX_REMOTE_VIDEO_SIZE
                {
                    return Err(format!(
                        "media unavailable: {}, {} bytes, exceeds 20 MB remote video transfer limit",
                        refs::escape_label(&image.filename), image.size_bytes
                    ));
                }
                let source =
                    crate::store::images::image_file_path(&image.project_id, id, &image.filename)?;
                let Some(workspace) = workspace_name else {
                    return crate::session_commands::write_image_to_temp_file(
                        &source, id, ext, None,
                    )
                    .ok_or_else(|| "media unavailable".to_string());
                };
                // The id-based name is stable, so a same-sized copy from an
                // earlier context build is this file; a truncated one is not.
                let path = format!("/tmp/staged-image-{id}.{ext}");
                if remote.size(workspace, &path) != Some(image.size_bytes) {
                    let bytes = std::fs::read(&source).map_err(|_| "media unavailable")?;
                    remote.write(workspace, &path, &bytes).map_err(|e| {
                        log::warn!("Failed to write note media to remote workspace: {e}");
                        "media unavailable".to_string()
                    })?;
                }
                Ok(path)
            });
        match replacement {
            // Our generated paths have no markdown punctuation except the OS
            // temp root; escape it so they remain valid in bare destinations.
            Ok(path) => edits.push((
                reference.destination,
                path.replace('\\', "\\\\")
                    .replace(' ', "%20")
                    .replace('(', "\\(")
                    .replace(')', "\\)"),
            )),
            Err(reason) => {
                let caption = if reference.alt.is_empty() {
                    String::new()
                } else {
                    format!("{} — ", refs::escape_label(&reference.alt))
                };
                edits.push((
                    reference.span,
                    format!("{caption}{}", refs::unavailable(&reference.target, reason)),
                ))
            }
        }
    }
    for (span, replacement) in edits.into_iter().rev() {
        result.replace_range(span, &replacement);
    }
    result
}
