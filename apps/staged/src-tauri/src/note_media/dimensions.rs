//! Pixel size and density of a stored raster image, read from its own
//! metadata rather than assumed.
//!
//! Note images arrive from very different sources: 2x Mac screenshots, 1x
//! captures from an external display, camera photos, pikchr/SVG renders,
//! rasterized PDF pages. Only the file itself knows which it is, so density
//! comes from what the encoder wrote — the PNG `pHYs` chunk (pixels per
//! metre; 5669 is the 144 dpi macOS stamps on Retina captures, 2835 is 72)
//! and a JPEG's JFIF density or EXIF `XResolution`/`ResolutionUnit`. Only
//! a whole multiple of 72 dpi (144, 216, 288 — what screenshot tools write)
//! is read as a screen density; a camera's or scanner's 300 or 600 is a print
//! intent and, like a file that declares nothing or an unspecified unit, is
//! 1x. A JPEG's EXIF `Orientation` is honoured too, since browsers draw the
//! photo rotated and the size must describe what they draw.

use crate::store::ImageDimensions;

/// Size and density of a PNG, JPEG, GIF, or WebP; None for anything else or
/// for headers that cannot be read, so a bad file degrades to the CSS-only
/// layout rather than failing the attach.
pub(crate) fn probe(bytes: &[u8]) -> Option<ImageDimensions> {
    let format = super::files::sniff_format(bytes)?;
    if !matches!(format, "png" | "jpg" | "gif" | "webp") {
        return None;
    }
    let size = imagesize::blob_size(bytes).ok()?;
    let width = u32::try_from(size.width).ok()?;
    let height = u32::try_from(size.height).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    let (dpi, transposed) = match format {
        "png" => (png_dpi(bytes), false),
        "jpg" => {
            let metadata = jpeg_metadata(bytes);
            (metadata.dpi, metadata.transposed)
        }
        _ => (None, false),
    };
    // Browsers default to `image-orientation: from-image`, so a phone photo
    // stored as landscape data with orientation 6 is drawn portrait; the
    // size has to describe the drawn image or `width`/`height` would be
    // swapped and the capped figure would letterbox it.
    let (width, height) = if transposed {
        (height, width)
    } else {
        (width, height)
    };
    Some(ImageDimensions {
        width,
        height,
        density: density_from_dpi(dpi),
    })
}

/// How far a declared resolution may sit from a multiple of 72 dpi and still
/// count as one: enough for the integer px/m and px/cm the formats store
/// (5669 px/m is 143.99 dpi, 57 px/cm is 144.8) and well short of anything
/// a camera or scanner writes.
const DPI_TOLERANCE: f64 = 2.0;

/// 2, 3, or 4 when the metadata says 144, 216, or 288 dpi, else 1.
///
/// Declared resolutions are a screen density only when a screenshot tool
/// wrote them: macOS stamps 144 on Retina captures, 216 is a 3x phone. A
/// camera's 300 or 350, a scanner's 300 or 600, and the 96 some Windows
/// encoders default to are print intent or convention, and reading them as
/// density would shrink a 1200x800 photo to 300x200 — so anything off the
/// 72 multiples is 1x, as is 72 itself, nothing at all, or an unspecified
/// unit.
fn density_from_dpi(dpi: Option<f64>) -> u8 {
    let Some(dpi) = dpi else { return 1 };
    let multiple = (dpi / 72.0).round();
    if (2.0..=4.0).contains(&multiple) && (dpi - multiple * 72.0).abs() <= DPI_TOLERANCE {
        multiple as u8
    } else {
        1
    }
}

/// Horizontal resolution from the `pHYs` chunk, when its unit is the metre.
fn png_dpi(bytes: &[u8]) -> Option<f64> {
    let reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .ok()?;
    let dims = reader.info().pixel_dims?;
    (dims.unit == png::Unit::Meter && dims.xppu > 0).then(|| f64::from(dims.xppu) * 0.0254)
}

/// What a JPEG's headers say about how it is to be drawn.
#[derive(Default)]
struct JpegMetadata {
    /// JFIF density when its unit is specified, else the EXIF resolution.
    dpi: Option<f64>,
    /// EXIF `Orientation` 5..=8: the stored rows run down the drawn image, so
    /// the drawn width is the stored height.
    transposed: bool,
}

