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
