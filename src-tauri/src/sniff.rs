//! File type detection from magic bytes.
//!
//! Extensions are never trusted. "Scan to PDF" phone apps routinely emit `.pdf`
//! files whose bytes are a bare JPEG, WhatsApp strips extensions, and a JPEG that
//! reaches the PDF merge path corrupts the export. Everything downstream — the
//! vault extension, whether OCR runs, whether orientation must be baked — keys off
//! what is actually in the file.
//!
//! Mirrors `src/lib/ingest/sniff.ts`; the two are tested against the same cases.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Jpeg,
    Png,
    Pdf,
    Heic,
    Tiff,
    Webp,
    Unknown,
}

impl FileKind {
    /// Read back what `sniff` decided, as stored in `ingest_items.file_kind`.
    pub fn parse(s: &str) -> Self {
        match s {
            "jpeg" => FileKind::Jpeg,
            "png" => FileKind::Png,
            "pdf" => FileKind::Pdf,
            "heic" => FileKind::Heic,
            "tiff" => FileKind::Tiff,
            "webp" => FileKind::Webp,
            _ => FileKind::Unknown,
        }
    }

    /// Vault extension. Deliberately does not round-trip the source extension —
    /// a JPEG named `.pdf` becomes `.jpg`.
    pub fn extension(self) -> &'static str {
        match self {
            FileKind::Jpeg => "jpg",
            FileKind::Png => "png",
            FileKind::Pdf => "pdf",
            FileKind::Heic => "heic",
            FileKind::Tiff => "tif",
            FileKind::Webp => "webp",
            FileKind::Unknown => "bin",
        }
    }

    pub fn is_raster(self) -> bool {
        matches!(
            self,
            FileKind::Jpeg | FileKind::Png | FileKind::Tiff | FileKind::Webp
        )
    }

    /// Nothing in this stack decodes HEIC, and sharp-style prebuilts never will
    /// due to Nokia's HEIF licensing. It must be recognised precisely so it can be
    /// reported clearly rather than failing as a corrupt JPEG.
    pub fn is_supported(self) -> bool {
        self.is_raster() || self == FileKind::Pdf
    }
}

pub fn sniff(buf: &[u8]) -> FileKind {
    if buf.len() < 12 {
        return FileKind::Unknown;
    }

    if buf.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return FileKind::Jpeg;
    }
    if buf.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return FileKind::Png;
    }
    if buf.starts_with(b"%PDF") {
        return FileKind::Pdf;
    }
    if buf.starts_with(&[0x49, 0x49, 0x2A, 0x00]) || buf.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]) {
        return FileKind::Tiff;
    }
    if buf.starts_with(b"RIFF") && buf[8..12] == *b"WEBP" {
        return FileKind::Webp;
    }
    // ISO-BMFF: [4-byte size]"ftyp"[brand]
    if buf[4..8] == *b"ftyp" {
        let brand = &buf[8..12];
        const HEIF: [&[u8; 4]; 8] = [
            b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1",
        ];
        if HEIF.iter().any(|b| *b == brand) {
            return FileKind::Heic;
        }
    }

    FileKind::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(prefix: &[u8]) -> Vec<u8> {
        let mut v = prefix.to_vec();
        v.resize(32, 0);
        v
    }

    #[test]
    fn detects_each_kind() {
        assert_eq!(sniff(&pad(&[0xFF, 0xD8, 0xFF, 0xE0])), FileKind::Jpeg);
        assert_eq!(sniff(&pad(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])), FileKind::Png);
        assert_eq!(sniff(&pad(b"%PDF-1.7")), FileKind::Pdf);
        assert_eq!(sniff(&pad(&[0x49, 0x49, 0x2A, 0x00])), FileKind::Tiff);
    }

    #[test]
    fn a_jpeg_named_pdf_is_still_a_jpeg() {
        // The exact case that corrupts an export if the extension is trusted.
        assert_eq!(sniff(&pad(&[0xFF, 0xD8, 0xFF, 0xE1])).extension(), "jpg");
    }

    #[test]
    fn webp_needs_both_riff_and_the_form_type() {
        let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
        webp.resize(32, 0);
        assert_eq!(sniff(&webp), FileKind::Webp);

        let mut wav = b"RIFF\0\0\0\0WAVE".to_vec();
        wav.resize(32, 0);
        assert_eq!(sniff(&wav), FileKind::Unknown);
    }

    #[test]
    fn heic_is_recognised_so_it_can_be_reported_not_mangled() {
        let mut heic = vec![0, 0, 0, 0x18];
        heic.extend_from_slice(b"ftypheic");
        heic.resize(32, 0);
        assert_eq!(sniff(&heic), FileKind::Heic);
        assert!(!FileKind::Heic.is_supported());
    }

    #[test]
    fn short_and_empty_input_is_unknown_not_a_panic() {
        assert_eq!(sniff(&[]), FileKind::Unknown);
        assert_eq!(sniff(&[0xFF, 0xD8]), FileKind::Unknown);
    }
}
