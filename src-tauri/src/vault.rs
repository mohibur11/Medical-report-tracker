//! Committing staged files into the browsable vault.
//!
//! Windows has no transactional multi-file rename — TxF is deprecated — and it
//! denies rename on any file with an open handle. One PDF viewer left open, one
//! Defender lock, one OneDrive sync or one power cut is enough to leave the tree
//! disagreeing with the database.
//!
//! So every filesystem mutation is written to `fs_journal` as an INTENT first,
//! applied second, and marked applied third. A crash between any two steps leaves
//! a pending row that is replayed at next startup. Replay is idempotent: it checks
//! the world before acting, because it cannot know how far the previous run got.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::naming;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommittedDocument {
    pub id: String,
    pub rel_path: String,
    pub file_name: String,
    pub title_truncated: bool,
}

/// Allocate the next collision suffix for a base name.
///
/// Suffixes are monotonic and NEVER reused: if `__02` is deleted, the next file
/// with the same base gets `__03`. Reusing it would let two different documents
/// have shared a filename over time, so an old export and a new one would
/// disagree about what `__02` referred to.
fn reserve_seq(conn: &Connection, dir_rel: &str, base_name: &str) -> Result<u32, String> {
    conn.execute(
        "INSERT INTO name_reservation (dir_rel, base_name, next_seq) VALUES (?1, ?2, 1)
         ON CONFLICT(dir_rel, base_name) DO UPDATE SET next_seq = next_seq + 1",
        params![dir_rel, base_name],
    )
    .map_err(|e| format!("cannot reserve name: {e}"))?;

    conn.query_row(
        "SELECT next_seq FROM name_reservation WHERE dir_rel = ?1 AND base_name = ?2",
        params![dir_rel, base_name],
        |r| r.get(0),
    )
    .map_err(|e| format!("cannot read reservation: {e}"))
}

/// Record what is about to happen, before it happens.
fn journal_intent(conn: &Connection, op: &str, from: &Path, to: &Path) -> Result<String, String> {
    let id = ulid::Ulid::new().to_string();
    let intent = serde_json::json!({
        "from": from.display().to_string(),
        "to": to.display().to_string(),
    });
    conn.execute(
        "INSERT INTO fs_journal (id, op, intent_json, created_at)
         VALUES (?1, ?2, ?3, datetime('now'))",
        params![id, op, intent.to_string()],
    )
    .map_err(|e| format!("cannot write journal: {e}"))?;
    Ok(id)
}

fn journal_done(conn: &Connection, id: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE fs_journal SET applied_at = datetime('now') WHERE id = ?1",
        params![id],
    )
    .map_err(|e| format!("cannot close journal entry: {e}"))?;
    Ok(())
}

/// Move a file, falling back to copy+delete across volumes.
///
/// `fs::rename` fails with a cross-device error when the vault lives on a
/// different drive from `%LOCALAPPDATA%` — which is exactly the case for anyone
/// whose Documents folder is redirected to a second disk.
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {parent:?}: {e}"))?;
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to).map_err(|e| format!("cannot copy into vault: {e}"))?;
            // A failure to remove the staged copy is not fatal — the vault has the
            // file, and a leftover staging file is cleaned up on next sweep.
            let _ = std::fs::remove_file(from);
            Ok(())
        }
    }
}

