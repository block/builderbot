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
pub async fn create_image(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: Option<String>,
    project_id: String,
    file_path: String,
    pending: Option<bool>,
) -> Result<crate::store::Image, String> {
    let store = crate::get_store(&store)?;
    // Storing may rasterize a PDF page; keep that off the main thread.
    tauri::async_runtime::spawn_blocking(move || {
        create_image_impl(store, branch_id, project_id, file_path, pending)
    })
    .await
    .map_err(|e| format!("Failed to store image: {e}"))?
}

fn create_image_impl(
    store: Arc<Store>,
    branch_id: Option<String>,
    project_id: String,
    file_path: String,
    pending: Option<bool>,
) -> Result<crate::store::Image, String> {
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
pub async fn create_image_from_data(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: Option<String>,
    project_id: String,
    filename: String,
    mime_type: String,
    data: String,
    pending: Option<bool>,
) -> Result<crate::store::Image, String> {
    let store = crate::get_store(&store)?;
    // See `create_image`: PDF rendering must not block the main thread.
    tauri::async_runtime::spawn_blocking(move || {
        create_image_from_data_impl(
            store, branch_id, project_id, filename, mime_type, data, pending,
        )
    })
    .await
    .map_err(|e| format!("Failed to store image: {e}"))?
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

    #[cfg(target_os = "macos")]
    #[test]
    fn pdf_attachments_are_rasterized_to_png() {
        use crate::note_media::pdf::tests::{tiny_pdf, PAGE_ONE};
        let f = Fixture::new();
        let image = attach(&f, "doc.pdf", "application/pdf", &tiny_pdf(&[PAGE_ONE])).unwrap();
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.filename, "doc.png");
        let path =
            crate::store::images::image_file_path(&image.project_id, &image.id, &image.filename)
                .unwrap();
        let png = std::fs::read(path).unwrap();
        assert_eq!(image.size_bytes, png.len() as i64);
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let pixmap = tiny_skia::Pixmap::decode_png(&png).unwrap();
        assert!(pixmap.width().max(pixmap.height()) <= 2048);
        let corner = pixmap.pixel(0, 0).unwrap();
        assert_eq!(
            (corner.red(), corner.green(), corner.blue(), corner.alpha()),
            (255, 255, 255, 255)
        );

        // Pasted PDFs arrive without a usable name, like pasted images.
        assert_eq!(
            attach(&f, "pasted", "application/pdf", &tiny_pdf(&[PAGE_ONE]))
                .unwrap()
                .filename,
            "pasted.png"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn malformed_pdf_is_rejected_without_hanging() {
        let f = Fixture::new();
        let err = attach(&f, "doc.pdf", "application/pdf", b"%PDF-1.4\ngarbage").unwrap_err();
        assert!(err.contains("PDF"), "{err}");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn pdf_attachments_report_platform_support() {
        use crate::note_media::pdf::tests::{tiny_pdf, PAGE_ONE};
        let f = Fixture::new();
        let err = attach(&f, "doc.pdf", "application/pdf", &tiny_pdf(&[PAGE_ONE])).unwrap_err();
        assert!(err.contains("macOS"), "{err}");
    }
}
