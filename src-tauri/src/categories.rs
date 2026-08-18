//! Categories — the second axis.
//!
//! The vault tree encodes exactly one hierarchy, `<Patient>/<Year>/`. Categories
//! are many-to-many and cut across it: a thyroid panel for a diabetic patient
//! belongs to both "Thyroid" and "Diabetes", and "Thyroid" spans family members.
//! That cannot be expressed in a folder tree, which is precisely why it lives here
//! and only materialises at export time.
//!
//! Documents reference `category_id`, never the name, so renaming a category is a
//! one-row UPDATE with zero disk I/O — no files move, no exports break.

use rusqlite::{params, params_from_iter, Connection};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub document_count: i64,
}

pub fn list(conn: &Connection, user_id: &str) -> Result<Vec<Category>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT c.id, c.name, c.color,
                    (SELECT count(*) FROM document_category dc
                       JOIN documents d ON d.id = dc.document_id
                      WHERE dc.category_id = c.id AND d.trashed_at IS NULL)
             FROM categories c
             WHERE c.owner_user_id = ?1 AND c.archived_at IS NULL
             ORDER BY c.name COLLATE NOCASE",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![user_id], |r| {
            Ok(Category {
                id: r.get(0)?,
                name: r.get(1)?,
                color: r.get(2)?,
                document_count: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn create(conn: &Connection, user_id: &str, name: &str, color: Option<&str>) -> Result<Category, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A category needs a name.".into());
    }

    let clash: i64 = conn
        .query_row(
            "SELECT count(*) FROM categories
             WHERE owner_user_id = ?1 AND lower(name) = lower(?2) AND archived_at IS NULL",
            params![user_id, name],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if clash > 0 {
        return Err(format!("'{name}' already exists."));
    }

    let id = ulid::Ulid::new().to_string();
    conn.execute(
        "INSERT INTO categories (id, owner_user_id, name, color, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'))",
        params![id, user_id, name, color],
    )
    .map_err(|e| format!("cannot create category: {e}"))?;

    Ok(Category { id, name: name.to_string(), color: color.map(str::to_string), document_count: 0 })
}

/// Rename. One row, no disk I/O — this is the whole reason documents reference
/// the id rather than the name.
pub fn rename(conn: &Connection, user_id: &str, id: &str, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A category needs a name.".into());
    }

    let clash: i64 = conn
        .query_row(
            "SELECT count(*) FROM categories
             WHERE owner_user_id = ?1 AND lower(name) = lower(?2) AND id != ?3 AND archived_at IS NULL",
            params![user_id, name, id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if clash > 0 {
        return Err(format!("'{name}' already exists."));
    }

    let n = conn
        .execute(
            "UPDATE categories SET name = ?3, updated_at = datetime('now')
             WHERE owner_user_id = ?1 AND id = ?2",
            params![user_id, id, name],
        )
        .map_err(|e| format!("cannot rename category: {e}"))?;
    if n == 0 {
        return Err("No such category.".into());
    }
    Ok(())
}

/// Archive rather than delete, so an export made last year remains explicable.
/// Existing tags are left intact; the category simply stops being offered.
pub fn archive(conn: &Connection, user_id: &str, id: &str) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE categories SET archived_at = datetime('now'), updated_at = datetime('now')
             WHERE owner_user_id = ?1 AND id = ?2 AND archived_at IS NULL",
            params![user_id, id],
        )
        .map_err(|e| format!("cannot archive category: {e}"))?;
    if n == 0 {
        return Err("No such category.".into());
    }
    Ok(())
}

