//! Image commands — image upload, retrieval, and management.

mod thumbnail;
mod tool_results;
pub use thumbnail::*;
pub use tool_results::*;

use crate::store::Store;
use std::collections::HashMap;
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
    // Reading and validating a 50 MB PDF is not main-thread work.
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
    let bytes = crate::note_media::files::read_local_attachment(src)?;
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

    // Best-effort removal of the file and any cached PDF thumbnail.
    crate::store::images::remove_image_files(&image.project_id, &image.id, &image.filename);

    Ok(())
}

/// A small preview of an attachment: the image itself, or for a PDF a PNG of
/// its first page, rendered on first use and cached beside the file.
#[tauri::command(rename_all = "camelCase")]
pub async fn get_attachment_thumbnail(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    image_id: String,
) -> Result<AttachmentThumbnail, String> {
    let store = crate::get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || attachment_thumbnail_impl(&store, &image_id))
        .await
        .map_err(|e| format!("Failed to load thumbnail: {e}"))?
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

/// Size and density of the raster images a note embeds, keyed by id, for
/// `mediaRendering.ts` to size them at logical pixels. Ids that are unknown,
/// not raster images, or unreadable are left out. Rows that predate the
/// dimension columns are read from disk and filled in on this first use,
/// whatever the read found, so a file is opened at most once; only a file
/// that is missing altogether is looked for again.
#[tauri::command(rename_all = "camelCase")]
pub async fn get_media_dimensions(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    image_ids: Vec<String>,
) -> Result<HashMap<String, crate::store::ImageDimensions>, String> {
    let store = crate::get_store(&store)?;
    // Backfilling reads whole files (headers only are parsed, but a legacy
    // note may embed a dozen multi-megabyte captures).
    tauri::async_runtime::spawn_blocking(move || media_dimensions_impl(&store, &image_ids))
        .await
        .map_err(|e| format!("Dimensions task failed: {e}"))
}

