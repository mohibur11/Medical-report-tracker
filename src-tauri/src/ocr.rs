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

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OcrWord {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

// Deserialize as well as Serialize: the stored ocr_json is read back as a cache,
// so a page is never recognised twice.
#[derive(Debug, Serialize, Deserialize, Default)]
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

/// The handle onto the Kotlin side, set once when the app starts.
///
/// A global because recognition is reached from deep inside the ingest path,
/// which has no reason to carry an app handle around for the one platform that
/// needs it.
#[cfg(target_os = "android")]
static ANDROID_OCR: std::sync::OnceLock<tauri::plugin::PluginHandle<tauri::Wry>> =
    std::sync::OnceLock::new();

#[cfg(target_os = "android")]
pub fn set_android_handle(handle: tauri::plugin::PluginHandle<tauri::Wry>) {
    let _ = ANDROID_OCR.set(handle);
}

#[cfg(target_os = "android")]
pub fn recognize(path: &std::path::Path) -> Result<OcrPage, String> {
    #[derive(serde::Serialize)]
    struct Args {
        path: String,
    }

    let handle = ANDROID_OCR
        .get()
        .ok_or("the text recogniser was not available when the app started")?;

    handle
        .run_mobile_plugin::<OcrPage>(
            "recognize",
            Args {
                path: path.display().to_string(),
            },
        )
        .map_err(|e| format!("cannot read this page: {e}"))
}

/// Open a socket on loopback for Google's answer, and report the port.
///
/// The listener lives on the Kotlin side. The same thing written in Rust bound
/// its port and then never accepted a connection, and the sign-in hung with it —
/// four times, through three different explanations. The platform schedules its
/// own threads reliably, so this half was moved to where that is guaranteed.
#[cfg(target_os = "android")]
pub fn start_loopback() -> Result<u16, String> {
    #[derive(serde::Deserialize)]
    struct Port {
        port: u16,
    }

    let handle = ANDROID_OCR
        .get()
        .ok_or("the sign-in was not available when the app started")?;

    handle
        .run_mobile_plugin::<Port>("startLoopback", ())
        .map(|p| p.port)
        .map_err(|e| format!("cannot listen for Google's reply: {e}"))
}

/// Block until the browser comes back, and return the query string it carried.
///
/// Blocks for as long as somebody takes to choose an account and press Allow, so
/// the caller must already be off the UI thread.
#[cfg(target_os = "android")]
pub fn await_redirect() -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Reply {
        query: String,
    }

    let handle = ANDROID_OCR
        .get()
        .ok_or("the sign-in was not available when the app started")?;

    handle
        .run_mobile_plugin::<Reply>("awaitRedirect", ())
        .map(|r| r.query)
        .map_err(|e| format!("{e}"))
}

/// Open a URL in a tab inside this app.
///
/// Used for the Google sign-in, which must not send the app to the background:
/// Android freezes cached processes, and the listener waiting for the redirect
/// stops accepting the moment that happens.
#[cfg(target_os = "android")]
pub fn open_in_app(url: &str) -> Result<(), String> {
    #[derive(serde::Serialize)]
    struct Args<'a> {
        url: &'a str,
    }

    let handle = ANDROID_OCR
        .get()
        .ok_or("the browser was not available when the app started")?;

    handle
        .run_mobile_plugin::<serde_json::Value>("openAuth", Args { url })
        .map(|_| ())
        .map_err(|e| format!("cannot open the sign-in page: {e}"))
}

/// Ask the phone for files, copying them into the shared inbox.
///
/// Returns how many were taken; `ingest::take_shared` is what turns them into
/// staged rows, so a picked file and a shared one follow exactly the same path.
#[cfg(target_os = "android")]
pub fn pick_files() -> Result<u32, String> {
    #[derive(serde::Deserialize)]
    struct Picked {
        taken: u32,
    }

    let handle = ANDROID_OCR
        .get()
        .ok_or("the file picker was not available when the app started")?;

    handle
        .run_mobile_plugin::<Picked>("pick", ())
        .map(|p| p.taken)
        .map_err(|e| format!("cannot open the file picker: {e}"))
}

