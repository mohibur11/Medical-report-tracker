//! Reconciling the database against the vault on disk.
//!
//! The vault is deliberately browsable, which means the user WILL open it in
//! Explorer and rename, move, delete and drop files there. An app that treats
//! that as corruption feels broken within a month.
//!
//! So this is a repair pass, not a validation pass:
//!   * fast path — a row whose file is where it should be, matched on path, size
//!     and mtime with no hashing at all
//!   * relink — a row whose file moved, found again by content hash
//!   * adopt — a file with a canonical name that no row claims
//!   * missing — a row whose file cannot be found. NEVER auto-purged: a file
//!     moved to a USB stick is not a deletion.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::naming;

/// Written by the export builder; not part of the archive.
const IGNORED_DIRS: &[&str] = &["Exports", "Trash", ".trash"];

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileReport {
    /// Rows whose file is exactly where the database says.
    pub ok: usize,
    /// Rows whose file was found elsewhere and repointed.
    pub relinked: Vec<String>,
    /// Files on disk that no row claimed, now adopted as documents.
    pub adopted: Vec<String>,
    /// Rows whose file could not be found. Flagged, never deleted.
    pub missing: Vec<String>,
    /// Files that are not ours — an unrecognised name, so left strictly alone.
    pub unknown: Vec<String>,
    /// Rows previously flagged missing whose file came back.
    pub recovered: Vec<String>,
}

fn sha256_file(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if IGNORED_DIRS.iter().any(|d| d.eq_ignore_ascii_case(&name)) {
                continue;
            }
            walk(&path, out);
        } else if path.is_file() {
            let name = entry.file_name().to_string_lossy().to_string();
            // Our own bookkeeping, not archive content: metadata sidecars and the
            // database snapshot. Reporting them as strays would be noise.
            if name.ends_with(crate::backup::SIDECAR_SUFFIX)
                || name.starts_with(crate::backup::SNAPSHOT_NAME)
            {
                continue;
            }
            out.push(path);
        }
    }
}

struct Row {
    id: String,
    rel_path: String,
    sha256: String,
    byte_size: i64,
    title: String,
    missing: bool,
}

/// Compare the vault with the database and repair what can be repaired.
pub fn run(conn: &mut Connection, user_id: &str, vault_root: &Path) -> Result<ReconcileReport, String> {
    let mut report = ReconcileReport::default();

    let rows: Vec<Row> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, rel_path, sha256, byte_size, title, missing_at
                 FROM documents WHERE owner_user_id = ?1 AND trashed_at IS NULL",
            )
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map(params![user_id], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    rel_path: r.get(1)?,
                    sha256: r.get(2)?,
                    byte_size: r.get(3)?,
                    title: r.get(4)?,
                    missing: r.get::<_, Option<String>>(5)?.is_some(),
                })
            })
            .map_err(|e| e.to_string())?;
        mapped.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
    };

    let mut on_disk = Vec::new();
    walk(vault_root, &mut on_disk);

    // Index disk files by their vault-relative path, using backslashes to match
    // how rel_path is stored.
    let mut by_rel: HashMap<String, PathBuf> = HashMap::new();
    for path in &on_disk {
        if let Ok(rel) = path.strip_prefix(vault_root) {
            by_rel.insert(rel.to_string_lossy().replace('/', "\\"), path.clone());
        }
    }

    let mut claimed: Vec<PathBuf> = Vec::new();
    let mut unresolved: Vec<&Row> = Vec::new();

    for row in &rows {
        match by_rel.get(&row.rel_path) {
            Some(path) => {
                // Fast path: no hashing. Size is a cheap sanity check; a changed
                // size means the file was replaced, which is worth re-hashing.
                let size_ok = std::fs::metadata(path).map(|m| m.len() as i64 == row.byte_size).unwrap_or(false);
                claimed.push(path.clone());
                if size_ok {
                    report.ok += 1;
                    if row.missing {
                        clear_missing(conn, &row.id)?;
                        report.recovered.push(row.title.clone());
                    }
                } else {
                    // Same name, different bytes — trust the disk and re-record.
                    if let Some(hash) = sha256_file(path) {
                        let size = std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
                        conn.execute(
                            "UPDATE documents SET sha256 = ?2, byte_size = ?3, missing_at = NULL,
                                                  updated_at = datetime('now') WHERE id = ?1",
                            params![row.id, hash, size],
                        ).map_err(|e| e.to_string())?;
                    }
                    report.ok += 1;
                }
            }
            None => unresolved.push(row),
        }
    }

    // Only now hash, and only the files nobody claimed — this is what keeps a
    // rescan fast on a library of a few thousand documents.
    let orphans: Vec<PathBuf> = on_disk.iter().filter(|p| !claimed.contains(p)).cloned().collect();
    let mut orphan_hashes: HashMap<String, PathBuf> = HashMap::new();
    for path in &orphans {
        if let Some(hash) = sha256_file(path) {
            orphan_hashes.entry(hash).or_insert_with(|| path.clone());
        }
    }

    let mut adopted_paths: Vec<PathBuf> = Vec::new();

    for row in unresolved {
        if let Some(found) = orphan_hashes.get(&row.sha256) {
            let rel = found
                .strip_prefix(vault_root)
                .map(|r| r.to_string_lossy().replace('/', "\\"))
                .unwrap_or_default();
            conn.execute(
                "UPDATE documents SET rel_path = ?2, missing_at = NULL, updated_at = datetime('now')
                 WHERE id = ?1",
                params![row.id, rel],
            ).map_err(|e| e.to_string())?;
            adopted_paths.push(found.clone());
            report.relinked.push(format!("{} → {rel}", row.title));
        } else {
            conn.execute(
                "UPDATE documents SET missing_at = coalesce(missing_at, datetime('now')),
                                      updated_at = datetime('now') WHERE id = ?1",
                params![row.id],
            ).map_err(|e| e.to_string())?;
            report.missing.push(row.title.clone());
        }
    }

    // Whatever is left is a file the database has never seen.
    for path in orphans {
        if adopted_paths.contains(&path) {
            continue;
        }
        let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match naming::parse_file_name(&file_name) {
            Some(parsed) => match adopt(conn, user_id, vault_root, &path, &parsed) {
                Ok(title) => report.adopted.push(title),
                Err(e) => report.unknown.push(format!("{file_name} — {e}")),
            },
            // Not ours. Leave it exactly where it is and say so.
            None => report.unknown.push(file_name),
        }
    }

    Ok(report)
}

