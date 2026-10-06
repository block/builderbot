//! Agent-facing paths for durable note references.

use super::{files, refs};
use crate::store::Store;
use std::collections::HashMap;

pub(crate) fn materialize_note_media(
    store: &Store,
    content: &str,
    workspace_name: Option<&str>,
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
                crate::session_commands::write_image_to_temp_file(&source, id, ext, workspace_name)
                    .ok_or_else(|| "media unavailable".to_string())
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
