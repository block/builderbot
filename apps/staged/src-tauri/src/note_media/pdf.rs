//! PDF attachments are stored as the PDF itself; this module rasterizes pages
//! to opaque bitmaps where one is needed: the thumbnail shown next to image
//! attachments, and the agent prompt, whose transports accept only image
//! blocks. Rendering uses CoreGraphics in-process (no subprocess, no temp
//! files, malformed input fails fast instead of hanging). Pages are PNG, or
//! JPEG when a caller allows it and the PNG cannot meet its size cap.
//!
//! macOS only — the shipped target. Other platforms compile stubs so the
//! Linux CI build stays green; they accept any `%PDF-` payload at store time
//! and report that rendering needs macOS. The stubs leave the shared types
//! only half used there, hence the module-wide allowance.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

#[cfg(target_os = "macos")]
use super::files::size_error;
use super::files::MAX_IMAGE_SIZE;

/// Long edge of a page rendered for the agent prompt, in pixels. The Claude
/// API rejects any image over 2000 px on a side once a request carries more
/// than 20 image blocks (it counts the images resent from earlier turns and
/// those in tool results, so a 20-page PDF plus one screenshot gets there),
/// and the high-resolution tier tops out at 2576 px, so 2000 loses nothing a
/// 2048 would have kept. A letter page lands at ~182 DPI, which keeps body
/// text legible to a vision model without approaching the attachment size cap.
pub(crate) const MAX_EDGE_PX: u32 = 2000;

/// Pages are never blown up beyond 4 px per PDF point, so a tiny page does
/// not become a huge blurry bitmap just to reach the long-edge target.
#[cfg(target_os = "macos")]
const MAX_SCALE: f64 = 4.0;

/// Quality of the JPEG a page falls back to when its PNG is over the cap even
/// at half scale — photographic scans, where a PNG is several times the size
/// of a JPEG that looks the same.
#[cfg(target_os = "macos")]
const JPEG_QUALITY: u8 = 85;

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

/// One page, encoded: PNG unless the [`Fit`] allowed a JPEG and it was needed.
pub(crate) struct EncodedPage {
    pub bytes: Vec<u8>,
    pub mime_type: &'static str,
}

impl std::fmt::Debug for EncodedPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncodedPage")
            .field("bytes", &self.bytes.len())
            .field("mime_type", &self.mime_type)
            .finish()
    }
}

/// How large an encoded page may be, and how far to go to get it there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fit {
    /// Ceiling on the encoded bytes.
    pub max_bytes: u64,
    /// When the PNG is over the ceiling even at half scale, try a full-scale
    /// JPEG before giving up on the page.
    pub jpeg_fallback: bool,
}

impl Fit {
    /// A PNG under the media cap: thumbnails, which are cached as PNG files.
    pub(crate) const PNG_MEDIA: Fit = Fit {
        max_bytes: MAX_IMAGE_SIZE,
        jpeg_fallback: false,
    };
}

/// Render page 1 to a white-backed PNG whose long edge is `max_edge_px`
/// (less when that would exceed [`MAX_SCALE`]).
pub(crate) fn render_first_page(bytes: &[u8], max_edge_px: u32) -> Result<RenderedPage, String> {
    let document = Document::open(bytes)?;
    let page = document.render_page(1, max_edge_px, Fit::PNG_MEDIA)?;
    Ok(RenderedPage {
        png: page.bytes,
        page_count: document.page_count(),
    })
}

/// Reject what CoreGraphics cannot open before it is stored, so a `%PDF-`
/// prefix on garbage never becomes an attachment that fails to render later.
#[cfg(target_os = "macos")]
pub(crate) fn check(bytes: &[u8]) -> Result<(), String> {
    Document::open(bytes).map(drop)
}