#[cfg(target_os = "android")]
pub fn available() -> bool {
    ANDROID_OCR.get().is_some()
}

#[cfg(not(any(windows, target_os = "android")))]
pub fn recognize(_path: &std::path::Path) -> Result<OcrPage, String> {
    Err("OCR is only available on Windows in this build.".into())
}

/// Is a recognizer installed for the current user's languages?
#[cfg(windows)]
pub fn available() -> bool {
    windows::Media::Ocr::OcrEngine::TryCreateFromUserProfileLanguages().is_ok()
}

#[cfg(not(any(windows, target_os = "android")))]
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
/// Everything recognition needs, so the caller can drop the database lock before
/// spending a third of a second per page holding it.
pub struct StagedTarget {
    /// Already recognised once. Returned as-is rather than read again.
    pub cached: Option<Vec<OcrPage>>,
    pub path: std::path::PathBuf,
    pub kind: String,
    /// Clockwise degrees the staged image has been turned to read upright.
    /// None means nobody has decided yet — the detector runs on the first read
    /// and only then. See `upright`.
    pub text_rotation: Option<u16>,
}

/// What a read produced, and what it did to the file on the way.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recognized {
    pub pages: Vec<OcrPage>,
    /// Clockwise degrees the staged image now stands turned by, so the row can
    /// say so and fetch its thumbnail again. Always 0 for a PDF.
    pub text_rotation: u16,
}

