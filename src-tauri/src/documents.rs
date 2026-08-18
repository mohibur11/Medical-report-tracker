//! Reading the filed library.

use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentRow {
    pub id: String,
    pub patient_id: String,
    pub patient: String,
    pub doc_date: String,
    pub title: String,
    pub doc_type: String,
    pub file_kind: String,
    pub rel_path: String,
    pub page_count: i64,
    pub byte_size: i64,
    /// What the paper does not say — what was advised, what to repeat and when.
    pub notes: Option<String>,
    /// Set by the reconciler when the file is no longer where the database says.
    /// Rows are never auto-purged — a file moved in Explorer is not a deletion.
    pub missing: bool,
}

pub fn list(conn: &Connection, user_id: &str) -> Result<Vec<DocumentRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT d.id, d.patient_id, p.display_name, d.doc_date, d.title, d.doc_type,
                    d.file_kind, d.rel_path, d.page_count, d.byte_size, d.notes, d.missing_at
             FROM documents d
             JOIN patients p ON p.id = d.patient_id
             WHERE d.owner_user_id = ?1 AND d.trashed_at IS NULL
             ORDER BY CASE WHEN d.doc_date LIKE '0000%' THEN 1 ELSE 0 END,
                      d.doc_date DESC, d.title",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![user_id], |r| {
            Ok(DocumentRow {
                id: r.get(0)?,
                patient_id: r.get(1)?,
                patient: r.get(2)?,
                doc_date: r.get(3)?,
                title: r.get(4)?,
                doc_type: r.get(5)?,
                file_kind: r.get(6)?,
                rel_path: r.get(7)?,
                page_count: r.get(8)?,
                byte_size: r.get(9)?,
                notes: r.get(10)?,
                missing: r.get::<_, Option<String>>(11)?.is_some(),
            })
        })
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Years present in the library, newest first, with 'Undated' last — the values
/// the export filter offers.
pub fn years(conn: &Connection, user_id: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT
                 CASE WHEN doc_date LIKE '0000%' THEN 'Undated' ELSE substr(doc_date,1,4) END AS y
             FROM documents
             WHERE owner_user_id = ?1 AND trashed_at IS NULL
             ORDER BY (y = 'Undated'), y DESC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![user_id], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (std::path::PathBuf, Connection, String, String) {
        let dir = std::env::temp_dir().join(format!("mrt-docs-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = crate::db::open(&dir.join("app.db")).unwrap();
        let user = crate::db::ensure_user(&conn).unwrap();
        let p = ulid::Ulid::new().to_string();
        conn.execute(
            "INSERT INTO patients (id, owner_user_id, display_name, folder_slug, created_at, updated_at)
             VALUES (?1,?2,'Rahim','Rahim', datetime('now'), datetime('now'))",
            params![p, user],
        ).unwrap();
        (dir, conn, user, p)
    }

    fn add(conn: &Connection, user: &str, patient: &str, date: &str, title: &str) {
        conn.execute(
            "INSERT INTO documents (id, owner_user_id, patient_id, doc_date, date_source, title,
                                    doc_type, page_count, rel_path, sha256, byte_size, file_kind,
                                    created_at, updated_at)
             VALUES (?1,?2,?3,?4,'manual',?5,'report',1,?6,'s',10,'jpeg', datetime('now'), datetime('now'))",
            params![ulid::Ulid::new().to_string(), user, patient, date, title, format!("p/{title}")],
        ).unwrap();
    }

    #[test]
    fn newest_first_with_undated_at_the_end() {
        let (_d, conn, user, p) = fixture();
        add(&conn, &user, &p, "2024-01-05", "Older");
        add(&conn, &user, &p, "0000-00-00", "Unknown");
        add(&conn, &user, &p, "2026-03-14", "Newest");

        let rows = list(&conn, &user).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            vec!["Newest", "Older", "Unknown"],
        );
    }

    #[test]
    fn years_are_distinct_newest_first_with_undated_last() {
        let (_d, conn, user, p) = fixture();
        add(&conn, &user, &p, "2024-01-05", "a");
        add(&conn, &user, &p, "2026-03-14", "b");
        add(&conn, &user, &p, "2026-07-01", "c");
        add(&conn, &user, &p, "0000-00-00", "d");

        assert_eq!(years(&conn, &user).unwrap(), vec!["2026", "2024", "Undated"]);
    }

    #[test]
    fn trashed_documents_are_hidden_but_not_gone() {
        let (_d, conn, user, p) = fixture();
        add(&conn, &user, &p, "2026-03-14", "Kept");
        add(&conn, &user, &p, "2026-03-15", "Trashed");
        conn.execute("UPDATE documents SET trashed_at = datetime('now') WHERE title = 'Trashed'", []).unwrap();

        let rows = list(&conn, &user).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Kept");

        let total: i64 = conn.query_row("SELECT count(*) FROM documents", [], |r| r.get(0)).unwrap();
        assert_eq!(total, 2, "trashing must not delete the row");
    }

    #[test]
    fn a_missing_file_is_flagged_rather_than_removed() {
        let (_d, conn, user, p) = fixture();
        add(&conn, &user, &p, "2026-03-14", "Moved In Explorer");
        conn.execute("UPDATE documents SET missing_at = datetime('now')", []).unwrap();

        let rows = list(&conn, &user).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].missing);
    }
}