/// An open PDF. Pages are rendered one at a time so a caller with a page cap
/// or a byte budget can stop without rasterizing the rest.
#[cfg(target_os = "macos")]
pub(crate) struct Document {
    document: objc2_core_foundation::CFRetained<objc2_core_graphics::CGPDFDocument>,
    page_count: usize,
}

#[cfg(target_os = "macos")]
impl Document {
    pub(crate) fn open(bytes: &[u8]) -> Result<Self, String> {
        use objc2_core_foundation::CFData;
        use objc2_core_graphics::{CGDataProvider, CGPDFDocument};

        let data = CFData::from_bytes(bytes);
        let provider =
            CGDataProvider::with_cf_data(Some(&data)).ok_or("could not read PDF data")?;
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

    pub(crate) fn page_count(&self) -> usize {
        self.page_count
    }

    /// Draw page `number` (1-based) at `max_edge_px` along its long edge
    /// (less when that would exceed [`MAX_SCALE`]) and encode it within
    /// `fit`: see [`fit_page`].
    pub(crate) fn render_page(
        &self,
        number: usize,
        max_edge_px: u32,
        fit: Fit,
    ) -> Result<EncodedPage, String> {
        use objc2_core_graphics::{CGPDFBox, CGPDFDocument, CGPDFPage};

        let page = CGPDFDocument::page(Some(&self.document), number)
            .ok_or_else(|| format!("could not read PDF page {number}"))?;

        // CropBox is what Preview and Quick Look display (CoreGraphics clips
        // it to the MediaBox, and returns the MediaBox when none is set), so
        // a print-ready PDF renders without its bleed and crop marks. Fall
        // back to the MediaBox only when the CropBox is unusable.
        let mut page_box = CGPDFBox::CropBox;
        let mut rect = CGPDFPage::box_rect(Some(&page), page_box);
        if !is_drawable(rect.size.width, rect.size.height) {
            page_box = CGPDFBox::MediaBox;
            rect = CGPDFPage::box_rect(Some(&page), page_box);
            if !is_drawable(rect.size.width, rect.size.height) {
                return Err("PDF page has no drawable area".into());
            }
        }
        // `drawing_transform` honours the page's own /Rotate, so the bitmap
        // must take the rotated orientation for the page to fill it.
        let rotated = CGPDFPage::rotation_angle(Some(&page)).rem_euclid(180) == 90;
        let (w_pt, h_pt) = if rotated {
            (rect.size.height, rect.size.width)
        } else {
            (rect.size.width, rect.size.height)
        };
        let scale = (f64::from(max_edge_px) / w_pt.max(h_pt)).min(MAX_SCALE);
        fit_page(
            |scale| render(&page, page_box, (w_pt, h_pt), scale),
            scale,
            fit,
        )
    }
}

/// Encode the page `render` draws at `scale` px/pt within `fit.max_bytes`:
/// as a PNG; if that is over, as a PNG of a render at half scale; if that is
/// still over and `fit` allows it, as a JPEG of the full-scale render; else
/// fail with the size message.
#[cfg(target_os = "macos")]
fn fit_page(
    render: impl Fn(f64) -> Result<tiny_skia::Pixmap, String>,
    scale: f64,
    fit: Fit,
) -> Result<EncodedPage, String> {
    let fits = |bytes: &[u8]| bytes.len() as u64 <= fit.max_bytes;
    let full = render(scale)?;
    let png = encode_png(&full)?;
    if fits(&png) {
        return Ok(EncodedPage::png(png));
    }
    let png = encode_png(&render(scale / 2.0)?)?;
    if fits(&png) {
        return Ok(EncodedPage::png(png));
    }
    if fit.jpeg_fallback {
        let jpeg = encode_jpeg(&full)?;
        if fits(&jpeg) {
            return Ok(EncodedPage {
                bytes: jpeg,
                mime_type: "image/jpeg",
            });
        }
    }
    Err(size_error("PDF page", fit.max_bytes))
}

#[cfg(target_os = "macos")]
impl EncodedPage {
    fn png(bytes: Vec<u8>) -> Self {
        EncodedPage {
            bytes,
            mime_type: "image/png",
        }
    }
}

#[cfg(target_os = "macos")]
fn encode_png(pixmap: &tiny_skia::Pixmap) -> Result<Vec<u8>, String> {
    pixmap
        .encode_png()
        .map_err(|e| format!("could not encode PDF page: {e}"))
}

/// Pages are drawn over opaque white, so demultiplying changes nothing today;
/// it keeps the conversion right should the background ever go.
#[cfg(target_os = "macos")]
fn encode_jpeg(pixmap: &tiny_skia::Pixmap) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;