/// A walk over the marker segments before the first scan: APP0 `JFIF\0`
/// carries a unit byte (1 inch, 2 centimetre, 0 aspect ratio only) and X/Y
/// densities; APP1 `Exif\0\0` carries a TIFF header. Segment lengths are
/// trusted only as far as the buffer goes, so a truncated file gives None.
fn jpeg_metadata(bytes: &[u8]) -> JpegMetadata {
    let mut at = 2;
    let mut jfif_dpi = None;
    let mut exif = None;
    loop {
        if bytes.get(at) != Some(&0xFF) {
            break;
        }
        let Some(&marker) = bytes.get(at + 1) else {
            break;
        };
        if marker == 0xFF {
            at += 1;
            continue;
        }
        // SOS (image data), EOI, and the length-less markers end the headers.
        if matches!(marker, 0xDA | 0xD9 | 0xD8 | 0x01 | 0xD0..=0xD7) {
            break;
        }
        let (Some(&high), Some(&low)) = (bytes.get(at + 2), bytes.get(at + 3)) else {
            break;
        };
        let length = usize::from(u16::from_be_bytes([high, low]));
        let Some(payload) = bytes.get(at + 4..at + 2 + length) else {
            break;
        };
        match marker {
            0xE0 if jfif_dpi.is_none() && payload.starts_with(b"JFIF\0") => {
                jfif_dpi = jfif_density(payload);
            }
            0xE1 if exif.is_none() => {
                if let Some(tiff) = payload.strip_prefix(b"Exif\0\0") {
                    exif = exif_ifd0(tiff);
                }
            }
            _ => {}
        }
        at += 2 + length;
    }
    let exif = exif.unwrap_or_default();
    JpegMetadata {
        dpi: jfif_dpi.or(exif.dpi),
        transposed: matches!(exif.orientation, Some(5..=8)),
    }
}

/// X density of a JFIF APP0 payload in dots per inch, when its unit is
/// inches or centimetres and the density is set.
fn jfif_density(payload: &[u8]) -> Option<f64> {
    let x_density = f64::from(u16::from_be_bytes([*payload.get(8)?, *payload.get(9)?]));
    match payload.get(7)? {
        1 if x_density > 0.0 => Some(x_density),
        2 if x_density > 0.0 => Some(x_density * 2.54),
        _ => None,
    }
}
/// The IFD0 tags read from an EXIF TIFF header.
#[derive(Default)]
struct ExifIfd0 {
    /// `XResolution` (tag 0x011A, RATIONAL) in `ResolutionUnit` (tag 0x0128,
    /// SHORT: 2 inch, 3 centimetre, default inch), as dots per inch.
    dpi: Option<f64>,
    /// `Orientation` (tag 0x0112, SHORT), 1..=8 when present.
    orientation: Option<u16>,
}

