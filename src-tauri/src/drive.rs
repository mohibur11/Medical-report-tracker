//! The vault, mirrored into Google Drive over the API.
//!
//! Drive has no paths. Every file is an id, and a folder is just a file that
//! other files name as their parent — so the shape of the vault has to be
//! recorded locally (`drive_file`) or each sync would have to walk the whole
//! remote tree to discover what already exists.
//!
//! Uploads are resumable rather than multipart, even for small files. It costs
//! one extra request and removes the case that matters: a twenty-megabyte scan
//! going up over a phone tether, interrupted at eighteen.
//!
//! Everything reaches only files this app created — the `drive.file` scope — so
//! nothing here can see, change or delete the rest of the user's Drive.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::google;
use crate::sync::{BackupTarget, SyncReport};

const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const FILES_ENDPOINT: &str = "https://www.googleapis.com/drive/v3/files";
const UPLOAD_ENDPOINT: &str = "https://www.googleapis.com/upload/drive/v3/files";

/// The folder created in the root of the user's Drive.
pub const ROOT_NAME: &str = "MedicineReportTracker";

/// Never mirrored, matching the folder target.
const SKIP_DIRS: &[&str] = &["Exports"];

pub struct DriveApiTarget<'a> {
    conn: &'a Connection,
}

impl<'a> DriveApiTarget<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        DriveApiTarget { conn }
    }

    fn token(&self) -> Result<String, String> {
        google::access_token(self.conn)
    }
}

/// One local file that the remote copy does not match.
#[derive(Debug, PartialEq, Eq)]
pub struct Upload {
    pub rel_path: String,
    pub size: u64,
    pub mtime: i64,
}

/// What a local file's identity is, for deciding whether it has already gone up.
fn stamp(path: &Path) -> Option<(u64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some((meta.len(), mtime))
}

/// Which files still need uploading.
///
/// Pure, and separated from the HTTP so the decision — the part that can silently
/// skip somebody's report — can be tested without a network.
pub fn plan_uploads(conn: &Connection, vault_root: &Path) -> Result<Vec<Upload>, String> {
    let mut known = std::collections::HashMap::<String, (Option<i64>, Option<i64>)>::new();
    {
        let mut stmt = conn
            .prepare("SELECT rel_path, size, mtime FROM drive_file WHERE is_folder = 0")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    (r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?),
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            known.insert(row.0, row.1);
        }
    }

    let mut out = Vec::new();
    let mut stack = vec![vault_root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if path.is_dir() {
                if !SKIP_DIRS.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                    stack.push(path);
                }
                continue;
            }
            // The same exclusions as the folder target: these are exactly the
            // files that must not travel.
            if name.ends_with(".partial") || name.ends_with("-wal") || name.ends_with("-shm") {
                continue;
            }

            let Ok(rel) = path.strip_prefix(vault_root) else { continue };
            let rel_path = rel.to_string_lossy().replace('/', "\\");
            let Some((size, mtime)) = stamp(&path) else { continue };

            let unchanged = known
                .get(&rel_path)
                .map(|(s, m)| *s == Some(size as i64) && *m == Some(mtime))
                .unwrap_or(false);
            if !unchanged {
                out.push(Upload { rel_path, size, mtime });
            }
        }
    }

    // Smallest first, so a slow link shows progress rather than stalling on one
    // large scan for minutes.
    out.sort_by(|a, b| a.size.cmp(&b.size).then_with(|| a.rel_path.cmp(&b.rel_path)));
    Ok(out)
}

fn remember(
    conn: &Connection,
    rel_path: &str,
    file_id: &str,
    is_folder: bool,
    size: Option<u64>,
    mtime: Option<i64>,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO drive_file (rel_path, file_id, is_folder, size, mtime, uploaded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
         ON CONFLICT(rel_path) DO UPDATE SET
           file_id = excluded.file_id, is_folder = excluded.is_folder,
           size = excluded.size, mtime = excluded.mtime, uploaded_at = excluded.uploaded_at",
        params![rel_path, file_id, is_folder as i64, size.map(|s| s as i64), mtime],
    )
    .map_err(|e| format!("cannot record the upload: {e}"))?;
    Ok(())
}

