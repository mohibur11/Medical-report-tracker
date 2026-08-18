//! Image normalization at ingest.
//!
//! Two jobs, both of which must happen once at import rather than at export time:
//!
//! 1. BAKE EXIF ORIENTATION INTO PIXELS. No PDF library on the market reads the
//!    orientation tag — pdf-lib issue #1284 is still open and pdfcpu punts to a
//!    separate Rotate command. Untreated, a large fraction of phone photos land
//!    sideways in every merged export. The tag is dropped afterwards so nothing
//!    downstream can apply it a second time.
//!
//! 2. PRODUCE A PRINT DERIVATIVE. Embedding 12 MP originals produces a 120-200 MB
//!    export nobody can email. A ~2000 px long edge at JPEG q78 keeps printed lab
//!    reports perfectly legible while landing an ordinary export in the tens of MB.
//!
//! The ORIGINAL FILE IS NEVER MODIFIED. Derivatives sit beside it. The vault has to
//! survive this app being uninstalled.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat};

/// Long edge of the derivative used to build exports. Roughly 200 DPI across an
/// A4 page, which is the point where printed report text stops improving.
pub const PRINT_LONG_EDGE: u32 = 2000;
pub const THUMB_LONG_EDGE: u32 = 320;

pub const PRINT_QUALITY: u8 = 78;
pub const THUMB_QUALITY: u8 = 70;

/// EXIF orientation, 1-8. 1 is upright; everything else needs baking.
pub type Orientation = u16;

/// Read the orientation tag. Returns 1 when there is no EXIF block, no orientation
/// field, or the value is out of range — the same treatment a viewer gives it.
pub fn read_orientation(bytes: &[u8]) -> Orientation {
    let mut cursor = Cursor::new(bytes);
    let reader = exif::Reader::new();

    let Ok(exif) = reader.read_from_container(&mut cursor) else {
        return 1;
    };
    let Some(field) = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY) else {
        return 1;
    };
    match field.value.get_uint(0) {
        Some(v) if (1..=8).contains(&v) => v as Orientation,
        _ => 1,
    }
}

pub fn needs_bake(orientation: Orientation) -> bool {
    orientation != 1
}

/// Apply the transform the EXIF value describes, so the stored pixels become the
/// pixels a viewer would have displayed.
///
/// `rotate90` in the image crate is clockwise. Values 2, 4, 5 and 7 are MIRRORED,
/// not merely rotated — dropping the mirror silently produces a horizontally
/// flipped scan, which on a lab report is subtle enough to survive review.
pub fn bake_orientation(img: DynamicImage, orientation: Orientation) -> DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

/// Scale so the long edge is at most `max_edge`. Never upscales — a 900 px scan of
/// a prescription gains nothing from being stretched to 2000 and costs bytes.
fn downscale(img: &DynamicImage, max_edge: u32, filter: image::imageops::FilterType) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w.max(h) <= max_edge {
        return img.clone();
    }
    let scale = max_edge as f32 / w.max(h) as f32;
    img.resize(
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
        filter,
    )
}

fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    // Baseline YCbCr 8-bit. RGBA sources must lose their alpha or the encoder errors;
    // a scan has no meaningful transparency anyway.
    let rgb = DynamicImage::ImageRgb8(img.to_rgb8());
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    encoder
        .encode_image(&rgb)
        .map_err(|e| format!("jpeg encode failed: {e}"))?;
    Ok(out)
}

pub struct Derivatives {
    /// Dimensions AFTER baking — width and height swap for orientations 5-8, and
    /// everything downstream (page fitting, thumbnails, OCR) needs the real ones.
    pub width: u32,
    pub height: u32,
    pub orientation: Orientation,
    pub print_jpeg: Vec<u8>,
    pub thumb_jpeg: Vec<u8>,
}

/// A thumbnail from bytes of unknown format — a page pdfcpu extracted, which may
/// be JPEG, PNG or TIFF depending on how the scanner stored it.
pub fn thumbnail_of_any(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("decode failed: {e}"))?;
    let thumb = downscale(&img, THUMB_LONG_EDGE, image::imageops::FilterType::Triangle);
    encode_jpeg(&thumb, THUMB_QUALITY)
}