    let rgb: Vec<u8> = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue()]
        })
        .collect();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .write_image(
            &rgb,
            pixmap.width(),
            pixmap.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("could not encode PDF page as JPEG: {e}"))?;
    Ok(out)
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
/// bitmap at `scale` px/pt as a tiny-skia pixmap (premultiplied RGBA8, the
/// same layout CoreGraphics writes for `PremultipliedLast`).
#[cfg(target_os = "macos")]
fn render(
    page: &objc2_core_graphics::CGPDFPage,
    page_box: objc2_core_graphics::CGPDFBox,
    (w_pt, h_pt): (f64, f64),
    scale: f64,
) -> Result<tiny_skia::Pixmap, String> {
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
    tiny_skia::Pixmap::from_vec(buf, size).ok_or_else(|| "invalid PDF page bitmap".into())
}

#[cfg(not(target_os = "macos"))]
pub(crate) const UNSUPPORTED: &str = "PDF rendering requires macOS";

/// Without a renderer only the signature is checked; the file is still
/// stored and viewed as a PDF.
#[cfg(not(target_os = "macos"))]
pub(crate) fn check(_bytes: &[u8]) -> Result<(), String> {
    Ok(())
}

/// No renderer: [`Document::open`] always fails, so no page is ever rendered.
#[cfg(not(target_os = "macos"))]
pub(crate) enum Document {}

#[cfg(not(target_os = "macos"))]
impl Document {
    pub(crate) fn open(_bytes: &[u8]) -> Result<Self, String> {
        Err(UNSUPPORTED.into())
    }

    pub(crate) fn page_count(&self) -> usize {
        match *self {}
    }

    pub(crate) fn render_page(
        &self,
        _number: usize,
        _max_edge_px: u32,
        _fit: Fit,
    ) -> Result<EncodedPage, String> {
        match *self {}
    }
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
        /// letter page rendered [`MAX_EDGE_PX`] tall.
        fn letter_rule_row(y_pt: f64) -> u32 {
            ((792.0 - y_pt) * f64::from(MAX_EDGE_PX) / 792.0) as u32
        }

        #[test]
        fn renders_first_page_to_opaque_png_at_the_long_edge() {
            let rendered =
                render_first_page(&tiny_pdf(&[PAGE_ONE, PAGE_TWO]), MAX_EDGE_PX).unwrap();
            assert_eq!(rendered.page_count, 2);
            let pixmap = decode(&rendered.png);
            // Letter: 2000 / 792 pt ≈ 2.53x, under the 4x ceiling.
            assert_eq!((pixmap.width(), pixmap.height()), (1545, 2000));
            for (x, y) in [(0, 0), (1544, 0), (0, 1999), (1544, 1999)] {
                assert!(is_opaque_white(&pixmap, x, y), "corner {x},{y}");
            }
            // The rule at y=650pt from the bottom is drawn; everything is opaque.
            let p = pixmap.pixel(800, letter_rule_row(650.0)).unwrap();
            assert!(p.red() < 128 && p.alpha() == 255, "rule pixel {p:?}");
            assert!(pixmap.pixels().iter().all(|p| p.alpha() == 255));
            // Page two's own rule, at y=400pt, is not drawn...
            assert!(is_opaque_white(&pixmap, 800, letter_rule_row(400.0)));
            // ...and would have been, had page two been the one rendered.
            let swapped = decode(
                &render_first_page(&tiny_pdf(&[PAGE_TWO]), MAX_EDGE_PX)
                    .unwrap()
                    .png,
            );
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
            let pixmap = decode(&render_first_page(&wide, MAX_EDGE_PX).unwrap().png);
            assert_eq!((pixmap.width(), pixmap.height()), (2000, 1143));

            let tiny = tiny_pdf_with(&[PAGE_ONE], "[0 0 100 50]", None, None);
            let pixmap = decode(&render_first_page(&tiny, MAX_EDGE_PX).unwrap().png);
            assert_eq!((pixmap.width(), pixmap.height()), (400, 200));
        }