fn known_id(conn: &Connection, rel_path: &str) -> Option<String> {
    conn.query_row(
        "SELECT file_id FROM drive_file WHERE rel_path = ?1",
        params![rel_path],
        |r| r.get(0),
    )
    .ok()
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        // A large scan over a slow link needs longer than the default.
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .unwrap_or_default()
}

/// Escape a value for Drive's query language, where strings are single-quoted.
fn escape_query(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Find a child by name, or make it.
fn ensure_folder_named(token: &str, name: &str, parent: Option<&str>) -> Result<String, String> {
    let parent_clause = match parent {
        Some(id) => format!(" and '{}' in parents", escape_query(id)),
        None => " and 'root' in parents".to_string(),
    };
    let q = format!(
        "name = '{}' and mimeType = '{FOLDER_MIME}' and trashed = false{parent_clause}",
        escape_query(name)
    );

    let found = client()
        .get(FILES_ENDPOINT)
        .bearer_auth(token)
        .query(&[("q", q.as_str()), ("fields", "files(id,name)"), ("pageSize", "1")])
        .send()
        .map_err(|e| format!("cannot reach Google Drive: {e}"))?;
    let found: serde_json::Value = found
        .json()
        .map_err(|e| format!("Drive's reply could not be read: {e}"))?;
    if let Some(id) = found["files"][0]["id"].as_str() {
        return Ok(id.to_string());
    }

    let mut metadata = serde_json::json!({ "name": name, "mimeType": FOLDER_MIME });
    if let Some(id) = parent {
        metadata["parents"] = serde_json::json!([id]);
    }
    let created = client()
        .post(FILES_ENDPOINT)
        .bearer_auth(token)
        .query(&[("fields", "id")])
        .json(&metadata)
        .send()
        .map_err(|e| format!("cannot create the folder in Drive: {e}"))?;
    if !created.status().is_success() {
        return Err(format!(
            "Drive refused to create '{name}': {}",
            created.text().unwrap_or_default()
        ));
    }
    let created: serde_json::Value = created
        .json()
        .map_err(|e| format!("Drive's reply could not be read: {e}"))?;
    created["id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "Drive created a folder without an id".to_string())
}

/// The Drive id of the folder holding `rel_path`, creating the chain as needed.
fn ensure_parents(conn: &Connection, token: &str, rel_path: &str) -> Result<String, String> {
    let mut parent = match known_id(conn, "") {
        Some(id) => id,
        None => {
            let id = ensure_folder_named(token, ROOT_NAME, None)?;
            remember(conn, "", &id, true, None, None)?;
            id
        }
    };

    let mut walked = String::new();
    let segments: Vec<&str> = rel_path.split('\\').collect();
    for segment in segments.iter().take(segments.len().saturating_sub(1)) {
        if segment.is_empty() {
            continue;
        }
        if !walked.is_empty() {
            walked.push('\\');
        }
        walked.push_str(segment);

        parent = match known_id(conn, &walked) {
            Some(id) => id,
            None => {
                let id = ensure_folder_named(token, segment, Some(&parent))?;
                remember(conn, &walked, &id, true, None, None)?;
                id
            }
        };
    }

    Ok(parent)
}

/// Upload one file, resumably, creating or replacing as needed.
fn upload_file(
    conn: &Connection,
    token: &str,
    vault_root: &Path,
    item: &Upload,
) -> Result<(), String> {
    let path = vault_root.join(item.rel_path.replace('\\', "/"));
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", item.rel_path))?;
    let name = Path::new(&item.rel_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| item.rel_path.clone());

    let existing = known_id(conn, &item.rel_path);
    let session = match &existing {
        // Replacing content keeps the id, so a link to the file in Drive survives.
        Some(id) => client()
            .patch(format!("{UPLOAD_ENDPOINT}/{id}"))
            .bearer_auth(token)
            .query(&[("uploadType", "resumable")])
            .json(&serde_json::json!({ "name": name })),
        None => {
            let parent = ensure_parents(conn, token, &item.rel_path)?;
            client()
                .post(UPLOAD_ENDPOINT)
                .bearer_auth(token)
                .query(&[("uploadType", "resumable"), ("fields", "id")])
                .json(&serde_json::json!({ "name": name, "parents": [parent] }))
        }
    };

    let started = session
        .send()
        .map_err(|e| format!("cannot start the upload of {}: {e}", item.rel_path))?;
    if !started.status().is_success() {
        return Err(format!(
            "Drive refused {}: {}",
            item.rel_path,
            started.text().unwrap_or_default()
        ));
    }
    let location = started
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| format!("Drive did not offer an upload session for {}", item.rel_path))?
        .to_string();

    let done = client()
        .put(location)
        .bearer_auth(token)
        .body(bytes)
        .send()
        .map_err(|e| format!("cannot upload {}: {e}", item.rel_path))?;
    if !done.status().is_success() {
        return Err(format!(
            "Drive rejected {}: {}",
            item.rel_path,
            done.text().unwrap_or_default()
        ));
    }

    let id = match existing {
        Some(id) => id,
        None => {
            let body: serde_json::Value = done
                .json()
                .map_err(|e| format!("Drive's reply could not be read: {e}"))?;
            body["id"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("Drive stored {} without an id", item.rel_path))?
        }
    };

    remember(conn, &item.rel_path, &id, false, Some(item.size), Some(item.mtime))
}

#[derive(Debug)]
struct RemoteFile {
    id: String,
    rel_path: String,
    size: u64,
}

/// Everything under the app's folder in Drive, with the paths it came from.
fn list_remote(conn: &Connection, token: &str) -> Result<Vec<RemoteFile>, String> {
    let root = match known_id(conn, "") {
        Some(id) => id,
        None => match find_root(token)? {
            Some(id) => {
                remember(conn, "", &id, true, None, None)?;
                id
            }
            None => return Ok(Vec::new()),
        },
    };

    let mut out = Vec::new();
    let mut stack = vec![(root, String::new())];

    while let Some((folder, prefix)) = stack.pop() {
        let mut page: Option<String> = None;
        loop {
            let q = format!("'{}' in parents and trashed = false", escape_query(&folder));
            let mut request = client()
                .get(FILES_ENDPOINT)
                .bearer_auth(token)
                .query(&[
                    ("q", q.as_str()),
                    ("fields", "nextPageToken, files(id,name,mimeType,size)"),
                    ("pageSize", "200"),
                ]);
            if let Some(token) = &page {
                request = request.query(&[("pageToken", token.as_str())]);
            }

            let response = request
                .send()
                .map_err(|e| format!("cannot list the Drive folder: {e}"))?;
            if !response.status().is_success() {
                return Err(format!(
                    "Drive refused to list the backup: {}",
                    response.text().unwrap_or_default()
                ));
            }
            let body: serde_json::Value = response
                .json()
                .map_err(|e| format!("Drive's reply could not be read: {e}"))?;

            for file in body["files"].as_array().unwrap_or(&Vec::new()) {
                let (Some(id), Some(name)) = (file["id"].as_str(), file["name"].as_str()) else {
                    continue;
                };
                let rel = if prefix.is_empty() {
                    name.to_string()
                } else {
                    format!("{prefix}\\{name}")
                };
                if file["mimeType"].as_str() == Some(FOLDER_MIME) {
                    stack.push((id.to_string(), rel));
                } else {
                    out.push(RemoteFile {
                        id: id.to_string(),
                        rel_path: rel,
                        size: file["size"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0),
                    });
                }
            }

            page = body["nextPageToken"].as_str().map(str::to_string);
            if page.is_none() {
                break;
            }
        }
    }

    Ok(out)
}

fn find_root(token: &str) -> Result<Option<String>, String> {
    let q = format!(
        "name = '{ROOT_NAME}' and mimeType = '{FOLDER_MIME}' and trashed = false and 'root' in parents"
    );
    let response = client()
        .get(FILES_ENDPOINT)
        .bearer_auth(token)
        .query(&[("q", q.as_str()), ("fields", "files(id)"), ("pageSize", "1")])
        .send()
        .map_err(|e| format!("cannot reach Google Drive: {e}"))?;
    let body: serde_json::Value = response
        .json()
        .map_err(|e| format!("Drive's reply could not be read: {e}"))?;
    Ok(body["files"][0]["id"].as_str().map(str::to_string))
}

fn download(token: &str, file: &RemoteFile, dest: &Path) -> Result<u64, String> {
    let response = client()
        .get(format!("{FILES_ENDPOINT}/{}", file.id))
        .bearer_auth(token)
        .query(&[("alt", "media")])
        .send()
        .map_err(|e| format!("cannot download {}: {e}", file.rel_path))?;
    if !response.status().is_success() {
        return Err(format!(
            "Drive refused {}: {}",
            file.rel_path,
            response.text().unwrap_or_default()
        ));
    }
    let bytes = response
        .bytes()
        .map_err(|e| format!("cannot read {}: {e}", file.rel_path))?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {parent:?}: {e}"))?;
    }
    // Written beside the target and renamed, so an interrupted download cannot
    // replace a good file with half of one.
    let temp = dest.with_extension("downloading");
    std::fs::write(&temp, &bytes).map_err(|e| format!("cannot write {}: {e}", file.rel_path))?;
    std::fs::rename(&temp, dest).map_err(|e| format!("cannot finish {}: {e}", file.rel_path))?;
    Ok(bytes.len() as u64)
}