/// Replay every unapplied journal entry. Called at startup, before anything else
/// touches the vault.
///
/// Returns the number of entries resolved. Idempotent by inspection: if the source
/// is gone and the destination exists, the move already happened and only the
/// bookkeeping was lost.
pub fn replay_journal(conn: &Connection) -> Result<usize, String> {
    let mut stmt = conn
        .prepare("SELECT id, op, intent_json FROM fs_journal WHERE applied_at IS NULL ORDER BY id")
        .map_err(|e| e.to_string())?;

    let pending: Vec<(String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    drop(stmt);

    let mut resolved = 0usize;
    for (id, op, intent_json) in pending {
        let intent: serde_json::Value = match serde_json::from_str(&intent_json) {
            Ok(v) => v,
            Err(e) => {
                mark_failed(conn, &id, &format!("unreadable intent: {e}"))?;
                continue;
            }
        };
        let from = PathBuf::from(intent["from"].as_str().unwrap_or_default());
        let to = PathBuf::from(intent["to"].as_str().unwrap_or_default());

        if op != "move" {
            mark_failed(conn, &id, &format!("unknown op {op}"))?;
            continue;
        }

        if to.exists() && !from.exists() {
            // Already done; only the bookkeeping was lost.
            journal_done(conn, &id)?;
            resolved += 1;
        } else if from.exists() {
            match move_file(&from, &to) {
                Ok(()) => {
                    journal_done(conn, &id)?;
                    resolved += 1;
                }
                Err(e) => mark_failed(conn, &id, &e)?,
            }
        } else {
            // Neither side exists. Nothing can be recovered here, and guessing
            // would be worse than leaving a visible failure.
            mark_failed(conn, &id, "source and destination both missing")?;
        }
    }
    Ok(resolved)
}

fn mark_failed(conn: &Connection, id: &str, error: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE fs_journal SET failed_at = datetime('now'), error = ?2 WHERE id = ?1",
        params![id, error],
    )
    .map_err(|e| format!("cannot mark journal failure: {e}"))?;
    Ok(())
}

pub struct CommitRequest<'a> {
    pub ingest_id: &'a str,
    pub patient_id: &'a str,
    pub doc_date: &'a str,
    pub title: &'a str,
    pub doc_type: &'a str,
}