        #[test]
        fn rotated_pages_take_their_displayed_orientation() {
            let landscape = tiny_pdf_with(&[PAGE_ONE], "[0 0 612 792]", None, Some(90));
            let pixmap = decode(&render_first_page(&landscape, MAX_EDGE_PX).unwrap().png);
            assert_eq!((pixmap.width(), pixmap.height()), (2000, 1545));
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
            let pixmap = decode(&render_first_page(&cropped, MAX_EDGE_PX).unwrap().png);
            // 2000 / 400 pt would be 5x, so the 4x ceiling applies.
            assert_eq!((pixmap.width(), pixmap.height()), (1200, 1600));
            let rule_y = ((500.0 - 300.0) * 4.0) as u32;
            let p = pixmap.pixel(600, rule_y).unwrap();
            assert!(p.red() < 128 && p.alpha() == 255, "rule pixel {p:?}");
            for (x, y) in [(0, 0), (1199, 0), (0, 1599), (1199, 1599)] {
                assert!(is_opaque_white(&pixmap, x, y), "corner {x},{y}");
            }

            // An empty CropBox is not drawable, so the MediaBox is used.
            let degenerate = tiny_pdf_with(&[PAGE_ONE], "[0 0 612 792]", Some("[0 0 0 0]"), None);
            let pixmap = decode(&render_first_page(&degenerate, MAX_EDGE_PX).unwrap().png);
            assert_eq!((pixmap.width(), pixmap.height()), (1545, 2000));
        }

        #[test]
        fn malformed_pdfs_are_rejected_without_hanging() {
            for bytes in [
                &b"%PDF-1.4\nnot really a pdf"[..],
                b"%PDF-",
                &tiny_pdf(&[])[..],
            ] {
                let err = render_first_page(bytes, MAX_EDGE_PX).unwrap_err();
                assert!(err.contains("PDF"), "{err}");
                assert_eq!(check(bytes).unwrap_err(), err);
            }
            check(&tiny_pdf(&[PAGE_ONE])).unwrap();
        }

        #[test]
        fn documents_render_pages_on_demand() {
            let document = Document::open(&tiny_pdf(&[PAGE_ONE, PAGE_TWO, PAGE_ONE])).unwrap();
            assert_eq!(document.page_count(), 3);
            let row = ((792.0 - 400.0) * 512.0 / 792.0) as u32;
            let first = document.render_page(1, 512, Fit::PNG_MEDIA).unwrap();
            let second = document.render_page(2, 512, Fit::PNG_MEDIA).unwrap();
            assert_eq!(
                (first.mime_type, second.mime_type),
                ("image/png", "image/png")
            );
            let first = decode(&first.bytes);
            let second = decode(&second.bytes);
            assert_eq!((first.width(), first.height()), (396, 512));
            assert!(is_opaque_white(&first, 200, row));
            assert!(second.pixel(200, row).unwrap().red() < 128);

            let err = document.render_page(4, 512, Fit::PNG_MEDIA).unwrap_err();
            assert!(err.contains("page 4"), "{err}");
        }

