//! Chat attachments as prompt content. The ACP agents accept only image
//! blocks (claude-agent-acp drops blob resources; codex-acp's input items are
//! text or image), so a PDF attachment is rendered page by page into image
//! blocks here, each captioned with its page number and filename, within a
//! page cap, a per-page size cap, and a byte budget shared by every image
//! block in the prompt; anything left out or unrenderable is reported in the
//! prompt text rather than silently dropped.

use crate::note_media::pdf::{self, Fit};
use crate::store::Image;
use acp_client::PromptImage;
use base64::Engine;

/// Pages of one PDF rendered into a prompt. Each page is a full image block,
/// so a long document would otherwise crowd out the conversation.
pub(crate) const MAX_PROMPT_PAGES: usize = 20;

/// Encoded bytes of every image block in one prompt — attached images and
/// rendered PDF pages together, however many attachments the message has.
/// The prompt travels as one JSON-RPC message and the Claude API refuses
/// requests over 32 MB; 20 MB of encoded bytes is ~27 MB as base64, which
/// leaves room for the text and the conversation around it.
pub(crate) const MAX_PROMPT_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// Ceiling on one rendered page. The direct Claude API accepts 10 MB of
/// base64 per image (Bedrock and Vertex less), so 5 MB of encoded bytes
/// (~6.7 MB as base64) is the conservative ceiling.
pub(crate) const MAX_PROMPT_IMAGE_SIZE: u64 = 5 * 1024 * 1024;

/// How much of the attachments goes into a prompt. The defaults are the
/// constants above; tests shrink them, and the render size, to stay fast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PromptLimits {
    /// Pages rendered from one PDF.
    pub pages: usize,
    /// Encoded bytes of every image block in the prompt.
    pub total_bytes: u64,
    /// Encoded bytes of one rendered page.
    pub page_bytes: u64,
    /// Long edge of a rendered page.
    pub edge_px: u32,
}