/// Move one staged file into `<Patient>/<Year>/` and create its document row.
pub fn commit(
    conn: &Connection,
    user_id: &str,
    vault_root: &Path,
    req: CommitRequest<'_>,
) -> Result<CommittedDocument, String> {
    let (staged_path, file_kind, sha256, byte_size, orientation): (
        Option<String>, String, Option<String>, i64, Option<i64>,
    ) = conn
        .query_row(
            "SELECT staged_path, file_kind, sha256, byte_size, exif_orientation
             FROM ingest_items WHERE id = ?1",
            params![req.ingest_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .map_err(|e| format!("no such staged item: {e}"))?;

    let staged = staged_path.ok_or("staged file is missing — re-import this file")?;
    let staged = PathBuf::from(staged);
    if !staged.exists() {
        return Err("staged file no longer exists on disk — re-import this file".into());
    }

    let patient_name: String = conn
        .query_row(
            "SELECT display_name FROM patients WHERE id = ?1",
            params![req.patient_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("no such patient: {e}"))?;

    let ext = staged
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_else(|| "bin".into());

    let root_len = vault_root.display().to_string().encode_utf16().count() + 1;

    // Reserve against the seq-1 name, which is the stable identity of the slot.
    let probe = naming::build_name(req.doc_date, &patient_name, req.title, &ext, 1, root_len);
    let dir_rel = format!("{}\\{}", probe.patient_folder, probe.year_folder);
    let base = probe
        .file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem.to_string())
        .unwrap_or_else(|| probe.file_name.clone());

    let seq = reserve_seq(conn, &dir_rel, &base)?;
    let built = naming::build_name(req.doc_date, &patient_name, req.title, &ext, seq, root_len);
    let dest = vault_root.join(built.rel_path.replace('\\', "/"));

    // Intent first, action second, bookkeeping third.
    let journal_id = journal_intent(conn, "move", &staged, &dest)?;
    move_file(&staged, &dest)?;
    journal_done(conn, &journal_id)?;

    let doc_id = ulid::Ulid::new().to_string();
    conn.execute(
        "INSERT INTO documents
           (id, owner_user_id, patient_id, doc_date, date_source, title, doc_type,
            page_count, rel_path, sha256, byte_size, file_kind, exif_orientation,
            created_at, updated_at)
         VALUES (?1,?2,?3,?4,'manual',?5,?6,1,?7,?8,?9,?10,?11, datetime('now'), datetime('now'))",
        params![
            doc_id,
            user_id,
            req.patient_id,
            req.doc_date,
            req.title,
            req.doc_type,
            built.rel_path,
            sha256,
            byte_size,
            file_kind,
            orientation,
        ],
    )
    .map_err(|e| format!("cannot record document: {e}"))?;

    conn.execute(
        "UPDATE ingest_items
         SET status = 'committed', document_id = ?2, chosen_patient_id = ?3,
             chosen_date = ?4, chosen_title = ?5, updated_at = datetime('now')
         WHERE id = ?1",
        params![req.ingest_id, doc_id, req.patient_id, req.doc_date, req.title],
    )
    .map_err(|e| format!("cannot close ingest item: {e}"))?;

    // Searchable immediately. A failure here must not undo a filed document —
    // the index is rebuildable, the file move is not.
    if let Err(e) = crate::search::index_document(conn, &doc_id) {
        eprintln!("indexing {doc_id} failed: {e}");
    }
    // The sidecar is what lets a bare folder tree be rebuilt into a library,
    // categories and notes included. Not worth failing a filed document over.
    if let Err(e) = crate::backup::write_sidecar(conn, vault_root, &doc_id) {
        eprintln!("sidecar for {doc_id} failed: {e}");
    }

    Ok(CommittedDocument {
        id: doc_id,
        rel_path: built.rel_path,
        file_name: built.file_name,
        title_truncated: built.title_truncated,
    })
}

/// Move a document to the vault's Trash folder and mark the row trashed.
///
/// Nothing is ever unlinked. A medical record deleted by a mis-click is not
/// recoverable from anywhere else, so "delete" means "move somewhere obvious" and
/// the row survives so the document can be put back.
pub fn trash(
    conn: &Connection,
    user_id: &str,
    vault_root: &Path,
    document_id: &str,
) -> Result<(), String> {
    let rel_path: String = conn
        .query_row(
            "SELECT rel_path FROM documents
             WHERE id = ?1 AND owner_user_id = ?2 AND trashed_at IS NULL",
            params![document_id, user_id],
            |r| r.get(0),
        )
        .map_err(|_| "No such document.".to_string())?;

    let from = vault_root.join(rel_path.replace('\\', "/"));
    // Keep the vault-relative shape inside Trash so it is obvious where a file
    // came from, and so two patients' identically named files cannot collide.
    let to = vault_root.join("Trash").join(rel_path.replace('\\', "/"));

    if from.exists() {
        let journal_id = journal_intent(conn, "move", &from, &to)?;
        move_file(&from, &to)?;
        journal_done(conn, &journal_id)?;
    }

    crate::backup::remove_sidecar(vault_root, &rel_path);

    conn.execute(
        "UPDATE documents SET trashed_at = datetime('now'), updated_at = datetime('now')
         WHERE id = ?1",
        params![document_id],
    )
    .map_err(|e| format!("cannot trash document: {e}"))?;

    let _ = crate::search::remove_document(conn, document_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fx {
        dir: PathBuf,
        conn: Connection,
        user: String,
        patient: String,
    }

    impl Fx {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("mrt-vault-{name}-{}", ulid::Ulid::new()));
            std::fs::create_dir_all(dir.join("staging")).unwrap();
            std::fs::create_dir_all(dir.join("vault")).unwrap();
            let conn = crate::db::open(&dir.join("app.db")).unwrap();
            let user = crate::db::ensure_user(&conn).unwrap();

            let patient = ulid::Ulid::new().to_string();
            conn.execute(
                "INSERT INTO patients (id, owner_user_id, display_name, folder_slug, created_at, updated_at)
                 VALUES (?1, ?2, 'Rahim Uddin', 'Rahim-Uddin', datetime('now'), datetime('now'))",
                params![patient, user],
            )
            .unwrap();

            Fx { dir, conn, user, patient }
        }

        fn vault(&self) -> PathBuf {
            self.dir.join("vault")
        }

        /// Create a staged file and its ingest row, as the ingest pipeline would.
        fn stage(&self, ext: &str, bytes: &[u8]) -> String {
            let id = ulid::Ulid::new().to_string();
            let p = self.dir.join("staging").join(format!("{id}.{ext}"));
            std::fs::write(&p, bytes).unwrap();
            self.conn
                .execute(
                    "INSERT INTO ingest_items
                       (id, owner_user_id, batch_id, src_path, staged_path, status,
                        file_kind, sha256, byte_size, created_at, updated_at)
                     VALUES (?1,?2,'batch','src',?3,'needs_date',?4,'deadbeef',?5,
                             datetime('now'), datetime('now'))",
                    params![id, self.user, p.display().to_string(), ext, bytes.len() as i64],
                )
                .unwrap();
            id
        }

        fn commit(&self, ingest_id: &str, date: &str, title: &str) -> Result<CommittedDocument, String> {
            commit(
                &self.conn,
                &self.user,
                &self.vault(),
                CommitRequest {
                    ingest_id,
                    patient_id: &self.patient,
                    doc_date: date,
                    title,
                    doc_type: "report",
                },
            )
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_committed_file_lands_at_patient_year_with_its_canonical_name() {
        let f = Fx::new("commit");
        let id = f.stage("jpg", b"image bytes");
        let doc = f.commit(&id, "2026-03-14", "Thyroid Profile").unwrap();

        assert_eq!(doc.file_name, "2026-03-14_Rahim-Uddin_Thyroid-Profile.jpg");
        assert_eq!(doc.rel_path, "Rahim-Uddin\\2026\\2026-03-14_Rahim-Uddin_Thyroid-Profile.jpg");

        let on_disk = f.vault().join("Rahim-Uddin").join("2026").join(&doc.file_name);
        assert!(on_disk.exists(), "file must exist at the canonical path");
        assert_eq!(std::fs::read(&on_disk).unwrap(), b"image bytes");

        // The staged copy is gone — the file moved, it was not duplicated.
        let staged: Option<String> = f.conn
            .query_row("SELECT staged_path FROM ingest_items WHERE id = ?1", params![id], |r| r.get(0))
            .unwrap();
        assert!(!PathBuf::from(staged.unwrap()).exists());
    }

    #[test]
    fn the_ingest_row_and_document_row_are_linked_after_commit() {
        let f = Fx::new("link");
        let id = f.stage("pdf", b"%PDF-1.7");
        let doc = f.commit(&id, "2026-03-14", "CBC").unwrap();

        let (status, doc_id): (String, Option<String>) = f.conn
            .query_row("SELECT status, document_id FROM ingest_items WHERE id = ?1", params![id],
                       |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(status, "committed");
        assert_eq!(doc_id.as_deref(), Some(doc.id.as_str()));
    }

    #[test]
    fn two_reports_of_the_same_test_on_the_same_day_do_not_overwrite() {
        let f = Fx::new("collide");
        let a = f.stage("jpg", b"first");
        let b = f.stage("jpg", b"second");

        let d1 = f.commit(&a, "2026-03-14", "CBC").unwrap();
        let d2 = f.commit(&b, "2026-03-14", "CBC").unwrap();

        assert_eq!(d1.file_name, "2026-03-14_Rahim-Uddin_CBC.jpg");
        assert_eq!(d2.file_name, "2026-03-14_Rahim-Uddin_CBC__02.jpg");

        let dir = f.vault().join("Rahim-Uddin").join("2026");
        assert_eq!(std::fs::read(dir.join(&d1.file_name)).unwrap(), b"first");
        assert_eq!(std::fs::read(dir.join(&d2.file_name)).unwrap(), b"second");
    }

    #[test]
    fn a_deleted_files_suffix_is_never_handed_to_a_later_document() {
        let f = Fx::new("noreuse");
        let a = f.stage("jpg", b"a");
        let b = f.stage("jpg", b"b");
        let d1 = f.commit(&a, "2026-03-14", "CBC").unwrap();
        let d2 = f.commit(&b, "2026-03-14", "CBC").unwrap();

        // Delete the second file entirely, as a user might in Explorer.
        std::fs::remove_file(f.vault().join("Rahim-Uddin").join("2026").join(&d2.file_name)).unwrap();

        let c = f.stage("jpg", b"c");
        let d3 = f.commit(&c, "2026-03-14", "CBC").unwrap();
        assert_eq!(d3.file_name, "2026-03-14_Rahim-Uddin_CBC__03.jpg",
                   "must not reuse __02 — an old export would disagree about what it meant");
        assert_ne!(d3.file_name, d1.file_name);
    }

    #[test]
    fn undated_documents_are_quarantined_not_filed_into_a_wrong_year() {
        let f = Fx::new("undated");
        let id = f.stage("jpg", b"x");
        let doc = f.commit(&id, "0000-00-00", "Prescription").unwrap();
        assert!(doc.rel_path.contains("\\Undated\\"));
        assert!(f.vault().join("Rahim-Uddin").join("Undated").join(&doc.file_name).exists());
    }

    #[test]
    fn the_journal_is_closed_on_success_and_nothing_is_left_pending() {
        let f = Fx::new("journal");
        let id = f.stage("jpg", b"x");
        f.commit(&id, "2026-03-14", "CBC").unwrap();

        let pending: i64 = f.conn
            .query_row("SELECT count(*) FROM fs_journal WHERE applied_at IS NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pending, 0);
    }

    #[test]
    fn replay_completes_a_move_that_a_crash_interrupted() {
        // Simulates dying after the intent was written but before the file moved.
        let f = Fx::new("replay-move");
        let from = f.dir.join("staging").join("orphan.jpg");
        std::fs::write(&from, b"interrupted").unwrap();
        let to = f.vault().join("Rahim-Uddin").join("2026").join("2026-03-14_Rahim-Uddin_CBC.jpg");

        journal_intent(&f.conn, "move", &from, &to).unwrap();
        assert!(!to.exists());

        assert_eq!(replay_journal(&f.conn).unwrap(), 1);
        assert!(to.exists(), "replay must finish the move");
        assert!(!from.exists());
        assert_eq!(std::fs::read(&to).unwrap(), b"interrupted");
    }

    #[test]
    fn replay_is_idempotent_when_the_move_already_happened() {
        // Simulates dying after the file moved but before the journal was closed.
        let f = Fx::new("replay-done");
        let from = f.dir.join("staging").join("gone.jpg");
        let to = f.vault().join("Rahim-Uddin").join("2026").join("done.jpg");
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::write(&to, b"already there").unwrap();

        journal_intent(&f.conn, "move", &from, &to).unwrap();
        assert_eq!(replay_journal(&f.conn).unwrap(), 1);

        assert_eq!(std::fs::read(&to).unwrap(), b"already there", "must not clobber");
        let pending: i64 = f.conn
            .query_row("SELECT count(*) FROM fs_journal WHERE applied_at IS NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pending, 0);
    }

    #[test]
    fn replay_records_a_failure_rather_than_guessing_when_both_sides_are_gone() {
        let f = Fx::new("replay-lost");
        let from = f.dir.join("staging").join("nowhere.jpg");
        let to = f.vault().join("Rahim-Uddin").join("2026").join("nowhere.jpg");
        journal_intent(&f.conn, "move", &from, &to).unwrap();

        assert_eq!(replay_journal(&f.conn).unwrap(), 0);
        let (failed, err): (i64, Option<String>) = f.conn
            .query_row("SELECT count(*), max(error) FROM fs_journal WHERE failed_at IS NOT NULL",
                       [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(failed, 1);
        assert!(err.unwrap().contains("both missing"));
    }

    /// The whole path the UI drives: a rotated phone photo is dropped, staged,
    /// baked upright, then filed under its canonical name.
    #[test]
    fn a_dropped_rotated_photo_ends_up_upright_at_its_canonical_path() {
        use image::{DynamicImage, ImageFormat, Rgb, RgbImage};

        let f = Fx::new("e2e");

        // A 400x200 landscape JPEG tagged "rotate 90 CW" — the common phone case.
        let mut img = RgbImage::from_pixel(400, 200, Rgb([230, 230, 230]));
        img.put_pixel(0, 0, Rgb([255, 0, 0]));
        let mut jpeg = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut jpeg), ImageFormat::Jpeg)
            .unwrap();

        let mut tiff: Vec<u8> = vec![0x49, 0x49, 0x2A, 0x00, 8, 0, 0, 0];
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x0112u16.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&6u16.to_le_bytes());
        tiff.extend_from_slice(&[0, 0]);
        tiff.extend_from_slice(&0u32.to_le_bytes());
        let mut app1: Vec<u8> = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);

        let mut src_bytes: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0xE1];
        src_bytes.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        src_bytes.extend_from_slice(&app1);
        src_bytes.extend_from_slice(&jpeg[2..]);

        let src = f.dir.join("IMG_4821.jpg");
        std::fs::write(&src, &src_bytes).unwrap();

        let staged = crate::ingest::stage_batch(
            &f.conn,
            &f.user,
            &f.dir.join("staging"),
            &[src.display().to_string()],
        )
        .unwrap();

        assert_eq!(staged.len(), 1);
        assert!(staged[0].orientation_baked);
        assert_eq!((staged[0].width, staged[0].height), (Some(200), Some(400)));

        let doc = f.commit(&staged[0].id, "2026-03-14", "USG of Whole Abdomen").unwrap();
        assert_eq!(doc.file_name, "2026-03-14_Rahim-Uddin_USG-of-Whole-Abdomen.jpg");

        let on_disk = f.vault().join("Rahim-Uddin").join("2026").join(&doc.file_name);
        assert!(on_disk.exists());

        // The filed file carries no orientation tag, so no viewer or PDF library
        // can rotate it a second time.
        let filed = std::fs::read(&on_disk).unwrap();
        assert_eq!(crate::imaging::read_orientation(&filed), 1);

        let (w, h) = {
            let i = image::load_from_memory(&filed).unwrap();
            (i.width(), i.height())
        };
        assert_eq!((w, h), (200, 400), "the filed pixels are upright, not just tagged");
    }

    #[test]
    fn trashing_moves_the_file_and_never_unlinks_it() {
        let f = Fx::new("trash");
        let id = f.stage("jpg", b"a scan worth keeping");
        let doc = f.commit(&id, "2026-03-14", "CBC").unwrap();

        let original = f.vault().join("Rahim-Uddin").join("2026").join(&doc.file_name);
        assert!(original.exists());

        trash(&f.conn, &f.user, &f.vault(), &doc.id).unwrap();

        assert!(!original.exists(), "the file leaves its filed location");
        let trashed = f.vault().join("Trash").join("Rahim-Uddin").join("2026").join(&doc.file_name);
        assert!(trashed.exists(), "and lands in Trash, not oblivion");
        assert_eq!(std::fs::read(&trashed).unwrap(), b"a scan worth keeping");

        // The row survives so the document can be restored.
        let (count, trashed_at): (i64, Option<String>) = f.conn
            .query_row("SELECT count(*), max(trashed_at) FROM documents WHERE id = ?1",
                       params![doc.id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(count, 1);
        assert!(trashed_at.is_some());
    }

    #[test]
    fn a_trashed_document_leaves_the_search_index_and_the_library() {
        let f = Fx::new("trash-index");
        let id = f.stage("jpg", b"x");
        let doc = f.commit(&id, "2026-03-14", "Thyroid Profile").unwrap();
        assert_eq!(crate::search::search(&f.conn, &f.user, "thyroid", 10).unwrap().len(), 1);

        trash(&f.conn, &f.user, &f.vault(), &doc.id).unwrap();

        assert!(crate::search::search(&f.conn, &f.user, "thyroid", 10).unwrap().is_empty());
        assert!(crate::documents::list(&f.conn, &f.user).unwrap().is_empty());
    }

    #[test]
    fn trashing_twice_is_refused_rather_than_moving_a_file_that_is_not_there() {
        let f = Fx::new("trash-twice");
        let id = f.stage("jpg", b"x");
        let doc = f.commit(&id, "2026-03-14", "CBC").unwrap();

        trash(&f.conn, &f.user, &f.vault(), &doc.id).unwrap();
        assert!(trash(&f.conn, &f.user, &f.vault(), &doc.id).unwrap_err().contains("No such document"));
    }

    #[test]
    fn committing_a_missing_staged_file_fails_before_touching_the_database() {
        let f = Fx::new("missing");
        let id = f.stage("jpg", b"x");
        let staged: String = f.conn
            .query_row("SELECT staged_path FROM ingest_items WHERE id = ?1", params![id], |r| r.get(0))
            .unwrap();
        std::fs::remove_file(&staged).unwrap();

        let err = f.commit(&id, "2026-03-14", "CBC").unwrap_err();
        assert!(err.contains("re-import"));

        let docs: i64 = f.conn.query_row("SELECT count(*) FROM documents", [], |r| r.get(0)).unwrap();
        assert_eq!(docs, 0, "no document row may be created for a file that is not there");
    }
}