        /// `fit_page` only sees what the closure returns, so the chain can be
        /// pinned with stand-in bitmaps whose encoded sizes are known to
        /// order as a photographic page's do: PNG at half scale under PNG at
        /// full scale, and the full-scale JPEG under both.
        #[test]
        fn pages_fall_back_to_half_scale_then_jpeg_to_meet_the_cap() {
            let photo = |scale: f64| Ok(photographic_pixmap((256.0 * scale) as u32));
            let png_full = encode_png(&photo(1.0).unwrap()).unwrap();
            let png_half = encode_png(&photo(0.5).unwrap()).unwrap();
            let jpeg_full = encode_jpeg(&photo(1.0).unwrap()).unwrap();
            assert!(
                jpeg_full.len() < png_half.len() && png_half.len() < png_full.len(),
                "stand-in sizes: png {} / half {} / jpeg {}",
                png_full.len(),
                png_half.len(),
                jpeg_full.len()
            );
            let fit = |max_bytes: usize, jpeg_fallback: bool| Fit {
                max_bytes: max_bytes as u64,
                jpeg_fallback,
            };

            let page = fit_page(photo, 1.0, fit(png_full.len(), false)).unwrap();
            assert_eq!(
                (page.mime_type, page.bytes.len()),
                ("image/png", png_full.len())
            );
            assert_eq!(decode(&page.bytes).width(), 256);

            let page = fit_page(photo, 1.0, fit(png_full.len() - 1, false)).unwrap();
            assert_eq!(
                (page.mime_type, page.bytes.len()),
                ("image/png", png_half.len())
            );
            assert_eq!(decode(&page.bytes).width(), 128);

            let page = fit_page(photo, 1.0, fit(png_half.len() - 1, true)).unwrap();
            assert_eq!(
                (page.mime_type, page.bytes.len()),
                ("image/jpeg", jpeg_full.len())
            );
            assert!(page.bytes.starts_with(&[0xFF, 0xD8, 0xFF]));

            let err = fit_page(photo, 1.0, fit(png_half.len() - 1, false)).unwrap_err();
            assert_eq!(err, "PDF page exceeds 0 MB limit");
            let err = fit_page(photo, 1.0, fit(jpeg_full.len() - 1, true)).unwrap_err();
            assert!(err.starts_with("PDF page exceeds"), "{err}");
        }

        /// Smooth colour gradients under low-amplitude per-pixel noise: PNG
        /// filters cannot predict the noise, JPEG quantizes it away.
        fn photographic_pixmap(edge: u32) -> tiny_skia::Pixmap {
            let mut pixmap = tiny_skia::Pixmap::new(edge, edge).unwrap();
            let mut seed = 0x2545_F491_4F6C_DD1Du64;
            let mut noise = move || {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed % 7) as f64 - 3.0
            };
            let channel = |v: f64| v.clamp(0.0, 255.0) as u8;
            for (i, pixel) in pixmap.pixels_mut().iter_mut().enumerate() {
                let x = f64::from(i as u32 % edge) / f64::from(edge);
                let y = f64::from(i as u32 / edge) / f64::from(edge);
                let r = channel(x * 200.0 + 30.0 + noise());
                let g = channel(y * 200.0 + 30.0 + noise());
                let b = channel(((x + y) * 3.0).sin() * 80.0 + 128.0 + noise());
                *pixel = tiny_skia::ColorU8::from_rgba(r, g, b, 255).premultiply();
            }
            pixmap
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn reports_platform_support() {
        let pdf = tiny_pdf(&[PAGE_ONE]);
        let err = render_first_page(&pdf, MAX_EDGE_PX).unwrap_err();
        assert!(err.contains("macOS"), "{err}");
        let err = Document::open(&pdf).err().unwrap();
        assert!(err.contains("macOS"), "{err}");
        // Storage only needs the signature here.
        check(&pdf).unwrap();
        check(b"%PDF-1.4\ngarbage").unwrap();
    }
}
