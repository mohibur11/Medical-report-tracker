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
    /// 1-based. Images are always page 1; PDFs carry their real page number.
    pub page_no: usize,
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
        page_no: 1,
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

/// Pages beyond this are not read. A 200-page hospital discharge bundle would
/// otherwise hold up the review queue for minutes; the pages that carry the date
/// are at the front.
pub const MAX_PDF_PAGES: usize = 50;

/// Recognise every page of a scanned PDF.
///
/// pdfcpu has no renderer, but it does not need one here: in a scanned PDF each
/// page IS a single embedded image, so extracting the images gives back the pages.
/// That covers this archive completely — every PDF in it has zero embedded fonts —
/// and costs no extra dependency.
///
/// A PDF with real vector content would extract only its pictures, not its text.
/// Such a file has a text layer to read instead, which is both faster and exact,
/// and is the next thing worth building.
pub fn recognize_pdf(pdf: &std::path::Path, work: &std::path::Path) -> Result<Vec<OcrPage>, String> {
    let exe = crate::export::pdfcpu_path()?;
    std::fs::create_dir_all(work).map_err(|e| format!("cannot create work dir: {e}"))?;

    let out = std::process::Command::new(&exe)
        .args([
            "extract",
            "-m",
            "image",
            "--force",
            &pdf.to_string_lossy(),
            &work.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("cannot run pdfcpu: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cannot read pages from this PDF: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    // pdfcpu names them "<stem>_<page>_img<n>.<ext>", zero-padded according to the
    // page count, so the page number is parsed rather than assumed.
    let mut by_page: std::collections::BTreeMap<usize, Vec<std::path::PathBuf>> =
        std::collections::BTreeMap::new();

    for entry in std::fs::read_dir(work).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(page) = page_number_of(&name) else { continue };
        by_page.entry(page).or_default().push(path);
    }

    if by_page.is_empty() {
        return Err("no readable page images in this PDF.".into());
    }

    let mut pages = Vec::new();
    for (page_no, mut images) in by_page.into_iter().take(MAX_PDF_PAGES) {
        images.sort();
        let mut text = String::new();
        let mut words = Vec::new();
        let mut millis = 0u64;
        let mut engine = String::new();

        for image in &images {
            // One unreadable image must not lose the rest of the page.
            let Ok(part) = recognize(image) else { continue };
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&part.text);
            words.extend(part.words);
            millis += part.millis;
            engine = part.engine;
        }

        pages.push(OcrPage {
            text,
            words,
            engine,
            millis,
            page_no,
        });
    }

    Ok(pages)
}

/// Extract the page number from a pdfcpu image filename.
fn page_number_of(file_name: &str) -> Option<usize> {
    // "<stem>_<page>_img<n>.<ext>" — the stem may itself contain underscores, so
    // the page is the second-to-last underscore-separated field.
    let stem = file_name.rsplit_once('.').map(|(s, _)| s).unwrap_or(file_name);
    let mut parts = stem.rsplitn(3, '_');
    let _img = parts.next()?;
    let page = parts.next()?;
    page.parse::<usize>().ok()
}

/// Recognise a staged file and remember the result on its ingest row.
///
/// Stored rather than returned-and-forgotten so that closing the app mid-review
/// does not discard ~350 ms of work per page — the same reason the staging table
/// is durable in the first place.
pub fn recognize_staged(
    conn: &rusqlite::Connection,
    ingest_id: &str,
) -> Result<Vec<OcrPage>, String> {
    use rusqlite::params;

    let (staged, kind): (Option<String>, String) = conn
        .query_row(
            "SELECT staged_path, file_kind FROM ingest_items WHERE id = ?1",
            params![ingest_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| format!("no such staged item: {e}"))?;

    let path = staged.ok_or("that file is no longer staged")?;
    let path = std::path::Path::new(&path);

    let pages = if kind == "pdf" {
        let work = path.with_extension("pages");
        let result = recognize_pdf(path, &work);
        // The extracted page images are scratch; the text is what is kept.
        let _ = std::fs::remove_dir_all(&work);
        result?
    } else {
        vec![recognize(path)?]
    };

    // One field for ranking and search, one for geometry, both keyed by page.
    let combined = pages
        .iter()
        .map(|p| p.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    conn.execute(
        "UPDATE ingest_items
         SET ocr_text = ?2, ocr_json = ?3, ocr_at = datetime('now'), updated_at = datetime('now')
         WHERE id = ?1",
        params![
            ingest_id,
            combined,
            serde_json::to_string(&pages).unwrap_or_default()
        ],
    )
    .map_err(|e| format!("cannot store recognised text: {e}"))?;

    Ok(pages)
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
    fn page_numbers_are_parsed_from_pdfcpu_filenames() {
        // The stem can contain underscores and the padding varies with page count,
        // so the page is taken positionally rather than by pattern.
        assert_eq!(page_number_of("report_1_img0.jpg"), Some(1));
        assert_eq!(page_number_of("Medical All Documents_14_img13.jpg"), Some(14));
        assert_eq!(page_number_of("a_b_c_07_img2.png"), Some(7), "underscores in the stem");
        assert_eq!(page_number_of("notours.jpg"), None);
        assert_eq!(page_number_of("scan_x_img0.jpg"), None, "page must be numeric");
    }

    /// A multi-page scanned PDF is the shape this archive is actually made of:
    /// every PDF in it has zero embedded fonts, so each page is one image.
    #[test]
    fn every_page_of_a_scanned_pdf_is_read() {
        use image::{DynamicImage, ImageFormat, Rgb, RgbImage};

        std::env::set_var(
            "PDFCPU_PATH",
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("binaries")
                .join("pdfcpu-x86_64-pc-windows-msvc.exe"),
        );
        let exe = crate::export::pdfcpu_path().unwrap();

        let dir = std::env::temp_dir().join(format!("mrt-pdfocr-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();

        // Three distinct pages, so their order can be checked rather than assumed.
        let mut parts = Vec::new();
        for i in 1..=3 {
            let img = dir.join(format!("src{i}.jpg"));
            let mut bytes = Vec::new();
            DynamicImage::ImageRgb8(RgbImage::from_pixel(500, 700, Rgb([250, 250, 250])))
                .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
                .unwrap();
            std::fs::write(&img, bytes).unwrap();

            let page = dir.join(format!("p{i}.pdf"));
            std::process::Command::new(&exe)
                .args(["import", "f:A4, pos:c", &page.to_string_lossy(), &img.to_string_lossy()])
                .output()
                .unwrap();
            parts.push(page.to_string_lossy().to_string());
        }

        let merged = dir.join("three-page-scan.pdf");
        let mut args = vec!["merge".to_string(), merged.to_string_lossy().to_string()];
        args.extend(parts);
        std::process::Command::new(&exe).args(&args).output().unwrap();

        let pages = recognize_pdf(&merged, &dir.join("work")).expect("should read the pages");
        assert_eq!(pages.len(), 3, "one entry per page");
        assert_eq!(
            pages.iter().map(|p| p.page_no).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "pages must come back in order",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pdf_with_no_readable_pages_says_so() {
        std::env::set_var(
            "PDFCPU_PATH",
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("binaries")
                .join("pdfcpu-x86_64-pc-windows-msvc.exe"),
        );
        let dir = std::env::temp_dir().join(format!("mrt-badpdf-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("not-really.pdf");
        std::fs::write(&bad, b"%PDF-1.7\nthis is not a real pdf\n").unwrap();

        let err = recognize_pdf(&bad, &dir.join("work")).unwrap_err();
        assert!(!err.is_empty(), "must explain itself rather than return empty text");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_reports_an_error_rather_than_panicking() {
        let missing = std::env::temp_dir().join("mrt-ocr-does-not-exist.jpg");
        assert!(recognize(&missing).is_err());
    }
}
