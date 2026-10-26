//! Pixel size and density of a stored raster image, read from its own
//! metadata rather than assumed.
//!
//! Note images arrive from very different sources: 2x Mac screenshots, 1x
//! captures from an external display, camera photos, pikchr/SVG renders,
//! rasterized PDF pages. Only the file itself knows which it is, so density
//! comes from what the encoder wrote — the PNG `pHYs` chunk (pixels per
//! metre; 5669 is the 144 dpi macOS stamps on Retina captures, 2835 is 72)
//! and a JPEG's JFIF density or EXIF `XResolution`/`ResolutionUnit`. A file
//! that declares nothing, an unspecified unit, or 72 dpi or less is 1x.

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
    let dpi = match format {
        "png" => png_dpi(bytes),
        "jpg" => jpeg_dpi(bytes),
        _ => None,
    };
    Some(ImageDimensions {
        width,
        height,
        density: density_from_dpi(dpi),
    })
}

/// `round(dpi / 72)` clamped to 1..=4. 72 dpi is the 1x baseline, so a file
/// at or below it — including the 72 many encoders write by default — is 1x
/// rather than a fraction; anything the metadata cannot settle is 1x too.
fn density_from_dpi(dpi: Option<f64>) -> u8 {
    match dpi {
        Some(dpi) if dpi > 72.0 => ((dpi / 72.0).round() as u8).clamp(1, 4),
        _ => 1,
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

/// JFIF density when its unit is specified, else the EXIF resolution.
///
/// A walk over the marker segments before the first scan: APP0 `JFIF\0`
/// carries a unit byte (1 inch, 2 centimetre, 0 aspect ratio only) and X/Y
/// densities; APP1 `Exif\0\0` carries a TIFF header. Segment lengths are
/// trusted only as far as the buffer goes, so a truncated file gives None.
fn jpeg_dpi(bytes: &[u8]) -> Option<f64> {
    let mut at = 2;
    let mut exif = None;
    while bytes.get(at) == Some(&0xFF) {
        let marker = *bytes.get(at + 1)?;
        if marker == 0xFF {
            at += 1;
            continue;
        }
        // SOS (image data), EOI, and the length-less markers end the headers.
        if matches!(marker, 0xDA | 0xD9 | 0xD8 | 0x01 | 0xD0..=0xD7) {
            break;
        }
        let length = usize::from(u16::from_be_bytes([
            *bytes.get(at + 2)?,
            *bytes.get(at + 3)?,
        ]));
        let payload = bytes.get(at + 4..at + 2 + length)?;
        match marker {
            0xE0 if payload.starts_with(b"JFIF\0") => {
                let x_density = f64::from(u16::from_be_bytes([*payload.get(8)?, *payload.get(9)?]));
                match payload.get(7)? {
                    1 if x_density > 0.0 => return Some(x_density),
                    2 if x_density > 0.0 => return Some(x_density * 2.54),
                    _ => {}
                }
            }
            0xE1 if exif.is_none() => {
                if let Some(tiff) = payload.strip_prefix(b"Exif\0\0") {
                    exif = exif_dpi(tiff);
                }
            }
            _ => {}
        }
        at += 2 + length;
    }
    exif
}

/// `XResolution` (tag 0x011A, RATIONAL) in `ResolutionUnit` (tag 0x0128,
/// SHORT: 2 inch, 3 centimetre, default inch) from IFD0 of a TIFF header —
/// the payload of a JPEG APP1 segment after `Exif\0\0`. Only these two tags
/// are read; a pointer that runs off the end gives None.
fn exif_dpi(tiff: &[u8]) -> Option<f64> {
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
            _ => {}
        }
    }
    match unit {
        2 => x_resolution,
        3 => x_resolution.map(|per_cm| per_cm * 2.54),
        _ => None,
    }
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
        // 3x phones, a 300 dpi scan (4.17x), and 600 dpi all stay in 1..=4.
        assert_eq!(
            probe(&png_with_phys(phys(8504, png::Unit::Meter))),
            dims(8, 6, 3)
        );
        assert_eq!(
            probe(&png_with_phys(phys(11811, png::Unit::Meter))),
            dims(8, 6, 4)
        );
        assert_eq!(
            probe(&png_with_phys(phys(23622, png::Unit::Meter))),
            dims(8, 6, 4)
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

    /// IFD0 with `XResolution = numerator/denominator` and `ResolutionUnit`.
    fn tiff(big_endian: bool, numerator: u32, denominator: u32, unit: u16) -> Vec<u8> {
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
        let mut bytes = Vec::new();
        bytes.extend_from_slice(if big_endian { b"MM" } else { b"II" });
        bytes.extend_from_slice(&u16b(42));
        bytes.extend_from_slice(&u32b(8));
        bytes.extend_from_slice(&u16b(2));
        // XResolution: RATIONAL at offset 38 (after 2 entries + next-IFD link).
        bytes.extend_from_slice(&u16b(0x011A));
        bytes.extend_from_slice(&u16b(5));
        bytes.extend_from_slice(&u32b(1));
        bytes.extend_from_slice(&u32b(38));
        bytes.extend_from_slice(&u16b(0x0128));
        bytes.extend_from_slice(&u16b(3));
        bytes.extend_from_slice(&u32b(1));
        bytes.extend_from_slice(&u16b(unit));
        bytes.extend_from_slice(&u16b(0));
        bytes.extend_from_slice(&u32b(0));
        assert_eq!(bytes.len(), 38);
        bytes.extend_from_slice(&u32b(numerator));
        bytes.extend_from_slice(&u32b(denominator));
        bytes
    }

    #[test]
    fn exif_resolution_honours_byte_order_and_unit() {
        assert_eq!(exif_dpi(&tiff(true, 144, 1, 2)), Some(144.0));
        assert_eq!(exif_dpi(&tiff(false, 144, 1, 2)), Some(144.0));
        assert_eq!(exif_dpi(&tiff(true, 57, 1, 3)).map(f64::round), Some(145.0));
        assert_eq!(exif_dpi(&tiff(true, 144, 1, 1)), None);
        assert_eq!(exif_dpi(&tiff(true, 144, 0, 2)), None);
        assert_eq!(exif_dpi(&tiff(true, 144, 1, 2)[..20]), None);
        assert_eq!(exif_dpi(b"XX\0\x2a"), None);
        assert_eq!(density_from_dpi(Some(300.0)), 4);
        assert_eq!(density_from_dpi(Some(96.0)), 1);
        assert_eq!(density_from_dpi(Some(f64::MAX)), 4);
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
