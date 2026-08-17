//! Keeping the archive recoverable.
//!
//! The most likely bad outcome for this app is not a security breach — it is the
//! user losing a decade of family medical history because a disk died or a folder
//! was deleted. Three layers guard against that, in increasing order of how much
//! they can recover from:
//!
//!  1. The filenames themselves. `2026-03-14_Rahim-Uddin_Thyroid-Profile.pdf`
//!     carries date, patient and title, so the reconciler can rebuild rows from a
//!     bare folder tree.
//!  2. A JSON sidecar per document, carrying what the filename cannot: categories,
//!     notes, document type. Written into the vault beside the file.
//!  3. A `VACUUM INTO` snapshot of the whole database, written into the vault so it
//!     travels with the archive when it is copied to a USB stick or synced.
//!
//! The live database deliberately lives OUTSIDE the vault, because Documents is
//! often cloud-synced and a WAL sidecar synced mid-transaction is a documented
//! corruption path. The snapshot is a single consistent file, which is safe to sync.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde::Serialize;

pub const SNAPSHOT_NAME: &str = "medicine-report-tracker.backup.db";

/// Sidecars use this suffix so the reconciler can tell them from stray files.
pub const SIDECAR_SUFFIX: &str = ".meta.json";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sidecar {
    pub schema: u32,
    pub document_id: String,
    pub patient: String,
    pub doc_date: String,
    pub title: String,
    pub doc_type: String,
    pub categories: Vec<String>,
    pub notes: Option<String>,
    pub sha256: String,
}

/// Write a consistent copy of the database into the vault.
///
/// `VACUUM INTO` produces a single defragmented file with no WAL to go with it,
/// which is exactly what is safe to copy or sync. Written to a temporary name and
/// renamed, so an interrupted backup cannot replace a good one with a partial file.
pub fn snapshot(conn: &Connection, vault_root: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(vault_root).map_err(|e| format!("cannot create vault: {e}"))?;

    let final_path = vault_root.join(SNAPSHOT_NAME);
    let temp_path = vault_root.join(format!("{SNAPSHOT_NAME}.partial"));
    let _ = std::fs::remove_file(&temp_path);

    // SQLite needs forward slashes and doubled quotes inside the literal.
    let target = temp_path.to_string_lossy().replace('\\', "/").replace('\'', "''");
    conn.execute(&format!("VACUUM INTO '{target}'"), [])
        .map_err(|e| format!("cannot write backup: {e}"))?;

    std::fs::rename(&temp_path, &final_path)
        .map_err(|e| format!("cannot finalise backup: {e}"))?;
    Ok(final_path)
}

fn sidecar_path(vault_root: &Path, rel_path: &str) -> PathBuf {
    let file = vault_root.join(rel_path.replace('\\', "/"));
    let mut name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    name.push_str(SIDECAR_SUFFIX);
    file.with_file_name(name)
}

/// Write the metadata a filename cannot carry, beside the document it describes.
pub fn write_sidecar(conn: &Connection, vault_root: &Path, document_id: &str) -> Result<(), String> {
    let (patient, doc_date, title, doc_type, notes, sha256, rel_path): (
        String, String, String, String, Option<String>, String, String,
    ) = conn
        .query_row(
            "SELECT p.display_name, d.doc_date, d.title, d.doc_type, d.notes, d.sha256, d.rel_path
             FROM documents d JOIN patients p ON p.id = d.patient_id
             WHERE d.id = ?1",
            params![document_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .map_err(|e| format!("no such document: {e}"))?;

    let categories: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT c.name FROM document_category dc
                 JOIN categories c ON c.id = dc.category_id
                 WHERE dc.document_id = ?1 ORDER BY c.name",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![document_id], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
    };

    let sidecar = Sidecar {
        schema: 1,
        document_id: document_id.to_string(),
        patient,
        doc_date,
        title,
        doc_type,
        categories,
        notes,
        sha256,
    };

    let path = sidecar_path(vault_root, &rel_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {parent:?}: {e}"))?;
    }
    let json = serde_json::to_string_pretty(&sidecar).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("cannot write sidecar: {e}"))?;
    Ok(())
}

/// Remove a document's sidecar — used when the document is trashed.
pub fn remove_sidecar(vault_root: &Path, rel_path: &str) {
    let _ = std::fs::remove_file(sidecar_path(vault_root, rel_path));
}