/// Decode, bake, and produce both derivatives from raw file bytes.
pub fn derive(bytes: &[u8], format: ImageFormat) -> Result<Derivatives, String> {
    let orientation = read_orientation(bytes);

    let img = image::load_from_memory_with_format(bytes, format)
        .map_err(|e| format!("decode failed: {e}"))?;

    let img = bake_orientation(img, orientation);

    let print = downscale(&img, PRINT_LONG_EDGE, image::imageops::FilterType::Lanczos3);
    let thumb = downscale(&img, THUMB_LONG_EDGE, image::imageops::FilterType::Triangle);

    Ok(Derivatives {
        width: img.width(),
        height: img.height(),
        orientation,
        print_jpeg: encode_jpeg(&print, PRINT_QUALITY)?,
        thumb_jpeg: encode_jpeg(&thumb, THUMB_QUALITY)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// A deliberately asymmetric image: 4 wide, 2 tall, with a single red pixel at
    /// the top-left. Both the dimensions and that pixel's destination distinguish
    /// every one of the eight transforms, including mirrors from rotations.
    fn probe() -> DynamicImage {
        let mut img = RgbImage::from_pixel(4, 2, Rgb([255, 255, 255]));
        img.put_pixel(0, 0, Rgb([255, 0, 0]));
        DynamicImage::ImageRgb8(img)
    }

    fn red_at(img: &DynamicImage) -> (u32, u32) {
        let rgb = img.to_rgb8();
        for (x, y, p) in rgb.enumerate_pixels() {
            if p.0 == [255, 0, 0] {
                return (x, y);
            }
        }
        panic!("probe pixel lost");
    }

    #[test]
    fn upright_images_are_untouched() {
        let out = bake_orientation(probe(), 1);
        assert_eq!((out.width(), out.height()), (4, 2));
        assert_eq!(red_at(&out), (0, 0));
    }

    #[test]
    fn rotations_swap_dimensions() {
        for o in [5u16, 6, 7, 8] {
            let out = bake_orientation(probe(), o);
            assert_eq!(
                (out.width(), out.height()),
                (2, 4),
                "orientation {o} must swap width and height",
            );
        }
        for o in [1u16, 2, 3, 4] {
            let out = bake_orientation(probe(), o);
            assert_eq!(
                (out.width(), out.height()),
                (4, 2),
                "orientation {o} must preserve width and height",
            );
        }
    }

    #[test]
    fn each_orientation_moves_the_probe_pixel_where_the_spec_says() {
        // Source is 4x2 with red at (0,0).
        assert_eq!(red_at(&bake_orientation(probe(), 1)), (0, 0));
        assert_eq!(red_at(&bake_orientation(probe(), 2)), (3, 0), "mirror horizontal");
        assert_eq!(red_at(&bake_orientation(probe(), 3)), (3, 1), "rotate 180");
        assert_eq!(red_at(&bake_orientation(probe(), 4)), (0, 1), "mirror vertical");
        assert_eq!(red_at(&bake_orientation(probe(), 6)), (1, 0), "rotate 90 CW");
        assert_eq!(red_at(&bake_orientation(probe(), 8)), (0, 3), "rotate 270 CW");
    }

    #[test]
    fn mirrored_orientations_differ_from_their_plain_rotations() {
        // 5 vs 6 and 7 vs 8 share dimensions; only the mirror separates them.
        // Losing that distinction produces a silently flipped scan.
        assert_ne!(red_at(&bake_orientation(probe(), 5)), red_at(&bake_orientation(probe(), 6)));
        assert_ne!(red_at(&bake_orientation(probe(), 7)), red_at(&bake_orientation(probe(), 8)));
    }

    #[test]
    fn out_of_range_orientation_is_treated_as_upright() {
        assert_eq!(red_at(&bake_orientation(probe(), 0)), (0, 0));
        assert_eq!(red_at(&bake_orientation(probe(), 99)), (0, 0));
    }

    #[test]
    fn missing_exif_reports_upright() {
        assert_eq!(read_orientation(&[0xFF, 0xD8, 0xFF, 0xDB, 0, 0]), 1);
        assert_eq!(read_orientation(b"not an image"), 1);
        assert!(!needs_bake(1));
        for o in 2..=8 {
            assert!(needs_bake(o));
        }
    }

    #[test]
    fn derive_bakes_orientation_and_never_upscales() {
        let mut small = RgbImage::from_pixel(100, 60, Rgb([200, 200, 200]));
        small.put_pixel(0, 0, Rgb([255, 0, 0]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(small)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();

        let d = derive(&bytes, ImageFormat::Png).unwrap();
        assert_eq!((d.width, d.height), (100, 60), "must not upscale a small scan");
        assert_eq!(d.orientation, 1);
        assert!(!d.print_jpeg.is_empty());
        assert!(!d.thumb_jpeg.is_empty());
        assert!(d.print_jpeg.starts_with(&[0xFF, 0xD8, 0xFF]), "derivative must be JPEG");
    }
}
