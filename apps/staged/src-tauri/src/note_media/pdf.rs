//! PDF attachments are stored as the PDF itself; this module rasterizes pages
//! to opaque PNGs where a bitmap is needed: the thumbnail shown next to image
//! attachments, and the agent prompt, whose transports accept only image
//! blocks. Rendering uses CoreGraphics in-process (no subprocess, no temp
//! files, malformed input fails fast instead of hanging).
//!
//! macOS only — the shipped target. Other platforms compile stubs so the
//! Linux CI build stays green; they accept any `%PDF-` payload at store time
//! and report that rendering needs macOS.

#[cfg(target_os = "macos")]
use super::files::{size_error, MAX_IMAGE_SIZE};

/// Long edge of a page rendered for the agent prompt, in pixels. A letter
/// page lands at ~186 DPI, which keeps body text legible to a vision model
/// without approaching the attachment size cap.
pub(crate) const MAX_EDGE_PX: u32 = 2048;

/// Pages are never blown up beyond 4 px per PDF point, so a tiny page does
/// not become a huge blurry bitmap just to reach the long-edge target.
#[cfg(target_os = "macos")]
const MAX_SCALE: f64 = 4.0;

pub(crate) fn is_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

pub(crate) struct RenderedPage {
    pub png: Vec<u8>,
    /// Pages in the document, so callers can say "page 1 of N".
    pub page_count: usize,
}

impl std::fmt::Debug for RenderedPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderedPage")
            .field("png_bytes", &self.png.len())
            .field("page_count", &self.page_count)
            .finish()
    }
}

/// The leading pages of a document, each as a white-backed PNG, plus the
/// total so a caller can say how many were left out.
pub(crate) struct RenderedDocument {
    pub pages: Vec<Vec<u8>>,
    pub page_count: usize,
}

/// Reject what CoreGraphics cannot open before it is stored, so a `%PDF-`
/// prefix on garbage never becomes an attachment that fails to render later.
#[cfg(target_os = "macos")]
pub(crate) fn check(bytes: &[u8]) -> Result<(), String> {
    open(bytes).map(drop)
}

/// Render page 1 to a white-backed PNG whose long edge is `max_edge_px`
/// (less when that would exceed [`MAX_SCALE`]).
#[cfg(target_os = "macos")]
pub(crate) fn render_first_page(bytes: &[u8], max_edge_px: u32) -> Result<RenderedPage, String> {
    let document = open(bytes)?;
    let png = render_page(&document, 1, max_edge_px)?;
    Ok(RenderedPage {
        png,
        page_count: document.page_count,
    })
}

/// Render pages 1..=`max_pages` (or every page, if fewer) like
/// [`render_first_page`].
#[cfg(target_os = "macos")]
pub(crate) fn render_pages(
    bytes: &[u8],
    max_pages: usize,
    max_edge_px: u32,
) -> Result<RenderedDocument, String> {
    let document = open(bytes)?;
    let pages = (1..=document.page_count.min(max_pages))
        .map(|number| render_page(&document, number, max_edge_px))
        .collect::<Result<_, _>>()?;
    Ok(RenderedDocument {
        pages,
        page_count: document.page_count,
    })
}

#[cfg(target_os = "macos")]
struct Document {
    document: objc2_core_foundation::CFRetained<objc2_core_graphics::CGPDFDocument>,
    page_count: usize,
}

#[cfg(target_os = "macos")]
fn open(bytes: &[u8]) -> Result<Document, String> {
    use objc2_core_foundation::CFData;
    use objc2_core_graphics::{CGDataProvider, CGPDFDocument};

    let data = CFData::from_bytes(bytes);
    let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or("could not read PDF data")?;
    // CoreGraphics returns no document for anything it cannot parse, so a
    // `%PDF-` prefix on garbage is rejected here rather than drawn as blank.
    let document = CGPDFDocument::with_provider(Some(&provider)).ok_or("could not open PDF")?;
    let page_count = CGPDFDocument::number_of_pages(Some(&document));
    if page_count == 0 {
        return Err("PDF has no pages".into());
    }
    Ok(Document {
        document,
        page_count,
    })
}

