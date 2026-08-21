//! Database open, migration and health.
//!
//! Hand-written SQL against rusqlite rather than an ORM: the schema has several
//! constraints an ORM would obscure or refuse to express (a UNIQUE index on
//! `upper(folder_slug)`, an external-content FTS5 virtual table, bare-text dates
//! with sentinel values), and the whole surface is about twenty queries.

use std::sync::Mutex;

use rusqlite::Connection;
use serde::Serialize;

/// Every migration ever shipped, in order. Applied by version number, never
/// re-run, and never edited after release — a changed migration silently diverges
/// existing installs from new ones.
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../../db/migrations/001_init.sql")),
    (2, include_str!("../../db/migrations/002_fts.sql")),
    (3, include_str!("../../db/migrations/003_ingest_ocr.sql")),
    (4, include_str!("../../db/migrations/004_export_presets.sql")),
    (5, include_str!("../../db/migrations/005_settings.sql")),
    (6, include_str!("../../db/migrations/006_drive.sql")),
];

pub struct Db(pub Mutex<Connection>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbHealth {
    pub sqlite_version: String,
    pub fts5: bool,
    pub journal_mode: String,
    pub schema_version: i64,
    pub db_path: String,
    pub vault_path: String,
    pub patient_count: i64,
    pub document_count: i64,
    pub pending_journal_ops: i64,
}

/// Read one stored setting.
pub fn setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM app_setting WHERE key = ?1",
        rusqlite::params![key],
        |r| r.get(0),
    )
    .ok()
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_setting (key, value, updated_at) VALUES (?1, ?2, datetime('now'))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        rusqlite::params![key, value],
    )
    .map_err(|e| format!("cannot save setting: {e}"))?;
    Ok(())
}

/// Forget one stored setting. Missing is not an error — the caller wants it gone.
pub fn clear_setting(conn: &Connection, key: &str) -> Result<(), String> {
    conn.execute("DELETE FROM app_setting WHERE key = ?1", rusqlite::params![key])
        .map_err(|e| format!("cannot clear setting: {e}"))?;
    Ok(())
}

/// How the database was obtained, so the window can say so.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OpenOutcome {
    /// The ordinary path: the existing database opened.
    Existing,
    /// Nothing was there. A first run, or a machine that has only just been set up.
    Created,
    /// The file was unreadable and a snapshot took its place. The damaged file is
    /// kept, because a corrupt database still holds rows a person might want back.
    Restored { from: String, kept: String },
}

/// Open the database, restoring it from a snapshot if what is there cannot be read.
///
/// The snapshot is looked for beside the vault first, then in any Google Drive
/// folder that has a backup — which is what makes a new machine work: install the
/// app, let Drive sync, launch, and the library is already there.
pub fn open_or_restore(
    path: &std::path::Path,
    snapshots: &[std::path::PathBuf],
) -> Result<(Connection, OpenOutcome), String> {
    if !path.exists() {
        for snapshot in snapshots.iter().filter(|p| p.exists()) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::copy(snapshot, path).is_ok() {
                if let Ok(conn) = open(path) {
                    return Ok((
                        conn,
                        OpenOutcome::Restored {
                            from: snapshot.display().to_string(),
                            kept: String::new(),
                        },
                    ));
                }
                // Unusable snapshot; try the next one rather than adopting it.
                let _ = std::fs::remove_file(path);
            }
        }
        return open(path).map(|c| (c, OpenOutcome::Created));
    }

    match open(path).and_then(|conn| check_integrity(&conn).map(|()| conn)) {
        Ok(conn) => Ok((conn, OpenOutcome::Existing)),
        Err(first) => {
            // Move the damaged file aside rather than deleting it. It is still the
            // most recent copy of everything typed since the last snapshot.
            let kept = path.with_extension(format!(
                "corrupt-{}.db",
                ulid::Ulid::new().to_string().to_lowercase()
            ));
            std::fs::rename(path, &kept)
                .map_err(|e| format!("database is unreadable ({first}) and cannot be moved: {e}"))?;
            // WAL sidecars belong to the file that was moved.
            for suffix in ["-wal", "-shm"] {
                let _ = std::fs::remove_file(path.with_file_name(format!(
                    "{}{suffix}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                )));
            }

            for snapshot in snapshots.iter().filter(|p| p.exists()) {
                if std::fs::copy(snapshot, path).is_ok() {
                    if let Ok(conn) = open(path) {
                        return Ok((
                            conn,
                            OpenOutcome::Restored {
                                from: snapshot.display().to_string(),
                                kept: kept.display().to_string(),
                            },
                        ));
                    }
                    let _ = std::fs::remove_file(path);
                }
            }

            // No snapshot anywhere. Start clean rather than refusing to launch:
            // the vault is still on disk and "Rescan vault" rebuilds from it.
            open(path).map(|c| (c, OpenOutcome::Created))
        }
    }
}

fn check_integrity(conn: &Connection) -> Result<(), String> {
    let verdict: String = conn
        .query_row("PRAGMA quick_check(1)", [], |r| r.get(0))
        .map_err(|e| format!("integrity check failed: {e}"))?;
    if verdict == "ok" {
        Ok(())
    } else {
        Err(format!("database is damaged: {verdict}"))
    }
}

pub fn open(path: &std::path::Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {parent:?}: {e}"))?;
    }

    let conn = Connection::open(path).map_err(|e| format!("cannot open {path:?}: {e}"))?;

    // WAL lets a background ingest worker write while the UI reads. NORMAL
    // synchronous is the right trade under WAL: a crash can lose the last
    // transaction but cannot corrupt the file, and every write here is
    // reconstructible from the vault.
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| format!("cannot enable WAL: {e}"))?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|e| format!("cannot set synchronous: {e}"))?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| format!("cannot enable foreign keys: {e}"))?;
    // A locked DB should wait, not fail: Defender and OneDrive both touch these files.
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| format!("cannot set busy timeout: {e}"))?;

    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<(), String> {
    let has_table: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| format!("cannot inspect schema: {e}"))?
        == 1;

    let current: i64 = if has_table {
        conn.query_row(
            "SELECT coalesce(max(version), 0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("cannot read schema version: {e}"))?
    } else {
        0
    };

    for (version, sql) in MIGRATIONS {
        if *version <= current {
            continue;
        }
        conn.execute_batch(sql)
            .map_err(|e| format!("migration {version} failed: {e}"))?;
    }
    Ok(())
}