impl BackupTarget for DriveApiTarget<'_> {
    fn describe(&self) -> String {
        match google::account(self.conn).email {
            Some(email) => format!("Google Drive ({email}) — {ROOT_NAME}"),
            None => format!("Google Drive — {ROOT_NAME}"),
        }
    }

    fn available(&self) -> bool {
        google::account(self.conn).connected
    }

    fn push(&self, vault_root: &Path) -> Result<SyncReport, String> {
        let token = self.token()?;
        let planned = plan_uploads(self.conn, vault_root)?;
        let total: usize = count_files(vault_root);

        let mut report = SyncReport {
            adopted: 0,
            location: self.describe(),
            unchanged: total.saturating_sub(planned.len()),
            ..Default::default()
        };

        for item in &planned {
            match upload_file(self.conn, &token, vault_root, item) {
                Ok(()) => {
                    report.copied += 1;
                    report.bytes += item.size;
                }
                // One rejected file must not cost the rest of the backup.
                Err(e) => report.failed.push(e),
            }
        }

        Ok(report)
    }

    fn pull(&self, vault_root: &Path) -> Result<SyncReport, String> {
        let token = self.token()?;
        let remote = list_remote(self.conn, &token)?;
        if remote.is_empty() {
            return Err("There is no backup in this Google Drive account yet.".into());
        }

        let mut report = SyncReport {
            adopted: 0,
            location: self.describe(),
            ..Default::default()
        };

        for file in &remote {
            let dest = vault_root.join(file.rel_path.replace('\\', "/"));
            // Same rule as the folder target: missing, or a different length.
            let local = stamp(&dest).map(|(size, _)| size);
            if local == Some(file.size) && file.size > 0 {
                report.unchanged += 1;
                continue;
            }
            match download(&token, file, &dest) {
                Ok(n) => {
                    report.copied += 1;
                    report.bytes += n;
                }
                Err(e) => report.failed.push(e),
            }
        }

        Ok(report)
    }

    fn snapshot(&self) -> Option<PathBuf> {
        // The snapshot in Drive is an id, not a path. `pull` brings it down with
        // everything else, and the startup restore reads it from the vault.
        None
    }
}

