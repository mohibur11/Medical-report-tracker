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

/// Refuse a date that cannot be right, saying which value to correct.
///
/// Shared by filing and editing. The review screen checks these too, but it is the
/// wrong place to rely on: a record saved wrong is not recoverable by reading it
/// later.
fn validate_date(
    conn: &Connection,
    doc_date: &str,
    patient_name: &str,
    patient_dob: Option<&str>,
) -> Result<(), String> {
    if !naming::is_valid_doc_date(doc_date) {
        return Err(format!("'{doc_date}' is not a real date. Correct the date before saving."));
    }

    // The unknown sentinel means exactly that, not the year zero, so it is exempt
    // from every comparison below.
    if doc_date.starts_with("0000") {
        return Ok(());
    }

    let today: String = conn
        .query_row("SELECT date('now')", [], |r| r.get(0))
        .unwrap_or_default();
    if !today.is_empty() && doc_date > today.as_str() {
        return Err(format!(
            "{} is in the future. Correct the date before saving.",
            display_dmy(doc_date)
        ));
    }

    if let Some(dob) = patient_dob.filter(|d| naming::is_valid_doc_date(d)) {
        if doc_date < dob {
            return Err(format!(
                "This report is dated {}, before {}'s date of birth ({}). \
                 One of the two is wrong - correct the date, or fix the date of birth \
                 on the patient, before saving.",
                display_dmy(doc_date),
                patient_name,
                display_dmy(dob)
            ));
        }
    }

    Ok(())
}