pub(crate) fn media_dimensions_impl(
    store: &Store,
    image_ids: &[String],
) -> HashMap<String, crate::store::ImageDimensions> {
    let mut found = HashMap::new();
    for id in image_ids {
        let Ok(Some(image)) = store.get_image(id) else {
            continue;
        };
        let dimensions = match image.dimensions {
            Some(dimensions) => Some(dimensions),
            None if image.mime_type.starts_with("image/")
                && !store.image_dimensions_recorded(id).unwrap_or(true) =>
            {
                let bytes = crate::store::images::image_file_path(
                    &image.project_id,
                    &image.id,
                    &image.filename,
                )
                .ok()
                .and_then(|path| std::fs::read(path).ok());
                // A missing file is left NULL: it costs a stat to miss again,
                // and a later transfer or restore may put it back.
                let Some(bytes) = bytes else { continue };
                let dimensions = crate::note_media::dimensions::probe(&bytes);
                if let Err(e) = store.set_image_dimensions(id, dimensions) {
                    log::warn!("Failed to record dimensions of image {id}: {e}");
                }
                dimensions
            }
            None => None,
        };
        if let Some(dimensions) = dimensions {
            found.insert(id.clone(), dimensions);
        }
    }
    found
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
    // See `create_image`: decoding a large payload must not block the main thread.
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
    let engine = base64::engine::general_purpose::STANDARD;
    // The cap depends on the payload (a PDF may be larger than an image), so
    // peek at the first six bytes — two base64 quanta — before decoding a
    // payload that may turn out to be oversized.
    let header = data
        .get(..8)
        .and_then(|quanta| engine.decode(quanta).ok())
        .unwrap_or_default();
    let (cap, what) = crate::note_media::files::attachment_limit(&header);
    if data.len() as u64 > cap.div_ceil(3) * 4 {
        return Err(crate::note_media::files::size_error(what, cap));
    }
    let bytes = engine
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

    fn dims(width: u32, height: u32, density: u8) -> Option<crate::store::ImageDimensions> {
        Some(crate::store::ImageDimensions {
            width,
            height,
            density,
        })
    }

    #[test]
    fn attachments_record_their_size_and_density() {
        use crate::note_media::dimensions::tests::{NO_PHYS_PNG, PHYS_144DPI_PNG};
        let f = Fixture::new();
        let retina = attach(&f, "retina.png", "image/png", PHYS_144DPI_PNG).unwrap();
        assert_eq!(retina.dimensions, dims(8, 6, 2));
        let plain = attach(&f, "plain.png", "image/png", NO_PHYS_PNG).unwrap();
        assert_eq!(plain.dimensions, dims(8, 6, 1));
        // Headers that cannot be read leave the columns NULL rather than
        // failing the attach.
        let stub = attach(&f, "stub.png", "image/png", PNG).unwrap();
        assert_eq!(stub.dimensions, None);

        for image in [&retina, &plain, &stub] {
            let stored = f.store.get_image(&image.id).unwrap().unwrap();
            assert_eq!(stored.dimensions, image.dimensions, "{}", image.filename);
        }
        let listed = f.store.list_all_images_for_branch(&f.branch.id).unwrap();
        assert_eq!(listed.len(), 3);
        assert!(listed.iter().any(|i| i.dimensions == dims(8, 6, 2)));
    }

    /// A row that predates the columns, with its file on disk.
    fn legacy_image(f: &Fixture, filename: &str, mime: &str, bytes: Option<&[u8]>) -> String {
        let mut image = crate::store::Image::new(
            Some(&f.branch.id),
            &f.project.id,
            filename,
            mime,
            bytes.map_or(0, |b| b.len() as i64),
            false,
        );
        image.session_id = Some(f.session.id.clone());
        if let Some(bytes) = bytes {
            let path =
                crate::store::images::image_file_path(&f.project.id, &image.id, filename).unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        f.store.create_image(&image).unwrap();
        assert_eq!(image.dimensions, None);
        image.id
    }

    #[test]
    fn media_dimensions_are_read_back_from_disk_once_for_legacy_rows() {
        use crate::note_media::dimensions::tests::PHYS_144DPI_PNG;
        let f = Fixture::new();
        let legacy = legacy_image(&f, "legacy.png", "image/png", Some(PHYS_144DPI_PNG));
        let video = legacy_image(&f, "clip.mp4", "video/mp4", Some(b"   ftypisom"));
        let stub = legacy_image(&f, "stub.png", "image/png", Some(PNG));
        let missing = legacy_image(&f, "gone.png", "image/png", None);
        let fresh = attach(&f, "fresh.png", "image/png", PHYS_144DPI_PNG).unwrap();

        let ids: Vec<String> = [
            &legacy,
            &video,
            &stub,
            &missing,
            &fresh.id,
            &"nope".to_string(),
        ]
        .into_iter()
        .cloned()
        .collect();
        let found = media_dimensions_impl(&f.store, &ids);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found.get(&legacy).copied(), dims(8, 6, 2));
        assert_eq!(found.get(&fresh.id).copied(), dims(8, 6, 2));

        // The backfill is persisted: the file is no longer consulted.
        let legacy_row = f.store.get_image(&legacy).unwrap().unwrap();
        assert_eq!(legacy_row.dimensions, dims(8, 6, 2));
        std::fs::remove_file(
            crate::store::images::image_file_path(&f.project.id, &legacy, "legacy.png").unwrap(),
        )
        .unwrap();
        let again = media_dimensions_impl(&f.store, std::slice::from_ref(&legacy));
        assert_eq!(again.get(&legacy).copied(), dims(8, 6, 2));

        // An unreadable header is absent, and recorded as such: the file is
        // not consulted again even once it would read.
        assert_eq!(f.store.get_image(&stub).unwrap().unwrap().dimensions, None);
        assert!(f.store.image_dimensions_recorded(&stub).unwrap());
        std::fs::write(
            crate::store::images::image_file_path(&f.project.id, &stub, "stub.png").unwrap(),
            PHYS_144DPI_PNG,
        )
        .unwrap();
        let again = media_dimensions_impl(&f.store, std::slice::from_ref(&stub));
        assert!(again.is_empty(), "{again:?}");
        assert_eq!(f.store.get_image(&stub).unwrap().unwrap().dimensions, None);

        // A missing file stays NULL, so it is found once it exists; a video
        // is never read.
        assert!(!f.store.image_dimensions_recorded(&missing).unwrap());
        assert!(!f.store.image_dimensions_recorded(&video).unwrap());
        std::fs::write(
            crate::store::images::image_file_path(&f.project.id, &missing, "gone.png").unwrap(),
            PHYS_144DPI_PNG,
        )
        .unwrap();
        let restored = media_dimensions_impl(&f.store, std::slice::from_ref(&missing));
        assert_eq!(restored.get(&missing).copied(), dims(8, 6, 2));
        assert!(!f.store.image_dimensions_recorded("nope").unwrap());
    }

    #[test]
    fn pdf_attachments_are_stored_as_pdfs() {
        use crate::note_media::pdf::tests::{tiny_pdf, PAGE_ONE};
        let f = Fixture::new();
        let pdf = tiny_pdf(&[PAGE_ONE]);
        let image = attach(&f, "doc.pdf", "application/pdf", &pdf).unwrap();
        assert_eq!(image.mime_type, "application/pdf");
        assert_eq!(image.filename, "doc.pdf");
        assert_eq!(image.size_bytes, pdf.len() as i64);
        let path =
            crate::store::images::image_file_path(&image.project_id, &image.id, &image.filename)
                .unwrap();
        assert_eq!(path.extension().unwrap(), "pdf");
        assert_eq!(std::fs::read(path).unwrap(), pdf);

        // Pasted PDFs arrive without a usable name, like pasted images, and a
        // mislabelled one takes the extension its bytes call for.
        assert_eq!(
            attach(&f, "pasted", "application/pdf", &pdf)
                .unwrap()
                .filename,
            "pasted.pdf"
        );
        assert_eq!(
            attach(&f, "scan.png", "image/png", &pdf).unwrap().filename,
            "scan.pdf"
        );
    }

    /// PDFs get the larger input cap on both routes; images keep theirs. The
    /// padded PDFs here are well-formed, so acceptance means the PDF passed
    /// the open check, not that the bytes merely got past the size check.
    #[test]
    fn pdf_input_cap_is_separate_from_the_image_cap() {
        use crate::note_media::files::{MAX_IMAGE_SIZE, MAX_PDF_INPUT_SIZE};
        use crate::note_media::pdf::tests::{padded_pdf, PAGE_ONE};
        use crate::note_media::tests::PNG;
        let f = Fixture::new();
        let large_pdf = padded_pdf(&[PAGE_ONE], MAX_IMAGE_SIZE as usize + 1);

        let image = attach(&f, "scan.pdf", "application/pdf", &large_pdf).unwrap();
        assert_eq!(image.mime_type, "application/pdf");
        assert_eq!(image.size_bytes, large_pdf.len() as i64);

        let path = f.dir.path().join("dropped.pdf");
        std::fs::write(&path, &large_pdf).unwrap();
        let image = create_image_impl(
            f.store.clone(),
            Some(f.branch.id.clone()),
            f.project.id.clone(),
            path.to_string_lossy().into_owned(),
            None,
        )
        .unwrap();
        assert_eq!(
            (image.mime_type.as_str(), image.filename.as_str()),
            ("application/pdf", "dropped.pdf")
        );

        // Over the PDF cap: refused before decoding (base64) or reading (path).
        let mut oversized = "JVBERi0x".to_owned(); // "%PDF-1" — a PDF signature
        oversized.push_str(&"A".repeat((MAX_PDF_INPUT_SIZE.div_ceil(3) * 4) as usize));
        let err = create_image_from_data_impl(
            f.store.clone(),
            None,
            f.project.id.clone(),
            "huge.pdf".into(),
            "application/pdf".into(),
            oversized,
            None,
        )
        .unwrap_err();
        assert_eq!(err, "PDF exceeds 50 MB limit");
        let sparse = f.dir.path().join("huge.pdf");
        let mut file = std::fs::File::create(&sparse).unwrap();
        std::io::Write::write_all(&mut file, b"%PDF-1.4\n").unwrap();
        file.set_len(MAX_PDF_INPUT_SIZE + 1).unwrap();
        let err = create_image_impl(
            f.store.clone(),
            None,
            f.project.id.clone(),
            sparse.to_string_lossy().into_owned(),
            None,
        )
        .unwrap_err();
        assert_eq!(err, "PDF exceeds 50 MB limit");

        // An image over its own cap is still refused with the image message,
        // on both routes, and from metadata alone on the path route.
        let mut png = PNG.to_vec();
        png.resize(MAX_IMAGE_SIZE as usize + 1, 0);
        let err = attach(&f, "big.png", "image/png", &png).unwrap_err();
        assert_eq!(err, "media exceeds 10 MB limit");
        let sparse = f.dir.path().join("big.png");
        let mut file = std::fs::File::create(&sparse).unwrap();
        std::io::Write::write_all(&mut file, PNG).unwrap();
        file.set_len(MAX_IMAGE_SIZE + 1).unwrap();
        let err = create_image_impl(
            f.store.clone(),
            None,
            f.project.id.clone(),
            sparse.to_string_lossy().into_owned(),
            None,
        )
        .unwrap_err();
        assert_eq!(err, "media exceeds 10 MB limit");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn malformed_pdf_is_rejected_without_hanging() {
        let f = Fixture::new();
        let err = attach(&f, "doc.pdf", "application/pdf", b"%PDF-1.4\ngarbage").unwrap_err();
        assert!(err.contains("PDF"), "{err}");
    }

    /// Without CoreGraphics only the signature is checked, so the file is
    /// still stored and viewable as a PDF.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn pdf_attachments_are_stored_without_a_renderer() {
        let f = Fixture::new();
        let image = attach(&f, "doc.pdf", "application/pdf", b"%PDF-1.4\ngarbage").unwrap();
        assert_eq!(image.mime_type, "application/pdf");
    }
}
