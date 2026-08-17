//! Patient profiles.
//!
//! Identity is an immutable ULID. `folder_slug` is a mutable label derived from
//! `display_name` — renaming a patient must never mean re-keying their documents.

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::naming;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Patient {
    pub id: String,
    pub display_name: String,
    pub folder_slug: String,
    pub dob: Option<String>,
    pub document_count: i64,
}

pub fn list(conn: &Connection, user_id: &str) -> Result<Vec<Patient>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT p.id, p.display_name, p.folder_slug, p.dob,
                    (SELECT count(*) FROM documents d
                      WHERE d.patient_id = p.id AND d.trashed_at IS NULL)
             FROM patients p
             WHERE p.owner_user_id = ?1 AND p.archived_at IS NULL
             ORDER BY p.display_name COLLATE NOCASE",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![user_id], |r| {
            Ok(Patient {
                id: r.get(0)?,
                display_name: r.get(1)?,
                folder_slug: r.get(2)?,
                dob: r.get(3)?,
                document_count: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Create a patient.
///
/// `dob` is stored because it is the strongest available signal for rejecting a
/// date-of-birth that OCR mistook for the report date — a wrong pick there files a
/// 2026 report under 1978, where it sorts to the top of the folder forever.
pub fn create(
    conn: &Connection,
    user_id: &str,
    display_name: &str,
    dob: Option<&str>,
) -> Result<Patient, String> {
    let name = display_name.trim();
    if name.is_empty() {
        return Err("A patient needs a name.".into());
    }
    if let Some(d) = dob {
        if !d.is_empty() && !naming::is_valid_doc_date(d) {
            return Err(format!("'{d}' is not a valid date of birth."));
        }
    }

    let slug = naming::patient_slug(name);

    // NTFS is case-insensitive but case-preserving. Without this check 'Rahim' and
    // 'rahim' become two rows pointing at ONE directory, and their files interleave.
    let taken: i64 = conn
        .query_row(
            "SELECT count(*) FROM patients
             WHERE owner_user_id = ?1 AND upper(folder_slug) = upper(?2) AND archived_at IS NULL",
            params![user_id, slug],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if taken > 0 {
        return Err(format!(
            "A patient already uses the folder '{slug}'. Names that differ only by \
             capitalisation or punctuation share one folder on Windows."
        ));
    }

    let id = ulid::Ulid::new().to_string();
    conn.execute(
        "INSERT INTO patients (id, owner_user_id, display_name, folder_slug, dob, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), datetime('now'))",
        params![id, user_id, name, slug, dob.filter(|d| !d.is_empty())],
    )
    .map_err(|e| format!("cannot create patient: {e}"))?;

    Ok(Patient {
        id,
        display_name: name.to_string(),
        folder_slug: slug,
        dob: dob.map(str::to_string),
        document_count: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> (std::path::PathBuf, Connection, String) {
        let dir = std::env::temp_dir().join(format!("mrt-pat-{name}-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = crate::db::open(&dir.join("app.db")).unwrap();
        let user = crate::db::ensure_user(&conn).unwrap();
        (dir, conn, user)
    }

    #[test]
    fn creating_a_patient_derives_a_path_safe_slug() {
        let (_d, conn, user) = fixture("create");
        let p = create(&conn, &user, "  Rahim Uddin  ", None).unwrap();
        assert_eq!(p.display_name, "Rahim Uddin");
        assert_eq!(p.folder_slug, "Rahim-Uddin");
        assert_eq!(p.document_count, 0);
    }

    #[test]
    fn names_differing_only_by_case_are_rejected_because_ntfs_merges_them() {
        let (_d, conn, user) = fixture("case");
        create(&conn, &user, "Rahim", None).unwrap();
        let err = create(&conn, &user, "rahim", None).unwrap_err();
        assert!(err.contains("already uses the folder"));
    }

    #[test]
    fn names_that_slugify_to_the_same_folder_are_rejected() {
        let (_d, conn, user) = fixture("slugclash");
        create(&conn, &user, "Rahim Uddin", None).unwrap();
        // Different display name, identical folder — same collision, less obvious.
        let err = create(&conn, &user, "Rahim/Uddin", None).unwrap_err();
        assert!(err.contains("already uses the folder"));
    }

    #[test]
    fn an_empty_name_is_refused_rather_than_becoming_unknown_patient() {
        let (_d, conn, user) = fixture("empty");
        assert!(create(&conn, &user, "   ", None).unwrap_err().contains("needs a name"));
    }

    #[test]
    fn an_invalid_dob_is_refused() {
        let (_d, conn, user) = fixture("dob");
        assert!(create(&conn, &user, "Rahim", Some("14/03/1978")).unwrap_err().contains("not a valid"));
        assert!(create(&conn, &user, "Karim", Some("1978-03-14")).is_ok());
    }

    #[test]
    fn listing_is_alphabetical_and_counts_documents() {
        let (_d, conn, user) = fixture("list");
        create(&conn, &user, "Zainab", None).unwrap();
        let rahim = create(&conn, &user, "Rahim", None).unwrap();
        create(&conn, &user, "amina", None).unwrap();

        conn.execute(
            "INSERT INTO documents (id, owner_user_id, patient_id, doc_date, date_source, title,
                                    doc_type, rel_path, sha256, byte_size, file_kind, created_at, updated_at)
             VALUES ('d1', ?1, ?2, '2026-03-14', 'manual', 'CBC', 'report', 'x', 'y', 1, 'jpeg',
                     datetime('now'), datetime('now'))",
            params![user, rahim.id],
        ).unwrap();

        let all = list(&conn, &user).unwrap();
        assert_eq!(
            all.iter().map(|p| p.display_name.as_str()).collect::<Vec<_>>(),
            vec!["amina", "Rahim", "Zainab"],
            "case-insensitive alphabetical",
        );
        assert_eq!(all.iter().find(|p| p.id == rahim.id).unwrap().document_count, 1);
    }
}