/// Dates are ISO on disk and day-first everywhere a person reads them.
fn display_dmy(iso: &str) -> String {
    match (iso.get(0..4), iso.get(5..7), iso.get(8..10)) {
        (Some(y), Some(m), Some(d)) => format!("{d}/{m}/{y}"),
        _ => iso.to_string(),
    }
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
    let (staged_path, file_kind, sha256, byte_size, orientation, page_count): (
        Option<String>, String, Option<String>, i64, Option<i64>, Option<i64>,
    ) = conn
        .query_row(
            "SELECT staged_path, file_kind, sha256, byte_size, exif_orientation, page_count
             FROM ingest_items WHERE id = ?1",
            params![req.ingest_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .map_err(|e| format!("no such staged item: {e}"))?;

    let staged = staged_path.ok_or("staged file is missing — re-import this file")?;
    let staged = PathBuf::from(staged);
    if !staged.exists() {
        return Err("staged file no longer exists on disk — re-import this file".into());
    }

    let (patient_name, patient_dob): (String, Option<String>) = conn
        .query_row(
            "SELECT display_name, dob FROM patients WHERE id = ?1",
            params![req.patient_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| format!("no such patient: {e}"))?;

    validate_date(conn, req.doc_date, &patient_name, patient_dob.as_deref())?;

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
         VALUES (?1,?2,?3,?4,'manual',?5,?6,?7,?8,?9,?10,?11,?12, datetime('now'), datetime('now'))",
        params![
            doc_id,
            user_id,
            req.patient_id,
            req.doc_date,
            req.title,
            req.doc_type,
            page_count.unwrap_or(1).max(1),
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

    // Carry any recognised text across, so it becomes searchable with the
    // document rather than being stranded on the staging row.
    let (ocr_text, ocr_json): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT ocr_text, ocr_json FROM ingest_items WHERE id = ?1",
            params![req.ingest_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((None, None));

    // One row per page, so a hit in a twelve-page report can say which page it was
    // on, and so page geometry stays attached to the page it belongs to.
    let per_page: Vec<(i64, String, String)> = ocr_json
        .as_deref()
        .and_then(|j| serde_json::from_str::<Vec<serde_json::Value>>(j).ok())
        .map(|pages| {
            pages
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let page_no = p["pageNo"].as_u64().unwrap_or(i as u64 + 1) as i64;
                    let text = p["text"].as_str().unwrap_or_default().to_string();
                    let words = p["words"].to_string();
                    (page_no, text, words)
                })
                .collect()
        })
        // Older staged rows predate per-page storage; fall back to the flat text.
        .unwrap_or_else(|| {
            ocr_text
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .map(|t| vec![(1i64, t.to_string(), String::new())])
                .unwrap_or_default()
        });

    for (page_no, text, words) in per_page {
        if text.trim().is_empty() {
            continue;
        }
        let _ = conn.execute(
            "INSERT OR REPLACE INTO document_page (document_id, page_no, ocr_text, ocr_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![doc_id, page_no, text, words],
        );
    }

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

/// Borrowed throughout, so passing it twice costs nothing.
#[derive(Clone, Copy)]
pub struct UpdateRequest<'a> {
    pub document_id: &'a str,
    pub patient_id: &'a str,
    pub doc_date: &'a str,
    pub title: &'a str,
    pub doc_type: &'a str,
    /// What the paper does not say: what the doctor advised, what to repeat and
    /// when. Searchable, and carried in the sidecar so it survives the database.
    pub notes: &'a str,
}

/// Change a filed document's details, moving the file if its canonical name or
/// folder changes.
///
/// Editing is not a database-only operation here. The date, the patient and the
/// title are all IN the filename, and the patient and year are the folders, so
/// correcting a typo can mean moving the file across the vault. That move gets the
/// same treatment as filing: intent journalled first, so a crash cannot leave the
/// tree disagreeing with the database.
///
/// The collision suffix is only re-reserved when the base name actually changes.
/// Re-reserving on every edit would burn a suffix each time the user fixed a
/// spelling, and suffixes are never reused.
pub fn update(
    conn: &Connection,
    user_id: &str,
    vault_root: &Path,
    req: UpdateRequest<'_>,
) -> Result<CommittedDocument, String> {
    let (old_rel, file_kind): (String, String) = conn
        .query_row(
            "SELECT rel_path, file_kind FROM documents
             WHERE id = ?1 AND owner_user_id = ?2 AND trashed_at IS NULL",
            params![req.document_id, user_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "No such document.".to_string())?;

    let (patient_name, patient_dob): (String, Option<String>) = conn
        .query_row(
            "SELECT display_name, dob FROM patients WHERE id = ?1 AND owner_user_id = ?2",
            params![req.patient_id, user_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "No such patient.".to_string())?;

    validate_date(conn, req.doc_date, &patient_name, patient_dob.as_deref())?;

    let title = req.title.trim();
    if title.is_empty() {
        return Err("A document needs a title.".into());
    }

    let old_path = vault_root.join(old_rel.replace('\\', "/"));
    let old_name = old_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = old_path
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_else(|| match file_kind.as_str() {
            "pdf" => "pdf".into(),
            _ => "jpg".into(),
        });

    let root_len = vault_root.display().to_string().encode_utf16().count() + 1;
    let probe = naming::build_name(req.doc_date, &patient_name, title, &ext, 1, root_len);
    let dir_rel = format!("{}\\{}", probe.patient_folder, probe.year_folder);
    let base = probe
        .file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem.to_string())
        .unwrap_or_else(|| probe.file_name.clone());

    // Keep the existing suffix when the identity of the slot has not changed.
    let existing = naming::parse_file_name(&old_name);
    let unchanged_slot = existing
        .as_ref()
        .map(|p| {
            p.doc_date == req.doc_date
                && p.patient_slug == probe.patient_folder
                && format!("{}_{}_{}", p.doc_date, p.patient_slug, p.title) == base
        })
        .unwrap_or(false);

    let seq = if unchanged_slot {
        existing.as_ref().map(|p| p.seq).unwrap_or(1)
    } else {
        reserve_seq(conn, &dir_rel, &base)?
    };

    let built = naming::build_name(req.doc_date, &patient_name, title, &ext, seq, root_len);
    let new_path = vault_root.join(built.rel_path.replace('\\', "/"));

    if built.rel_path != old_rel {
        if old_path.exists() {
            let journal_id = journal_intent(conn, "move", &old_path, &new_path)?;
            move_file(&old_path, &new_path)?;
            journal_done(conn, &journal_id)?;
        }
        // The sidecar describes the old location and would otherwise be orphaned.
        crate::backup::remove_sidecar(vault_root, &old_rel);
    }

    let notes = req.notes.trim();
    conn.execute(
        "UPDATE documents
         SET patient_id = ?2, doc_date = ?3, title = ?4, doc_type = ?5, rel_path = ?6,
             notes = ?7, updated_at = datetime('now')
         WHERE id = ?1",
        params![
            req.document_id,
            req.patient_id,
            req.doc_date,
            title,
            req.doc_type,
            built.rel_path,
            // Empty means absent, not an empty string, so a note that was cleared
            // does not linger in the sidecar as "".
            Some(notes).filter(|n| !n.is_empty()),
        ],
    )
    .map_err(|e| format!("cannot update document: {e}"))?;

    let _ = crate::search::index_document(conn, req.document_id);
    let _ = crate::backup::write_sidecar(conn, vault_root, req.document_id);

    Ok(CommittedDocument {
        id: req.document_id.to_string(),
        rel_path: built.rel_path,
        file_name: built.file_name,
        title_truncated: built.title_truncated,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameReport {
    pub display_name: String,
    pub folder_slug: String,
    /// Files that changed place on disk.
    pub moved: usize,
    /// Rows updated whose file was already missing — the reconciler's problem,
    /// not this one's.
    pub missing: usize,
    /// Files that could not be moved, each with the reason. A locked file is the
    /// normal case here: a PDF left open in a viewer, or a sync client holding a
    /// handle while it uploads.
    pub left_behind: Vec<String>,
}

/// Rename a patient, moving every one of their documents to match.
///
/// The patient's name is in three places at once — the folder, every filename,
/// and the database — so this is a bulk file operation, not a text edit. Each
/// document is journalled and moved individually rather than the folder being
/// renamed wholesale, because a folder rename fails entirely if any single file
/// inside it is locked, and leaves nothing behind to say how far it got.
///
/// Partial success is a real outcome and is reported rather than hidden: rows
/// that moved are updated, rows that did not keep pointing at the file that still
/// exists, and running the rename again picks up the stragglers.
pub fn rename_patient(
    conn: &Connection,
    user_id: &str,
    vault_root: &Path,
    patient_id: &str,
    new_name: &str,
    new_dob: Option<&str>,
) -> Result<RenameReport, String> {
    let (_old_name, old_slug): (String, String) = conn
        .query_row(
            "SELECT display_name, folder_slug FROM patients
             WHERE id = ?1 AND owner_user_id = ?2 AND archived_at IS NULL",
            params![patient_id, user_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "No such patient.".to_string())?;

    let name = new_name.trim();
    if name.is_empty() {
        return Err("A patient needs a name.".into());
    }

    let dob = new_dob.map(str::trim).filter(|d| !d.is_empty());
    if let Some(d) = dob {
        if !naming::is_valid_doc_date(d) {
            return Err(format!("'{d}' is not a valid date of birth."));
        }
    }

    if !naming::is_nameable(name) {
        return Err(format!(
            "'{name}' has no characters Windows allows in a folder name. Use letters or digits."
        ));
    }
    let new_slug = naming::patient_slug(name);

    // Same rule as creating a patient: NTFS is case-insensitive, so two names that
    // differ only by case or punctuation would share one folder and interleave.
    let taken: i64 = conn
        .query_row(
            "SELECT count(*) FROM patients
             WHERE owner_user_id = ?1 AND upper(folder_slug) = upper(?2)
               AND id <> ?3 AND archived_at IS NULL",
            params![user_id, new_slug, patient_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if taken > 0 {
        return Err(format!(
            "Another patient already uses the folder '{new_slug}'. Names that differ only by \
             capitalisation or punctuation share one folder on Windows."
        ));
    }

    // A date of birth that is later than a document already filed for this patient
    // would make the library self-contradictory the moment it is saved.
    if let Some(d) = dob {
        let earliest: Option<String> = conn
            .query_row(
                "SELECT min(doc_date) FROM documents
                 WHERE patient_id = ?1 AND trashed_at IS NULL AND doc_date NOT LIKE '0000%'",
                params![patient_id],
                |r| r.get(0),
            )
            .unwrap_or(None);
        if let Some(earliest) = earliest.filter(|e| e.as_str() < d) {
            return Err(format!(
                "{name} already has a report dated {}, which is before this date of birth ({}). \
                 Correct one of the two.",
                display_dmy(&earliest),
                display_dmy(d)
            ));
        }
    }

    // Renamed in the database first, then on disk. The sidecars written during
    // the move read the patient's name from this row, and they are what someone
    // reading the vault without the app has to go on — a sidecar carrying the old
    // name beside a file carrying the new one is worse than either alone.
    conn.execute(
        "UPDATE patients SET display_name = ?2, folder_slug = ?3, dob = ?4,
                             updated_at = datetime('now')
         WHERE id = ?1",
        params![patient_id, name, new_slug, dob],
    )
    .map_err(|e| format!("cannot rename patient: {e}"))?;

    let root_len = vault_root.display().to_string().encode_utf16().count() + 1;
    let mut report = RenameReport {
        display_name: name.to_string(),
        folder_slug: new_slug.clone(),
        moved: 0,
        missing: 0,
        left_behind: Vec::new(),
    };

    let docs: Vec<(String, String, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, rel_path, doc_date, title FROM documents
                 WHERE patient_id = ?1 AND owner_user_id = ?2 AND trashed_at IS NULL
                 ORDER BY rel_path",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![patient_id, user_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
    };

    for (doc_id, old_rel, doc_date, title) in docs {
        let old_path = vault_root.join(old_rel.replace('\\', "/"));
        let old_file = old_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let ext = old_path
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_else(|| "bin".into());

        // Keep the collision suffix the file already has when the slot it would
        // land in is free. Renaming a patient and back again should not stamp
        // `__02` onto every file, and suffixes are never reused.
        let existing_seq = naming::parse_file_name(&old_file).map(|p| p.seq).unwrap_or(1);
        let candidate = naming::build_name(&doc_date, name, &title, &ext, existing_seq, root_len);
        let claimed: i64 = conn
            .query_row(
                "SELECT count(*) FROM documents
                 WHERE rel_path = ?1 AND id <> ?2 AND trashed_at IS NULL",
                params![candidate.rel_path, doc_id],
                |r| r.get(0),
            )
            .unwrap_or(1);
        let candidate_free =
            claimed == 0 && !vault_root.join(candidate.rel_path.replace('\\', "/")).exists();

        let built = if candidate_free || candidate.rel_path == old_rel {
            candidate
        } else {
            let probe = naming::build_name(&doc_date, name, &title, &ext, 1, root_len);
            let dir_rel = format!("{}\\{}", probe.patient_folder, probe.year_folder);
            let base = probe
                .file_name
                .rsplit_once('.')
                .map(|(stem, _)| stem.to_string())
                .unwrap_or_else(|| probe.file_name.clone());
            let seq = reserve_seq(conn, &dir_rel, &base)?;
            naming::build_name(&doc_date, name, &title, &ext, seq, root_len)
        };

        if built.rel_path == old_rel {
            continue;
        }
        let new_path = vault_root.join(built.rel_path.replace('\\', "/"));

        if old_path.exists() {
            let journal_id = journal_intent(conn, "move", &old_path, &new_path)?;
            match move_file(&old_path, &new_path) {
                Ok(()) => {
                    journal_done(conn, &journal_id)?;
                    report.moved += 1;
                }
                Err(e) => {
                    // One locked file must not abandon the other four hundred. The
                    // row keeps pointing at the file that still exists, so nothing
                    // is lost, and running the rename again retries it.
                    mark_failed(conn, &journal_id, &e)?;
                    report.left_behind.push(format!("{old_file} - {e}"));
                    continue;
                }
            }
        } else {
            // Already gone from disk. Move the row anyway so it lines up with the
            // new name; the reconciler is what finds the file again.
            report.missing += 1;
        }

        crate::backup::remove_sidecar(vault_root, &old_rel);
        conn.execute(
            "UPDATE documents SET rel_path = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![doc_id, built.rel_path],
        )
        .map_err(|e| format!("cannot update document path: {e}"))?;
        let _ = crate::backup::write_sidecar(conn, vault_root, &doc_id);
    }

    // Tidy up what the moves emptied. Only ever removes directories that are
    // already empty, so anything the user put there themselves survives.
    if new_slug != old_slug {
        prune_empty_dirs(&vault_root.join(&old_slug));
    }

    Ok(report)
}

/// Remove `dir` and its subdirectories if, and only if, they contain nothing.
fn prune_empty_dirs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            prune_empty_dirs(&entry.path());
        }
    }
    let _ = std::fs::remove_dir(dir);
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

/// A document in the vault's Trash, and where it came from.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashedDocument {
    pub id: String,
    pub title: String,
    pub patient: String,
    pub doc_date: String,
    pub rel_path: String,
    pub trashed_at: String,
    /// False when the file is not in Trash any more — moved or emptied by hand.
    pub recoverable: bool,
}

pub fn list_trashed(
    conn: &Connection,
    user_id: &str,
    vault_root: &Path,
) -> Result<Vec<TrashedDocument>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT d.id, d.title, p.display_name, d.doc_date, d.rel_path, d.trashed_at
               FROM documents d JOIN patients p ON p.id = d.patient_id
              WHERE d.owner_user_id = ?1 AND d.trashed_at IS NOT NULL
              ORDER BY d.trashed_at DESC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![user_id], |r| {
            let rel_path: String = r.get(4)?;
            Ok(TrashedDocument {
                id: r.get(0)?,
                title: r.get(1)?,
                patient: r.get(2)?,
                doc_date: r.get(3)?,
                recoverable: vault_root
                    .join("Trash")
                    .join(rel_path.replace('\\', "/"))
                    .exists(),
                rel_path,
                trashed_at: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Put a trashed document back where it was.
///
/// The exact reverse of `trash`, and journalled the same way. Trash keeps the
/// vault-relative shape of the path, so a restore knows the year folder and the
/// patient folder without having to guess or re-derive them — which matters,
/// because the patient may have been renamed since.
pub fn restore(
    conn: &Connection,
    user_id: &str,
    vault_root: &Path,
    document_id: &str,
) -> Result<(), String> {
    let rel_path: String = conn
        .query_row(
            "SELECT rel_path FROM documents
             WHERE id = ?1 AND owner_user_id = ?2 AND trashed_at IS NOT NULL",
            params![document_id, user_id],
            |r| r.get(0),
        )
        .map_err(|_| "That document is not in the Trash.".to_string())?;

    let from = vault_root.join("Trash").join(rel_path.replace('\\', "/"));
    let to = vault_root.join(rel_path.replace('\\', "/"));

    if to.exists() {
        return Err(format!(
            "Something is already at {rel_path}. Move it aside before restoring this one."
        ));
    }

    if from.exists() {
        let journal_id = journal_intent(conn, "move", &from, &to)?;
        move_file(&from, &to)?;
        journal_done(conn, &journal_id)?;
    } else {
        // The row can still come back — the reconciler is what finds a file that
        // was moved by hand — but the user must not be told the file returned.
        return Err(format!(
            "The file is no longer in the Trash folder. Put it back at Trash\\{rel_path}, \
             or use Rescan vault if you moved it somewhere else."
        ));
    }

    conn.execute(
        "UPDATE documents SET trashed_at = NULL, updated_at = datetime('now') WHERE id = ?1",
        params![document_id],
    )
    .map_err(|e| format!("cannot restore the document: {e}"))?;

    let _ = crate::search::index_document(conn, document_id);
    let _ = crate::backup::write_sidecar(conn, vault_root, document_id);
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

    impl Fx {
        fn rename(&self, name: &str, dob: Option<&str>) -> Result<RenameReport, String> {
            rename_patient(&self.conn, &self.user, &self.vault(), &self.patient, name, dob)
        }

        fn rel_path(&self, doc_id: &str) -> String {
            self.conn
                .query_row(
                    "SELECT rel_path FROM documents WHERE id = ?1",
                    params![doc_id],
                    |r| r.get(0),
                )
                .unwrap()
        }

        fn exists(&self, rel: &str) -> bool {
            self.vault().join(rel.replace('\\', "/")).exists()
        }
    }

    #[test]
    fn a_trashed_document_comes_back_where_it_was() {
        let f = Fx::new("restore");
        let doc = f.commit(&f.stage("jpg", b"the scan"), "2025-03-14", "USG of Thyroid").unwrap();
        let original = f.rel_path(&doc.id);

        trash(&f.conn, &f.user, &f.vault(), &doc.id).unwrap();
        assert!(!f.exists(&original), "trashing moved it out");

        let waiting = list_trashed(&f.conn, &f.user, &f.vault()).unwrap();
        assert_eq!(waiting.len(), 1);
        assert!(waiting[0].recoverable, "the file is sitting in Trash");

        restore(&f.conn, &f.user, &f.vault(), &doc.id).unwrap();

        assert!(f.exists(&original), "and it is back at the same path");
        assert_eq!(
            std::fs::read(f.vault().join(original.replace('\\', "/"))).unwrap(),
            b"the scan",
            "byte for byte",
        );
        assert!(
            list_trashed(&f.conn, &f.user, &f.vault()).unwrap().is_empty(),
            "and no longer counted as deleted",
        );
    }

    #[test]
    fn a_restored_document_is_searchable_again() {
        let f = Fx::new("restoresearch");
        let doc = f.commit(&f.stage("jpg", b"x"), "2025-03-14", "Lipid Profile").unwrap();
        trash(&f.conn, &f.user, &f.vault(), &doc.id).unwrap();
        assert!(crate::search::search(&f.conn, &f.user, "Lipid", 20).unwrap().is_empty());

        restore(&f.conn, &f.user, &f.vault(), &doc.id).unwrap();
        let hits = crate::search::search(&f.conn, &f.user, "Lipid", 20).unwrap();
        assert!(hits.iter().any(|h| h.document_id == doc.id), "it must be findable again");
    }

    #[test]
    fn restoring_refuses_to_overwrite_something_already_there() {
        let f = Fx::new("restoreclash");
        let doc = f.commit(&f.stage("jpg", b"original"), "2025-03-14", "CBC").unwrap();
        let rel = f.rel_path(&doc.id);
        trash(&f.conn, &f.user, &f.vault(), &doc.id).unwrap();

        // Something else has taken the name in the meantime.
        let occupied = f.vault().join(rel.replace('\\', "/"));
        std::fs::create_dir_all(occupied.parent().unwrap()).unwrap();
        std::fs::write(&occupied, b"a different file").unwrap();

        let err = restore(&f.conn, &f.user, &f.vault(), &doc.id).unwrap_err();
        assert!(err.contains("already at"), "{err}");
        assert_eq!(
            std::fs::read(&occupied).unwrap(),
            b"a different file",
            "the file in the way must be left alone",
        );
    }

    #[test]
    fn restoring_a_file_that_left_the_trash_says_where_to_put_it() {
        let f = Fx::new("restoregone");
        let doc = f.commit(&f.stage("jpg", b"x"), "2025-03-14", "CBC").unwrap();
        let rel = f.rel_path(&doc.id);
        trash(&f.conn, &f.user, &f.vault(), &doc.id).unwrap();
        std::fs::remove_file(f.vault().join("Trash").join(rel.replace('\\', "/"))).unwrap();

        let listed = list_trashed(&f.conn, &f.user, &f.vault()).unwrap();
        assert!(!listed[0].recoverable, "the list must not promise what it cannot do");

        let err = restore(&f.conn, &f.user, &f.vault(), &doc.id).unwrap_err();
        assert!(err.contains("Rescan vault"), "{err}");
    }

    #[test]
    fn restoring_something_that_was_never_trashed_is_refused() {
        let f = Fx::new("restorelive");
        let doc = f.commit(&f.stage("jpg", b"x"), "2025-03-14", "CBC").unwrap();
        assert!(restore(&f.conn, &f.user, &f.vault(), &doc.id)
            .unwrap_err()
            .contains("not in the Trash"));
    }

    #[test]
    fn a_note_is_saved_searchable_and_carried_into_the_sidecar() {
        let f = Fx::new("notes");
        let doc = f.commit(&f.stage("jpg", b"x"), "2025-03-14", "Thyroid Profile").unwrap();

        update(
            &f.conn,
            &f.user,
            &f.vault(),
            UpdateRequest {
                document_id: &doc.id,
                patient_id: &f.patient,
                doc_date: "2025-03-14",
                title: "Thyroid Profile",
                doc_type: "report",
                notes: "Dr Karim said repeat in three months",
            },
        )
        .unwrap();

        let stored: Option<String> = f
            .conn
            .query_row("SELECT notes FROM documents WHERE id = ?1", params![doc.id], |r| r.get(0))
            .unwrap();
        assert_eq!(stored.as_deref(), Some("Dr Karim said repeat in three months"));

        // Searchable, because a note nobody can find again is not worth typing.
        let hits = crate::search::search(&f.conn, &f.user, "repeat", 20).unwrap();
        assert!(hits.iter().any(|h| h.document_id == doc.id), "the note should be findable");

        // And in the sidecar, which is what someone reading the vault without the
        // app — or restoring from it — actually has.
        let sidecar = f.vault().join(format!(
            "{}{}",
            f.rel_path(&doc.id).replace('\\', "/"),
            crate::backup::SIDECAR_SUFFIX
        ));
        assert!(std::fs::read_to_string(sidecar).unwrap().contains("repeat in three months"));
    }

    #[test]
    fn clearing_a_note_removes_it_rather_than_storing_emptiness() {
        let f = Fx::new("notesclear");
        let doc = f.commit(&f.stage("jpg", b"x"), "2025-03-14", "CBC").unwrap();
        let mut req = UpdateRequest {
            document_id: &doc.id,
            patient_id: &f.patient,
            doc_date: "2025-03-14",
            title: "CBC",
            doc_type: "report",
            notes: "fasting sample",
        };
        update(&f.conn, &f.user, &f.vault(), req).unwrap();

        req.notes = "   ";
        update(&f.conn, &f.user, &f.vault(), req).unwrap();

        let stored: Option<String> = f
            .conn
            .query_row("SELECT notes FROM documents WHERE id = ?1", params![doc.id], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, None, "whitespace is not a note");
        assert!(
            crate::search::search(&f.conn, &f.user, "fasting", 20).unwrap().is_empty(),
            "and the old note must leave the index with it",
        );
    }

    #[test]
    fn a_note_does_not_move_the_file() {
        let f = Fx::new("notesmove");
        let doc = f.commit(&f.stage("jpg", b"x"), "2025-03-14", "CBC").unwrap();
        let before = f.rel_path(&doc.id);

        update(
            &f.conn,
            &f.user,
            &f.vault(),
            UpdateRequest {
                document_id: &doc.id,
                patient_id: &f.patient,
                doc_date: "2025-03-14",
                title: "CBC",
                doc_type: "report",
                notes: "anything at all",
            },
        )
        .unwrap();

        assert_eq!(f.rel_path(&doc.id), before, "notes are not part of the filename");
        assert!(f.exists(&before));
    }

    #[test]
    fn renaming_a_patient_moves_every_file_they_own() {
        let f = Fx::new("rename");
        let a = f.commit(&f.stage("jpg", b"one"), "2025-03-14", "Thyroid Profile").unwrap();
        let b = f.commit(&f.stage("pdf", b"two"), "2024-11-02", "Lipid Profile").unwrap();
        assert!(f.exists(&a.rel_path) && f.exists(&b.rel_path));

        let report = f.rename("Rahim Uddin Ahmed", None).unwrap();
        assert_eq!(report.moved, 2);
        assert_eq!(report.folder_slug, "Rahim-Uddin-Ahmed");
        assert!(report.left_behind.is_empty());

        // The name is in the folder AND in every filename.
        for id in [&a.id, &b.id] {
            let rel = f.rel_path(id);
            assert!(rel.starts_with("Rahim-Uddin-Ahmed\\"), "folder should change: {rel}");
            assert!(rel.contains("_Rahim-Uddin-Ahmed_"), "filename should change: {rel}");
            assert!(f.exists(&rel), "the file must be where the row says: {rel}");
        }

        assert!(!f.exists(&a.rel_path), "nothing may be left at the old path");
        assert!(
            !f.vault().join("Rahim-Uddin").exists(),
            "the emptied folder should not linger",
        );
    }

    #[test]
    fn the_bytes_survive_the_move() {
        let f = Fx::new("bytes");
        let doc = f.commit(&f.stage("jpg", b"exact contents"), "2025-03-14", "CBC").unwrap();
        f.rename("Karim Uddin", None).unwrap();
        let moved = f.vault().join(f.rel_path(&doc.id).replace('\\', "/"));
        assert_eq!(std::fs::read(moved).unwrap(), b"exact contents");
    }

    #[test]
    fn a_name_another_patient_already_folds_onto_is_refused() {
        let f = Fx::new("clash");
        f.conn
            .execute(
                "INSERT INTO patients (id, owner_user_id, display_name, folder_slug, created_at, updated_at)
                 VALUES ('other', ?1, 'Karim Uddin', 'Karim-Uddin', datetime('now'), datetime('now'))",
                params![f.user],
            )
            .unwrap();
        let doc = f.commit(&f.stage("jpg", b"x"), "2025-03-14", "CBC").unwrap();
        let before = f.rel_path(&doc.id);

        // Differs only by case and punctuation, so Windows would merge the folders.
        let err = f.rename("karim/uddin", None).unwrap_err();
        assert!(err.contains("already uses the folder"), "{err}");

        assert_eq!(f.rel_path(&doc.id), before, "a refused rename moves nothing");
        assert!(f.exists(&before));
    }

    #[test]
    fn an_empty_name_is_refused() {
        let f = Fx::new("emptyname");
        assert!(f.rename("   ", None).unwrap_err().contains("needs a name"));
    }

    #[test]
    fn a_name_windows_cannot_spell_is_refused_rather_than_silently_emptied() {
        let f = Fx::new("unspellable");
        let err = f.rename("///", None).unwrap_err();
        assert!(err.contains("no characters Windows allows"), "{err}");
    }

    #[test]
    fn correcting_the_date_of_birth_moves_nothing() {
        let f = Fx::new("dob");
        let doc = f.commit(&f.stage("jpg", b"x"), "2025-03-14", "CBC").unwrap();
        let before = f.rel_path(&doc.id);

        let report = f.rename("Rahim Uddin", Some("1978-03-12")).unwrap();
        assert_eq!(report.moved, 0, "the name did not change, so no file should");
        assert_eq!(f.rel_path(&doc.id), before);

        let dob: Option<String> = f
            .conn
            .query_row("SELECT dob FROM patients WHERE id = ?1", params![f.patient], |r| r.get(0))
            .unwrap();
        assert_eq!(dob.as_deref(), Some("1978-03-12"));
    }

    #[test]
    fn a_date_of_birth_after_an_existing_report_is_refused() {
        let f = Fx::new("dobclash");
        f.commit(&f.stage("jpg", b"x"), "2020-05-06", "CBC").unwrap();
        // Saving this would make the library contradict itself: a report filed
        // before the patient was born.
        let err = f.rename("Rahim Uddin", Some("2021-01-01")).unwrap_err();
        assert!(err.contains("before this date of birth"), "{err}");
        assert!(err.contains("06/05/2020"), "it should name the report that disagrees: {err}");
    }

    #[test]
    fn an_invalid_date_of_birth_is_refused() {
        let f = Fx::new("dobbad");
        assert!(f.rename("Rahim Uddin", Some("12/03/1978")).unwrap_err().contains("not a valid"));
    }

    #[test]
    fn renaming_back_does_not_stamp_a_suffix_onto_every_file() {
        let f = Fx::new("roundtrip");
        let doc = f.commit(&f.stage("jpg", b"x"), "2025-03-14", "CBC").unwrap();
        let original = f.rel_path(&doc.id);

        f.rename("Karim Uddin", None).unwrap();
        f.rename("Rahim Uddin", None).unwrap();

        assert_eq!(
            f.rel_path(&doc.id),
            original,
            "a round trip should land back on the same name, not CBC__02",
        );
        assert!(f.exists(&original));
    }

    #[test]
    fn a_file_already_gone_from_disk_still_gets_its_row_moved() {
        let f = Fx::new("missing");
        let doc = f.commit(&f.stage("jpg", b"x"), "2025-03-14", "CBC").unwrap();
        std::fs::remove_file(f.vault().join(doc.rel_path.replace('\\', "/"))).unwrap();

        let report = f.rename("Karim Uddin", None).unwrap();
        assert_eq!(report.moved, 0);
        assert_eq!(report.missing, 1, "reported, not silently counted as moved");
        assert!(
            f.rel_path(&doc.id).contains("Karim-Uddin"),
            "the row should still line up with the new name",
        );
    }

    #[test]
    fn a_rename_leaves_the_journal_clean() {
        let f = Fx::new("journal");
        f.commit(&f.stage("jpg", b"one"), "2025-03-14", "CBC").unwrap();
        f.commit(&f.stage("pdf", b"two"), "2025-04-01", "ESR").unwrap();
        f.rename("Karim Uddin", None).unwrap();

        let pending: i64 = f
            .conn
            .query_row(
                "SELECT count(*) FROM fs_journal WHERE applied_at IS NULL AND failed_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0, "every move must be closed out");
        assert_eq!(replay_journal(&f.conn).unwrap(), 0, "nothing left to replay");
    }

    #[test]
    fn sidecars_follow_the_files() {
        let f = Fx::new("sidecar");
        let doc = f.commit(&f.stage("jpg", b"x"), "2025-03-14", "CBC").unwrap();
        crate::backup::write_sidecar(&f.conn, &f.vault(), &doc.id).unwrap();
        let old_sidecar = f.vault().join(format!("{}{}", doc.rel_path.replace('\\', "/"), crate::backup::SIDECAR_SUFFIX));
        assert!(old_sidecar.exists());

        f.rename("Karim Uddin", None).unwrap();

        assert!(!old_sidecar.exists(), "the old sidecar would describe a file that moved");
        let new_sidecar = f
            .vault()
            .join(format!("{}{}", f.rel_path(&doc.id).replace('\\', "/"), crate::backup::SIDECAR_SUFFIX));
        assert!(new_sidecar.exists(), "and a new one should sit beside the moved file");
        let json = std::fs::read_to_string(new_sidecar).unwrap();
        assert!(json.contains("Karim Uddin"), "the sidecar should carry the new name");
    }

    #[test]
    fn a_folder_holding_anything_else_is_left_alone() {
        let f = Fx::new("keepdir");
        f.commit(&f.stage("jpg", b"x"), "2025-03-14", "CBC").unwrap();
        // Something the user put there themselves.
        let stray = f.vault().join("Rahim-Uddin").join("notes.txt");
        std::fs::write(&stray, b"my own notes").unwrap();

        f.rename("Karim Uddin", None).unwrap();

        assert!(stray.exists(), "pruning empty folders must never take a file with it");
    }

    #[test]
    fn trashed_documents_are_not_dragged_back_out() {
        let f = Fx::new("trashed");
        let doc = f.commit(&f.stage("jpg", b"x"), "2025-03-14", "CBC").unwrap();
        trash(&f.conn, &f.user, &f.vault(), &doc.id).unwrap();

        let report = f.rename("Karim Uddin", None).unwrap();
        assert_eq!(report.moved, 0);
        assert!(
            f.vault().join("Trash").join(doc.rel_path.replace('\\', "/")).exists(),
            "a trashed file stays in Trash under the name it was trashed with",
        );
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
    fn editing_a_title_renames_the_file_in_place() {
        let f = Fx::new("edit-title");
        let id = f.stage("jpg", b"scan");
        let doc = f.commit(&id, "2026-03-14", "CBC").unwrap();
        let before = f.vault().join("Rahim-Uddin").join("2026").join(&doc.file_name);
        assert!(before.exists());

        let updated = update(&f.conn, &f.user, &f.vault(), UpdateRequest {
            document_id: &doc.id,
            patient_id: &f.patient,
            doc_date: "2026-03-14",
            title: "Complete Blood Count",
            doc_type: "report",
            notes: "",
        }).unwrap();

        assert_eq!(updated.file_name, "2026-03-14_Rahim-Uddin_Complete-Blood-Count.jpg");
        assert!(!before.exists(), "the old name must not linger");
        let after = f.vault().join("Rahim-Uddin").join("2026").join(&updated.file_name);
        assert_eq!(std::fs::read(&after).unwrap(), b"scan", "same bytes, new name");
    }

    #[test]
    fn correcting_the_date_moves_the_file_to_the_right_year() {
        let f = Fx::new("edit-date");
        let id = f.stage("jpg", b"x");
        let doc = f.commit(&id, "2026-03-14", "CBC").unwrap();

        let updated = update(&f.conn, &f.user, &f.vault(), UpdateRequest {
            document_id: &doc.id,
            patient_id: &f.patient,
            doc_date: "2024-11-28",
            title: "CBC",
            doc_type: "report",
            notes: "",
        }).unwrap();

        assert!(updated.rel_path.contains("2024"), "got {}", updated.rel_path);
        assert!(f.vault().join("Rahim-Uddin").join("2024").join(&updated.file_name).exists());
        assert!(!f.vault().join("Rahim-Uddin").join("2026").join(&doc.file_name).exists());
    }

    #[test]
    fn moving_a_document_to_another_patient_moves_its_folder() {
        let f = Fx::new("edit-patient");
        let other = ulid::Ulid::new().to_string();
        f.conn.execute(
            "INSERT INTO patients (id, owner_user_id, display_name, folder_slug, created_at, updated_at)
             VALUES (?1, ?2, 'Karim Uddin', 'Karim-Uddin', datetime('now'), datetime('now'))",
            params![other, f.user],
        ).unwrap();

        let id = f.stage("jpg", b"x");
        let doc = f.commit(&id, "2026-03-14", "CBC").unwrap();

        let updated = update(&f.conn, &f.user, &f.vault(), UpdateRequest {
            document_id: &doc.id,
            patient_id: &other,
            doc_date: "2026-03-14",
            title: "CBC",
            doc_type: "report",
            notes: "",
        }).unwrap();

        assert_eq!(updated.file_name, "2026-03-14_Karim-Uddin_CBC.jpg");
        assert!(f.vault().join("Karim-Uddin").join("2026").join(&updated.file_name).exists());
        assert!(!f.vault().join("Rahim-Uddin").join("2026").join(&doc.file_name).exists());
    }

    #[test]
    fn editing_nothing_does_not_burn_a_collision_suffix() {
        // Re-reserving on every save would append __02, __03, __04 each time the
        // user corrected a spelling, and suffixes are never reused.
        let f = Fx::new("edit-idempotent");
        let id = f.stage("jpg", b"x");
        let doc = f.commit(&id, "2026-03-14", "CBC").unwrap();

        for _ in 0..3 {
            let u = update(&f.conn, &f.user, &f.vault(), UpdateRequest {
                document_id: &doc.id,
                patient_id: &f.patient,
                doc_date: "2026-03-14",
                title: "CBC",
                doc_type: "report",
                notes: "",
            }).unwrap();
            assert_eq!(u.file_name, doc.file_name, "unchanged details must not rename");
        }
    }

    #[test]
    fn an_edit_that_would_be_wrong_is_refused_and_changes_nothing() {
        let f = Fx::new("edit-refused");
        f.conn.execute("UPDATE patients SET dob = '1992-03-09' WHERE id = ?1", params![f.patient]).unwrap();
        let id = f.stage("jpg", b"x");
        let doc = f.commit(&id, "2026-03-14", "CBC").unwrap();

        let err = update(&f.conn, &f.user, &f.vault(), UpdateRequest {
            document_id: &doc.id,
            patient_id: &f.patient,
            doc_date: "1985-01-01",
            title: "CBC",
            doc_type: "report",
            notes: "",
        }).unwrap_err();
        assert!(err.contains("before"), "got: {err}");

        let still: String = f.conn
            .query_row("SELECT doc_date FROM documents WHERE id = ?1", params![doc.id], |r| r.get(0))
            .unwrap();
        assert_eq!(still, "2026-03-14", "a refused edit must leave the record alone");
        assert!(f.vault().join("Rahim-Uddin").join("2026").join(&doc.file_name).exists());
    }

    #[test]
    fn an_edited_document_stays_searchable_under_its_new_title() {
        let f = Fx::new("edit-search");
        let id = f.stage("jpg", b"x");
        let doc = f.commit(&id, "2026-03-14", "CBC").unwrap();

        update(&f.conn, &f.user, &f.vault(), UpdateRequest {
            document_id: &doc.id,
            patient_id: &f.patient,
            doc_date: "2026-03-14",
            title: "Thyroid Profile",
            doc_type: "report",
            notes: "",
        }).unwrap();

        assert_eq!(crate::search::search(&f.conn, &f.user, "thyroid", 10).unwrap().len(), 1);
        assert!(crate::search::search(&f.conn, &f.user, "CBC", 10).unwrap().is_empty());
    }

    #[test]
    fn a_blank_title_is_refused() {
        let f = Fx::new("edit-blank");
        let id = f.stage("jpg", b"x");
        let doc = f.commit(&id, "2026-03-14", "CBC").unwrap();
        let err = update(&f.conn, &f.user, &f.vault(), UpdateRequest {
            document_id: &doc.id,
            patient_id: &f.patient,
            doc_date: "2026-03-14",
            title: "   ",
            doc_type: "report",
            notes: "",
        }).unwrap_err();
        assert!(err.contains("needs a title"), "got: {err}");
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
    fn a_report_dated_before_the_patient_was_born_is_refused_with_advice() {
        let f = Fx::new("before-birth");
        f.conn.execute("UPDATE patients SET dob = '1992-03-09' WHERE id = ?1", params![f.patient]).unwrap();

        let id = f.stage("jpg", b"x");
        let err = f.commit(&id, "1985-06-01", "CBC").unwrap_err();

        assert!(err.contains("before"), "got: {err}");
        assert!(err.contains("Rahim Uddin"), "must name the patient: {err}");
        assert!(err.contains("09/03/1992"), "must show the DOB day-first: {err}");
        assert!(err.contains("correct the date"), "must say what to fix: {err}");

        let docs: i64 = f.conn.query_row("SELECT count(*) FROM documents", [], |r| r.get(0)).unwrap();
        assert_eq!(docs, 0, "nothing may be filed");
    }

    #[test]
    fn a_future_date_is_refused() {
        let f = Fx::new("future");
        let id = f.stage("jpg", b"x");
        let err = f.commit(&id, "2999-01-01", "CBC").unwrap_err();
        assert!(err.contains("in the future"), "got: {err}");
        assert!(err.contains("Correct the date"), "got: {err}");
    }

    #[test]
    fn a_date_that_is_not_a_date_is_refused() {
        let f = Fx::new("notadate");
        let id = f.stage("jpg", b"x");
        let err = f.commit(&id, "2026-02-31", "CBC").unwrap_err();
        assert!(err.contains("not a real date"), "got: {err}");
    }

    #[test]
    fn an_undated_document_is_still_allowed_past_the_birth_check() {
        // '0000-00-00' means "unknown", not "the year zero" — it must not be
        // compared against a date of birth.
        let f = Fx::new("undated-ok");
        f.conn.execute("UPDATE patients SET dob = '1992-03-09' WHERE id = ?1", params![f.patient]).unwrap();
        let id = f.stage("jpg", b"x");
        assert!(f.commit(&id, "0000-00-00", "Prescription").is_ok());
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
