//! Small previews for chat attachments. Images are their own preview; a PDF
//! has its first page rendered once and cached beside the file.

use crate::note_media::pdf;
use crate::store::Store;
use base64::Engine;
use serde::Serialize;

/// Long edge of a cached PDF thumbnail. The composer and chat history show
/// attachments at 48–200 CSS px, so this covers a 2x display with room to
/// spare while staying a few tens of KB.
pub(crate) const THUMBNAIL_EDGE_PX: u32 = 512;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentThumbnail {
    /// PNG (or the image's own) data URL, or `None` when a PDF could not be
    /// rendered — the frontend shows a document icon instead.
    pub data_url: Option<String>,
    /// The attachment's stored MIME, so a PDF can be badged as one.
    pub mime_type: String,
    pub filename: String,
}

pub(crate) fn attachment_thumbnail_impl(
    store: &Store,
    image_id: &str,
) -> Result<AttachmentThumbnail, String> {
    let image = store
        .get_image(image_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Image not found: {image_id}"))?;
    let path =
        crate::store::images::image_file_path(&image.project_id, &image.id, &image.filename)?;
    let engine = base64::engine::general_purpose::STANDARD;
    let data_url = if image.mime_type == "application/pdf" {
        pdf_thumbnail_png(&image.project_id, &image.id, &path)
            .map(|png| format!("data:image/png;base64,{}", engine.encode(png)))
    } else {
        let bytes = std::fs::read(&path).map_err(|e| format!("Failed to read image: {e}"))?;
        Some(format!(
            "data:{};base64,{}",
            image.mime_type,
            engine.encode(bytes)
        ))
    };
    Ok(AttachmentThumbnail {
        data_url,
        mime_type: image.mime_type,
        filename: image.filename,
    })
}

/// The cached first-page PNG, rendering and caching it on first use. `None`
/// when the page cannot be rendered (no renderer on this platform, or a PDF
/// CoreGraphics opens but cannot draw); a thumbnail is not worth failing the
/// attachment over, and the cache is never written for a failed render so a
/// later attempt can succeed.
fn pdf_thumbnail_png(
    project_id: &str,
    image_id: &str,
    pdf_path: &std::path::Path,
) -> Option<Vec<u8>> {
    let cache = crate::store::images::thumbnail_file_path(project_id, image_id).ok()?;
    if let Ok(png) = std::fs::read(&cache) {
        if png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some(png);
        }
    }
    let bytes = std::fs::read(pdf_path)
        .map_err(|e| log::warn!("Cannot read PDF attachment {image_id}: {e}"))
        .ok()?;
    let rendered = pdf::render_first_page(&bytes, THUMBNAIL_EDGE_PX)
        .map_err(|e| log::warn!("Cannot render thumbnail for PDF attachment {image_id}: {e}"))
        .ok()?;
    if let Err(e) = std::fs::write(&cache, &rendered.png) {
        log::warn!("Cannot cache thumbnail {}: {e}", cache.display());
    }
    Some(rendered.png)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note_media::files::store_sniffed_image;
    use crate::note_media::pdf::tests::{tiny_pdf, PAGE_ONE};
    use crate::note_media::tests::{Fixture, PNG};

    fn stored_pdf(f: &Fixture) -> crate::store::Image {
        store_sniffed_image(
            &f.store,
            Some(&f.branch.id),
            &f.project.id,
            None,
            "doc.pdf",
            &tiny_pdf(&[PAGE_ONE]),
        )
        .unwrap()
    }

    #[test]
    fn images_are_their_own_thumbnail() {
        let f = Fixture::new();
        let image =
            store_sniffed_image(&f.store, None, &f.project.id, None, "shot.png", PNG).unwrap();
        let thumbnail = attachment_thumbnail_impl(&f.store, &image.id).unwrap();
        assert_eq!(
            thumbnail,
            AttachmentThumbnail {
                data_url: Some(format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(PNG)
                )),
                mime_type: "image/png".into(),
                filename: "shot.png".into(),
            }
        );
        assert!(
            crate::store::images::thumbnail_file_path(&f.project.id, &image.id)
                .map(|p| !p.exists())
                .unwrap_or(true)
        );

        let err = attachment_thumbnail_impl(&f.store, "missing").unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pdf_thumbnails_are_rendered_once_and_cached_beside_the_pdf() {
        let f = Fixture::new();
        let image = stored_pdf(&f);
        let thumbnail = attachment_thumbnail_impl(&f.store, &image.id).unwrap();
        assert_eq!(thumbnail.mime_type, "application/pdf");
        assert_eq!(thumbnail.filename, "doc.pdf");
        let data_url = thumbnail.data_url.unwrap();
        let png = base64::engine::general_purpose::STANDARD
            .decode(data_url.strip_prefix("data:image/png;base64,").unwrap())
            .unwrap();
        let pixmap = tiny_skia::Pixmap::decode_png(&png).unwrap();
        // Letter page at a 512 px long edge.
        assert_eq!((pixmap.width(), pixmap.height()), (396, 512));
        assert!(pixmap.pixels().iter().any(|p| p.red() < 128));

        let cache = crate::store::images::thumbnail_file_path(&f.project.id, &image.id).unwrap();
        assert_eq!(std::fs::read(&cache).unwrap(), png);

        // The cache is what is served from now on, even once the PDF is gone.
        std::fs::remove_file(
            crate::store::images::image_file_path(&image.project_id, &image.id, &image.filename)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            attachment_thumbnail_impl(&f.store, &image.id)
                .unwrap()
                .data_url,
            Some(data_url)
        );

        // Deleting the attachment removes the thumbnail with it.
        crate::note_media::files::delete_media_file(&f.store, &image).unwrap();
        assert!(!cache.exists());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn pdf_thumbnails_are_absent_without_a_renderer() {
        let f = Fixture::new();
        let image = stored_pdf(&f);
        let thumbnail = attachment_thumbnail_impl(&f.store, &image.id).unwrap();
        assert_eq!(thumbnail.data_url, None);
        assert_eq!(thumbnail.mime_type, "application/pdf");
    }
}
