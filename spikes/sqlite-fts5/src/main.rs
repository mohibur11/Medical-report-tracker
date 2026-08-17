//! Phase 0 spike 2: prove SQLite is usable as designed before the schema commits to it.
//!
//! Three things must hold, and each fails silently at a different moment:
//!   1. FTS5 is compiled in. Absent, `CREATE VIRTUAL TABLE ... USING fts5` errors at
//!      runtime — after the schema already depends on full-text search.
//!   2. WAL mode engages. The plan puts the live DB in %LOCALAPPDATA% precisely so a
//!      cloud-synced WAL sidecar cannot corrupt it; that only matters if WAL is on.
//!   3. `VACUUM INTO` exists (SQLite >= 3.27). It is the snapshot-on-close backup.

use rusqlite::{Connection, Result};

fn main() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    let version: String = conn.query_row("SELECT sqlite_version()", [], |r| r.get(0))?;
    println!("=== SQLITE / FTS5 SPIKE ===");
    println!("sqlite version : {version}");

    // --- 1. FTS5 -----------------------------------------------------------
    let fts5_compiled: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_compile_options WHERE compile_options LIKE 'ENABLE_FTS5%')",
            [],
            |r| r.get::<_, i32>(0),
        )
        .map(|v| v == 1)?;
    println!("ENABLE_FTS5    : {fts5_compiled}");

    conn.execute_batch(
        "CREATE VIRTUAL TABLE document_fts USING fts5(title, ocr_text, tokenize='unicode61');",
    )?;
    println!("create fts5    : OK");

    conn.execute(
        "INSERT INTO document_fts (title, ocr_text) VALUES (?1, ?2)",
        (
            "Thyroid Profile",
            "POPULAR DIAGNOSTIC CENTRE Sample Collected 14/03/2026 TSH 6.82 uIU/mL elevated creatinine",
        ),
    )?;
    conn.execute(
        "INSERT INTO document_fts (title, ocr_text) VALUES (?1, ?2)",
        ("Lipid Profile", "Cholesterol 220 mg/dL Triglycerides 180 HDL 38"),
    )?;

    // The plan's justification for FTS5: searching a term across five years of
    // scans is worth more than perfect field extraction, and degrades gracefully.
    let hits: i64 = conn.query_row(
        "SELECT count(*) FROM document_fts WHERE document_fts MATCH 'creatinine'",
        [],
        |r| r.get(0),
    )?;
    println!("match query    : {hits} hit(s) for 'creatinine'");

    let snippet: String = conn.query_row(
        "SELECT snippet(document_fts, 1, '[', ']', '...', 8) \
         FROM document_fts WHERE document_fts MATCH 'thyroid OR tsh'",
        [],
        |r| r.get(0),
    )?;
    println!("snippet()      : {snippet}");

    let prefix: i64 = conn.query_row(
        "SELECT count(*) FROM document_fts WHERE document_fts MATCH 'chol*'",
        [],
        |r| r.get(0),
    )?;
    println!("prefix query   : {prefix} hit(s) for 'chol*'");

    // --- 2. WAL + 3. VACUUM INTO -------------------------------------------
    let dir = std::env::temp_dir().join("mrt-spike");
    std::fs::create_dir_all(&dir).ok();
    let db_path = dir.join("spike.db");
    let _ = std::fs::remove_file(&db_path);

    let disk = Connection::open(&db_path)?;
    let mode: String = disk.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    println!("journal_mode   : {mode}");

    disk.execute_batch("CREATE TABLE t(a); INSERT INTO t VALUES (1),(2),(3);")?;

    let snap = dir.join("snapshot.db");
    let _ = std::fs::remove_file(&snap);
    disk.execute(&format!("VACUUM INTO '{}'", snap.to_string_lossy().replace('\\', "/")), [])?;
    let snap_size = std::fs::metadata(&snap).map(|m| m.len()).unwrap_or(0);
    println!("VACUUM INTO    : OK ({snap_size} bytes)");

    println!("\n--- GATE ---");
    let pass = fts5_compiled && mode.eq_ignore_ascii_case("wal") && snap_size > 0;
    println!("{}", if pass { "PASS - schema may depend on FTS5, WAL and VACUUM INTO." } else { "FAIL" });
    if !pass {
        std::process::exit(1);
    }
    Ok(())
}
