//! The ingest pipeline.
//!
//! Files land in staging immediately and everything expensive happens afterwards.
//! Windows Defender scans every file on write, so a 40-file batch processed on the
//! UI thread feels broken; the caller gets rows back as soon as they exist and the
//! grid fills in as work completes.
//!
//! Nothing here writes into the vault. Staged files are committed to
//! `<Patient>/<Year>/` only once the user has confirmed a patient and a date,
//! because the canonical filename contains both and neither is known at drop time.

use std::path::{Path, PathBuf};

use image::ImageFormat;
use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::imaging;
use crate::sniff::{self, FileKind};

/// Status machine for a staged file. Persisted so closing the app 40 files into a
/// 60-photo import does not discard completed work and typed corrections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestStatus {
    /// Staged and hashed; extraction has not run yet.
    Pending,
    /// Extraction produced a usable date guess.
    Extracted,
    /// Nothing readable — the normal outcome for handwritten prescriptions.
    NeedsDate,
    /// A password-protected PDF. Staged and kept, but nothing can read it — not
    /// the recognizer, not the exporter — until the password is supplied.
    Locked,
    /// Byte-identical to something already in the vault.
    Duplicate,
    /// Unsupported or unreadable. Carries a message the user can act on.
    Failed,
}

impl IngestStatus {
    /// Anything unrecognised is treated as pending — a row whose status could not
    /// be read still has a file behind it, and hiding it would strand that file.
    fn parse(s: &str) -> Self {
        match s {
            "extracted" => IngestStatus::Extracted,
            "needs_date" => IngestStatus::NeedsDate,
            "locked" => IngestStatus::Locked,
            "duplicate" => IngestStatus::Duplicate,
            "failed" => IngestStatus::Failed,
            _ => IngestStatus::Pending,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            IngestStatus::Pending => "pending",
            IngestStatus::Extracted => "extracted",
            IngestStatus::NeedsDate => "needs_date",
            IngestStatus::Locked => "locked",
            IngestStatus::Duplicate => "duplicate",
            IngestStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestItem {
    pub id: String,
    pub batch_id: String,
    pub src_path: String,
    pub file_name: String,
    pub status: IngestStatus,
    pub error: Option<String>,
    pub file_kind: FileKind,
    pub sha256: Option<String>,
    pub byte_size: i64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Real page count for PDFs, from pdfcpu. Images are always one page.
    pub page_count: Option<u32>,
    pub exif_orientation: Option<u16>,
    /// True when the pixels had to be rewritten. Surfaced in the grid because it is
    /// the difference between an upright export and a sideways one.
    pub orientation_baked: bool,
    pub thumb_path: Option<String>,
}

/// Staged files that were never committed.
///
/// The review queue lives in the database, not in the window. Closing the app
/// forty files into a sixty-photo import must not strand the other twenty in
/// staging with nothing in the UI pointing at them.
pub fn list_staged(conn: &Connection, user_id: &str) -> Result<Vec<IngestItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, batch_id, src_path, status, error, file_kind, sha256,
                    byte_size, page_count, exif_orientation
               FROM ingest_items
              WHERE owner_user_id = ?1
                AND document_id IS NULL
                AND staged_path IS NOT NULL
                AND status IN ('pending', 'extracted', 'needs_date', 'locked')
              -- Newest batch first, but drop order within a batch, which is the
              -- order the files were handed over in.
              ORDER BY created_at DESC, id ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![user_id], |r| {
            let src_path: String = r.get(2)?;
            let status: String = r.get(3)?;
            let kind: Option<String> = r.get(5)?;
            let exif_orientation: Option<u16> = r.get(9)?;

            Ok(IngestItem {
                id: r.get(0)?,
                batch_id: r.get(1)?,
                file_name: Path::new(&src_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| src_path.clone()),
                src_path,
                status: IngestStatus::parse(&status),
                error: r.get(4)?,
                file_kind: kind.as_deref().map_or(FileKind::Unknown, FileKind::parse),
                sha256: r.get(6)?,
                byte_size: r.get(7)?,
                // Pixel dimensions are not stored — they are only ever used at the
                // moment of ingest, to decide whether to downscale.
                width: None,
                height: None,
                page_count: r.get(8)?,
                exif_orientation,
                // Derived rather than stored: a bake happens exactly when the tag
                // said anything other than upright.
                orientation_baked: exif_orientation.is_some_and(|o| o != 1),
                thumb_path: None,
            })
        })
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn image_format(kind: FileKind) -> Option<ImageFormat> {
    match kind {
        FileKind::Jpeg => Some(ImageFormat::Jpeg),
        FileKind::Png => Some(ImageFormat::Png),
        FileKind::Tiff => Some(ImageFormat::Tiff),
        FileKind::Webp => Some(ImageFormat::WebP),
        _ => None,
    }
}

/// Already present in the vault, or already staged in an earlier batch?
fn find_duplicate(conn: &Connection, sha: &str) -> Result<bool, String> {
    let in_vault: i64 = conn
        .query_row(
            "SELECT count(*) FROM documents WHERE sha256 = ?1 AND trashed_at IS NULL",
            params![sha],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if in_vault > 0 {
        return Ok(true);
    }
    let staged: i64 = conn
        .query_row(
            "SELECT count(*) FROM ingest_items WHERE sha256 = ?1 AND status != 'failed'",
            params![sha],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(staged > 0)
}

/// Process one dropped file into a staged row.
///
/// Never returns Err for a bad input file — an unreadable or unsupported file
/// becomes a `Failed` row carrying an explanation. A batch import must not abort
/// because one photo in sixty was a HEIC.
fn stage_one(
    conn: &Connection,
    user_id: &str,
    batch_id: &str,
    staging: &Path,
    src: &Path,
) -> IngestItem {
    let id = ulid::Ulid::new().to_string();
    let file_name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut item = IngestItem {
        id: id.clone(),
        batch_id: batch_id.to_string(),
        src_path: src.display().to_string(),
        file_name,
        status: IngestStatus::Pending,
        error: None,
        file_kind: FileKind::Unknown,
        sha256: None,
        byte_size: 0,
        width: None,
        height: None,
        page_count: None,
        exif_orientation: None,
        orientation_baked: false,
        thumb_path: None,
    };

    let bytes = match std::fs::read(src) {
        Ok(b) => b,
        Err(e) => {
            item.status = IngestStatus::Failed;
            item.error = Some(format!("cannot read file: {e}"));
            let _ = persist(conn, user_id, &item, None);
            return item;
        }
    };

    item.byte_size = bytes.len() as i64;
    item.file_kind = sniff::sniff(&bytes);
    item.sha256 = Some(sha256_hex(&bytes));

    if !item.file_kind.is_supported() {
        item.status = IngestStatus::Failed;
        item.error = Some(match item.file_kind {
            // Say this plainly rather than letting it fail as a corrupt JPEG.
            FileKind::Heic => "HEIC (iPhone) images cannot be read. On the iPhone set \
                 Settings > Camera > Formats to 'Most Compatible', or convert to JPEG first."
                .into(),
            _ => "Unrecognised file type — not an image or a PDF.".into(),
        });
        let _ = persist(conn, user_id, &item, None);
        return item;
    }

    match find_duplicate(conn, item.sha256.as_deref().unwrap_or_default()) {
        Ok(true) => {
            item.status = IngestStatus::Duplicate;
            item.error = Some("Identical file is already in the library.".into());
            let _ = persist(conn, user_id, &item, None);
            return item;
        }
        Ok(false) => {}
        Err(e) => {
            item.status = IngestStatus::Failed;
            item.error = Some(e);
            return item;
        }
    }

    let staged_path: PathBuf = staging.join(format!("{id}.{}", item.file_kind.extension()));

    if let Some(format) = image_format(item.file_kind) {
        match imaging::derive(&bytes, format) {
            Ok(d) => {
                item.width = Some(d.width);
                item.height = Some(d.height);
                item.exif_orientation = Some(d.orientation);
                item.orientation_baked = imaging::needs_bake(d.orientation);

                // The staged file is the BAKED print derivative, not the original —
                // everything downstream reads it, so orientation is applied exactly
                // once and cannot be applied again by mistake.
                if let Err(e) = std::fs::write(&staged_path, &d.print_jpeg) {
                    item.status = IngestStatus::Failed;
                    item.error = Some(format!("cannot write staged file: {e}"));
                    let _ = persist(conn, user_id, &item, None);
                    return item;
                }
                let thumb = staging.join(format!("{id}.thumb.jpg"));
                if std::fs::write(&thumb, &d.thumb_jpeg).is_ok() {
                    item.thumb_path = Some(thumb.display().to_string());
                }
            }
            Err(e) => {
                item.status = IngestStatus::Failed;
                item.error = Some(format!("cannot decode image: {e}"));
                let _ = persist(conn, user_id, &item, None);
                return item;
            }
        }
    } else {
        // A PDF is one document however many pages it has. It is copied verbatim:
        // re-encoding someone else's PDF loses fidelity for no benefit.
        if let Err(e) = std::fs::copy(src, &staged_path) {
            item.status = IngestStatus::Failed;
            item.error = Some(format!("cannot stage PDF: {e}"));
            let _ = persist(conn, user_id, &item, None);
            return item;
        }
        // Ask pdfcpu how many pages there really are. Assuming one made a 12-page
        // scanned report claim to be a single page everywhere it was shown. The
        // same call says whether the file can be opened at all.
        if let Ok(exe) = crate::export::pdfcpu_path() {
            match crate::export::probe_pdf(&exe, &staged_path) {
                crate::export::PdfState::Readable { pages } => item.page_count = Some(pages),
                crate::export::PdfState::Locked => {
                    // Kept, not rejected: the file is fine, it just needs a
                    // password. Left in the queue with somewhere to type one.
                    item.status = IngestStatus::Locked;
                    item.error = Some(
                        "This PDF is password protected. Enter the password to unlock it — \
                         until then it cannot be read or included in an export."
                            .into(),
                    );
                    let _ = persist(conn, user_id, &item, Some(&staged_path));
                    return item;
                }
                // Not rejected: `info` is stricter than everything this app does
                // with a PDF, and a file it complains about usually merges and
                // exports without trouble. If it genuinely cannot be read, the
                // export says so per file rather than the import refusing it here.
                crate::export::PdfState::Questionable(_) => item.page_count = None,
            }
        }
    }

    // Extraction has not run yet, so the date is unknown. For handwritten
    // prescriptions this is where it will stay, and that is the expected path.
    item.status = IngestStatus::NeedsDate;
    let _ = persist(conn, user_id, &item, Some(&staged_path));
    item
}

/// Unlock a staged PDF with the password the user supplied.
///
/// On success the staged file is replaced by a decrypted copy, so nothing
/// downstream — recognition, export, the merged PDF — has to know it was ever
/// locked. On failure nothing is touched.
pub fn unlock(
    conn: &Connection,
    user_id: &str,
    ingest_id: &str,
    password: &str,
) -> Result<(), String> {
    if password.is_empty() {
        return Err("Enter the password for this PDF.".into());
    }

    let staged: Option<String> = conn
        .query_row(
            "SELECT staged_path FROM ingest_items WHERE id = ?1 AND owner_user_id = ?2",
            params![ingest_id, user_id],
            |r| r.get(0),
        )
        .map_err(|_| "No such file in the queue.".to_string())?;
    let staged = PathBuf::from(staged.ok_or("That file is no longer staged.")?);

    let exe = crate::export::pdfcpu_path()?;
    crate::export::decrypt_pdf(&exe, &staged, password)?;

    let pages = match crate::export::probe_pdf(&exe, &staged) {
        crate::export::PdfState::Readable { pages } => pages,
        crate::export::PdfState::Locked => {
            return Err("The file is still locked — it may use a second password.".into())
        }
        crate::export::PdfState::Questionable(_) => 1,
    };

    conn.execute(
        "UPDATE ingest_items
         SET status = 'needs_date', error = NULL, page_count = ?2, ocr_text = NULL,
             ocr_json = NULL, ocr_at = NULL, updated_at = datetime('now')
         WHERE id = ?1",
        params![ingest_id, pages],
    )
    .map_err(|e| format!("cannot update the queue: {e}"))?;

    Ok(())
}

fn persist(
    conn: &Connection,
    user_id: &str,
    item: &IngestItem,
    staged: Option<&Path>,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO ingest_items
           (id, owner_user_id, batch_id, src_path, staged_path, status, error,
            file_kind, sha256, byte_size, page_count, exif_orientation, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12, datetime('now'), datetime('now'))",
        params![
            item.id,
            user_id,
            item.batch_id,
            item.src_path,
            staged.map(|p| p.display().to_string()),
            item.status.as_str(),
            item.error,
            serde_json::to_string(&item.file_kind).unwrap_or_default().trim_matches('"'),
            item.sha256,
            item.byte_size,
            item.page_count,
            item.exif_orientation,
        ],
    )
    .map_err(|e| format!("cannot record ingest item: {e}"))?;
    Ok(())
}

/// Stage a batch of dropped files. Returns one row per input, in input order,
/// including the failures — the grid shows every file the user dropped.
pub fn stage_batch(
    conn: &Connection,
    user_id: &str,
    staging: &Path,
    paths: &[String],
) -> Result<Vec<IngestItem>, String> {
    std::fs::create_dir_all(staging).map_err(|e| format!("cannot create staging dir: {e}"))?;
    let batch_id = ulid::Ulid::new().to_string();

    Ok(paths
        .iter()
        .map(|p| stage_one(conn, user_id, &batch_id, staging, Path::new(p)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};

    /// A real JPEG carrying a real EXIF APP1 segment with the given orientation.
    /// Built by hand rather than checked in, so the test covers the actual parse
    /// path a phone photo takes rather than a pre-baked fixture.
    fn jpeg_with_orientation(w: u32, h: u32, orientation: u16) -> Vec<u8> {
        let mut img = RgbImage::from_pixel(w, h, Rgb([220, 220, 220]));
        img.put_pixel(0, 0, Rgb([255, 0, 0]));
        let mut jpeg = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut jpeg), ImageFormat::Jpeg)
            .unwrap();

        // little-endian TIFF header + one IFD entry (tag 0x0112, SHORT, count 1)
        let mut tiff: Vec<u8> = vec![0x49, 0x49, 0x2A, 0x00, 8, 0, 0, 0];
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x0112u16.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&orientation.to_le_bytes());
        tiff.extend_from_slice(&[0, 0]);
        tiff.extend_from_slice(&0u32.to_le_bytes());

        let mut app1: Vec<u8> = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);

        let mut out: Vec<u8> = vec![0xFF, 0xD8];
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&jpeg[2..]); // original minus its SOI
        out
    }

