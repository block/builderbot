//! Image commands — image upload, retrieval, and management.

use crate::store::Store;
use std::path::Path;
use std::sync::{Arc, Mutex};

const MAX_IMAGE_SIZE: u64 = 10_485_760; // 10 MB

const ALLOWED_IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

fn mime_type_for_extension(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

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
    if !src.exists() {
        return Err(format!("File not found: {file_path}"));
    }

    let filename = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "Invalid filename".to_string())?
        .to_string();

    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if !ALLOWED_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!(
            "Unsupported image format: .{ext}. Allowed: {}",
            ALLOWED_IMAGE_EXTENSIONS.join(", ")
        ));
    }

    let metadata = std::fs::metadata(src).map_err(|e| format!("Cannot read file metadata: {e}"))?;
    if metadata.len() > MAX_IMAGE_SIZE {
        return Err(format!(
            "File too large ({} bytes). Maximum is {} bytes.",
            metadata.len(),
            MAX_IMAGE_SIZE
        ));
    }

    let mime_type = mime_type_for_extension(&ext).to_string();
    let size_bytes = metadata.len() as i64;

    let filename = store
        .unique_image_filename(branch_id.as_deref(), &project_id, &filename)
        .map_err(|e| e.to_string())?;

    let image = crate::store::Image::new(
        branch_id.as_deref(),
        &project_id,
        &filename,
        &mime_type,
        size_bytes,
        pending.unwrap_or(false),
    );

    // Compute destination path and ensure the images directory exists.
    let dest = crate::store::images::image_file_path(&project_id, &image.id, &filename)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create images directory: {e}"))?;
    }

    // Copy the file to the images directory.
    std::fs::copy(src, &dest).map_err(|e| format!("Cannot copy image file: {e}"))?;

    // Persist the DB record.
    if let Err(e) = store.create_image(&image) {
        let _ = std::fs::remove_file(&dest);
        return Err(e.to_string());
    }

    Ok(image)
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
/// See [`create_image`] for the meaning of the `pending` flag.
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
    mime_type: String,
    data: String,
    pending: Option<bool>,
) -> Result<crate::store::Image, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&data)
        .map_err(|e| format!("Invalid base64 data: {e}"))?;

    // Validate size
    if bytes.len() as u64 > MAX_IMAGE_SIZE {
        return Err(format!(
            "Image too large: {} bytes (max {})",
            bytes.len(),
            MAX_IMAGE_SIZE
        ));
    }

    // Validate extension
    let ext = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !ALLOWED_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!("Unsupported image format: .{ext}"));
    }

    const ALLOWED_MIME_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
    let mime = if mime_type.is_empty() {
        mime_type_for_extension(&ext).to_string()
    } else {
        if !ALLOWED_MIME_TYPES.contains(&mime_type.as_str()) {
            return Err(format!(
                "Unsupported MIME type: {mime_type}. Allowed: {}",
                ALLOWED_MIME_TYPES.join(", ")
            ));
        }
        mime_type
    };

    let filename = store
        .unique_image_filename(branch_id.as_deref(), &project_id, &filename)
        .map_err(|e| e.to_string())?;

    let image = crate::store::Image::new(
        branch_id.as_deref(),
        &project_id,
        &filename,
        &mime,
        bytes.len() as i64,
        pending.unwrap_or(false),
    );
    let path = crate::store::images::image_file_path(&project_id, &image.id, &filename)
        .map_err(|e| e.to_string())?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create images directory: {e}"))?;
    }
    std::fs::write(&path, &bytes).map_err(|e| format!("Failed to save image: {e}"))?;

    if let Err(e) = store.create_image(&image) {
        let _ = std::fs::remove_file(&path);
        return Err(e.to_string());
    }
    Ok(image)
}