/// Replace a document's tags with exactly `category_ids`.
pub fn set_for_document(
    conn: &mut Connection,
    user_id: &str,
    document_id: &str,
    category_ids: &[String],
) -> Result<(), String> {
    let owns: i64 = conn
        .query_row(
            "SELECT count(*) FROM documents WHERE id = ?1 AND owner_user_id = ?2",
            params![document_id, user_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if owns == 0 {
        return Err("No such document.".into());
    }

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM document_category WHERE document_id = ?1", params![document_id])
        .map_err(|e| e.to_string())?;
    for cid in category_ids {
        tx.execute(
            "INSERT OR IGNORE INTO document_category (document_id, category_id, created_at)
             VALUES (?1, ?2, datetime('now'))",
            params![document_id, cid],
        )
        .map_err(|e| format!("cannot tag document: {e}"))?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// Add one category to many documents at once — the bulk path the review grid
/// and library selection need.
pub fn tag_many(
    conn: &mut Connection,
    user_id: &str,
    document_ids: &[String],
    category_id: &str,
) -> Result<usize, String> {
    if document_ids.is_empty() {
        return Ok(0);
    }
    let owns: i64 = conn
        .query_row(
            "SELECT count(*) FROM categories WHERE id = ?1 AND owner_user_id = ?2",
            params![category_id, user_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if owns == 0 {
        return Err("No such category.".into());
    }

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut n = 0usize;
    for did in document_ids {
        n += tx
            .execute(
                "INSERT OR IGNORE INTO document_category (document_id, category_id, created_at)
                 SELECT ?1, ?2, datetime('now')
                 WHERE EXISTS (SELECT 1 FROM documents WHERE id = ?1 AND owner_user_id = ?3)",
                params![did, category_id, user_id],
            )
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

/// Take a category off many documents at once.
///
/// The counterpart of `tag_many`: tagging a batch is easy to get wrong, and
/// undoing it one popover at a time is the kind of chore that stops people
/// tagging at all.
pub fn untag_many(
    conn: &mut Connection,
    user_id: &str,
    document_ids: &[String],
    category_id: &str,
) -> Result<usize, String> {
    if document_ids.is_empty() {
        return Ok(0);
    }

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut n = 0usize;
    for did in document_ids {
        n += tx
            .execute(
                "DELETE FROM document_category
                  WHERE document_id = ?1 AND category_id = ?2
                    AND EXISTS (SELECT 1 FROM documents WHERE id = ?1 AND owner_user_id = ?3)",
                params![did, category_id, user_id],
            )
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

/// Tags for a set of documents, as (document_id, category_id) pairs. The UI joins
/// them client-side rather than issuing a query per row.
pub fn for_documents(
    conn: &Connection,
    user_id: &str,
    document_ids: &[String],
) -> Result<Vec<(String, String)>, String> {
    if document_ids.is_empty() {
        return Ok(Vec::new());
    }
    let holes = vec!["?"; document_ids.len()].join(",");
    let sql = format!(
        "SELECT dc.document_id, dc.category_id
         FROM document_category dc
         JOIN documents d ON d.id = dc.document_id
         WHERE d.owner_user_id = ? AND dc.document_id IN ({holes})"
    );

    let mut binds: Vec<String> = vec![user_id.to_string()];
    binds.extend(document_ids.iter().cloned());

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params_from_iter(binds.iter()), |r| Ok((r.get(0)?, r.get(1)?)))
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
            let dir = std::env::temp_dir().join(format!("mrt-cat-{}", ulid::Ulid::new()));
            std::fs::create_dir_all(&dir).unwrap();
            let conn = crate::db::open(&dir.join("app.db")).unwrap();
            let user = crate::db::ensure_user(&conn).unwrap();
            let patient = ulid::Ulid::new().to_string();
            conn.execute(
                "INSERT INTO patients (id, owner_user_id, display_name, folder_slug, created_at, updated_at)
                 VALUES (?1,?2,'Rahim','Rahim', datetime('now'), datetime('now'))",
                params![patient, user],
            ).unwrap();
            Fx { dir, conn, user, patient }
        }

        fn doc(&self, title: &str, date: &str) -> String {
            let id = ulid::Ulid::new().to_string();
            self.conn.execute(
                "INSERT INTO documents (id, owner_user_id, patient_id, doc_date, date_source, title,
                                        doc_type, page_count, rel_path, sha256, byte_size, file_kind,
                                        created_at, updated_at)
                 VALUES (?1,?2,?3,?4,'manual',?5,'report',1,?6,'s',1,'jpeg', datetime('now'), datetime('now'))",
                params![id, self.user, self.patient, date, title, format!("p/{title}")],
            ).unwrap();
            id
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
    }

    #[test]
    fn categories_are_created_and_counted() {
        let f = Fx::new();
        let c = create(&f.conn, &f.user, "  Thyroid  ", Some("#0ea5e9")).unwrap();
        assert_eq!(c.name, "Thyroid");
        assert_eq!(c.document_count, 0);
        assert_eq!(list(&f.conn, &f.user).unwrap().len(), 1);
    }

    #[test]
    fn duplicate_names_are_refused_case_insensitively() {
        let f = Fx::new();
        create(&f.conn, &f.user, "Thyroid", None).unwrap();
        assert!(create(&f.conn, &f.user, "thyroid", None).unwrap_err().contains("already exists"));
    }

    #[test]
    fn a_document_can_belong_to_several_categories_at_once() {
        // The case a folder tree cannot express, and the one requirement 5 exercises.
        let mut f = Fx::new();
        let thyroid = create(&f.conn, &f.user, "Thyroid", None).unwrap();
        let diabetes = create(&f.conn, &f.user, "Diabetes", None).unwrap();
        let doc = f.doc("Thyroid Profile", "2026-03-14");

        set_for_document(&mut f.conn, &f.user, &doc, &[thyroid.id.clone(), diabetes.id.clone()]).unwrap();

        let tags = for_documents(&f.conn, &f.user, &[doc.clone()]).unwrap();
        assert_eq!(tags.len(), 2);

        let counts = list(&f.conn, &f.user).unwrap();
        assert!(counts.iter().all(|c| c.document_count == 1));
    }

    #[test]
    fn setting_tags_replaces_rather_than_accumulates() {
        let mut f = Fx::new();
        let a = create(&f.conn, &f.user, "A", None).unwrap();
        let b = create(&f.conn, &f.user, "B", None).unwrap();
        let doc = f.doc("Report", "2026-03-14");

        set_for_document(&mut f.conn, &f.user, &doc, &[a.id.clone(), b.id.clone()]).unwrap();
        set_for_document(&mut f.conn, &f.user, &doc, &[b.id.clone()]).unwrap();

        let tags = for_documents(&f.conn, &f.user, &[doc]).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].1, b.id);
    }

    #[test]
    fn renaming_touches_one_row_and_keeps_every_tag() {
        let mut f = Fx::new();
        let c = create(&f.conn, &f.user, "Thyroid", None).unwrap();
        let doc = f.doc("Report", "2026-03-14");
        set_for_document(&mut f.conn, &f.user, &doc, &[c.id.clone()]).unwrap();

        rename(&f.conn, &f.user, &c.id, "Thyroid & Endocrine").unwrap();

        let all = list(&f.conn, &f.user).unwrap();
        assert_eq!(all[0].name, "Thyroid & Endocrine");
        assert_eq!(all[0].document_count, 1, "tags reference the id, so a rename cannot lose them");
    }

    #[test]
    fn renaming_onto_an_existing_name_is_refused() {
        let f = Fx::new();
        create(&f.conn, &f.user, "Thyroid", None).unwrap();
        let b = create(&f.conn, &f.user, "Diabetes", None).unwrap();
        assert!(rename(&f.conn, &f.user, &b.id, "thyroid").unwrap_err().contains("already exists"));
    }

    #[test]
    fn archiving_hides_the_category_without_destroying_history() {
        let mut f = Fx::new();
        let c = create(&f.conn, &f.user, "Thyroid", None).unwrap();
        let doc = f.doc("Report", "2026-03-14");
        set_for_document(&mut f.conn, &f.user, &doc, &[c.id.clone()]).unwrap();

        archive(&f.conn, &f.user, &c.id).unwrap();
        assert!(list(&f.conn, &f.user).unwrap().is_empty());

        // The tag survives, so an export made before the archive still explains itself.
        let rows: i64 = f.conn
            .query_row("SELECT count(*) FROM document_category", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn bulk_tagging_is_idempotent() {
        let mut f = Fx::new();
        let c = create(&f.conn, &f.user, "Thyroid", None).unwrap();
        let docs: Vec<String> = (0..3).map(|i| f.doc(&format!("R{i}"), "2026-03-14")).collect();

        assert_eq!(tag_many(&mut f.conn, &f.user, &docs, &c.id).unwrap(), 3);
        // Re-tagging the same set must not duplicate rows or inflate counts.
        tag_many(&mut f.conn, &f.user, &docs, &c.id).unwrap();
        assert_eq!(list(&f.conn, &f.user).unwrap()[0].document_count, 3);
    }

    #[test]
    fn tagging_a_document_that_is_not_yours_is_refused() {
        let mut f = Fx::new();
        let c = create(&f.conn, &f.user, "Thyroid", None).unwrap();
        assert!(set_for_document(&mut f.conn, &f.user, "not-a-document", &[c.id]).unwrap_err().contains("No such document"));
    }

    #[test]
    fn a_trashed_document_stops_counting_towards_its_categories() {
        let mut f = Fx::new();
        let c = create(&f.conn, &f.user, "Thyroid", None).unwrap();
        let doc = f.doc("Report", "2026-03-14");
        set_for_document(&mut f.conn, &f.user, &doc, &[c.id]).unwrap();

        f.conn.execute("UPDATE documents SET trashed_at = datetime('now')", []).unwrap();
        assert_eq!(list(&f.conn, &f.user).unwrap()[0].document_count, 0);
    }
}