/// Draw one page at `max_edge_px` along its long edge (less when that would
/// exceed [`MAX_SCALE`]). If the PNG is over [`MAX_IMAGE_SIZE`] —
/// photographic pages — it is re-rendered once at half scale before failing
/// with the usual size message.
#[cfg(target_os = "macos")]
fn render_page(document: &Document, number: usize, max_edge_px: u32) -> Result<Vec<u8>, String> {
    use objc2_core_graphics::{CGPDFBox, CGPDFDocument, CGPDFPage};

    let page = CGPDFDocument::page(Some(&document.document), number)
        .ok_or_else(|| format!("could not read PDF page {number}"))?;

    // CropBox is what Preview and Quick Look display (CoreGraphics clips it
    // to the MediaBox, and returns the MediaBox when none is set), so a
    // print-ready PDF renders without its bleed and crop marks. Fall back to
    // the MediaBox only when the CropBox is unusable.
    let mut page_box = CGPDFBox::CropBox;
    let mut rect = CGPDFPage::box_rect(Some(&page), page_box);
    if !is_drawable(rect.size.width, rect.size.height) {
        page_box = CGPDFBox::MediaBox;
        rect = CGPDFPage::box_rect(Some(&page), page_box);
        if !is_drawable(rect.size.width, rect.size.height) {
            return Err("PDF page has no drawable area".into());
        }
    }
    // `drawing_transform` honours the page's own /Rotate, so the bitmap must
    // take the rotated orientation for the page to fill it.
    let rotated = CGPDFPage::rotation_angle(Some(&page)).rem_euclid(180) == 90;
    let (w_pt, h_pt) = if rotated {
        (rect.size.height, rect.size.width)
    } else {
        (rect.size.width, rect.size.height)
    };
    let scale = (f64::from(max_edge_px) / w_pt.max(h_pt)).min(MAX_SCALE);

    let png = render(&page, page_box, (w_pt, h_pt), scale)?;
    let png = if png.len() as u64 > MAX_IMAGE_SIZE {
        render(&page, page_box, (w_pt, h_pt), scale / 2.0)?
    } else {
        png
    };
    if png.len() as u64 > MAX_IMAGE_SIZE {
        return Err(size_error("media", MAX_IMAGE_SIZE));
    }
    Ok(png)
}

#[cfg(target_os = "macos")]
fn is_drawable(width: f64, height: f64) -> bool {
    width.is_finite() && height.is_finite() && width >= 1.0 && height >= 1.0
}

#[cfg(target_os = "macos")]
fn pixel_size(w_pt: f64, h_pt: f64, scale: f64) -> (u32, u32) {
    let px = |pt: f64| (pt * scale).round().max(1.0) as u32;
    (px(w_pt), px(h_pt))
}