/// Rewrite every sidecar. Used after a bulk change, and as the repair path.
pub fn write_all_sidecars(conn: &Connection, vault_root: &Path, user_id: &str) -> Result<usize, String> {
    let ids: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT id FROM documents WHERE owner_user_id = ?1 AND trashed_at IS NULL")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![user_id], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
    };
    let mut n = 0;
    for id in &ids {
        // One unwritable sidecar must not stop the rest.
        if write_sidecar(conn, vault_root, id).is_ok() {
            n += 1;
        }
    }
    Ok(n)
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
            let dir = std::env::temp_dir().join(format!("mrt-backup-{}", ulid::Ulid::new()));
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

        fn doc(&self, title: &str, notes: Option<&str>) -> String {
            let rel = format!("Rahim-Uddin\\2026\\2026-03-14_Rahim-Uddin_{}.jpg", title.replace(' ', "-"));
            let abs = self.vault().join(rel.replace('\\', "/"));
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::fs::write(&abs, b"scan").unwrap();

            let id = ulid::Ulid::new().to_string();
            self.conn.execute(
                "INSERT INTO documents (id, owner_user_id, patient_id, doc_date, date_source, title,
                                        notes, doc_type, page_count, rel_path, sha256, byte_size,
                                        file_kind, created_at, updated_at)
                 VALUES (?1,?2,'pat1','2026-03-14','manual',?3,?4,'report',1,?5,'abc123',4,'jpeg',
                         datetime('now'), datetime('now'))",
                params![id, self.user, title, notes, rel],
            ).unwrap();
            id
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
    }

    #[test]
    fn a_snapshot_is_a_complete_readable_database() {
        let f = Fx::new();
        f.doc("Thyroid Profile", None);

        let path = snapshot(&f.conn, &f.vault()).unwrap();
        assert!(path.exists());
        assert_eq!(path.file_name().unwrap(), SNAPSHOT_NAME);

        // The point of a backup is that it opens on its own, with no WAL beside it.
        let restored = Connection::open(&path).unwrap();
        let docs: i64 = restored.query_row("SELECT count(*) FROM documents", [], |r| r.get(0)).unwrap();
        assert_eq!(docs, 1);
        let patients: i64 = restored.query_row("SELECT count(*) FROM patients", [], |r| r.get(0)).unwrap();
        assert_eq!(patients, 1);

        assert!(!f.vault().join(format!("{SNAPSHOT_NAME}.partial")).exists(), "temp file must be gone");
    }

    #[test]
    fn a_second_snapshot_replaces_the_first_without_losing_it_midway() {
        let f = Fx::new();
        f.doc("First", None);
        snapshot(&f.conn, &f.vault()).unwrap();

        f.doc("Second", None);
        let path = snapshot(&f.conn, &f.vault()).unwrap();

        let restored = Connection::open(&path).unwrap();
        let docs: i64 = restored.query_row("SELECT count(*) FROM documents", [], |r| r.get(0)).unwrap();
        assert_eq!(docs, 2);
    }

    #[test]
    fn a_sidecar_carries_what_the_filename_cannot() {
        let f = Fx::new();
        let id = f.doc("Thyroid Profile", Some("TSH elevated, repeat in 3 months"));

        let cat = crate::categories::create(&f.conn, &f.user, "Thyroid", None).unwrap();
        let cat2 = crate::categories::create(&f.conn, &f.user, "Endocrine", None).unwrap();
        let mut conn = crate::db::open(&f.dir.join("app.db")).unwrap();
        crate::categories::set_for_document(&mut conn, &f.user, &id, &[cat.id, cat2.id]).unwrap();

        write_sidecar(&conn, &f.vault(), &id).unwrap();

        let path = f.vault().join("Rahim-Uddin").join("2026")
            .join(format!("2026-03-14_Rahim-Uddin_Thyroid-Profile.jpg{SIDECAR_SUFFIX}"));
        assert!(path.exists(), "sidecar must sit beside the document");

        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["patient"], "Rahim Uddin");
        assert_eq!(json["title"], "Thyroid Profile");
        assert_eq!(json["notes"], "TSH elevated, repeat in 3 months");
        // Categories are the thing a filename genuinely cannot encode.
        let cats: Vec<String> = json["categories"].as_array().unwrap()
            .iter().map(|v| v.as_str().unwrap().to_string()).collect();
        assert_eq!(cats, vec!["Endocrine", "Thyroid"]);
    }

    #[test]
    fn rewriting_a_sidecar_reflects_the_current_state() {
        let f = Fx::new();
        let id = f.doc("Thyroid Profile", None);
        write_sidecar(&f.conn, &f.vault(), &id).unwrap();

        f.conn.execute("UPDATE documents SET notes = 'added later' WHERE id = ?1", params![id]).unwrap();
        write_sidecar(&f.conn, &f.vault(), &id).unwrap();

        let path = f.vault().join("Rahim-Uddin").join("2026")
            .join(format!("2026-03-14_Rahim-Uddin_Thyroid-Profile.jpg{SIDECAR_SUFFIX}"));
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["notes"], "added later");
    }

    #[test]
    fn write_all_sidecars_covers_the_whole_library() {
        let f = Fx::new();
        f.doc("One", None);
        f.doc("Two", None);
        assert_eq!(write_all_sidecars(&f.conn, &f.vault(), &f.user).unwrap(), 2);
    }

    #[test]
    fn removing_a_sidecar_leaves_the_document_alone() {
        let f = Fx::new();
        let id = f.doc("Thyroid Profile", None);
        write_sidecar(&f.conn, &f.vault(), &id).unwrap();

        let doc_path = f.vault().join("Rahim-Uddin").join("2026")
            .join("2026-03-14_Rahim-Uddin_Thyroid-Profile.jpg");
        let side = f.vault().join("Rahim-Uddin").join("2026")
            .join(format!("2026-03-14_Rahim-Uddin_Thyroid-Profile.jpg{SIDECAR_SUFFIX}"));

        remove_sidecar(&f.vault(), "Rahim-Uddin\\2026\\2026-03-14_Rahim-Uddin_Thyroid-Profile.jpg");
        assert!(!side.exists());
        assert!(doc_path.exists(), "the scan itself must never be touched");
    }
}