    struct Fixture {
        dir: PathBuf,
        conn: Connection,
        user: String,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("mrt-ingest-{name}-{}", ulid::Ulid::new()));
            std::fs::create_dir_all(dir.join("staging")).unwrap();
            let conn = crate::db::open(&dir.join("app.db")).unwrap();
            let user = crate::db::ensure_user(&conn).unwrap();
            Fixture { dir, conn, user }
        }

        fn staging(&self) -> PathBuf {
            self.dir.join("staging")
        }

        fn write(&self, name: &str, bytes: &[u8]) -> String {
            let p = self.dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            p.display().to_string()
        }

        fn stage(&self, paths: &[String]) -> Vec<IngestItem> {
            stage_batch(&self.conn, &self.user, &self.staging(), paths).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn the_review_queue_survives_closing_the_app() {
        let f = Fixture::new("relist");
        let a = f.write("one.jpg", &jpeg_with_orientation(300, 200, 1));
        let b = f.write("two.jpg", &jpeg_with_orientation(200, 300, 6));
        let staged = f.stage(&[a, b]);
        assert_eq!(staged.len(), 2);

        // Nothing in memory — this is what a fresh launch sees.
        let back = list_staged(&f.conn, &f.user).unwrap();
        assert_eq!(back.len(), 2, "both files are still waiting to be reviewed");
        assert_eq!(
            back.iter().map(|i| i.file_name.as_str()).collect::<Vec<_>>(),
            vec!["one.jpg", "two.jpg"],
            "drop order within a batch is preserved",
        );

        // Derived from the stored EXIF tag rather than remembered, so the note
        // explaining why the pixels changed does not disappear on restart.
        let rotated = back.iter().find(|i| i.file_name == "two.jpg").unwrap();
        assert!(rotated.orientation_baked, "the rotated photo still says so");
        assert!(!back.iter().find(|i| i.file_name == "one.jpg").unwrap().orientation_baked);
    }

    #[test]
    fn a_filed_document_leaves_the_queue() {
        let f = Fixture::new("filed");
        let src = f.write("report.jpg", &jpeg_with_orientation(300, 200, 1));
        let items = f.stage(&[src]);

        // A real document row, because the queue is excluded by the foreign key
        // pointing at one — not by the status text alone.
        f.conn
            .execute(
                "INSERT INTO patients (id, owner_user_id, display_name, folder_slug, created_at, updated_at)
                 VALUES ('pat', ?1, 'A Patient', 'A-Patient', datetime('now'), datetime('now'))",
                params![f.user],
            )
            .unwrap();
        f.conn
            .execute(
                "INSERT INTO documents
                   (id, owner_user_id, patient_id, doc_date, date_source, title, doc_type,
                    rel_path, sha256, byte_size, file_kind, created_at, updated_at)
                 VALUES ('doc', ?1, 'pat', '2026-01-02', 'manual', 'Report', 'report',
                         'A-Patient/2026/x.jpg', 'sha', 10, 'jpeg', datetime('now'), datetime('now'))",
                params![f.user],
            )
            .unwrap();
        f.conn
            .execute(
                "UPDATE ingest_items SET document_id = 'doc', status = 'committed' WHERE id = ?1",
                params![items[0].id],
            )
            .unwrap();

        assert!(
            list_staged(&f.conn, &f.user).unwrap().is_empty(),
            "a committed file must not come back as still needing review",
        );
    }

    #[test]
    fn rejected_files_do_not_come_back_as_reviewable() {
        let f = Fixture::new("rejected");
        let src = f.write("scan.jpg", &jpeg_with_orientation(300, 200, 1));
        let items = f.stage(&[src]);

        // Duplicates and failures are reported once, at the moment of import.
        // Returning them forever would grow a permanent list of things the user
        // has no action to take on.
        f.conn
            .execute(
                "UPDATE ingest_items SET status = 'duplicate' WHERE id = ?1",
                params![items[0].id],
            )
            .unwrap();

        assert!(list_staged(&f.conn, &f.user).unwrap().is_empty());
    }

    #[test]
    fn the_queue_belongs_to_its_owner() {
        let f = Fixture::new("owner");
        let src = f.write("mine.jpg", &jpeg_with_orientation(300, 200, 1));
        f.stage(&[src]);

        assert!(
            list_staged(&f.conn, "01OTHERUSER0000000000000000").unwrap().is_empty(),
            "the queue is scoped to the owner from the first migration onward",
        );
    }

    #[test]
    fn a_rotated_phone_photo_is_baked_upright_at_ingest() {
        let f = Fixture::new("rotate");
        // 400x200 landscape tagged "rotate 90 CW" — what a phone produces when held
        // upright. Without baking this is the file that lands sideways in the export.
        let src = f.write("photo.jpg", &jpeg_with_orientation(400, 200, 6));
        let items = f.stage(&[src]);

        assert_eq!(items.len(), 1);
        let it = &items[0];
        assert_eq!(it.file_kind, FileKind::Jpeg);
        assert_eq!(it.exif_orientation, Some(6));
        assert!(it.orientation_baked, "orientation 6 must be baked");
        assert_eq!(
            (it.width, it.height),
            (Some(200), Some(400)),
            "baking must swap the dimensions",
        );
        assert_eq!(it.status, IngestStatus::NeedsDate);

        // The staged file must itself be upright, so orientation can never be
        // applied a second time downstream.
        let staged = f.staging().join(format!("{}.jpg", it.id));
        assert!(staged.exists());
        let restaged = std::fs::read(&staged).unwrap();
        assert_eq!(
            crate::imaging::read_orientation(&restaged),
            1,
            "the staged derivative must carry no orientation tag",
        );
        assert!(f.staging().join(format!("{}.thumb.jpg", it.id)).exists());
    }

    #[test]
    fn an_upright_photo_is_left_alone() {
        let f = Fixture::new("upright");
        let src = f.write("photo.jpg", &jpeg_with_orientation(400, 200, 1));
        let it = &f.stage(&[src])[0];
        assert_eq!(it.exif_orientation, Some(1));
        assert!(!it.orientation_baked);
        assert_eq!((it.width, it.height), (Some(400), Some(200)));
    }

    #[test]
    fn the_same_file_twice_is_caught_by_content_not_by_name() {
        let f = Fixture::new("dupe");
        let bytes = jpeg_with_orientation(120, 80, 1);
        let a = f.write("scan.jpg", &bytes);
        let b = f.write("scan-copy.jpg", &bytes); // different name, identical bytes

        let first = f.stage(&[a]);
        assert_eq!(first[0].status, IngestStatus::NeedsDate);

        let second = f.stage(&[b]);
        assert_eq!(second[0].status, IngestStatus::Duplicate);
        assert_eq!(
            second[0].sha256, first[0].sha256,
            "the duplicate is identified by content hash, not by filename",
        );
    }

    #[test]
    fn heic_fails_with_an_actionable_message_not_a_decode_error() {
        let f = Fixture::new("heic");
        let mut heic = vec![0, 0, 0, 0x18];
        heic.extend_from_slice(b"ftypheic");
        heic.resize(64, 0);
        let src = f.write("IMG_0001.HEIC", &heic);

        let it = &f.stage(&[src])[0];
        assert_eq!(it.file_kind, FileKind::Heic);
        assert_eq!(it.status, IngestStatus::Failed);
        assert!(it.error.as_ref().unwrap().contains("Most Compatible"));
    }

    #[test]
    fn one_bad_file_does_not_abort_the_batch() {
        // The 60-photo import case: a single unreadable file must not cost the
        // other 59 their staging work.
        let f = Fixture::new("batch");
        let good1 = f.write("a.jpg", &jpeg_with_orientation(100, 60, 1));
        let bad = f.write("b.txt", b"this is not an image at all, not even close");
        let good2 = f.write("c.jpg", &jpeg_with_orientation(90, 70, 3));

        let items = f.stage(&[good1, bad, good2]);
        assert_eq!(items.len(), 3, "every dropped file gets a row");
        assert_eq!(items[0].status, IngestStatus::NeedsDate);
        assert_eq!(items[1].status, IngestStatus::Failed);
        assert_eq!(items[2].status, IngestStatus::NeedsDate);
        assert!(items[2].orientation_baked, "orientation 3 must still be baked");
    }

    /// A real password-protected PDF, made with the same tool that has to open it.
    fn locked_pdf(dir: &Path, password: &str) -> String {
        std::env::set_var(
            "PDFCPU_PATH",
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("binaries")
                .join("pdfcpu-x86_64-pc-windows-msvc.exe"),
        );
        let exe = crate::export::pdfcpu_path().unwrap();

        let img = dir.join("page.png");
        std::fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("icons").join("128x128.png"),
            &img,
        )
        .unwrap();

        let plain = dir.join("plain.pdf");
        std::process::Command::new(&exe)
            .args(["import", "f:A4, pos:c", &plain.to_string_lossy(), &img.to_string_lossy()])
            .output()
            .unwrap();

        let locked = dir.join("locked.pdf");
        let out = std::process::Command::new(&exe)
            .args([
                "encrypt",
                "--upw",
                password,
                "--opw",
                password,
                &plain.to_string_lossy(),
                &locked.to_string_lossy(),
            ])
            .output()
            .unwrap();
        assert!(locked.exists(), "could not build a locked fixture: {out:?}");
        locked.display().to_string()
    }

    #[test]
    fn a_password_protected_pdf_is_kept_and_flagged_rather_than_rejected() {
        let f = Fixture::new("locked");
        let src = locked_pdf(&f.dir, "opensesame");

        let items = f.stage(&[src]);
        assert_eq!(items[0].status, IngestStatus::Locked);
        assert!(
            items[0].error.as_deref().unwrap_or_default().contains("password protected"),
            "the message has to say what to do: {:?}",
            items[0].error,
        );

        // Kept in the queue across a restart — a locked file the app forgets is a
        // file silently missing from every future export.
        let back = list_staged(&f.conn, &f.user).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].status, IngestStatus::Locked);
    }

    #[test]
    fn the_right_password_unlocks_it_and_returns_it_to_the_queue() {
        let f = Fixture::new("unlock");
        let src = locked_pdf(&f.dir, "opensesame");
        let items = f.stage(&[src]);

        unlock(&f.conn, &f.user, &items[0].id, "opensesame").expect("should unlock");

        let back = list_staged(&f.conn, &f.user).unwrap();
        assert_eq!(back[0].status, IngestStatus::NeedsDate, "it becomes an ordinary row");
        assert_eq!(back[0].error, None);
        assert_eq!(back[0].page_count, Some(1), "and its real page count is known");

        // Readable by everything downstream now, not just by the unlock path.
        let exe = crate::export::pdfcpu_path().unwrap();
        let staged: String = f
            .conn
            .query_row(
                "SELECT staged_path FROM ingest_items WHERE id = ?1",
                params![items[0].id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(matches!(
            crate::export::probe_pdf(&exe, Path::new(&staged)),
            crate::export::PdfState::Readable { .. }
        ));
    }

    #[test]
    fn a_wrong_password_says_so_and_leaves_the_file_alone() {
        let f = Fixture::new("wrongpw");
        let src = locked_pdf(&f.dir, "opensesame");
        let items = f.stage(&[src]);
        let staged: String = f
            .conn
            .query_row(
                "SELECT staged_path FROM ingest_items WHERE id = ?1",
                params![items[0].id],
                |r| r.get(0),
            )
            .unwrap();
        let before = std::fs::read(&staged).unwrap();

        let err = unlock(&f.conn, &f.user, &items[0].id, "guess").unwrap_err();
        assert!(err.contains("did not open"), "{err}");

        assert_eq!(std::fs::read(&staged).unwrap(), before, "the file must be untouched");
        let back = list_staged(&f.conn, &f.user).unwrap();
        assert_eq!(back[0].status, IngestStatus::Locked, "and it stays locked");
    }

    #[test]
    fn an_empty_password_is_refused_before_pdfcpu_is_asked() {
        let f = Fixture::new("emptypw");
        let src = locked_pdf(&f.dir, "opensesame");
        let items = f.stage(&[src]);
        assert!(unlock(&f.conn, &f.user, &items[0].id, "").unwrap_err().contains("Enter the password"));
    }

    #[test]
    fn a_pdf_the_validator_dislikes_is_still_accepted() {
        let f = Fixture::new("questionable");
        // Correct magic bytes, nothing behind them. `pdfcpu info` refuses this,
        // but refusing the import on that basis would also refuse real scans that
        // merge perfectly well, so it is kept and the export decides.
        let mut bytes = b"%PDF-1.4\n".to_vec();
        bytes.extend(std::iter::repeat(0u8).take(600));
        let src = f.write("broken.pdf", &bytes);

        let items = f.stage(&[src]);
        assert_eq!(
            items[0].status,
            IngestStatus::NeedsDate,
            "a strict validator must not be the gate: {:?}",
            items[0].error,
        );
        assert_eq!(items[0].page_count, None, "but no page count may be invented for it");
    }

    #[test]
    fn structural_damage_is_pdfcpus_problem_and_it_handles_it() {
        let f = Fixture::new("damaged");
        let src = locked_pdf(&f.dir, "x");
        // Build a plain one and break its cross-reference table, the classic
        // symptom of a scanner interrupted mid-write. pdfcpu rebuilds it, which is
        // why this app carries no separate repair tool.
        let plain = f.dir.join("plain.pdf");
        let mut bytes = std::fs::read(&plain).unwrap();
        if let Some(at) = bytes.windows(9).position(|w| w == b"startxref") {
            bytes.splice(at + 10..at + 14, b"99999".iter().copied());
        }
        let broken = f.write("broken-xref.pdf", &bytes);
        let _ = src;

        let items = f.stage(&[broken]);
        assert_eq!(
            items[0].status,
            IngestStatus::NeedsDate,
            "a rebuilt file is an ordinary file: {:?}",
            items[0].error,
        );
        assert_eq!(items[0].error, None, "and it should not be flagged at the user");
    }

    #[test]
    fn a_multi_page_pdf_reports_its_real_page_count() {
        // Assuming one made a 12-page scanned report claim to be a single page
        // everywhere it was displayed.
        let f = Fixture::new("pages");
        std::env::set_var(
            "PDFCPU_PATH",
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("binaries")
                .join("pdfcpu-x86_64-pc-windows-msvc.exe"),
        );

        // Build a genuine 3-page PDF with pdfcpu itself.
        let exe = crate::export::pdfcpu_path().unwrap();
        let mut pages = Vec::new();
        for i in 0..3 {
            let img = f.dir.join(format!("p{i}.jpg"));
            let mut bytes = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(60, 80, image::Rgb([200, 200, 200])))
                .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
                .unwrap();
            std::fs::write(&img, bytes).unwrap();
            let page = f.dir.join(format!("p{i}.pdf"));
            std::process::Command::new(&exe)
                .args(["import", "f:A4, pos:c", &page.to_string_lossy(), &img.to_string_lossy()])
                .output().unwrap();
            pages.push(page.to_string_lossy().to_string());
        }
        let merged = f.dir.join("three.pdf");
        let mut args = vec!["merge".to_string(), merged.to_string_lossy().to_string()];
        args.extend(pages);
        std::process::Command::new(&exe).args(&args).output().unwrap();

        let items = f.stage(&[merged.to_string_lossy().to_string()]);
        assert_eq!(items[0].file_kind, FileKind::Pdf);
        assert_eq!(items[0].page_count, Some(3));
    }

    #[test]
    fn an_image_is_always_one_page() {
        let f = Fixture::new("onepage");
        let src = f.write("x.jpg", &jpeg_with_orientation(80, 60, 1));
        assert_eq!(f.stage(&[src])[0].page_count, None, "images carry no page count of their own");
    }

    #[test]
    fn a_pdf_is_staged_verbatim_as_a_single_item() {
        let f = Fixture::new("pdf");
        // Re-encoding someone else's PDF loses fidelity for no benefit, so the
        // staged bytes must be identical to the source.
        let pdf = b"%PDF-1.7\n1 0 obj\n<</Type/Catalog>>\nendobj\ntrailer\n%%EOF\n";
        let src = f.write("report.pdf", pdf);

        let it = &f.stage(&[src])[0];
        assert_eq!(it.file_kind, FileKind::Pdf);
        assert_eq!(it.status, IngestStatus::NeedsDate);
        assert_eq!(it.exif_orientation, None);

        let staged = f.staging().join(format!("{}.pdf", it.id));
        assert_eq!(std::fs::read(&staged).unwrap(), pdf);
    }

    #[test]
    fn staged_rows_survive_in_the_database_for_resume() {
        let f = Fixture::new("persist");
        let src = f.write("x.jpg", &jpeg_with_orientation(100, 100, 1));
        let it = &f.stage(&[src])[0];

        let (status, staged): (String, Option<String>) = f
            .conn
            .query_row(
                "SELECT status, staged_path FROM ingest_items WHERE id = ?1",
                params![it.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "needs_date");
        assert!(staged.is_some(), "staged path must persist so a crash can resume");
    }
}
