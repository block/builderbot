//! Chat attachments as prompt content. The ACP agents accept only image
//! blocks (claude-agent-acp drops blob resources; codex-acp's input items are
//! text or image), so a PDF attachment is rendered page by page into image
//! blocks here, each captioned with its page number and filename, within a
//! page cap, a per-page size cap, and a byte budget for the whole document;
//! anything left out or unrenderable is reported in the prompt text rather
//! than silently dropped.

use crate::note_media::pdf::{self, Fit};
use crate::store::Image;
use acp_client::PromptImage;
use base64::Engine;

/// Pages of one PDF rendered into a prompt. Each page is a full image block,
/// so a long document would otherwise crowd out the conversation.
pub(crate) const MAX_PROMPT_PAGES: usize = 20;

/// Encoded bytes of all the pages of one PDF in a prompt. Every page may be
/// up to [`MAX_PROMPT_IMAGE_SIZE`] and the prompt travels as one JSON-RPC
/// message (the Claude API also refuses requests over 32 MB), so a scanned
/// document stops here before it reaches the page cap.
pub(crate) const MAX_PROMPT_PAGE_BYTES: u64 = 20 * 1024 * 1024;

/// The Claude API's limit for one image, applied to each rendered page.
pub(crate) const MAX_PROMPT_IMAGE_SIZE: u64 = 5 * 1024 * 1024;

/// How much of a PDF goes into a prompt. The defaults are the constants
/// above; tests shrink them, and the render size, to stay fast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PageLimits {
    pub pages: usize,
    pub total_bytes: u64,
    pub page_bytes: u64,
    pub edge_px: u32,
}

