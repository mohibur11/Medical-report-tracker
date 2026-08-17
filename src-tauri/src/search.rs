//! Full-text search over the library.
//!
//! Searching "creatinine" across five years of scans is worth more than perfect
//! field extraction, and it degrades gracefully: at a 6% character error rate most
//! words still match, whereas a single mis-parsed date silently misfiles a report.
//!
//! Today the index holds titles and notes. When OCR lands it gains the recognised
//! page text, and nothing above this layer has to change.

use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub document_id: String,
    pub title: String,
    pub patient: String,
    pub doc_date: String,
    /// Matched text with the hit wrapped in [ ], for showing why it matched.
    pub snippet: String,
}

/// Add or replace a document's entry. Called on commit and after any edit that
/// changes searchable text.
pub fn index_document(conn: &Connection, document_id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM document_fts WHERE doc_id = ?1", params![document_id])
        .map_err(|e| format!("cannot clear index entry: {e}"))?;

    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT title, notes FROM documents WHERE id = ?1 AND trashed_at IS NULL",
            params![document_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();

    // A trashed or deleted document simply leaves the index — the DELETE above
    // already removed it, so this is not an error.
    let Some((title, notes)) = row else { return Ok(()) };

    let ocr_text: Option<String> = conn
        .query_row(
            "SELECT group_concat(ocr_text, ' ') FROM document_page WHERE document_id = ?1",
            params![document_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();

    conn.execute(
        "INSERT INTO document_fts (doc_id, title, notes, ocr_text) VALUES (?1, ?2, ?3, ?4)",
        params![document_id, title, notes.unwrap_or_default(), ocr_text.unwrap_or_default()],
    )
    .map_err(|e| format!("cannot index document: {e}"))?;
    Ok(())
}

pub fn remove_document(conn: &Connection, document_id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM document_fts WHERE doc_id = ?1", params![document_id])
        .map_err(|e| format!("cannot remove index entry: {e}"))?;
    Ok(())
}

/// Rebuild the whole index. Used after a vault rescan, and as the repair path if
/// the index and the documents ever disagree.
pub fn reindex_all(conn: &Connection, user_id: &str) -> Result<usize, String> {
    conn.execute("DELETE FROM document_fts", [])
        .map_err(|e| e.to_string())?;

    let ids: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT id FROM documents WHERE owner_user_id = ?1 AND trashed_at IS NULL")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![user_id], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
    };

    for id in &ids {
        index_document(conn, id)?;
    }
    Ok(ids.len())
}

/// Turn what a person types into an FTS5 MATCH expression.
///
/// Raw input cannot be passed through: `-`, `"`, `*`, `NEAR` and friends are
/// operators, so "S. Creatinine" or "T3/T4" would be a syntax error rather than a
/// search. Each word becomes a quoted prefix term, which is what a person means
/// when they type half a word.
fn to_match_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();

    if terms.is_empty() {
        return None;
    }
    Some(terms.join(" AND "))
}

pub fn search(conn: &Connection, user_id: &str, query: &str, limit: usize) -> Result<Vec<SearchHit>, String> {
    let Some(match_expr) = to_match_query(query) else {
        return Ok(Vec::new());
    };

    let mut stmt = conn
        .prepare(
            "SELECT f.doc_id, d.title, p.display_name, d.doc_date,
                    snippet(document_fts, -1, '[', ']', '…', 12)
             FROM document_fts f
             JOIN documents d ON d.id = f.doc_id
             JOIN patients  p ON p.id = d.patient_id
             WHERE document_fts MATCH ?1
               AND d.owner_user_id = ?2
               AND d.trashed_at IS NULL
             ORDER BY rank
             LIMIT ?3",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![match_expr, user_id, limit as i64], |r| {
            Ok(SearchHit {
                document_id: r.get(0)?,
                title: r.get(1)?,
                patient: r.get(2)?,
                doc_date: r.get(3)?,
                snippet: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fx {
        dir: std::path::PathBuf,
        conn: Connection,
        user: String,
        patient: String,
    }

    impl Fx {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("mrt-search-{}", ulid::Ulid::new()));
            std::fs::create_dir_all(&dir).unwrap();
            let conn = crate::db::open(&dir.join("app.db")).unwrap();
            let user = crate::db::ensure_user(&conn).unwrap();
            let patient = ulid::Ulid::new().to_string();
            conn.execute(
                "INSERT INTO patients (id, owner_user_id, display_name, folder_slug, created_at, updated_at)
                 VALUES (?1,?2,'Rahim Uddin','Rahim-Uddin', datetime('now'), datetime('now'))",
                params![patient, user],
            ).unwrap();
            Fx { dir, conn, user, patient }
        }

        fn doc(&self, title: &str, notes: Option<&str>) -> String {
            let id = ulid::Ulid::new().to_string();
            self.conn.execute(
                "INSERT INTO documents (id, owner_user_id, patient_id, doc_date, date_source, title,
                                        notes, doc_type, page_count, rel_path, sha256, byte_size,
                                        file_kind, created_at, updated_at)
                 VALUES (?1,?2,?3,'2026-03-14','manual',?4,?5,'report',1,?6,'s',1,'jpeg',
                         datetime('now'), datetime('now'))",
                params![id, self.user, self.patient, title, notes, format!("p/{title}")],
            ).unwrap();
            index_document(&self.conn, &id).unwrap();
            id
        }

        fn find(&self, q: &str) -> Vec<SearchHit> {
            search(&self.conn, &self.user, q, 20).unwrap()
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
    }

    #[test]
    fn finds_a_document_by_title() {
        let f = Fx::new();
        f.doc("Thyroid Profile", None);
        f.doc("Lipid Profile", None);

        let hits = f.find("thyroid");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Thyroid Profile");
        assert_eq!(hits[0].patient, "Rahim Uddin");
    }

    #[test]
    fn snippets_show_why_a_document_matched() {
        // The whole reason 002 rebuilt the index: a contentless FTS5 table
        // returns nothing from snippet().
        let f = Fx::new();
        f.doc("Renal Function", Some("Serum creatinine elevated, repeat in 3 months"));

        let hits = f.find("creatinine");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains('['), "expected a highlighted snippet, got {:?}", hits[0].snippet);
        assert!(hits[0].snippet.to_lowercase().contains("creatinine"));
    }

    #[test]
    fn partial_words_match_because_people_type_half_a_term() {
        let f = Fx::new();
        f.doc("Complete Blood Count", None);
        assert_eq!(f.find("blo").len(), 1);
        assert_eq!(f.find("compl blo").len(), 1, "all terms must match, in any order");
    }

    #[test]
    fn punctuation_in_a_query_is_not_treated_as_fts_syntax() {
        // "S. Creatinine" and "T3/T4" would be syntax errors if passed through raw.
        let f = Fx::new();
        f.doc("S. Creatinine", None);
        f.doc("T3/T4 Panel", None);

        assert_eq!(f.find("S. Creatinine").len(), 1);
        assert_eq!(f.find("T3/T4").len(), 1);
        assert_eq!(f.find("\"unbalanced").len(), 0, "must not error");
        assert_eq!(f.find("-  -").len(), 0);
    }

    #[test]
    fn an_empty_query_returns_nothing_rather_than_everything() {
        let f = Fx::new();
        f.doc("Thyroid Profile", None);
        assert!(f.find("").is_empty());
        assert!(f.find("   ").is_empty());
    }

    #[test]
    fn re_indexing_replaces_rather_than_duplicates() {
        let f = Fx::new();
        let id = f.doc("Thyroid Profile", None);

        f.conn.execute("UPDATE documents SET title = 'Thyroid Panel' WHERE id = ?1", params![id]).unwrap();
        index_document(&f.conn, &id).unwrap();

        assert_eq!(f.find("thyroid").len(), 1, "one row per document, not one per re-index");
        assert_eq!(f.find("panel").len(), 1);
        assert_eq!(f.find("profile").len(), 0, "the old title must be gone");
    }

    #[test]
    fn trashed_documents_leave_the_index() {
        let f = Fx::new();
        let id = f.doc("Thyroid Profile", None);
        f.conn.execute("UPDATE documents SET trashed_at = datetime('now') WHERE id = ?1", params![id]).unwrap();
        index_document(&f.conn, &id).unwrap();
        assert!(f.find("thyroid").is_empty());
    }

    #[test]
    fn ocr_page_text_is_searchable_once_it_exists() {
        // Nothing writes document_page yet; this pins the contract OCR will use.
        let f = Fx::new();
        let id = f.doc("Untitled Scan", None);
        f.conn.execute(
            "INSERT INTO document_page (document_id, page_no, ocr_text) VALUES (?1, 1, ?2)",
            params![id, "POPULAR DIAGNOSTIC CENTRE TSH 6.82 uIU/mL elevated"],
        ).unwrap();
        index_document(&f.conn, &id).unwrap();

        let hits = f.find("tsh");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.to_lowercase().contains("tsh"));
    }

    #[test]
    fn reindex_all_rebuilds_from_scratch() {
        let f = Fx::new();
        f.doc("Thyroid Profile", None);
        f.doc("Lipid Profile", None);

        f.conn.execute("DELETE FROM document_fts", []).unwrap();
        assert!(f.find("profile").is_empty());

        assert_eq!(reindex_all(&f.conn, &f.user).unwrap(), 2);
        assert_eq!(f.find("profile").len(), 2);
    }
}
