//! Saved export filters.
//!
//! The export screen asks the same five questions every time and the answers
//! rarely change — this doctor wants the thyroid history, that one wants
//! everything from last year. What is stored is the filter, never the result: a
//! preset opened a year from now picks up everything filed since, which is what
//! "all thyroid reports" means to the person asking for it.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreset {
    pub id: String,
    pub name: String,
    pub patient_ids: Vec<String>,
    pub years: Vec<String>,
    pub category_ids: Vec<String>,
    pub doc_types: Vec<String>,
    pub preset: String,
    pub max_bytes: Option<i64>,
}

fn to_json(values: &[String]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".into())
}

fn from_json(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

pub fn list(conn: &Connection, user_id: &str) -> Result<Vec<ExportPreset>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, patient_ids, years, category_ids, doc_types, preset, max_bytes
             FROM export_preset WHERE owner_user_id = ?1
             -- Most recently used first: the one wanted again is usually the one
             -- used last.
             ORDER BY used_at IS NULL, used_at DESC, name COLLATE NOCASE",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![user_id], |r| {
            let patients: String = r.get(2)?;
            let years: String = r.get(3)?;
            let categories: String = r.get(4)?;
            let types: String = r.get(5)?;
            Ok(ExportPreset {
                id: r.get(0)?,
                name: r.get(1)?,
                patient_ids: from_json(&patients),
                years: from_json(&years),
                category_ids: from_json(&categories),
                doc_types: from_json(&types),
                preset: r.get(6)?,
                max_bytes: r.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Save under `name`, replacing any preset already using it.
///
/// Overwriting rather than refusing: saving over a preset is what someone means
/// when they adjust a filter and save it under the same name, and a second
/// "Thyroid" that differs invisibly from the first is worse than losing the old
/// one.
pub fn save(
    conn: &Connection,
    user_id: &str,
    name: &str,
    p: &ExportPreset,
) -> Result<ExportPreset, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the preset a name.".into());
    }

    let id = conn
        .query_row(
            "SELECT id FROM export_preset WHERE owner_user_id = ?1 AND lower(name) = lower(?2)",
            params![user_id, name],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_else(|_| ulid::Ulid::new().to_string());

    conn.execute(
        "INSERT INTO export_preset
           (id, owner_user_id, name, patient_ids, years, category_ids, doc_types,
            preset, max_bytes, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9, datetime('now'))
         ON CONFLICT(id) DO UPDATE SET
           name = excluded.name, patient_ids = excluded.patient_ids,
           years = excluded.years, category_ids = excluded.category_ids,
           doc_types = excluded.doc_types, preset = excluded.preset,
           max_bytes = excluded.max_bytes",
        params![
            id,
            user_id,
            name,
            to_json(&p.patient_ids),
            to_json(&p.years),
            to_json(&p.category_ids),
            to_json(&p.doc_types),
            p.preset,
            p.max_bytes,
        ],
    )
    .map_err(|e| format!("cannot save preset: {e}"))?;

    Ok(ExportPreset {
        id,
        name: name.to_string(),
        ..p.clone()
    })
}

/// Note that a preset was used, so the list stays ordered by habit.
pub fn touch(conn: &Connection, user_id: &str, id: &str) {
    let _ = conn.execute(
        "UPDATE export_preset SET used_at = datetime('now')
         WHERE id = ?1 AND owner_user_id = ?2",
        params![id, user_id],
    );
}

pub fn remove(conn: &Connection, user_id: &str, id: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM export_preset WHERE id = ?1 AND owner_user_id = ?2",
        params![id, user_id],
    )
    .map_err(|e| format!("cannot delete preset: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> (std::path::PathBuf, Connection, String) {
        let dir = std::env::temp_dir().join(format!("mrt-preset-{name}-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = crate::db::open(&dir.join("app.db")).unwrap();
        let user = crate::db::ensure_user(&conn).unwrap();
        (dir, conn, user)
    }

    fn sample() -> ExportPreset {
        ExportPreset {
            id: String::new(),
            name: String::new(),
            patient_ids: vec!["pat-1".into()],
            years: vec!["2025".into(), "2024".into()],
            category_ids: vec!["cat-thyroid".into()],
            doc_types: vec!["report".into()],
            preset: "emailSafe".into(),
            max_bytes: Some(25 * 1024 * 1024),
        }
    }

    #[test]
    fn a_saved_preset_comes_back_with_every_filter_intact() {
        let (_d, conn, user) = fixture("roundtrip");
        save(&conn, &user, "Thyroid for Dr Karim", &sample()).unwrap();

        let all = list(&conn, &user).unwrap();
        assert_eq!(all.len(), 1);
        let p = &all[0];
        assert_eq!(p.name, "Thyroid for Dr Karim");
        assert_eq!(p.patient_ids, vec!["pat-1"]);
        assert_eq!(p.years, vec!["2025", "2024"]);
        assert_eq!(p.category_ids, vec!["cat-thyroid"]);
        assert_eq!(p.doc_types, vec!["report"]);
        assert_eq!(p.preset, "emailSafe");
        assert_eq!(p.max_bytes, Some(25 * 1024 * 1024));
    }

    #[test]
    fn saving_the_same_name_again_replaces_it_rather_than_duplicating() {
        let (_d, conn, user) = fixture("replace");
        save(&conn, &user, "Thyroid", &sample()).unwrap();

        let mut changed = sample();
        changed.years = vec!["2023".into()];
        changed.preset = "original".into();
        save(&conn, &user, "thyroid", &changed).unwrap();

        let all = list(&conn, &user).unwrap();
        assert_eq!(all.len(), 1, "case-insensitively the same name");
        assert_eq!(all[0].years, vec!["2023"]);
        assert_eq!(all[0].preset, "original");
    }

    #[test]
    fn an_unnamed_preset_is_refused() {
        let (_d, conn, user) = fixture("noname");
        let err = save(&conn, &user, "  ", &sample()).unwrap_err();
        assert!(err.contains("Give the preset a name"), "{err}");
    }

    #[test]
    fn empty_filters_survive_the_round_trip_as_empty_not_missing() {
        let (_d, conn, user) = fixture("empty");
        let mut everything = sample();
        everything.patient_ids.clear();
        everything.years.clear();
        everything.category_ids.clear();
        everything.doc_types.clear();
        everything.max_bytes = None;
        save(&conn, &user, "Everything", &everything).unwrap();

        let p = list(&conn, &user).unwrap().remove(0);
        assert!(p.patient_ids.is_empty() && p.years.is_empty());
        assert_eq!(p.max_bytes, None, "no split is a real choice, not a missing one");
    }

    #[test]
    fn the_most_recently_used_preset_is_offered_first() {
        let (_d, conn, user) = fixture("order");
        let a = save(&conn, &user, "Alpha", &sample()).unwrap();
        save(&conn, &user, "Beta", &sample()).unwrap();

        // Alphabetical until one is used.
        assert_eq!(list(&conn, &user).unwrap()[0].name, "Alpha");

        let b = list(&conn, &user).unwrap().into_iter().find(|p| p.name == "Beta").unwrap();
        touch(&conn, &user, &b.id);
        assert_eq!(list(&conn, &user).unwrap()[0].name, "Beta", "used last, offered first");

        touch(&conn, &user, &a.id);
        assert_eq!(list(&conn, &user).unwrap()[0].name, "Alpha");
    }

    #[test]
    fn deleting_one_leaves_the_others() {
        let (_d, conn, user) = fixture("delete");
        let a = save(&conn, &user, "Alpha", &sample()).unwrap();
        save(&conn, &user, "Beta", &sample()).unwrap();

        remove(&conn, &user, &a.id).unwrap();
        let names: Vec<String> = list(&conn, &user).unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["Beta"]);
    }

    #[test]
    fn presets_belong_to_their_owner() {
        let (_d, conn, user) = fixture("owner");
        save(&conn, &user, "Mine", &sample()).unwrap();
        assert!(list(&conn, "01OTHERUSER0000000000000000").unwrap().is_empty());
    }
}
