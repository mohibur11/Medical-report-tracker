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

/// The staged print derivative's encoding, for anything that rewrites one.
pub fn encode_print(img: &DynamicImage) -> Result<Vec<u8>, String> {
    encode_jpeg(img, PRINT_QUALITY)
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

/// Turn an image by a multiple of a quarter turn, clockwise. Anything else is a
/// caller bug and leaves the image alone rather than guessing.
pub fn rotate_cw(img: &DynamicImage, degrees: u16) -> DynamicImage {
    match degrees % 360 {
        90 => img.rotate90(),
        180 => img.rotate180(),
        270 => img.rotate270(),
        _ => img.clone(),
    }
}

/// The print derivative and its thumbnail, from an image that is already baked
/// and already print-sized — a staged file being turned after the fact.
///
/// The print copy is re-encoded, so it goes through JPEG twice. At q78 on a
/// photographed document the second generation is not visible, and the
/// alternative — keeping the original around to turn from — would double what
/// staging holds for every image on the off chance one needs turning.
pub struct Turned {
    pub print_jpeg: Vec<u8>,
    pub thumb_jpeg: Vec<u8>,
}

pub fn turn_staged(bytes: &[u8], degrees: u16) -> Result<Turned, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("decode failed: {e}"))?;
    let img = rotate_cw(&img, degrees);
    let thumb = downscale(&img, THUMB_LONG_EDGE, image::imageops::FilterType::Triangle);
    Ok(Turned {
        print_jpeg: encode_jpeg(&img, PRINT_QUALITY)?,
        thumb_jpeg: encode_jpeg(&thumb, THUMB_QUALITY)?,
    })
}

/// Decode, bake, and produce both derivatives from raw file bytes.
pub fn derive(bytes: &[u8], format: ImageFormat) -> Result<Derivatives, String> {
    let orientation = read_orientation(bytes);

    let img = image::load_from_memory_with_format(bytes, format)
        .map_err(|e| format!("decode failed: {e}"))?;

    let img = bake_orientation(img, orientation);

    // Lanczos is worth its cost on a desktop, where this runs once per file on a
    // machine with power to spare. On a phone it is the single slowest step of an
    // import, on a battery, while somebody watches a spinner — and at these
    // reductions the visible difference on a photographed document is nil.
    #[cfg(target_os = "android")]
    let print_filter = image::imageops::FilterType::Triangle;
    #[cfg(not(target_os = "android"))]
    let print_filter = image::imageops::FilterType::Lanczos3;

    let print = downscale(&img, PRINT_LONG_EDGE, print_filter);
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
    fn quarter_turns_are_clockwise_and_anything_else_is_ignored() {
        // Source is 4x2 with red at (0,0). A clockwise quarter turn puts the
        // top-left corner at the top-right.
        assert_eq!(red_at(&rotate_cw(&probe(), 90)), (1, 0));
        assert_eq!(red_at(&rotate_cw(&probe(), 180)), (3, 1));
        assert_eq!(red_at(&rotate_cw(&probe(), 270)), (0, 3));
        assert_eq!(red_at(&rotate_cw(&probe(), 0)), (0, 0));
        assert_eq!(red_at(&rotate_cw(&probe(), 45)), (0, 0), "not a quarter turn: untouched");
        assert_eq!(rotate_cw(&probe(), 450).width(), 2, "wraps past a full turn");
    }

    #[test]
    fn turning_a_staged_file_swaps_its_dimensions_and_redraws_the_thumbnail() {
        let mut page = RgbImage::from_pixel(400, 200, Rgb([255, 255, 255]));
        page.put_pixel(0, 0, Rgb([255, 0, 0]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(page)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();

        let t = turn_staged(&bytes, 90).unwrap();
        assert!(t.print_jpeg.starts_with(&[0xFF, 0xD8, 0xFF]));
        let print = image::load_from_memory(&t.print_jpeg).unwrap();
        assert_eq!((print.width(), print.height()), (200, 400));
        let thumb = image::load_from_memory(&t.thumb_jpeg).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (160, 320), "thumbnail follows the turn");
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