fn clear_missing(conn: &Connection, id: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE documents SET missing_at = NULL, updated_at = datetime('now') WHERE id = ?1",
        params![id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Create a document row for a file that already carries a canonical name.
///
/// Two sources, in order: the sidecar written beside the file, which carries what
/// a filename cannot — the person's name as typed, the notes, the tags — and the
/// filename itself, which carries date, patient and title and is why a vault
/// restored with no database at all is still a library.
fn adopt(
    conn: &Connection,
    user_id: &str,
    vault_root: &Path,
    path: &Path,
    parsed: &naming::ParsedName,
) -> Result<String, String> {
    let side = read_sidecar(path);

    // The person as they were written down, or the folder name with its hyphens
    // turned back into spaces.
    let display = side
        .as_ref()
        .map(|s| s.patient.clone())
        .unwrap_or_else(|| parsed.patient_slug.replace('-', " "));
    let patient_id = patient_for(conn, user_id, &parsed.patient_slug, &display)?;

    let rel = path
        .strip_prefix(vault_root)
        .map(|r| r.to_string_lossy().replace('/', "\\"))
        .map_err(|_| "file is outside the vault".to_string())?;

    let size = std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
    let hash = sha256_file(path).unwrap_or_default();
    let kind = match parsed.ext.as_str() {
        "pdf" => "pdf",
        "png" => "png",
        _ => "jpeg",
    };

    // The title is a slug on disk; restore the spaces a person would have typed.
    let title = side
        .as_ref()
        .map(|s| s.title.clone())
        .unwrap_or_else(|| parsed.title.replace('-', " "));
    let doc_type = side.as_ref().map_or("report", |s| s.doc_type.as_str()).to_string();
    let doc_date = side.as_ref().map_or(parsed.doc_date.as_str(), |s| s.doc_date.as_str());
    let notes = side.as_ref().and_then(|s| s.notes.clone());
    let id = ulid::Ulid::new().to_string();

    conn.execute(
        "INSERT INTO documents
           (id, owner_user_id, patient_id, doc_date, date_source, title, doc_type, page_count,
            rel_path, sha256, byte_size, file_kind, notes, created_at, updated_at)
         VALUES (?1,?2,?3,?4,'manual',?5,?6,1,?7,?8,?9,?10,?11, datetime('now'), datetime('now'))",
        params![id, user_id, patient_id, doc_date, title, doc_type, rel, hash, size, kind, notes],
    )
    .map_err(|e| format!("cannot adopt: {e}"))?;

    if let Some(side) = &side {
        restore_tags(conn, user_id, &id, &side.categories);
    }

    let _ = crate::search::index_document(conn, &id);
    Ok(title)
}

/// What was written beside this file when it was filed, if it is still there.
fn read_sidecar(path: &Path) -> Option<crate::backup::Sidecar> {
    let mut name = path.file_name()?.to_string_lossy().to_string();
    name.push_str(crate::backup::SIDECAR_SUFFIX);
    let raw = std::fs::read_to_string(path.with_file_name(name)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// The patient this folder belongs to, created if this is the first sight of them.
///
/// A restored phone has the files and no database, so refusing to adopt anything
/// until somebody re-types the names by hand would make the backup useless
/// exactly when it is needed. The folder is the record of who: it was written by
/// this app, from a name a person typed.
fn patient_for(
    conn: &Connection,
    user_id: &str,
    slug: &str,
    display_name: &str,
) -> Result<String, String> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM patients
             WHERE owner_user_id = ?1 AND upper(folder_slug) = upper(?2) AND archived_at IS NULL",
            params![user_id, slug],
            |r| r.get(0),
        )
        .ok();
    if let Some(id) = existing {
        return Ok(id);
    }

    let created = crate::patients::create(conn, user_id, display_name, None)
        .map_err(|e| format!("cannot restore the person '{display_name}': {e}"))?;

    // The folder on disk is the authority. A display name that slugifies to
    // something else would strand every other file in the same folder.
    conn.execute(
        "UPDATE patients SET folder_slug = ?2 WHERE id = ?1",
        params![created.id, slug],
    )
    .map_err(|e| e.to_string())?;

    Ok(created.id)
}

/// Put back the tags the sidecar recorded, creating any that no longer exist.
fn restore_tags(conn: &Connection, user_id: &str, document_id: &str, names: &[String]) {
    for name in names {
        let id: Option<String> = conn
            .query_row(
                "SELECT id FROM categories WHERE owner_user_id = ?1 AND upper(name) = upper(?2)",
                params![user_id, name],
                |r| r.get(0),
            )
            .ok();

        let id = match id {
            Some(id) => id,
            None => match crate::categories::create(conn, user_id, name, None) {
                Ok(c) => c.id,
                Err(_) => continue,
            },
        };

        let _ = conn.execute(
            "INSERT OR IGNORE INTO document_category (document_id, category_id, created_at)
             VALUES (?1, ?2, datetime('now'))",
            params![document_id, id],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fx {
        dir: PathBuf,
        conn: Connection,
        user: String,
    }

    impl Fx {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("mrt-recon-{}", ulid::Ulid::new()));
            std::fs::create_dir_all(dir.join("vault")).unwrap();
            let conn = crate::db::open(&dir.join("app.db")).unwrap();
            let user = crate::db::ensure_user(&conn).unwrap();
            conn.execute(
                "INSERT INTO patients (id, owner_user_id, display_name, folder_slug, created_at, updated_at)
                 VALUES ('pat1', ?1, 'Rahim Uddin', 'Rahim-Uddin', datetime('now'), datetime('now'))",
                params![user],
            ).unwrap();
            Fx { dir, conn, user }
        }

        fn vault(&self) -> PathBuf { self.dir.join("vault") }

        /// Write a file into the vault and record it, as a normal commit would.
        fn filed(&self, year: &str, name: &str, bytes: &[u8]) -> String {
            let rel = format!("Rahim-Uddin\\{year}\\{name}");
            let abs = self.vault().join("Rahim-Uddin").join(year).join(name);
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::fs::write(&abs, bytes).unwrap();

            let mut h = Sha256::new();
            h.update(bytes);
            let hash: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();

            let id = ulid::Ulid::new().to_string();
            self.conn.execute(
                "INSERT INTO documents (id, owner_user_id, patient_id, doc_date, date_source, title,
                                        doc_type, page_count, rel_path, sha256, byte_size, file_kind,
                                        created_at, updated_at)
                 VALUES (?1,?2,'pat1','2026-03-14','manual',?3,'report',1,?4,?5,?6,'jpeg',
                         datetime('now'), datetime('now'))",
                params![id, self.user, name, rel, hash, bytes.len() as i64],
            ).unwrap();
            id
        }

        fn run(&mut self) -> ReconcileReport {
            let vault = self.vault();
            run(&mut self.conn, &self.user.clone(), &vault).unwrap()
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
    }

    #[test]
    fn an_untouched_vault_reports_everything_ok_and_changes_nothing() {
        let mut f = Fx::new();
        f.filed("2026", "2026-03-14_Rahim-Uddin_CBC.jpg", b"a");
        f.filed("2026", "2026-03-15_Rahim-Uddin_TSH.jpg", b"b");

        let r = f.run();
        assert_eq!(r.ok, 2);
        assert!(r.missing.is_empty() && r.adopted.is_empty() && r.relinked.is_empty());
    }

    #[test]
    fn a_file_moved_in_explorer_is_found_again_by_content() {
        let mut f = Fx::new();
        let id = f.filed("2026", "2026-03-14_Rahim-Uddin_CBC.jpg", b"unique bytes");

        // User drags it into the wrong year folder.
        let from = f.vault().join("Rahim-Uddin").join("2026").join("2026-03-14_Rahim-Uddin_CBC.jpg");
        let to_dir = f.vault().join("Rahim-Uddin").join("2025");
        std::fs::create_dir_all(&to_dir).unwrap();
        std::fs::rename(&from, to_dir.join("2026-03-14_Rahim-Uddin_CBC.jpg")).unwrap();

        let r = f.run();
        assert_eq!(r.relinked.len(), 1, "must follow the file, not declare it lost");
        assert!(r.missing.is_empty());
        assert!(r.adopted.is_empty(), "the moved file must not also be adopted as a new document");

        let rel: String = f.conn
            .query_row("SELECT rel_path FROM documents WHERE id = ?1", params![id], |r| r.get(0)).unwrap();
        assert_eq!(rel, "Rahim-Uddin\\2025\\2026-03-14_Rahim-Uddin_CBC.jpg");
    }

    #[test]
    fn a_deleted_file_is_flagged_and_never_purged() {
        let mut f = Fx::new();
        let id = f.filed("2026", "2026-03-14_Rahim-Uddin_CBC.jpg", b"x");
        std::fs::remove_file(f.vault().join("Rahim-Uddin").join("2026").join("2026-03-14_Rahim-Uddin_CBC.jpg")).unwrap();

        let r = f.run();
        assert_eq!(r.missing.len(), 1);

        let (count, missing): (i64, Option<String>) = f.conn
            .query_row("SELECT count(*), max(missing_at) FROM documents WHERE id = ?1", params![id],
                       |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(count, 1, "a file moved to a USB stick is not a deletion");
        assert!(missing.is_some());
    }

    #[test]
    fn a_file_that_comes_back_clears_its_missing_flag() {
        let mut f = Fx::new();
        let id = f.filed("2026", "2026-03-14_Rahim-Uddin_CBC.jpg", b"x");
        let path = f.vault().join("Rahim-Uddin").join("2026").join("2026-03-14_Rahim-Uddin_CBC.jpg");

        std::fs::remove_file(&path).unwrap();
        assert_eq!(f.run().missing.len(), 1);

        std::fs::write(&path, b"x").unwrap();
        let r = f.run();
        assert_eq!(r.recovered.len(), 1);

        let missing: Option<String> = f.conn
            .query_row("SELECT missing_at FROM documents WHERE id = ?1", params![id], |r| r.get(0)).unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn a_canonically_named_file_dropped_into_a_folder_is_adopted() {
        let mut f = Fx::new();
        let dir = f.vault().join("Rahim-Uddin").join("2025");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2025-07-04_Rahim-Uddin_Chest-X-Ray.jpg"), b"dropped").unwrap();

        let r = f.run();
        assert_eq!(r.adopted, vec!["Chest X Ray"]);

        let (title, date): (String, String) = f.conn
            .query_row("SELECT title, doc_date FROM documents", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(title, "Chest X Ray");
        assert_eq!(date, "2025-07-04");

        // And it is immediately searchable.
        assert_eq!(crate::search::search(&f.conn, &f.user, "x-ray", 10).unwrap().len(), 1);
    }

    #[test]
    fn a_foreign_file_is_reported_and_left_strictly_alone() {
        let mut f = Fx::new();
        let dir = f.vault().join("Rahim-Uddin").join("2026");
        std::fs::create_dir_all(&dir).unwrap();
        let stray = dir.join("IMG_4821.jpg");
        std::fs::write(&stray, b"holiday photo").unwrap();

        let r = f.run();
        assert_eq!(r.unknown, vec!["IMG_4821.jpg"]);
        assert!(stray.exists(), "a file we do not understand must never be touched");

        let docs: i64 = f.conn.query_row("SELECT count(*) FROM documents", [], |r| r.get(0)).unwrap();
        assert_eq!(docs, 0);
    }

    #[test]
    fn a_folder_for_a_person_nobody_has_heard_of_brings_them_back() {
        // What a restore looks like: files on disk, and a database that has never
        // seen any of them. Refusing until somebody re-types the names by hand
        // would make the backup useless at the one moment it is needed.
        let mut f = Fx::new();
        let dir = f.vault().join("Someone-Else").join("2026");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2026-03-14_Someone-Else_CBC.jpg"), b"x").unwrap();
        std::fs::write(dir.join("2026-04-02_Someone-Else_ESR.jpg"), b"y").unwrap();

        let r = f.run();
        assert_eq!(r.adopted.len(), 2, "{:?}", r.unknown);
        assert!(r.unknown.is_empty());

        let created: i64 = f
            .conn
            .query_row(
                "SELECT count(*) FROM patients WHERE owner_user_id = ?1 AND folder_slug = 'Someone-Else'",
                params![f.user],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(created, 1, "both files belong to one person, not one each");
    }

    #[test]
    fn a_sidecar_restores_what_the_filename_could_not_carry() {
        let mut f = Fx::new();
        let dir = f.vault().join("Rahim-Uddin").join("2026");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2026-03-14_Rahim-Uddin_Thyroid-Profile.jpg"), b"x").unwrap();
        std::fs::write(
            dir.join(format!("2026-03-14_Rahim-Uddin_Thyroid-Profile.jpg{}", crate::backup::SIDECAR_SUFFIX)),
            br#"{"schema":1,"documentId":"old","patient":"Rahim Uddin",
                 "docDate":"2026-03-14","title":"Thyroid Profile (TSH, FT4)",
                 "docType":"report","categories":["Thyroid"],"notes":"fasting",
                 "sha256":"whatever"}"#,
        )
        .unwrap();

        let r = f.run();
        assert_eq!(r.adopted.len(), 1, "{:?}", r.unknown);

        let (title, notes, patient): (String, Option<String>, String) = f
            .conn
            .query_row(
                "SELECT d.title, d.notes, p.display_name FROM documents d
                 JOIN patients p ON p.id = d.patient_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        // The slug would have given "Thyroid Profile (TSH, FT4)" back as
        // "Thyroid-Profile" — punctuation and all, gone.
        assert_eq!(title, "Thyroid Profile (TSH, FT4)");
        assert_eq!(notes.as_deref(), Some("fasting"));
        assert_eq!(patient, "Rahim Uddin");

        let tags: i64 = f
            .conn
            .query_row("SELECT count(*) FROM document_category", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tags, 1, "a tag recorded at backup time comes back with it");
    }

    #[test]
    fn our_own_sidecars_and_snapshot_are_not_reported_as_strays() {
        let mut f = Fx::new();
        f.filed("2026", "2026-03-14_Rahim-Uddin_CBC.jpg", b"a");
        let dir = f.vault().join("Rahim-Uddin").join("2026");
        std::fs::write(
            dir.join(format!("2026-03-14_Rahim-Uddin_CBC.jpg{}", crate::backup::SIDECAR_SUFFIX)),
            b"{}",
        ).unwrap();
        std::fs::write(f.vault().join(crate::backup::SNAPSHOT_NAME), b"sqlite").unwrap();

        let r = f.run();
        assert_eq!(r.ok, 1);
        assert!(r.unknown.is_empty(), "bookkeeping files must not look like strays: {:?}", r.unknown);
        assert!(r.adopted.is_empty());
    }

    #[test]
    fn the_exports_folder_is_not_treated_as_part_of_the_archive() {
        let mut f = Fx::new();
        let exports = f.vault().join("Exports");
        std::fs::create_dir_all(&exports).unwrap();
        std::fs::write(exports.join("Rahim Uddin - Medical History.pdf"), b"%PDF-1.7").unwrap();

        let r = f.run();
        assert!(r.unknown.is_empty(), "our own exports must not be reported as strays");
        assert!(r.adopted.is_empty());
    }

    #[test]
    fn a_file_edited_in_place_has_its_hash_and_size_re_recorded() {
        let mut f = Fx::new();
        let id = f.filed("2026", "2026-03-14_Rahim-Uddin_CBC.jpg", b"original");
        let path = f.vault().join("Rahim-Uddin").join("2026").join("2026-03-14_Rahim-Uddin_CBC.jpg");
        std::fs::write(&path, b"replaced with a better scan").unwrap();

        let r = f.run();
        assert_eq!(r.ok, 1);
        assert!(r.missing.is_empty());

        let size: i64 = f.conn
            .query_row("SELECT byte_size FROM documents WHERE id = ?1", params![id], |r| r.get(0)).unwrap();
        assert_eq!(size, "replaced with a better scan".len() as i64);
    }
}
