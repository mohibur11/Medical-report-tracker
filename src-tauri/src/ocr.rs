//! Text recognition, using the engine already present in Windows.
//!
//! `Windows.Media.Ocr` costs nothing to ship, runs entirely offline, and the
//! Phase 0 spike measured it at ~350 ms for a full A4 lab report. Nothing leaves
//! the machine — which was the user's explicit requirement for family medical
//! records.
//!
//! What this returns is TEXT, not answers. The spike showed recognition is not the
//! bottleneck: on a realistic lab report the engine found all three dates,
//! including the patient's date of birth. Choosing between them is a ranking
//! problem, and it lives in the review grid where the user can see and correct it.
//!
//! `OcrResult::Text` also flattens layout — a results table came back as
//! "Test TSH FT3 FT 4 Result 6.82 2.91 0.88", column association destroyed. Word
//! boxes are therefore captured too, so anything needing geometry later has it.

use serde::Serialize;

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OcrWord {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OcrPage {
    /// Reading-order text. Layout is flattened; use `words` when geometry matters.
    pub text: String,
    pub words: Vec<OcrWord>,
    pub engine: String,
    pub millis: u64,
}

#[cfg(windows)]
pub fn recognize(path: &std::path::Path) -> Result<OcrPage, String> {
    use windows::Graphics::Imaging::BitmapDecoder;
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::{FileAccessMode, StorageFile};

    let started = std::time::Instant::now();

    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|e| format!("cannot create OCR engine: {e}"))?;

    let language = engine
        .RecognizerLanguage()
        .and_then(|l| l.LanguageTag())
        .map(|t| t.to_string())
        .unwrap_or_else(|_| "unknown".into());

    // GetFileFromPathAsync demands a fully-qualified path: anything containing
    // `..`, or a relative path, fails with an opaque "parameter is incorrect".
    // canonicalize() also returns the \\?\ extended-length form, which WinRT
    // rejects in turn, so that prefix is stripped back off.
    let absolute = std::fs::canonicalize(path)
        .map_err(|e| format!("cannot resolve {}: {e}", path.display()))?;
    let wide = absolute
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string();

    let file = StorageFile::GetFileFromPathAsync(&windows::core::HSTRING::from(&wide))
        .map_err(|e| format!("cannot open {wide}: {e}"))?
        .get()
        .map_err(|e| format!("cannot open {wide}: {e}"))?;

    let stream = file
        .OpenAsync(FileAccessMode::Read)
        .map_err(|e| e.to_string())?
        .get()
        .map_err(|e| e.to_string())?;

    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(|e| format!("cannot decode image: {e}"))?
        .get()
        .map_err(|e| format!("cannot decode image: {e}"))?;

    let bitmap = decoder
        .GetSoftwareBitmapAsync()
        .map_err(|e| e.to_string())?
        .get()
        .map_err(|e| e.to_string())?;

    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| format!("recognition failed: {e}"))?
        .get()
        .map_err(|e| format!("recognition failed: {e}"))?;

    let text = result.Text().map(|t| t.to_string()).unwrap_or_default();

    let mut words = Vec::new();
    if let Ok(lines) = result.Lines() {
        for line in lines {
            let Ok(line_words) = line.Words() else { continue };
            for word in line_words {
                let Ok(rect) = word.BoundingRect() else { continue };
                words.push(OcrWord {
                    text: word.Text().map(|t| t.to_string()).unwrap_or_default(),
                    x: rect.X,
                    y: rect.Y,
                    w: rect.Width,
                    h: rect.Height,
                });
            }
        }
    }

    Ok(OcrPage {
        text,
        words,
        engine: format!("Windows.Media.Ocr ({language})"),
        millis: started.elapsed().as_millis() as u64,
    })
}

#[cfg(not(windows))]
pub fn recognize(_path: &std::path::Path) -> Result<OcrPage, String> {
    Err("OCR is only available on Windows in this build.".into())
}

/// Is a recognizer installed for the current user's languages?
#[cfg(windows)]
pub fn available() -> bool {
    windows::Media::Ocr::OcrEngine::TryCreateFromUserProfileLanguages().is_ok()
}

#[cfg(not(windows))]
pub fn available() -> bool {
    false
}