impl Default for PromptLimits {
    fn default() -> Self {
        PromptLimits {
            pages: MAX_PROMPT_PAGES,
            total_bytes: MAX_PROMPT_IMAGE_BYTES,
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
    limits: PromptLimits,
    /// Encoded bytes of the image blocks accepted so far, against
    /// `limits.total_bytes`.
    used_bytes: u64,
}

impl PromptAttachments {
    /// Tests shrink the limits so budget cases render in milliseconds.
    #[cfg(test)]
    fn with_limits(limits: PromptLimits) -> Self {
        PromptAttachments {
            limits,
            ..Self::default()
        }
    }

    pub(crate) fn add(&mut self, image: &Image, bytes: &[u8]) {
        if image.mime_type != "application/pdf" {
            if !self.reserve(bytes.len()) {
                self.notes.push(format!(
                    "[Attached image {} ({} bytes) not included in this prompt: {}]",
                    image.filename,
                    bytes.len(),
                    self.over_budget()
                ));
                return;
            }
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
        let limits = self.limits;
        let fit = Fit {
            max_bytes: limits.page_bytes,
            jpeg_fallback: true,
        };
        let first_image = self.images.len();
        // Pages are rendered in order and only as far as they are accepted,
        // so memory holds the accepted set plus the page being decided.
        let mut omitted: Option<(usize, String)> = None;
        for page in 1..=total {
            if page > limits.pages {
                omitted = Some((page, format!("over the {}-page limit", limits.pages)));
                break;
            }
            // A spent budget refuses every page, so none is rendered only to
            // be turned away.
            if self.used_bytes >= limits.total_bytes {
                omitted = Some((page, self.over_budget()));
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
            if !self.reserve(encoded.bytes.len()) {
                omitted = Some((page, self.over_budget()));
                break;
            }
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

    /// Count `len` encoded bytes against the prompt's image budget; false,
    /// with nothing counted, when they would cross it.
    fn reserve(&mut self, len: usize) -> bool {
        let len = len as u64;
        if self.used_bytes + len > self.limits.total_bytes {
            return false;
        }
        self.used_bytes += len;
        true
    }

    fn over_budget(&self) -> String {
        format!(
            "over the {} image budget",
            describe_size(self.limits.total_bytes)
        )
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

    fn png_image() -> Image {
        image("shot.png", "image/png", PNG.len())
    }

    /// Letter pages at a 256 px long edge: enough to tell the pages apart,
    /// cheap enough to render dozens.
    fn small(limits: PromptLimits) -> PromptAttachments {
        PromptAttachments::with_limits(PromptLimits {
            edge_px: 256,
            ..limits
        })
    }

    fn budget(total_bytes: u64) -> PromptAttachments {
        small(PromptLimits {
            total_bytes,
            ..PromptLimits::default()
        })
    }

    #[test]
    fn images_are_sent_as_they_are_stored() {
        let mut attachments = PromptAttachments::default();
        attachments.add(&png_image(), PNG);
        assert_eq!(
            attachments.images,
            [PromptImage::new(encode(PNG), "image/png")]
        );
        assert!(attachments.notes.is_empty());
        assert_eq!(attachments.prompt("look"), "look");
    }

    #[test]
    fn images_over_the_budget_are_noted_not_dropped() {
        let total_bytes = PNG.len() as u64 * 3 / 2;
        let mut attachments = budget(total_bytes);
        attachments.add(&png_image(), PNG);
        attachments.add(&image("second.png", "image/png", PNG.len()), PNG);
        assert_eq!(attachments.images.len(), 1);
        assert_eq!(
            attachments.notes,
            [format!(
                "[Attached image second.png ({} bytes) not included in this prompt: over the {} image budget]",
                PNG.len(),
                describe_size(total_bytes)
            )]
        );
        // The refused image was not counted, so a smaller one still fits.
        attachments.add(&image("tiny.gif", "image/gif", 1), &PNG[..PNG.len() / 2]);
        assert_eq!(attachments.images.len(), 2);
        assert_eq!(attachments.notes.len(), 1);
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
            assert_eq!((pixmap.width(), pixmap.height()), (1545, 2000));
        }
        // The pages differ: page two's rule at y=400pt is only on page two.
        let row = ((792.0 - 400.0) * f64::from(pdf::MAX_EDGE_PX) / 792.0) as u32;
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
        let mut attachments = small(PromptLimits::default());
        attachments.add(&image("long.pdf", "application/pdf", pdf.len()), &pdf);
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

        // Two pages fit; the third would cross the budget, so it and the
        // rest are left out and the last page sent says so.
        let mut attachments = budget(page_bytes * 5 / 2);
        attachments.add(&pdf_image, &pdf);
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
        let mut attachments = budget(page_bytes * 3);
        attachments.add(&pdf_image, &pdf);
        assert_eq!(attachments.images.len(), 3);
        let label = attachments.images[2].label.as_deref().unwrap();
        assert!(
            label.starts_with("page 3 of 4 of scan.pdf (page 4 not included: over the "),
            "{label}"
        );

        // When not even the first page fits, the prompt text carries the note
        // and no label from an earlier attachment is touched.
        let mut attachments = budget(page_bytes - 1);
        attachments.add(&image("tiny.png", "image/png", 1), &[0u8; 1]);
        attachments.add(&pdf_image, &pdf);
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
    fn the_byte_budget_is_shared_by_every_attachment_in_the_prompt() {
        let page_bytes = small_page_bytes();
        let pdf = tiny_pdf(&[PAGE_ONE, PAGE_ONE]);
        let first = image("a.pdf", "application/pdf", pdf.len());
        let second = image("b.pdf", "application/pdf", pdf.len());

        // Two PDFs: the first takes two of the three pages the budget
        // holds, so the second gets one page and names the one left out.
        let mut attachments = budget(page_bytes * 7 / 2);
        attachments.add(&first, &pdf);
        attachments.add(&second, &pdf);
        assert!(attachments.notes.is_empty());
        let labels: Vec<_> = attachments
            .images
            .iter()
            .map(|image| image.label.as_deref().unwrap())
            .collect();
        assert_eq!(
            labels,
            [
                "page 1 of 2 of a.pdf",
                "page 2 of 2 of a.pdf",
                &format!(
                    "page 1 of 2 of b.pdf (page 2 not included: over the {} image budget)",
                    describe_size(page_bytes * 7 / 2)
                ),
            ]
        );

        // An image before a PDF counts against the same budget: the PDF
        // gets the one page that is left.
        let mut attachments = budget(PNG.len() as u64 + page_bytes * 3 / 2);
        attachments.add(&png_image(), PNG);
        attachments.add(&first, &pdf);
        assert!(attachments.notes.is_empty());
        assert_eq!(attachments.images.len(), 2);
        assert_eq!(attachments.images[0].label, None);
        assert!(attachments.images[1]
            .label
            .as_deref()
            .unwrap()
            .starts_with("page 1 of 2 of a.pdf (page 2 not included: over the "),);

        // A PDF that uses the budget up leaves a later image noted in the
        // prompt text, with the PDF's own pages and labels untouched.
        let mut attachments = budget(page_bytes * 2);
        attachments.add(&first, &pdf);
        attachments.add(&png_image(), PNG);
        assert_eq!(attachments.images.len(), 2);
        assert_eq!(
            attachments.images[1].label.as_deref(),
            Some("page 2 of 2 of a.pdf")
        );
        assert_eq!(
            attachments.notes,
            [format!(
                "[Attached image shot.png ({} bytes) not included in this prompt: over the {} image budget]",
                PNG.len(),
                describe_size(page_bytes * 2)
            )]
        );
        // A later PDF finds the budget spent before its first page is
        // rendered and is noted in the prompt text like one that did not fit.
        attachments.add(&second, &pdf);
        assert_eq!(attachments.images.len(), 2);
        assert_eq!(attachments.notes.len(), 2);
        assert_eq!(
            attachments.notes[1],
            format!(
                "[Attached PDF b.pdf: pages 1–2 not included: over the {} image budget]",
                describe_size(page_bytes * 2)
            )
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pages_over_the_per_page_cap_are_noted_and_the_rest_still_sent() {
        let pdf = tiny_pdf(&[PAGE_ONE, PAGE_TWO]);
        // Under the smallest text page even as a JPEG: every page is refused
        // on its own, each with a note, and nothing stops the loop.
        let mut attachments = small(PromptLimits {
            page_bytes: 64,
            ..PromptLimits::default()
        });
        attachments.add(&image("scan.pdf", "application/pdf", pdf.len()), &pdf);
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
        assert_eq!(describe_size(MAX_PROMPT_IMAGE_BYTES), "20 MB");
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