pub fn staged_target(conn: &rusqlite::Connection, ingest_id: &str) -> Result<StagedTarget, String> {
    use rusqlite::params;

    let (staged, kind, json, text_rotation): (Option<String>, String, Option<String>, Option<u16>) =
        conn.query_row(
            "SELECT staged_path, file_kind, ocr_json, text_rotation FROM ingest_items WHERE id = ?1",
            params![ingest_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|e| format!("no such staged item: {e}"))?;

    let path = staged.ok_or("that file is no longer staged")?;

    // A staged file never changes, so its text never does either. Without this a
    // backlog import re-reads every page each time the queue is reopened.
    let cached = json
        .as_deref()
        .and_then(|j| serde_json::from_str::<Vec<OcrPage>>(j).ok())
        .filter(|pages| !pages.is_empty());

    Ok(StagedTarget {
        cached,
        path: std::path::PathBuf::from(path),
        kind,
        text_rotation,
    })
}

pub fn recognize_file(path: &std::path::Path, kind: &str) -> Result<Vec<OcrPage>, String> {
    // Android has no pdfcpu to extract page images with, but it does have
    // PdfRenderer, so the Kotlin side reads the whole document itself.
    #[cfg(target_os = "android")]
    if kind == "pdf" {
        #[derive(serde::Serialize)]
        struct Args {
            path: String,
        }
        #[derive(serde::Deserialize)]
        struct Pages {
            pages: Vec<OcrPage>,
        }

        let handle = ANDROID_OCR
            .get()
            .ok_or("the text recogniser was not available when the app started")?;

        return handle
            .run_mobile_plugin::<Pages>(
                "recognizePdf",
                Args {
                    path: path.display().to_string(),
                },
            )
            .map(|p| p.pages)
            .map_err(|e| format!("cannot read this PDF: {e}"));
    }

    if kind == "pdf" {
        let work = path.with_extension("pages");
        let result = recognize_pdf(path, &work);
        // The extracted page images are scratch; the text is what is kept.
        let _ = std::fs::remove_dir_all(&work);
        result
    } else {
        Ok(vec![recognize(path)?])
    }
}

/// Read a staged file, turning an image upright first if that has never been
/// decided for it. The caller holds no database lock across this.
pub fn recognize_target(target: &StagedTarget) -> Result<Recognized, String> {
    if target.kind != "pdf" && target.text_rotation.is_none() {
        let up = crate::upright::recognize_upright(&target.path)?;
        return Ok(Recognized {
            pages: vec![up.page],
            text_rotation: up.rotation,
        });
    }
    Ok(Recognized {
        pages: recognize_file(&target.path, &target.kind)?,
        text_rotation: target.text_rotation.unwrap_or(0),
    })
}

pub fn store_ocr(
    conn: &rusqlite::Connection,
    ingest_id: &str,
    read: &Recognized,
) -> Result<(), String> {
    use rusqlite::params;

    // One field for ranking and search, one for geometry, both keyed by page.
    let combined = read
        .pages
        .iter()
        .map(|p| p.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    // The turn is recorded with the text it was decided by. A PDF's is 0, which
    // marks it decided too — nothing turns a PDF, so nothing should ask again.
    conn.execute(
        "UPDATE ingest_items
         SET ocr_text = ?2, ocr_json = ?3, ocr_at = datetime('now'), text_rotation = ?4,
             updated_at = datetime('now')
         WHERE id = ?1",
        params![
            ingest_id,
            combined,
            serde_json::to_string(&read.pages).unwrap_or_default(),
            read.text_rotation,
        ],
    )
    .map_err(|e| format!("cannot store recognised text: {e}"))?;

    Ok(())
}

/// The three steps composed. The command splits them so it can drop the database
/// lock while recognising; the tests want the whole path in one call.
#[cfg(test)]
pub fn recognize_staged(
    conn: &rusqlite::Connection,
    ingest_id: &str,
) -> Result<Vec<OcrPage>, String> {
    let target = staged_target(conn, ingest_id)?;
    if let Some(pages) = target.cached {
        return Ok(pages);
    }
    let read = recognize_target(&target)?;
    store_ocr(conn, ingest_id, &read)?;
    Ok(read.pages)
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

    /// Set up one staged row pointing at `path`, and hand back its connection.
    fn staged_row(name: &str, path: &std::path::Path, kind: &str) -> (rusqlite::Connection, String) {
        let dir = std::env::temp_dir().join(format!("mrt-ocrdb-{name}-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = crate::db::open(&dir.join("app.db")).unwrap();
        let user = crate::db::ensure_user(&conn).unwrap();
        let id = ulid::Ulid::new().to_string();
        conn.execute(
            "INSERT INTO ingest_items
               (id, owner_user_id, batch_id, src_path, staged_path, status, file_kind,
                created_at, updated_at)
             VALUES (?1, ?2, 'batch', ?3, ?3, 'pending', ?4, datetime('now'), datetime('now'))",
            rusqlite::params![id, user, path.to_string_lossy(), kind],
        )
        .unwrap();
        (conn, id)
    }

    #[test]
    fn a_page_is_never_recognised_twice() {
        let dir = std::env::temp_dir().join(format!("mrt-ocrcache-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("page.jpg");
        text_image(&img);

        let (conn, id) = staged_row("cache", &img, "jpeg");
        let first = recognize_staged(&conn, &id).expect("first read");
        assert!(!first.is_empty(), "the page should produce at least one entry");

        // Deleting the file proves the second call never touched it. Recognition
        // costs about a third of a second a page; a backlog import reopening the
        // queue must not pay that again.
        std::fs::remove_file(&img).unwrap();
        let second = recognize_staged(&conn, &id).expect("second read comes from the cache");
        assert_eq!(
            second.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(),
            first.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(),
            "the cached text must match what was recognised",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_cache_is_not_treated_as_a_result() {
        let dir = std::env::temp_dir().join(format!("mrt-ocrempty-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("page.jpg");
        text_image(&img);

        let (conn, id) = staged_row("empty", &img, "jpeg");
        // A page that read as nothing must be retried, not remembered as done.
        conn.execute(
            "UPDATE ingest_items SET ocr_json = '[]' WHERE id = ?1",
            rusqlite::params![id],
        )
        .unwrap();

        let target = staged_target(&conn, &id).unwrap();
        assert!(target.cached.is_none(), "an empty result is not a cache hit");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_staged_file_that_vanished_before_any_read_says_so() {
        let dir = std::env::temp_dir().join(format!("mrt-ocrgone-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("gone.jpg");

        let (conn, id) = staged_row("gone", &missing, "jpeg");
        let err = recognize_staged(&conn, &id).expect_err("nothing to read");
        assert!(!err.is_empty(), "the failure must be explained, not silent");

        let _ = std::fs::remove_dir_all(&dir);
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