/// The single local user. Auth arrives in Phase 2; until then every row still
/// carries a real `owner_user_id`, because adding a tenant column to a populated
/// database later is the worst migration in this design.
pub fn ensure_user(conn: &Connection) -> Result<String, String> {
    let existing: Option<String> = conn
        .query_row("SELECT id FROM users ORDER BY created_at LIMIT 1", [], |r| {
            r.get(0)
        })
        .ok();
    if let Some(id) = existing {
        return Ok(id);
    }

    let id = ulid::Ulid::new().to_string();
    conn.execute(
        "INSERT INTO users (id, email, pw_hash, created_at, updated_at)
         VALUES (?1, '', '', datetime('now'), datetime('now'))",
        rusqlite::params![id],
    )
    .map_err(|e| format!("cannot create local user: {e}"))?;
    Ok(id)
}

pub fn health(
    conn: &Connection,
    db_path: String,
    vault_path: String,
) -> Result<DbHealth, String> {
    let q = |sql: &str| -> Result<i64, String> {
        conn.query_row(sql, [], |r| r.get::<_, i64>(0))
            .map_err(|e| format!("{sql}: {e}"))
    };

    Ok(DbHealth {
        sqlite_version: conn
            .query_row("SELECT sqlite_version()", [], |r| r.get(0))
            .map_err(|e| e.to_string())?,
        fts5: q("SELECT EXISTS(SELECT 1 FROM pragma_compile_options WHERE compile_options LIKE 'ENABLE_FTS5%')")? == 1,
        journal_mode: conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .map_err(|e| e.to_string())?,
        schema_version: q("SELECT coalesce(max(version), 0) FROM schema_migrations")?,
        patient_count: q("SELECT count(*) FROM patients WHERE archived_at IS NULL")?,
        document_count: q("SELECT count(*) FROM documents WHERE trashed_at IS NULL")?,
        // Non-zero means a previous run died mid-rename and the journal must be
        // replayed before anything else touches the vault.
        pending_journal_ops: q("SELECT count(*) FROM fs_journal WHERE applied_at IS NULL")?,
        db_path,
        vault_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mrt-db-{name}-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A real snapshot, produced the way the app produces one.
    fn snapshot_of(dir: &std::path::Path, patient: &str) -> std::path::PathBuf {
        let live = dir.join("source.db");
        let conn = open(&live).unwrap();
        let user = ensure_user(&conn).unwrap();
        conn.execute(
            "INSERT INTO patients (id, owner_user_id, display_name, folder_slug, created_at, updated_at)
             VALUES ('p1', ?1, ?2, 'slug', datetime('now'), datetime('now'))",
            rusqlite::params![user, patient],
        )
        .unwrap();
        let snap = dir.join("snapshot.db");
        conn.execute(
            &format!("VACUUM INTO '{}'", snap.display().to_string().replace('\\', "/")),
            [],
        )
        .unwrap();
        snap
    }

    #[test]
    fn a_missing_database_is_restored_from_a_snapshot() {
        let dir = temp("missing");
        let snap = snapshot_of(&dir, "Rahim Uddin");
        let live = dir.join("app.db");

        let (conn, outcome) = open_or_restore(&live, &[snap]).unwrap();
        assert!(matches!(outcome, OpenOutcome::Restored { .. }), "{outcome:?}");

        let name: String = conn
            .query_row("SELECT display_name FROM patients", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "Rahim Uddin", "the library came back with it");
    }

    #[test]
    fn a_damaged_database_is_replaced_and_the_damaged_one_is_kept() {
        let dir = temp("corrupt");
        let snap = snapshot_of(&dir, "Karim Uddin");
        let live = dir.join("app.db");

        // A file that is the right size and shape but is not a database any more
        // — what a failing disk or an interrupted sync leaves behind.
        std::fs::write(&live, vec![0x51; 40_000]).unwrap();

        let (conn, outcome) = open_or_restore(&live, &[snap]).unwrap();
        let OpenOutcome::Restored { kept, .. } = outcome else {
            panic!("expected a restore");
        };

        assert!(!kept.is_empty(), "the damaged file must be kept, not deleted");
        assert!(std::path::Path::new(&kept).exists());
        let name: String = conn
            .query_row("SELECT display_name FROM patients", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "Karim Uddin");
    }

    #[test]
    fn with_no_snapshot_anywhere_the_app_still_starts() {
        let dir = temp("nosnapshot");
        let live = dir.join("app.db");
        std::fs::write(&live, b"not a database at all").unwrap();

        let (conn, outcome) = open_or_restore(&live, &[]).unwrap();
        assert_eq!(outcome, OpenOutcome::Created, "refusing to launch helps nobody");
        // Usable: the vault is still on disk and Rescan vault rebuilds from it.
        assert_eq!(ensure_user(&conn).is_ok(), true);
    }

    #[test]
    fn a_healthy_database_is_left_exactly_as_it_is() {
        let dir = temp("healthy");
        let snap = snapshot_of(&dir, "Should Not Be Used");
        let live = dir.join("app.db");

        {
            let conn = open(&live).unwrap();
            let user = ensure_user(&conn).unwrap();
            conn.execute(
                "INSERT INTO patients (id, owner_user_id, display_name, folder_slug, created_at, updated_at)
                 VALUES ('p9', ?1, 'The Real One', 'real', datetime('now'), datetime('now'))",
                rusqlite::params![user],
            )
            .unwrap();
        }

        let (conn, outcome) = open_or_restore(&live, &[snap]).unwrap();
        assert_eq!(outcome, OpenOutcome::Existing);
        let name: String = conn
            .query_row("SELECT display_name FROM patients", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "The Real One", "a snapshot must never overwrite a working database");
    }

    #[test]
    fn an_unreadable_snapshot_is_passed_over_for_the_next_one() {
        let dir = temp("badsnap");
        let good = snapshot_of(&dir, "Good Copy");
        let bad = dir.join("bad-snapshot.db");
        std::fs::write(&bad, b"corrupt too").unwrap();
        let live = dir.join("app.db");

        let (conn, outcome) = open_or_restore(&live, &[bad, good]).unwrap();
        assert!(matches!(outcome, OpenOutcome::Restored { .. }));
        let name: String = conn
            .query_row("SELECT display_name FROM patients", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "Good Copy");
    }

    #[test]
    fn settings_round_trip_and_overwrite() {
        let dir = temp("settings");
        let conn = open(&dir.join("app.db")).unwrap();

        assert_eq!(setting(&conn, "drive_folder"), None);
        set_setting(&conn, "drive_folder", "G:\\My Drive").unwrap();
        assert_eq!(setting(&conn, "drive_folder").as_deref(), Some("G:\\My Drive"));

        set_setting(&conn, "drive_folder", "D:\\Backup").unwrap();
        assert_eq!(setting(&conn, "drive_folder").as_deref(), Some("D:\\Backup"));
    }
}
