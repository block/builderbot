//! Chat attachments as prompt content. The ACP agents accept only image
//! blocks (claude-agent-acp drops blob resources; codex-acp's input items are
//! text or image), so a PDF attachment is rendered page by page into PNG
//! image blocks here, each captioned with its page number and filename, and
//! anything that cannot be rendered is reported in the prompt text rather
//! than silently left out.

use crate::note_media::pdf;
use crate::store::Image;
use acp_client::PromptImage;
use base64::Engine;

/// Pages of one PDF rendered into a prompt. Each page is a full image block,
/// so a long document would otherwise crowd out the conversation.
pub(crate) const MAX_PROMPT_PAGES: usize = 20;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PromptAttachments {
    pub images: Vec<PromptImage>,
    /// Appended to the prompt text: attachments that could not be sent as
    /// images, and why.
    pub notes: Vec<String>,
}

impl PromptAttachments {
    pub(crate) fn add(&mut self, image: &Image, bytes: &[u8]) {
        if image.mime_type != "application/pdf" {
            self.images
                .push(PromptImage::new(encode(bytes), image.mime_type.clone()));
            return;
        }
        match pdf::render_pages(bytes, MAX_PROMPT_PAGES, pdf::MAX_EDGE_PX) {
            Ok(rendered) => {
                let sent = rendered.pages.len();
                let total = rendered.page_count;
                for (index, png) in rendered.pages.into_iter().enumerate() {
                    let page = index + 1;
                    let mut label = format!("page {page} of {total} of {}", image.filename);
                    if page == sent && sent < total {
                        label.push_str(&format!(" (pages {}–{total} not included)", sent + 1));
                    }
                    self.images
                        .push(PromptImage::new(encode(&png), "image/png").with_label(label));
                }
            }
            Err(e) => {
                log::warn!(
                    "Cannot render PDF attachment {} for the prompt: {e}",
                    image.id
                );
                self.notes.push(format!(
                    "[Attached PDF {} ({} bytes) could not be rendered for this prompt: {e}]",
                    image.filename, image.size_bytes
                ));
            }
        }
    }

    /// The prompt text with the notes, if any, after it.
    pub(crate) fn prompt(&self, prompt: &str) -> String {
        let mut prompt = prompt.to_owned();
        for note in &self.notes {
            prompt.push_str("\n\n");
            prompt.push_str(note);
        }
        prompt
    }
}

fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note_media::pdf::tests::{tiny_pdf, PAGE_ONE, PAGE_TWO};
    use crate::note_media::tests::PNG;

    fn image(filename: &str, mime_type: &str, size: usize) -> Image {
        Image::new(None, "project", filename, mime_type, size as i64, false)
    }

    #[test]
    fn images_are_sent_as_they_are_stored() {
        let mut attachments = PromptAttachments::default();
        attachments.add(&image("shot.png", "image/png", PNG.len()), PNG);
        assert_eq!(
            attachments.images,
            [PromptImage::new(encode(PNG), "image/png")]
        );
        assert!(attachments.notes.is_empty());
        assert_eq!(attachments.prompt("look"), "look");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pdf_pages_become_captioned_png_images() {
        let pdf = tiny_pdf(&[PAGE_ONE, PAGE_TWO]);
        let mut attachments = PromptAttachments::default();
        attachments.add(&image("report.pdf", "application/pdf", pdf.len()), &pdf);
        assert!(attachments.notes.is_empty());
        assert_eq!(attachments.images.len(), 2);
        for (index, prompt_image) in attachments.images.iter().enumerate() {
            assert_eq!(prompt_image.mime_type, "image/png");
            assert_eq!(
                prompt_image.label.as_deref(),
                Some(format!("page {} of 2 of report.pdf", index + 1).as_str())
            );
            let png = base64::engine::general_purpose::STANDARD
                .decode(&prompt_image.data)
                .unwrap();
            let pixmap = tiny_skia::Pixmap::decode_png(&png).unwrap();
            assert_eq!((pixmap.width(), pixmap.height()), (1583, 2048));
        }
        // The pages differ: page two's rule at y=400pt is only on page two.
        let row = ((792.0 - 400.0) * 2048.0 / 792.0) as u32;
        let pixel = |i: usize| {
            let png = base64::engine::general_purpose::STANDARD
                .decode(&attachments.images[i].data)
                .unwrap();
            tiny_skia::Pixmap::decode_png(&png)
                .unwrap()
                .pixel(800, row)
                .unwrap()
                .red()
        };
        assert_eq!(pixel(0), 255);
        assert!(pixel(1) < 128);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn long_pdfs_stop_at_the_page_cap_and_say_so() {
        let pages = vec![PAGE_ONE; MAX_PROMPT_PAGES + 5];
        let pdf = tiny_pdf(&pages);
        let mut attachments = PromptAttachments::default();
        attachments.add(&image("long.pdf", "application/pdf", pdf.len()), &pdf);
        assert_eq!(attachments.images.len(), MAX_PROMPT_PAGES);
        assert_eq!(
            attachments.images[MAX_PROMPT_PAGES - 2].label.as_deref(),
            Some("page 19 of 25 of long.pdf")
        );
        assert_eq!(
            attachments.images[MAX_PROMPT_PAGES - 1].label.as_deref(),
            Some("page 20 of 25 of long.pdf (pages 21–25 not included)")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unrenderable_pdfs_are_reported_in_the_prompt() {
        let bytes = b"%PDF-1.4\nnot really";
        let mut attachments = PromptAttachments::default();
        attachments.add(&image("bad.pdf", "application/pdf", bytes.len()), bytes);
        assert!(attachments.images.is_empty());
        assert_eq!(attachments.notes.len(), 1);
        let prompt = attachments.prompt("look at this");
        assert!(
            prompt.starts_with(
                "look at this\n\n[Attached PDF bad.pdf (19 bytes) could not be rendered"
            ),
            "{prompt}"
        );
        assert!(prompt.contains("PDF"), "{prompt}");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn pdfs_are_reported_where_they_cannot_be_rendered() {
        let pdf = tiny_pdf(&[PAGE_ONE, PAGE_TWO]);
        let mut attachments = PromptAttachments::default();
        attachments.add(&image("report.pdf", "application/pdf", pdf.len()), &pdf);
        assert!(attachments.images.is_empty());
        let prompt = attachments.prompt("look");
        assert!(
            prompt.contains("report.pdf") && prompt.contains("macOS"),
            "{prompt}"
        );
    }
}
