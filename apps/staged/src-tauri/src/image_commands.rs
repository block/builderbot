//! Image commands — image upload, retrieval, and management.

mod tool_results;
pub use tool_results::*;

use crate::store::Store;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Create an image record and copy the file to the project images directory.
///
/// When `pending` is true the image is hidden from the branch timeline until
/// a session is started (the session runner overwrites the sentinel with the
/// real session ID).  Pass `false` for images that should appear in the
/// timeline immediately (e.g. direct branch-card drops).
#[tauri::command(rename_all = "camelCase")]
pub fn create_image(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: Option<String>,
    project_id: String,
    file_path: String,
    pending: Option<bool>,
) -> Result<crate::store::Image, String> {
    let store = crate::get_store(&store)?;

    let src = Path::new(&file_path);
    let filename = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Invalid filename")?;
    let bytes =
        crate::note_media::files::read_local(src, crate::note_media::files::MAX_IMAGE_SIZE)?;
    crate::note_media::files::store_sniffed_image(
        &store,
        branch_id.as_deref(),
        &project_id,
        pending
            .unwrap_or(false)
            .then_some(crate::store::models::PENDING_SESSION_ID),
        filename,
        &bytes,
    )
}

/// Return the filesystem path for an image (the frontend uses convertFileSrc).
#[tauri::command(rename_all = "camelCase")]
pub fn get_image_path(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    image_id: String,
) -> Result<String, String> {
    let store = crate::get_store(&store)?;
    let image = store
        .get_image(&image_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Image not found: {image_id}"))?;
    let path =
        crate::store::images::image_file_path(&image.project_id, &image.id, &image.filename)?;
    Ok(path.to_string_lossy().to_string())
}

/// Delete an image record and its file on disk.
#[tauri::command(rename_all = "camelCase")]
pub fn delete_image(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    image_id: String,
) -> Result<(), String> {
    let store = crate::get_store(&store)?;
    let image = store
        .get_image(&image_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Image not found: {image_id}"))?;

    // Delete the DB record first (triggers session cleanup).
    store.delete_image(&image_id).map_err(|e| e.to_string())?;

    // Best-effort file removal.
    if let Ok(path) =
        crate::store::images::image_file_path(&image.project_id, &image.id, &image.filename)
    {
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("Failed to remove image file {}: {e}", path.display());
        }
    }

    Ok(())
}

/// List all images for a branch.
#[tauri::command(rename_all = "camelCase")]
pub fn list_branch_images(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<Vec<crate::store::Image>, String> {
    crate::get_store(&store)?
        .list_images_for_branch(&branch_id)
        .map_err(|e| e.to_string())
}

/// Read an image file and return its data as a base64-encoded data URL.
#[tauri::command(rename_all = "camelCase")]
pub fn get_image_data(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    image_id: String,
) -> Result<String, String> {
    let store = crate::get_store(&store)?;
    let image = store
        .get_image(&image_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Image not found: {image_id}"))?;
    let path = crate::store::images::image_file_path(&image.project_id, &image.id, &image.filename)
        .map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&path).map_err(|e| format!("Failed to read image: {e}"))?;
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{};base64,{}", image.mime_type, encoded))
}

/// Create an image from base64-encoded data (for browser file input / clipboard paste).
///
/// `mime_type` is the browser's label and is only accepted for compatibility;
/// the stored format follows the file signature. See [`create_image`] for the
/// meaning of the `pending` flag.
#[tauri::command(rename_all = "camelCase")]
pub fn create_image_from_data(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: Option<String>,
    project_id: String,
    filename: String,
    mime_type: String,
    data: String,
    pending: Option<bool>,
) -> Result<crate::store::Image, String> {
    let store = crate::get_store(&store)?;
    create_image_from_data_impl(
        store, branch_id, project_id, filename, mime_type, data, pending,
    )
}

pub(crate) fn create_image_from_data_impl(
    store: Arc<Store>,
    branch_id: Option<String>,
    project_id: String,
    filename: String,
    _mime_type: String,
    data: String,
    pending: Option<bool>,
) -> Result<crate::store::Image, String> {
    use base64::Engine;
    if data.len() as u64 > crate::note_media::files::MAX_IMAGE_SIZE.div_ceil(3) * 4 {
        return Err("Image too large (max 10 MB)".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&data)
        .map_err(|e| format!("Invalid base64 data: {e}"))?;

    crate::note_media::files::store_sniffed_image(
        &store,
        branch_id.as_deref(),
        &project_id,
        pending
            .unwrap_or(false)
            .then_some(crate::store::models::PENDING_SESSION_ID),
        &filename,
        &bytes,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note_media::tests::{Fixture, PNG};
    use base64::Engine;

    fn attach(
        f: &Fixture,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<crate::store::Image, String> {
        create_image_from_data_impl(
            f.store.clone(),
            Some(f.branch.id.clone()),
            f.project.id.clone(),
            name.into(),
            mime.into(),
            base64::engine::general_purpose::STANDARD.encode(bytes),
            Some(true),
        )
    }

    #[test]
    fn chat_attachments_are_stored_under_their_sniffed_format() {
        let f = Fixture::new();
        let webp = b"RIFF1234WEBPfixture";
        let image = attach(&f, "shot.png", "image/png", webp).unwrap();
        assert_eq!(image.mime_type, "image/webp");
        assert_eq!(image.filename, "shot.webp");
        let path =
            crate::store::images::image_file_path(&image.project_id, &image.id, &image.filename)
                .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), webp);

        // A truthful name is kept, including the alternate JPEG spelling.
        assert_eq!(
            attach(&f, "a.jpeg", "", b"\xff\xd8\xffx").unwrap().filename,
            "a.jpeg"
        );
        assert_eq!(
            attach(&f, "shot.png", "image/png", PNG).unwrap().filename,
            "shot.png"
        );
        assert_eq!(
            attach(&f, "pasted", "", PNG).unwrap().filename,
            "pasted.png"
        );

        let err = attach(&f, "shot.png", "image/png", b"not an image").unwrap_err();
        assert!(err.contains("unsupported image format"), "{err}");
        let err = attach(&f, "clip.png", "", b"\0\0\0\x18ftypisom\0\0\0\0").unwrap_err();
        assert!(err.contains("only images"), "{err}");
    }
}