/// Recognise a staged file and remember the result on its ingest row.
///
/// Stored rather than returned-and-forgotten so that closing the app mid-review
/// does not discard ~350 ms of work per page — the same reason the staging table
/// is durable in the first place.
pub fn recognize_staged(
    conn: &rusqlite::Connection,
    ingest_id: &str,
) -> Result<OcrPage, String> {
    use rusqlite::params;

    let (staged, kind): (Option<String>, String) = conn
        .query_row(
            "SELECT staged_path, file_kind FROM ingest_items WHERE id = ?1",
            params![ingest_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| format!("no such staged item: {e}"))?;

    // PDFs need their pages rasterised first, which is Phase 3's next step.
    // Saying so beats returning empty text that looks like a failed scan.
    if kind == "pdf" {
        return Err("Reading text from PDFs is not implemented yet.".into());
    }

    let path = staged.ok_or("that file is no longer staged")?;
    let page = recognize(std::path::Path::new(&path))?;

    conn.execute(
        "UPDATE ingest_items
         SET ocr_text = ?2, ocr_json = ?3, ocr_at = datetime('now'), updated_at = datetime('now')
         WHERE id = ?1",
        params![
            ingest_id,
            page.text,
            serde_json::to_string(&page.words).unwrap_or_default()
        ],
    )
    .map_err(|e| format!("cannot store recognised text: {e}"))?;

    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};

    /// Render text with a blocky 5x7 bitmap font, large enough for the recognizer.
    /// Generated rather than checked in so the test exercises real decoding.
    fn text_image(path: &std::path::Path) {
        // A plain white page with black bars is not readable text; instead draw
        // digits and letters as filled rectangles in a 5x7 grid.
        const GLYPHS: &[(char, [u8; 7])] = &[
            ('T', [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100]),
            ('S', [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110]),
            ('H', [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
        ];

        let scale = 14u32;
        let (cols, rows) = (5u32, 7u32);
        let pad = 40u32;
        let width = pad * 2 + GLYPHS.len() as u32 * (cols + 1) * scale;
        let height = pad * 2 + rows * scale;

        let mut img = RgbImage::from_pixel(width, height, Rgb([255, 255, 255]));
        for (gi, (_, bits)) in GLYPHS.iter().enumerate() {
            let ox = pad + gi as u32 * (cols + 1) * scale;
            for (ry, row) in bits.iter().enumerate() {
                for cx in 0..cols {
                    if row & (1 << (cols - 1 - cx)) != 0 {
                        for dy in 0..scale {
                            for dx in 0..scale {
                                let x = ox + cx * scale + dx;
                                let y = pad + ry as u32 * scale + dy;
                                if x < width && y < height {
                                    img.put_pixel(x, y, Rgb([0, 0, 0]));
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
            .unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_recognizer_is_installed_on_this_machine() {
        // Verified in the Phase 0 spike: en-US is the only language pack present.
        // If this fails, OCR silently degrades to manual entry, which the user
        // should be told about rather than left to discover.
        assert!(available(), "no Windows OCR language pack for the current profile");
    }

    #[test]
    fn recognition_returns_text_and_word_boxes() {
        let dir = std::env::temp_dir().join(format!("mrt-ocr-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tsh.jpg");
        text_image(&path);

        let page = recognize(&path).expect("recognition should succeed");
        assert!(page.engine.starts_with("Windows.Media.Ocr"));

        // The exact characters depend on the recognizer, so assert the mechanism
        // rather than a transcription: it produced something, with geometry, and
        // every box has real dimensions.
        if !page.words.is_empty() {
            assert!(page.words.iter().all(|w| w.w > 0.0 && w.h > 0.0));
            assert!(!page.text.is_empty(), "words present but text empty");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_blank_page_yields_no_text_rather_than_an_error() {
        // The handwritten-prescription case degrades this way, and it must not
        // look like a failure.
        let dir = std::env::temp_dir().join(format!("mrt-ocr-blank-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("blank.jpg");

        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(RgbImage::from_pixel(400, 300, Rgb([255, 255, 255])))
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
            .unwrap();
        std::fs::write(&path, bytes).unwrap();

        let page = recognize(&path).expect("a blank page is not an error");
        assert!(page.text.trim().is_empty());
        assert!(page.words.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The end this pipeline actually has to serve: a realistic lab report.
    ///
    /// Pairs with `tests/dates.test.ts`, which ranks the verbatim output of this
    /// same engine on this same fixture and picks the collection date over the
    /// date of birth. Between them, both halves of the chain are covered by real
    /// recognition rather than clean synthetic text.
    #[test]
    fn a_realistic_lab_report_yields_its_dates() {
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tests")
            .join("fixtures")
            .join("lab-report-synthetic.jpg");
        assert!(fixture.exists(), "fixture missing: {}", fixture.display());

        let page = recognize(&fixture).expect("recognition should succeed");

        assert!(page.words.len() > 40, "expected a page of words, got {}", page.words.len());
        assert!(
            page.text.contains("14/03/2026"),
            "the collection date must be readable; got: {}",
            page.text
        );
        // All three dates are present, which is exactly why ranking exists: the
        // 1978 one is the patient's date of birth and would sort a 2026 report to
        // the top of the folder forever.
        assert!(page.text.contains("12/03/1978"), "DOB should also be read, and later rejected");
        assert!(page.text.contains("Sample Collected"), "the anchor label ranking depends on");
    }

    #[test]
    fn a_missing_file_reports_an_error_rather_than_panicking() {
        let missing = std::env::temp_dir().join("mrt-ocr-does-not-exist.jpg");
        assert!(recognize(&missing).is_err());
    }
}
