use crate::note_media::files::{mime_for_extension, size_error, MAX_IMAGE_SIZE};
const ALLOWED_IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];
use base64::Engine;
use std::io::Read;
use std::path::Path;

/// Load a local image referenced by a tool result without persisting an attachment.
///
/// Runs [`read_image_file_impl`] on the blocking pool: as a synchronous
/// command the read of up to [`MAX_IMAGE_SIZE`] and its base64 encoding held
/// the main thread, and so the window.
#[tauri::command(rename_all = "camelCase")]
pub async fn read_image_file(file_path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || read_image_file_impl(file_path))
        .await
        .map_err(|e| format!("read_image_file task failed: {e}"))?
}

/// The body of [`read_image_file`], shared with the web dispatcher's arm.
pub(crate) fn read_image_file_impl(file_path: String) -> Result<String, String> {
    let path = Path::new(&file_path);
    if !path.is_absolute() {
        return Err("Image path must be absolute".to_string());
    }
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !ALLOWED_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!("Unsupported image format: .{ext}"));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("Failed to open image: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("Failed to read image metadata: {e}"))?;
    if !metadata.is_file() {
        return Err("Image path is not a file".to_string());
    }
    if metadata.len() > MAX_IMAGE_SIZE {
        return Err(size_error("Image", MAX_IMAGE_SIZE));
    }
    let mut bytes = Vec::new();
    file.take(MAX_IMAGE_SIZE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Failed to read image: {e}"))?;
    if bytes.len() as u64 > MAX_IMAGE_SIZE {
        return Err(size_error("Image", MAX_IMAGE_SIZE));
    }
    Ok(format!(
        "data:{};base64,{}",
        mime_for_extension(&ext).unwrap(),
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_image_as_data_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview.PNG");
        let bytes = include_bytes!("../../icons/32x32.png");
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(
            read_image_file_impl(path.to_string_lossy().into_owned()).unwrap(),
            format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            )
        );
    }

    #[test]
    fn rejects_unsupported_missing_and_relative_files() {
        let dir = tempfile::tempdir().unwrap();
        for path in [
            dir.path().join("missing.png"),
            dir.path().join("document.txt"),
            Path::new("relative.png").to_path_buf(),
        ] {
            assert!(read_image_file_impl(path.to_string_lossy().into_owned()).is_err());
        }
        let directory = dir.path().join("directory.png");
        std::fs::create_dir(&directory).unwrap();
        assert!(read_image_file_impl(directory.to_string_lossy().into_owned()).is_err());
    }

    #[test]
    fn rejects_oversized_images() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.png");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_IMAGE_SIZE + 1)
            .unwrap();
        assert!(read_image_file_impl(path.to_string_lossy().into_owned())
            .unwrap_err()
            .contains("exceeds 10 MB limit"));
    }
}