fn count_files(root: &Path) -> usize {
    let mut n = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if !SKIP_DIRS.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                    stack.push(path);
                }
            } else if !(name.ends_with(".partial") || name.ends_with("-wal") || name.ends_with("-shm")) {
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fx {
        dir: PathBuf,
        conn: Connection,
    }

    impl Fx {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("mrt-drive-{name}-{}", ulid::Ulid::new()));
            std::fs::create_dir_all(dir.join("vault")).unwrap();
            let conn = crate::db::open(&dir.join("app.db")).unwrap();
            Fx { dir, conn }
        }

        fn vault(&self) -> PathBuf {
            self.dir.join("vault")
        }

        fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let p = self.vault().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes).unwrap();
            p
        }

        /// Pretend a file has already been uploaded, exactly as it is on disk.
        fn mark_uploaded(&self, rel: &str) {
            let path = self.vault().join(rel.replace('\\', "/"));
            let (size, mtime) = stamp(&path).unwrap();
            remember(&self.conn, rel, "remote-id", false, Some(size), Some(mtime)).unwrap();
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn everything_is_planned_on_the_first_sync() {
        let f = Fx::new("first");
        f.write("Rahim-Uddin/2025/a.jpg", b"one");
        f.write("Rahim-Uddin/2025/a.jpg.meta.json", b"{}");
        f.write("medicine-report-tracker.backup.db", b"snapshot");

        let plan = plan_uploads(&f.conn, &f.vault()).unwrap();
        assert_eq!(plan.len(), 3);
        assert!(plan.iter().any(|u| u.rel_path == "Rahim-Uddin\\2025\\a.jpg"));
    }

    #[test]
    fn a_file_already_uploaded_unchanged_is_not_planned_again() {
        let f = Fx::new("skip");
        f.write("a.jpg", b"one");
        f.write("b.jpg", b"two");
        f.mark_uploaded("a.jpg");

        let plan = plan_uploads(&f.conn, &f.vault()).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].rel_path, "b.jpg");
    }

    #[test]
    fn an_edited_file_is_planned_again() {
        let f = Fx::new("changed");
        f.write("a.jpg", b"one");
        f.mark_uploaded("a.jpg");
        assert!(plan_uploads(&f.conn, &f.vault()).unwrap().is_empty());

        // Editing a document rewrites it in place; a longer file is a different one.
        f.write("a.jpg", b"one, corrected");
        let plan = plan_uploads(&f.conn, &f.vault()).unwrap();
        assert_eq!(plan.len(), 1, "a changed file must never be skipped");
    }

    #[test]
    fn exports_and_wal_sidecars_are_never_planned() {
        let f = Fx::new("exclusions");
        f.write("Rahim-Uddin/2025/a.jpg", b"keep");
        f.write("Exports/Rahim Uddin — Medical History.pdf", b"rebuildable");
        f.write("medicine-report-tracker.backup.db-wal", b"half a transaction");
        f.write("medicine-report-tracker.backup.db.partial", b"interrupted");

        let plan = plan_uploads(&f.conn, &f.vault()).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].rel_path, "Rahim-Uddin\\2025\\a.jpg");
    }

    #[test]
    fn the_smallest_files_go_first() {
        let f = Fx::new("order");
        f.write("big.jpg", &vec![0u8; 5000]);
        f.write("small.jpg", b"tiny");
        f.write("middle.jpg", &vec![0u8; 500]);

        let plan = plan_uploads(&f.conn, &f.vault()).unwrap();
        let names: Vec<&str> = plan.iter().map(|u| u.rel_path.as_str()).collect();
        assert_eq!(names, vec!["small.jpg", "middle.jpg", "big.jpg"]);
    }

    #[test]
    fn folder_ids_are_remembered_so_they_are_created_once() {
        let f = Fx::new("folders");
        remember(&f.conn, "Rahim-Uddin", "folder-1", true, None, None).unwrap();
        assert_eq!(known_id(&f.conn, "Rahim-Uddin").as_deref(), Some("folder-1"));

        // Re-recording the same path replaces rather than duplicating it.
        remember(&f.conn, "Rahim-Uddin", "folder-2", true, None, None).unwrap();
        let n: i64 = f
            .conn
            .query_row("SELECT count(*) FROM drive_file", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(known_id(&f.conn, "Rahim-Uddin").as_deref(), Some("folder-2"));
    }

    #[test]
    fn a_query_value_cannot_break_out_of_its_quotes() {
        // A patient named O'Brien is a folder name in a Drive query string.
        assert_eq!(escape_query("O'Brien"), "O\\'Brien");
        assert_eq!(escape_query("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn the_target_reports_itself_as_unavailable_until_an_account_is_connected() {
        let f = Fx::new("unconnected");
        let target = DriveApiTarget::new(&f.conn);
        assert!(!target.available());
        assert!(target.describe().contains("Google Drive"));
    }

    #[test]
    fn counting_matches_what_planning_walks() {
        let f = Fx::new("count");
        f.write("a.jpg", b"one");
        f.write("sub/b.jpg", b"two");
        f.write("Exports/c.pdf", b"skip");
        assert_eq!(count_files(&f.vault()), 2);
    }
}