/// Draw the page, displayed at `(w_pt, h_pt)` points, into a fresh RGBA
/// bitmap at `scale` px/pt and PNG-encode it with tiny-skia (its pixmaps are
/// premultiplied RGBA8, the same layout CoreGraphics writes for
/// `PremultipliedLast`).
#[cfg(target_os = "macos")]
fn render(
    page: &objc2_core_graphics::CGPDFPage,
    page_box: objc2_core_graphics::CGPDFBox,
    (w_pt, h_pt): (f64, f64),
    scale: f64,
) -> Result<Vec<u8>, String> {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGColorSpace, CGContext, CGImageAlphaInfo, CGPDFPage,
    };

    let (width, height) = pixel_size(w_pt, h_pt, scale);
    let (w, h) = (width as usize, height as usize);
    let mut buf = vec![0u8; w * h * 4];
    let space = CGColorSpace::new_device_rgb().ok_or("could not create color space")?;
    let bitmap = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: f64::from(width),
            height: f64::from(height),
        },
    };
    // In points, after the CTM is scaled. `drawing_transform` only ever
    // scales a page *down* to fit its rect (otherwise it centres the page at
    // 1:1), so the magnification has to come from the CTM, and the rect is
    // the page's own displayed size so the box lands flush at the origin.
    let page_rect = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: w_pt,
            height: h_pt,
        },
    };
    {
        // SAFETY: the context draws into `buf`, which outlives it — the
        // context is released at the end of this block, before `buf` moves
        // into the pixmap — and the width, height, and row stride describe
        // exactly the RGBA8 buffer allocated above.
        let context = unsafe {
            CGBitmapContextCreate(
                buf.as_mut_ptr().cast(),
                w,
                h,
                8,
                w * 4,
                Some(&space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .ok_or("could not create bitmap context")?;
        let ctx = Some(&*context);
        // White behind the page: PDFs have no background, and a transparent
        // PNG would lose black text on the dark theme and for the agent.
        CGContext::set_rgb_fill_color(ctx, 1.0, 1.0, 1.0, 1.0);
        CGContext::fill_rect(ctx, bitmap);
        CGContext::scale_ctm(ctx, scale, scale);
        CGContext::concat_ctm(
            ctx,
            CGPDFPage::drawing_transform(Some(page), page_box, page_rect, 0, true),
        );
        CGContext::draw_pdf_page(ctx, Some(page));
    }
    let size = tiny_skia::IntSize::from_wh(width, height).ok_or("invalid PDF page size")?;
    let pixmap = tiny_skia::Pixmap::from_vec(buf, size).ok_or("invalid PDF page bitmap")?;
    pixmap
        .encode_png()
        .map_err(|e| format!("could not encode PDF page: {e}"))
}

#[cfg(not(target_os = "macos"))]
pub(crate) const UNSUPPORTED: &str = "PDF rendering requires macOS";

/// Without a renderer only the signature is checked; the file is still
/// stored and viewed as a PDF.
#[cfg(not(target_os = "macos"))]
pub(crate) fn check(_bytes: &[u8]) -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn render_first_page(_bytes: &[u8], _max_edge_px: u32) -> Result<RenderedPage, String> {
    Err(UNSUPPORTED.into())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn render_pages(
    _bytes: &[u8],
    _max_pages: usize,
    _max_edge_px: u32,
) -> Result<RenderedDocument, String> {
    Err(UNSUPPORTED.into())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal PDF with one letter-size page per content stream, using the
    /// built-in Helvetica. Offsets in the xref table are computed, so the
    /// output is well-formed rather than relying on reader repair.
    pub(crate) fn tiny_pdf(pages: &[&str]) -> Vec<u8> {
        tiny_pdf_with(pages, "[0 0 612 792]", None, None)
    }

    pub(crate) fn tiny_pdf_with(
        pages: &[&str],
        media_box: &str,
        crop_box: Option<&str>,
        rotate: Option<u32>,
    ) -> Vec<u8> {
        build_pdf(pages, media_box, crop_box, rotate, 0)
    }

    /// [`tiny_pdf`] grown past `min_len` bytes with comment lines after the
    /// header, so a large file is still a well-formed, instantly rendered PDF
    /// (the xref offsets skip the comments) — for exercising the size caps.
    pub(crate) fn padded_pdf(pages: &[&str], min_len: usize) -> Vec<u8> {
        let pdf = build_pdf(pages, "[0 0 612 792]", None, None, min_len);
        assert!(pdf.len() >= min_len);
        pdf
    }

    fn build_pdf(
        pages: &[&str],
        media_box: &str,
        crop_box: Option<&str>,
        rotate: Option<u32>,
        padding: usize,
    ) -> Vec<u8> {
        let font_id = 3 + pages.len() * 2;
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                (0..pages.len())
                    .map(|i| format!("{} 0 R", 3 + i * 2))
                    .collect::<Vec<_>>()
                    .join(" "),
                pages.len()
            ),
        ];
        for (i, content) in pages.iter().enumerate() {
            let crop_box = crop_box
                .map(|b| format!(" /CropBox {b}"))
                .unwrap_or_default();
            let rotate = rotate.map(|r| format!(" /Rotate {r}")).unwrap_or_default();
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox {media_box}{crop_box}{rotate} \
                 /Resources << /Font << /F1 {font_id} 0 R >> >> /Contents {} 0 R >>",
                4 + i * 2
            ));
            objects.push(format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ));
        }
        objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned());

        let mut out = b"%PDF-1.4\n".to_vec();
        let comment = format!("%{}\n", "x".repeat(4095));
        while out.len() < padding {
            out.extend_from_slice(comment.as_bytes());
        }
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    pub(crate) const PAGE_ONE: &str =
        "BT /F1 24 Tf 72 700 Td (PAGE ONE Staged PDF probe) Tj ET\n4 w 72 650 m 540 650 l S";

    /// Page one's text plus a second rule, at y=400 pt, that page one lacks.
    pub(crate) const PAGE_TWO: &str =
        "BT /F1 24 Tf 72 700 Td (PAGE TWO should not appear) Tj ET\n4 w 72 400 m 540 400 l S";

    #[test]
    fn detects_the_pdf_signature_only() {
        assert!(is_pdf(&tiny_pdf(&[PAGE_ONE])));
        assert!(is_pdf(b"%PDF-1.7 junk"));
        assert!(!is_pdf(b"\x89PNG\r\n\x1a\n"));
        assert!(!is_pdf(b" %PDF-1.4"));
        assert!(!is_pdf(b""));
    }

    #[cfg(target_os = "macos")]
    mod macos {
        use super::*;

        fn decode(png: &[u8]) -> tiny_skia::Pixmap {
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
            tiny_skia::Pixmap::decode_png(png).unwrap()
        }

        fn is_opaque_white(pixmap: &tiny_skia::Pixmap, x: u32, y: u32) -> bool {
            let p = pixmap.pixel(x, y).unwrap();
            (p.red(), p.green(), p.blue(), p.alpha()) == (255, 255, 255, 255)
        }

        /// Bitmap row through the middle of a 4 pt rule drawn at `y_pt` on a
        /// letter page rendered 2048 px tall.
        fn letter_rule_row(y_pt: f64) -> u32 {
            ((792.0 - y_pt) * 2048.0 / 792.0) as u32
        }

        #[test]
        fn renders_first_page_to_opaque_png_at_the_long_edge() {
            let rendered = render_first_page(&tiny_pdf(&[PAGE_ONE, PAGE_TWO]), 2048).unwrap();
            assert_eq!(rendered.page_count, 2);
            let pixmap = decode(&rendered.png);
            // Letter: 2048 / 792 pt ≈ 2.59x, under the 4x ceiling.
            assert_eq!((pixmap.width(), pixmap.height()), (1583, 2048));
            for (x, y) in [(0, 0), (1582, 0), (0, 2047), (1582, 2047)] {
                assert!(is_opaque_white(&pixmap, x, y), "corner {x},{y}");
            }
            // The rule at y=650pt from the bottom is drawn; everything is opaque.
            let p = pixmap.pixel(800, letter_rule_row(650.0)).unwrap();
            assert!(p.red() < 128 && p.alpha() == 255, "rule pixel {p:?}");
            assert!(pixmap.pixels().iter().all(|p| p.alpha() == 255));
            // Page two's own rule, at y=400pt, is not drawn...
            assert!(is_opaque_white(&pixmap, 800, letter_rule_row(400.0)));
            // ...and would have been, had page two been the one rendered.
            let swapped = decode(&render_first_page(&tiny_pdf(&[PAGE_TWO]), 2048).unwrap().png);
            let p = swapped.pixel(800, letter_rule_row(400.0)).unwrap();
            assert!(
                p.red() < 128 && p.alpha() == 255,
                "page-two rule pixel {p:?}"
            );
        }

        #[test]
        fn wide_pages_are_downscaled_and_tiny_pages_capped_at_4x() {
            // Same box as the onboarding flow chart fixture: 3150×1800 pt.
            let wide = tiny_pdf_with(&[PAGE_ONE], "[0 0 3150 1800]", None, None);
            let pixmap = decode(&render_first_page(&wide, 2048).unwrap().png);
            assert_eq!((pixmap.width(), pixmap.height()), (2048, 1170));

            let tiny = tiny_pdf_with(&[PAGE_ONE], "[0 0 100 50]", None, None);
            let pixmap = decode(&render_first_page(&tiny, 2048).unwrap().png);
            assert_eq!((pixmap.width(), pixmap.height()), (400, 200));
        }

        #[test]
        fn rotated_pages_take_their_displayed_orientation() {
            let landscape = tiny_pdf_with(&[PAGE_ONE], "[0 0 612 792]", None, Some(90));
            let pixmap = decode(&render_first_page(&landscape, 2048).unwrap().png);
            assert_eq!((pixmap.width(), pixmap.height()), (2048, 1583));
            assert!(pixmap.pixels().iter().any(|p| p.red() < 128));
        }

        #[test]
        fn crop_box_wins_over_media_box() {
            // Letter MediaBox with bleed trimmed to a 300×400 pt CropBox; the
            // rule is drawn inside the crop, the page-one text outside it.
            const INSIDE_CROP: &str = "4 w 100 300 m 400 300 l S";
            let cropped = tiny_pdf_with(
                &[&format!("{PAGE_ONE}\n{INSIDE_CROP}")],
                "[0 0 612 792]",
                Some("[100 100 400 500]"),
                None,
            );
            let pixmap = decode(&render_first_page(&cropped, 2048).unwrap().png);
            // 2048 / 400 pt would be 5.12x, so the 4x ceiling applies.
            assert_eq!((pixmap.width(), pixmap.height()), (1200, 1600));
            let rule_y = ((500.0 - 300.0) * 4.0) as u32;
            let p = pixmap.pixel(600, rule_y).unwrap();
            assert!(p.red() < 128 && p.alpha() == 255, "rule pixel {p:?}");
            for (x, y) in [(0, 0), (1199, 0), (0, 1599), (1199, 1599)] {
                assert!(is_opaque_white(&pixmap, x, y), "corner {x},{y}");
            }

            // An empty CropBox is not drawable, so the MediaBox is used.
            let degenerate = tiny_pdf_with(&[PAGE_ONE], "[0 0 612 792]", Some("[0 0 0 0]"), None);
            let pixmap = decode(&render_first_page(&degenerate, 2048).unwrap().png);
            assert_eq!((pixmap.width(), pixmap.height()), (1583, 2048));
        }

        #[test]
        fn malformed_pdfs_are_rejected_without_hanging() {
            for bytes in [
                &b"%PDF-1.4\nnot really a pdf"[..],
                b"%PDF-",
                &tiny_pdf(&[])[..],
            ] {
                let err = render_first_page(bytes, 2048).unwrap_err();
                assert!(err.contains("PDF"), "{err}");
                assert_eq!(check(bytes).unwrap_err(), err);
            }
            check(&tiny_pdf(&[PAGE_ONE])).unwrap();
        }

        #[test]
        fn render_pages_stops_at_the_cap_and_reports_the_total() {
            let pdf = tiny_pdf(&[PAGE_ONE, PAGE_TWO, PAGE_ONE]);
            let rendered = render_pages(&pdf, 2, 512).unwrap();
            assert_eq!(rendered.page_count, 3);
            assert_eq!(rendered.pages.len(), 2);
            let row = ((792.0 - 400.0) * 512.0 / 792.0) as u32;
            let first = decode(&rendered.pages[0]);
            let second = decode(&rendered.pages[1]);
            assert_eq!((first.width(), first.height()), (396, 512));
            assert!(is_opaque_white(&first, 200, row));
            assert!(second.pixel(200, row).unwrap().red() < 128);

            let all = render_pages(&pdf, 20, 512).unwrap();
            assert_eq!((all.pages.len(), all.page_count), (3, 3));
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn reports_platform_support() {
        let pdf = tiny_pdf(&[PAGE_ONE]);
        let err = render_first_page(&pdf, 2048).unwrap_err();
        assert!(err.contains("macOS"), "{err}");
        let err = render_pages(&pdf, 1, 2048).unwrap_err();
        assert!(err.contains("macOS"), "{err}");
        // Storage only needs the signature here.
        check(&pdf).unwrap();
        check(b"%PDF-1.4\ngarbage").unwrap();
    }
}