/// IFD0 of a TIFF header — the payload of a JPEG APP1 segment after
/// `Exif\0\0`. Only the three tags above are read; a pointer that runs off
/// the end gives None.
fn exif_ifd0(tiff: &[u8]) -> Option<ExifIfd0> {
    let big_endian = match tiff.get(0..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let raw: [u8; 2] = tiff.get(at..at + 2)?.try_into().ok()?;
        Some(if big_endian {
            u16::from_be_bytes(raw)
        } else {
            u16::from_le_bytes(raw)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let raw: [u8; 4] = tiff.get(at..at + 4)?.try_into().ok()?;
        Some(if big_endian {
            u32::from_be_bytes(raw)
        } else {
            u32::from_le_bytes(raw)
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = u32_at(4)? as usize;
    let entries = usize::from(u16_at(ifd)?);
    let mut x_resolution = None;
    let mut unit = 2;
    let mut orientation = None;
    for index in 0..entries {
        let entry = ifd + 2 + index * 12;
        match (u16_at(entry)?, u16_at(entry + 2)?) {
            (0x011A, 5) => {
                let at = u32_at(entry + 8)? as usize;
                let (numerator, denominator) = (u32_at(at)?, u32_at(at + 4)?);
                if denominator != 0 {
                    x_resolution = Some(f64::from(numerator) / f64::from(denominator));
                }
            }
            (0x0128, 3) => unit = u16_at(entry + 8)?,
            (0x0112, 3) => orientation = Some(u16_at(entry + 8)?),
            _ => {}
        }
    }
    Some(ExifIfd0 {
        dpi: match unit {
            2 => x_resolution,
            3 => x_resolution.map(|per_cm| per_cm * 2.54),
            _ => None,
        },
        orientation,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 8x6 PNGs written with and without `pHYs 5669 5669 1` (144 dpi).
    pub(crate) const PHYS_144DPI_PNG: &[u8] = include_bytes!("fixtures/phys-144dpi.png");
    pub(crate) const NO_PHYS_PNG: &[u8] = include_bytes!("fixtures/no-phys.png");
    /// The same picture through `sips -s dpiWidth 144 -s dpiHeight 144`: a
    /// JFIF APP0 whose density is 144x144 but whose unit byte is 0 (aspect
    /// ratio only), then an EXIF APP1 with `XResolution 144/1` and
    /// `ResolutionUnit 2` — so as written, the EXIF segment is what settles it.
    const SIPS_144DPI_JPG: &[u8] = include_bytes!("fixtures/sips-144dpi.jpg");

    fn dims(width: u32, height: u32, density: u8) -> Option<ImageDimensions> {
        Some(ImageDimensions {
            width,
            height,
            density,
        })
    }

    fn png_with_phys(phys: Option<png::PixelDimensions>) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 8, 6);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_pixel_dims(phys);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0; 8 * 6 * 3]).unwrap();
        writer.finish().unwrap();
        bytes
    }

    fn phys(ppm: u32, unit: png::Unit) -> Option<png::PixelDimensions> {
        Some(png::PixelDimensions {
            xppu: ppm,
            yppu: ppm,
            unit,
        })
    }

    #[test]
    fn png_density_comes_from_phys() {
        assert_eq!(probe(PHYS_144DPI_PNG), dims(8, 6, 2));
        assert_eq!(probe(NO_PHYS_PNG), dims(8, 6, 1));
        // 72 dpi is the baseline, not a fraction; an unspecified unit says
        // nothing about physical size even when its numbers look Retina.
        assert_eq!(
            probe(&png_with_phys(phys(2835, png::Unit::Meter))),
            dims(8, 6, 1)
        );
        assert_eq!(
            probe(&png_with_phys(phys(5669, png::Unit::Unspecified))),
            dims(8, 6, 1)
        );
        // 216 and 288 dpi (3x and 4x) in whole pixels per metre.
        assert_eq!(
            probe(&png_with_phys(phys(8504, png::Unit::Meter))),
            dims(8, 6, 3)
        );
        assert_eq!(
            probe(&png_with_phys(phys(11339, png::Unit::Meter))),
            dims(8, 6, 4)
        );
        // A 300 or 600 dpi scan is print intent, not a screen density.
        assert_eq!(
            probe(&png_with_phys(phys(11811, png::Unit::Meter))),
            dims(8, 6, 1)
        );
        assert_eq!(
            probe(&png_with_phys(phys(23622, png::Unit::Meter))),
            dims(8, 6, 1)
        );
    }

    /// The fixture's segments: SOI, APP0 (2 + 16 bytes), APP1 (2 + 128 bytes).
    fn jpeg_without(app0: bool, app1: bool) -> Vec<u8> {
        let (app0_end, app1_end) = (2 + 2 + 16, 2 + 2 + 16 + 2 + 128);
        assert_eq!(&SIPS_144DPI_JPG[2..4], b"\xff\xe0");
        assert_eq!(&SIPS_144DPI_JPG[app0_end..app0_end + 2], b"\xff\xe1");
        let mut bytes = SIPS_144DPI_JPG[..2].to_vec();
        if !app0 {
            bytes.extend_from_slice(&SIPS_144DPI_JPG[2..app0_end]);
        }
        if !app1 {
            bytes.extend_from_slice(&SIPS_144DPI_JPG[app0_end..app1_end]);
        }
        bytes.extend_from_slice(&SIPS_144DPI_JPG[app1_end..]);
        bytes
    }

    /// JFIF APP0 only, with the unit byte (offset 13) and X density set.
    fn jfif_only(unit: u8, x_density: u16) -> Vec<u8> {
        let mut bytes = jpeg_without(false, true);
        assert_eq!(&bytes[6..11], b"JFIF\0");
        bytes[13] = unit;
        bytes[14..16].copy_from_slice(&x_density.to_be_bytes());
        bytes
    }

    #[test]
    fn jpeg_density_comes_from_jfif_then_exif() {
        // As sips wrote it: JFIF unit 0 says nothing, EXIF says 144 dpi.
        assert_eq!(probe(SIPS_144DPI_JPG), dims(8, 6, 2));
        assert_eq!(probe(&jpeg_without(true, false)), dims(8, 6, 2));
        // JFIF alone, in dots per inch and per centimetre.
        assert_eq!(probe(&jfif_only(1, 144)), dims(8, 6, 2));
        assert_eq!(probe(&jfif_only(1, 72)), dims(8, 6, 1));
        assert_eq!(probe(&jfif_only(2, 57)), dims(8, 6, 2));
        // Unit 0 is an aspect ratio, not a resolution, however Retina the
        // numbers look.
        assert_eq!(probe(&jfif_only(0, 144)), dims(8, 6, 1));
        // Neither segment: 1x.
        assert_eq!(probe(&jpeg_without(true, true)), dims(8, 6, 1));
    }

    /// IFD0 with `XResolution = numerator/denominator`, `ResolutionUnit`,
    /// and optionally `Orientation`.
    fn tiff(
        big_endian: bool,
        numerator: u32,
        denominator: u32,
        unit: u16,
        orientation: Option<u16>,
    ) -> Vec<u8> {
        let u16b = |v: u16| {
            if big_endian {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let u32b = |v: u32| {
            if big_endian {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let entries = 2 + usize::from(orientation.is_some());
        // The RATIONAL sits after the entry count, the entries, and the
        // next-IFD link.
        let rational_at = 8 + 2 + entries * 12 + 4;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(if big_endian { b"MM" } else { b"II" });
        bytes.extend_from_slice(&u16b(42));
        bytes.extend_from_slice(&u32b(8));
        bytes.extend_from_slice(&u16b(entries as u16));
        if let Some(orientation) = orientation {
            bytes.extend_from_slice(&u16b(0x0112));
            bytes.extend_from_slice(&u16b(3));
            bytes.extend_from_slice(&u32b(1));
            bytes.extend_from_slice(&u16b(orientation));
            bytes.extend_from_slice(&u16b(0));
        }
        bytes.extend_from_slice(&u16b(0x011A));
        bytes.extend_from_slice(&u16b(5));
        bytes.extend_from_slice(&u32b(1));
        bytes.extend_from_slice(&u32b(rational_at as u32));
        bytes.extend_from_slice(&u16b(0x0128));
        bytes.extend_from_slice(&u16b(3));
        bytes.extend_from_slice(&u32b(1));
        bytes.extend_from_slice(&u16b(unit));
        bytes.extend_from_slice(&u16b(0));
        bytes.extend_from_slice(&u32b(0));
        assert_eq!(bytes.len(), rational_at);
        bytes.extend_from_slice(&u32b(numerator));
        bytes.extend_from_slice(&u32b(denominator));
        bytes
    }

    fn exif_dpi(tiff: &[u8]) -> Option<f64> {
        exif_ifd0(tiff)?.dpi
    }

    #[test]
    fn exif_resolution_honours_byte_order_and_unit() {
        assert_eq!(exif_dpi(&tiff(true, 144, 1, 2, None)), Some(144.0));
        assert_eq!(exif_dpi(&tiff(false, 144, 1, 2, None)), Some(144.0));
        assert_eq!(
            exif_dpi(&tiff(true, 57, 1, 3, None)).map(f64::round),
            Some(145.0)
        );
        assert_eq!(exif_dpi(&tiff(true, 144, 1, 1, None)), None);
        assert_eq!(exif_dpi(&tiff(true, 144, 0, 2, None)), None);
        assert!(exif_ifd0(&tiff(true, 144, 1, 2, None)[..20]).is_none());
        assert!(exif_ifd0(b"XX\0\x2a").is_none());
    }

    #[test]
    fn only_whole_multiples_of_72_dpi_are_screen_densities() {
        assert_eq!(density_from_dpi(None), 1);
        assert_eq!(density_from_dpi(Some(72.0)), 1);
        assert_eq!(density_from_dpi(Some(144.0)), 2);
        assert_eq!(density_from_dpi(Some(216.0)), 3);
        assert_eq!(density_from_dpi(Some(288.0)), 4);
        // Within the tolerance that integer px/m and px/cm need.
        assert_eq!(density_from_dpi(Some(5669.0 * 0.0254)), 2);
        assert_eq!(density_from_dpi(Some(57.0 * 2.54)), 2);
        assert_eq!(density_from_dpi(Some(113.0 * 2.54)), 4);
        // Print intent and encoder conventions: cameras (300, 350), scanners
        // (300, 600), Windows (96), and anything past 4x.
        for dpi in [96.0, 150.0, 300.0, 350.0, 600.0, 360.0, f64::MAX] {
            assert_eq!(density_from_dpi(Some(dpi)), 1, "{dpi}");
        }
    }

    /// The fixture's picture with its APP0 and APP1 replaced by one APP1
    /// carrying `tiff`.
    fn jpeg_with_exif(tiff: &[u8]) -> Vec<u8> {
        let mut bytes = SIPS_144DPI_JPG[..2].to_vec();
        bytes.extend_from_slice(b"\xff\xe1");
        bytes.extend_from_slice(&u16::try_from(2 + 6 + tiff.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(b"Exif\0\0");
        bytes.extend_from_slice(tiff);
        bytes.extend_from_slice(&jpeg_without(true, true)[2..]);
        bytes
    }

    #[test]
    fn exif_orientation_transposes_the_drawn_size() {
        // Sanity: the synthesized segment reads like the fixture's own.
        assert_eq!(
            probe(&jpeg_with_exif(&tiff(true, 144, 1, 2, None))),
            dims(8, 6, 2)
        );
        for big_endian in [true, false] {
            // 1 (as stored) and 3 (rotated 180) keep the stored axes.
            for upright in [1, 3] {
                assert_eq!(
                    probe(&jpeg_with_exif(&tiff(big_endian, 144, 1, 2, Some(upright)))),
                    dims(8, 6, 2),
                    "orientation {upright}, big_endian {big_endian}"
                );
            }
            // 5..=8 are rotated by a quarter turn: an iPhone portrait shot is
            // landscape data with orientation 6, drawn with the stored height
            // as its width.
            for quarter_turn in 5..=8 {
                assert_eq!(
                    probe(&jpeg_with_exif(&tiff(
                        big_endian,
                        144,
                        1,
                        2,
                        Some(quarter_turn)
                    ))),
                    dims(6, 8, 2),
                    "orientation {quarter_turn}, big_endian {big_endian}"
                );
            }
        }
        assert_eq!(
            exif_ifd0(&tiff(false, 144, 1, 2, Some(6))).map(|ifd| ifd.orientation),
            Some(Some(6))
        );
        // Orientation is only read from the TIFF header, so a JFIF-only file
        // and the fixture (sips writes no Orientation tag) keep the stored
        // axes.
        assert_eq!(probe(&jfif_only(1, 144)), dims(8, 6, 2));
        assert_eq!(probe(SIPS_144DPI_JPG), dims(8, 6, 2));
    }

    #[test]
    fn gif_and_webp_have_a_size_and_no_density() {
        let gif = b"GIF89a\x40\x00\x30\x00\x00\x00\x00;";
        assert_eq!(probe(gif), dims(64, 48, 1));
        // VP8L: 14-bit width-1 and height-1 packed little-endian after 0x2f.
        let mut webp = b"RIFF\x12\x00\x00\x00WEBPVP8L\x06\x00\x00\x00\x2f".to_vec();
        webp.extend_from_slice(&(63u32 | (47u32 << 14)).to_le_bytes());
        webp.push(0);
        assert_eq!(probe(&webp), dims(64, 48, 1));
    }

    #[test]
    fn unreadable_or_non_raster_bytes_have_no_dimensions() {
        assert_eq!(probe(super::super::tests::PNG), None);
        assert_eq!(probe(b"\x1a\x45\xdf\xa3webm"), None);
        assert_eq!(probe(b"not an image"), None);
        assert_eq!(probe(&SIPS_144DPI_JPG[..40]), None);
    }
}