impl Default for PageLimits {
    fn default() -> Self {
        PageLimits {
            pages: MAX_PROMPT_PAGES,
            total_bytes: MAX_PROMPT_PAGE_BYTES,
            page_bytes: MAX_PROMPT_IMAGE_SIZE,
            edge_px: pdf::MAX_EDGE_PX,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PromptAttachments {
    pub images: Vec<PromptImage>,
    /// Appended to the prompt text: attachments that could not be sent as
    /// images, and why.
    pub notes: Vec<String>,
}

impl PromptAttachments {
    pub(crate) fn add(&mut self, image: &Image, bytes: &[u8]) {
        self.add_with_limits(image, bytes, PageLimits::default());
    }

    pub(crate) fn add_with_limits(&mut self, image: &Image, bytes: &[u8], limits: PageLimits) {
        if image.mime_type != "application/pdf" {
            self.images
                .push(PromptImage::new(encode(bytes), image.mime_type.clone()));
            return;
        }
        let document = match pdf::Document::open(bytes) {
            Ok(document) => document,
            Err(e) => {
                log::warn!(
                    "Cannot render PDF attachment {} for the prompt: {e}",
                    image.id
                );
                self.notes.push(format!(
                    "[Attached PDF {} ({} bytes) could not be rendered for this prompt: {e}]",
                    image.filename, image.size_bytes
                ));
                return;
            }
        };
        let total = document.page_count();
        let fit = Fit {
            max_bytes: limits.page_bytes,
            jpeg_fallback: true,
        };
        let first_image = self.images.len();
        let mut used_bytes = 0u64;
        // Pages are rendered in order and only as far as they are accepted,
        // so memory holds the accepted set plus the page being decided.
        let mut omitted: Option<(usize, String)> = None;
        for page in 1..=total {
            if page > limits.pages {
                omitted = Some((page, format!("over the {}-page limit", limits.pages)));
                break;
            }
            let encoded = match document.render_page(page, limits.edge_px, fit) {
                Ok(encoded) => encoded,
                Err(e) => {
                    log::warn!(
                        "Cannot render page {page} of PDF attachment {} for the prompt: {e}",
                        image.id
                    );
                    self.notes.push(format!(
                        "[Page {page} of {} could not be included in this prompt: {e}]",
                        image.filename
                    ));
                    continue;
                }
            };
            if used_bytes + encoded.bytes.len() as u64 > limits.total_bytes {
                omitted = Some((
                    page,
                    format!(
                        "over the {} image budget",
                        describe_size(limits.total_bytes)
                    ),
                ));
                break;
            }
            used_bytes += encoded.bytes.len() as u64;
            self.images.push(
                PromptImage::new(encode(&encoded.bytes), encoded.mime_type)
                    .with_label(format!("page {page} of {total} of {}", image.filename)),
            );
        }
        if let Some((first, reason)) = omitted {
            let pages = if first == total {
                format!("page {total}")
            } else {
                format!("pages {first}–{total}")
            };
            let what = format!("{pages} not included: {reason}");
            // Said on the last page sent, where the agent is reading page
            // numbers; in the prompt text when no page made it.
            if self.images.len() > first_image {
                if let Some(label) = self.images.last_mut().and_then(|last| last.label.as_mut()) {
                    label.push_str(&format!(" ({what})"));
                }
            } else {
                self.notes
                    .push(format!("[Attached PDF {}: {what}]", image.filename));
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

/// Whole MB or KB where the size is one, else bytes: the shipped budget reads
/// "20 MB", a test's "3 KB".
fn describe_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * KB;
    if bytes >= MB && bytes % MB == 0 {
        format!("{} MB", bytes / MB)
    } else if bytes >= KB && bytes % KB == 0 {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} bytes")
    }
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

    /// Letter pages at a 256 px long edge: enough to tell the pages apart,
    /// cheap enough to render dozens.
    fn small(limits: PageLimits) -> PageLimits {
        PageLimits {
            edge_px: 256,
            ..limits
        }
    }

    /// Encoded size of one small [`PAGE_ONE`] render, for budgets.
    #[cfg(target_os = "macos")]
    fn small_page_bytes() -> u64 {
        let document = pdf::Document::open(&tiny_pdf(&[PAGE_ONE])).unwrap();
        let fit = Fit {
            max_bytes: MAX_PROMPT_IMAGE_SIZE,
            jpeg_fallback: true,
        };
        document.render_page(1, 256, fit).unwrap().bytes.len() as u64
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn long_pdfs_stop_at_the_page_cap_and_say_so() {
        let pages = vec![PAGE_ONE; MAX_PROMPT_PAGES + 5];
        let pdf = tiny_pdf(&pages);
        let mut attachments = PromptAttachments::default();
        attachments.add_with_limits(
            &image("long.pdf", "application/pdf", pdf.len()),
            &pdf,
            small(PageLimits::default()),
        );
        assert_eq!(attachments.images.len(), MAX_PROMPT_PAGES);
        assert!(attachments.notes.is_empty());
        assert_eq!(
            attachments.images[MAX_PROMPT_PAGES - 2].label.as_deref(),
            Some("page 19 of 25 of long.pdf")
        );
        assert_eq!(
            attachments.images[MAX_PROMPT_PAGES - 1].label.as_deref(),
            Some("page 20 of 25 of long.pdf (pages 21–25 not included: over the 20-page limit)")
        );
        let png = base64::engine::general_purpose::STANDARD
            .decode(&attachments.images[0].data)
            .unwrap();
        assert_eq!(tiny_skia::Pixmap::decode_png(&png).unwrap().width(), 198);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pages_stop_at_the_byte_budget_and_say_so() {
        let page_bytes = small_page_bytes();
        let pdf = tiny_pdf(&[PAGE_ONE; 4]);
        let pdf_image = image("scan.pdf", "application/pdf", pdf.len());
        let budget = |total_bytes: u64| {
            small(PageLimits {
                total_bytes,
                ..PageLimits::default()
            })
        };

        // Two pages fit; the third would cross the budget, so it and the
        // rest are left out and the last page sent says so.
        let mut attachments = PromptAttachments::default();
        attachments.add_with_limits(&pdf_image, &pdf, budget(page_bytes * 5 / 2));
        assert_eq!(attachments.images.len(), 2);
        assert!(attachments.notes.is_empty());
        assert_eq!(
            attachments.images[1].label.as_deref(),
            Some(
                format!(
                    "page 2 of 4 of scan.pdf (pages 3–4 not included: over the {} image budget)",
                    describe_size(page_bytes * 5 / 2)
                )
                .as_str()
            )
        );

        // A single page left out is named as one.
        let mut attachments = PromptAttachments::default();
        attachments.add_with_limits(&pdf_image, &pdf, budget(page_bytes * 3));
        assert_eq!(attachments.images.len(), 3);
        let label = attachments.images[2].label.as_deref().unwrap();
        assert!(
            label.starts_with("page 3 of 4 of scan.pdf (page 4 not included: over the "),
            "{label}"
        );

        // When not even the first page fits, the prompt text carries the note
        // and no label from an earlier attachment is touched.
        let mut attachments = PromptAttachments::default();
        attachments.add(&image("shot.png", "image/png", PNG.len()), PNG);
        attachments.add_with_limits(&pdf_image, &pdf, budget(page_bytes - 1));
        assert_eq!(attachments.images.len(), 1);
        assert_eq!(attachments.images[0].label, None);
        assert_eq!(attachments.notes.len(), 1);
        assert!(
            attachments.notes[0]
                .starts_with("[Attached PDF scan.pdf: pages 1–4 not included: over the "),
            "{}",
            attachments.notes[0]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pages_over_the_per_page_cap_are_noted_and_the_rest_still_sent() {
        let pdf = tiny_pdf(&[PAGE_ONE, PAGE_TWO]);
        let mut attachments = PromptAttachments::default();
        // Under the smallest text page even as a JPEG: every page is refused
        // on its own, each with a note, and nothing stops the loop.
        attachments.add_with_limits(
            &image("scan.pdf", "application/pdf", pdf.len()),
            &pdf,
            small(PageLimits {
                page_bytes: 64,
                ..PageLimits::default()
            }),
        );
        assert!(attachments.images.is_empty());
        assert_eq!(attachments.notes.len(), 2);
        assert!(
            attachments.notes[0].starts_with(
                "[Page 1 of scan.pdf could not be included in this prompt: PDF page exceeds"
            ),
            "{}",
            attachments.notes[0]
        );
        assert!(attachments.notes[1].starts_with("[Page 2 of scan.pdf"));

        // The shipped per-page cap is far above a rendered text page.
        assert!(small_page_bytes() < MAX_PROMPT_IMAGE_SIZE / 100);
    }

    #[test]
    fn sizes_read_in_whole_units() {
        assert_eq!(describe_size(MAX_PROMPT_PAGE_BYTES), "20 MB");
        assert_eq!(describe_size(3 * 1024), "3 KB");
        assert_eq!(describe_size(1500), "1500 bytes");
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
