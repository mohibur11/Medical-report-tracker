//! Keeping a second copy of the vault somewhere that is not this disk.
//!
//! The most likely way to lose a decade of family medical history is not an
//! attacker — it is one dead drive. The vault is already plain files with
//! self-describing names and a JSON sidecar each, so a copy of it is a complete,
//! readable archive that does not need this app to be useful.
//!
//! Today the copy goes to a Google Drive folder mounted by Drive for desktop,
//! which means no account in the app, no tokens stored next to a deliberately
//! unencrypted database, and no weekly re-authorisation. `BackupTarget` exists so
//! that an implementation talking to the Drive API directly — needed on a machine
//! with no Drive client, and later on Android — can be dropped in without
//! changing what backup and restore mean.
//!
//! Deliberately NOT copied:
//!
//!  - `Exports/`, which is regenerated from the documents on demand and would
//!    otherwise consume the user's storage twice over.
//!  - The live database. It lives outside the vault precisely because syncing a
//!    WAL sidecar mid-write is a documented way to corrupt SQLite. The snapshot
//!    that `Back up now` writes into the vault is a single consistent file, and
//!    that is what travels.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Never copied, in either direction.
const SKIP_DIRS: &[&str] = &["Exports"];

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    /// Where the copy lives, for the UI to show.
    pub location: String,
    pub copied: usize,
    /// Already identical, so not copied again.
    pub unchanged: usize,
    pub bytes: u64,
    pub failed: Vec<String>,
    /// Files taken out of the backup because they are no longer in the vault.
    ///
    /// A backup that only ever adds is not a copy of the library, it is a copy of
    /// everything the library has ever contained: rename a person and both names
    /// live in Drive forever, delete a report and it stays.
    #[serde(default)]
    pub removed: usize,
    /// Restored files that became documents again.
    ///
    /// Copying the files back is only half of a restore. The database is what
    /// knows whose report a file is and what it says; on a phone that has been
    /// reinstalled there is no database, so the reconciler reads it back out of
    /// the filenames — which is the whole reason they are named the way they are.
    #[serde(default)]
    pub adopted: usize,
}

/// Somewhere a copy of the vault can live.
///
/// Both directions are file-by-file rather than archive-based: a backup that can
/// only be restored whole is no use when one document is corrupt, and an archive
/// cannot be read from Drive's web view.
pub trait BackupTarget {
    /// Human-readable, shown in the UI.
    fn describe(&self) -> String;
    /// Is the destination present right now? A Drive folder is not mounted when
    /// the client is signed out or stopped.
    fn available(&self) -> bool;
    /// Copy the vault out.
    fn push(&self, vault_root: &Path) -> Result<SyncReport, String>;
    /// Copy back anything missing or different locally.
    fn pull(&self, vault_root: &Path) -> Result<SyncReport, String>;
    /// The database snapshot held at the target, if there is one.
    fn snapshot(&self) -> Option<PathBuf>;
}

/// A folder that some sync client is watching — Google Drive for desktop today.
pub struct FolderTarget {
    /// The folder inside the sync root that holds this app's copy.
    root: PathBuf,
}

impl FolderTarget {
    /// `sync_root` is the mounted drive or synced folder, e.g. `G:\\My Drive`.
    pub fn new(sync_root: &Path) -> Self {
        FolderTarget {
            root: sync_root.join("MedicineReportTracker"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl BackupTarget for FolderTarget {
    fn describe(&self) -> String {
        self.root.display().to_string()
    }

    fn available(&self) -> bool {
        // The parent, not the folder itself: the app creates its own folder, but
        // it cannot create the drive that Drive for desktop mounts.
        self.root.parent().map(Path::exists).unwrap_or(false)
    }

    fn push(&self, vault_root: &Path) -> Result<SyncReport, String> {
        if !self.available() {
            return Err(format!(
                "{} is not there. Google Drive for desktop may be signed out or stopped.",
                self.root.parent().unwrap_or(&self.root).display()
            ));
        }
        std::fs::create_dir_all(&self.root)
            .map_err(|e| format!("cannot create {:?}: {e}", self.root))?;

        let mut report = copy_tree(vault_root, &self.root, self.describe())?;
        report.removed = prune_tree(vault_root, &self.root);
        Ok(report)
    }

    fn pull(&self, vault_root: &Path) -> Result<SyncReport, String> {
        if !self.root.exists() {
            return Err(format!("There is no backup at {}.", self.describe()));
        }
        copy_tree(&self.root, vault_root, self.describe())
    }

    fn snapshot(&self) -> Option<PathBuf> {
        let p = self.root.join(crate::backup::SNAPSHOT_NAME);
        p.exists().then_some(p)
    }
}

/// Take out of the backup whatever is no longer in the vault.
///
/// Returns how many files went. Empty directories left behind by the removals go
/// too, so a person renamed last month does not keep a folder in Drive forever.
///
/// Refuses to remove anything at all when the vault has nothing in it. That is
/// not a library somebody emptied on purpose — it is a fresh install, or a phone
/// whose storage was wiped, and it is the exact moment somebody would press
/// "Back up now" and destroy the only copy they have left.
fn prune_tree(vault_root: &Path, backup_root: &Path) -> usize {
    if !has_any_content(vault_root) {
        return 0;
    }

    let mut removed = 0;
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut stack = vec![backup_root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if path.is_dir() {
                // Never touched on the way in, so never touched on the way out.
                if SKIP_DIRS.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                    continue;
                }
                dirs.push(path.clone());
                stack.push(path);
                continue;
            }

            let Ok(rel) = path.strip_prefix(backup_root) else { continue };
            if vault_root.join(rel).exists() {
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }

    // Deepest first, so a folder emptied by the loop above can go with it.
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for dir in dirs {
        if !vault_root.join(dir.strip_prefix(backup_root).unwrap_or(&dir)).exists() {
            let _ = std::fs::remove_dir(&dir); // Fails while it still holds anything.
        }
    }

    removed
}

/// Is there anything in the vault worth calling a library?
pub fn has_any_content(vault_root: &Path) -> bool {
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
            // The snapshot alone is not content: a fresh install writes one before
            // it has a single report in it.
            if name.starts_with(crate::backup::SNAPSHOT_NAME) {
                continue;
            }
            return true;
        }
    }
    false
}

/// Copy everything under `from` into `to`, skipping what is already identical.
///
/// "Identical" is same size and same modification time to the second. Hashing
/// every file would be certain but reads the whole archive twice on every sync,
/// and the cheap test only errs by copying again — never by skipping something
/// that differs in length.
fn copy_tree(from: &Path, to: &Path, location: String) -> Result<SyncReport, String> {
    let mut report = SyncReport {
        adopted: 0,
        removed: 0,
        location,
        ..Default::default()
    };
    let mut stack = vec![from.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                report.failed.push(format!("{} — {e}", dir.display()));
                continue;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if path.is_dir() {
                if !SKIP_DIRS.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                    stack.push(path);
                }
                continue;
            }

            // Half-written files from an interrupted backup, and the WAL sidecars
            // of any database that should not have been here in the first place.
            if name.ends_with(".partial") || name.ends_with("-wal") || name.ends_with("-shm") {
                continue;
            }

            let Ok(rel) = path.strip_prefix(from) else { continue };
            let dest = to.join(rel);

            if same_file(&path, &dest) {
                report.unchanged += 1;
                continue;
            }

            if let Some(parent) = dest.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    report.failed.push(format!("{} — {e}", rel.display()));
                    continue;
                }
            }

            match std::fs::copy(&path, &dest) {
                Ok(n) => {
                    report.copied += 1;
                    report.bytes += n;
                    // Carry the modification time across, so the next sync can
                    // tell that this file is already done.
                    if let Ok(meta) = std::fs::metadata(&path) {
                        if let Ok(mtime) = meta.modified() {
                            let _ = filetime_set(&dest, mtime);
                        }
                    }
                }
                // One locked or half-synced file must not stop the rest. Drive
                // holds handles on files it is uploading.
                Err(e) => report.failed.push(format!("{} — {e}", rel.display())),
            }
        }
    }

    Ok(report)
}

fn same_file(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    if ma.len() != mb.len() {
        return false;
    }
    match (ma.modified(), mb.modified()) {
        (Ok(ta), Ok(tb)) => {
            let secs = |t: std::time::SystemTime| {
                t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
            };
            secs(ta) == secs(tb)
        }
        _ => false,
    }
}

/// Set a file's modification time, so an unchanged file stays unchanged.
fn filetime_set(path: &Path, when: std::time::SystemTime) -> std::io::Result<()> {
    let file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.set_modified(when)
}

/// Where Google Drive for desktop usually mounts, in the order worth trying.
///
/// Guessing beats asking here: the app can offer a folder it has actually found
/// rather than a file picker opened on nothing.
pub fn likely_drive_roots() -> Vec<PathBuf> {
    let mut found = Vec::new();

    // Drive for desktop mounts a virtual drive whose root holds "My Drive".
    for letter in 'D'..='Z' {
        let candidate = PathBuf::from(format!("{letter}:\\My Drive"));
        if candidate.is_dir() {
            found.push(candidate);
        }
    }

    // The older client, and anyone who chose to mirror into their profile.
    if let Ok(home) = std::env::var("USERPROFILE") {
        for name in ["Google Drive", "My Drive"] {
            let candidate = PathBuf::from(&home).join(name);
            if candidate.is_dir() {
                found.push(candidate);
            }
        }
    }

    found
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fx {
        dir: PathBuf,
    }

    impl Fx {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("mrt-sync-{name}-{}", ulid::Ulid::new()));
            std::fs::create_dir_all(dir.join("vault")).unwrap();
            std::fs::create_dir_all(dir.join("drive")).unwrap();
            Fx { dir }
        }

        fn vault(&self) -> PathBuf {
            self.dir.join("vault")
        }

        fn target(&self) -> FolderTarget {
            FolderTarget::new(&self.dir.join("drive"))
        }

        fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let p = self.vault().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes).unwrap();
            p
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_backup_stops_carrying_what_the_vault_no_longer_has() {
        // Otherwise Drive holds every document the library has ever contained:
        // rename a person and both names live there forever.
        let f = Fx::new("prune");
        f.write("Rahim-Uddin/2026/keep.jpg", b"keep");
        f.write("Rahim-Uddin/2026/gone.jpg", b"gone");

        let target = f.target();
        target.push(&f.vault()).unwrap();
        assert!(f.dir.join("drive/MedicineReportTracker/Rahim-Uddin/2026/gone.jpg").exists());

        std::fs::remove_file(f.vault().join("Rahim-Uddin/2026/gone.jpg")).unwrap();
        let report = target.push(&f.vault()).unwrap();

        assert_eq!(report.removed, 1);
        assert!(!f.dir.join("drive/MedicineReportTracker/Rahim-Uddin/2026/gone.jpg").exists());
        assert!(f.dir.join("drive/MedicineReportTracker/Rahim-Uddin/2026/keep.jpg").exists());
    }

    #[test]
    fn a_folder_nobody_has_reports_in_any_more_goes_too() {
        let f = Fx::new("prune-dir");
        f.write("Old-Name/2026/one.jpg", b"x");

        let target = f.target();
        target.push(&f.vault()).unwrap();

        // What a rename looks like on disk: the whole folder moves.
        std::fs::remove_dir_all(f.vault().join("Old-Name")).unwrap();
        f.write("New-Name/2026/one.jpg", b"x");
        target.push(&f.vault()).unwrap();

        assert!(!f.dir.join("drive/MedicineReportTracker/Old-Name").exists(), "the old folder must not linger");
        assert!(f.dir.join("drive/MedicineReportTracker/New-Name/2026/one.jpg").exists());
    }

    #[test]
    fn an_empty_vault_never_empties_the_backup() {
        // The whole point of the backup is the moment this protects: a fresh
        // install, or a phone whose storage was wiped, and somebody pressing
        // "Back up now" before they think to press Restore.
        let f = Fx::new("prune-guard");
        f.write("Rahim-Uddin/2026/only-copy.jpg", b"irreplaceable");

        let target = f.target();
        target.push(&f.vault()).unwrap();

        std::fs::remove_dir_all(f.vault()).unwrap();
        std::fs::create_dir_all(f.vault()).unwrap();

        let report = target.push(&f.vault()).unwrap();
        assert_eq!(report.removed, 0);
        assert!(
            f.dir.join("drive/MedicineReportTracker/Rahim-Uddin/2026/only-copy.jpg").exists(),
            "an empty vault is a wiped phone, not a library somebody cleared"
        );
    }

    #[test]
    fn a_snapshot_on_its_own_does_not_count_as_a_library() {
        // A fresh install writes one of these before it holds a single report.
        let f = Fx::new("snapshot-only");
        f.write(&format!("{}", crate::backup::SNAPSHOT_NAME), b"db");
        assert!(!has_any_content(&f.vault()));

        f.write("Rahim-Uddin/2026/one.jpg", b"x");
        assert!(has_any_content(&f.vault()));
    }

    #[test]
    fn a_backup_copies_documents_and_their_sidecars() {
        let f = Fx::new("push");
        f.write("Rahim-Uddin/2025/2025-03-14_Rahim-Uddin_CBC.jpg", b"scan bytes");
        f.write("Rahim-Uddin/2025/2025-03-14_Rahim-Uddin_CBC.jpg.meta.json", b"{}");
        f.write("medicine-report-tracker.backup.db", b"snapshot");

        let target = f.target();
        let report = target.push(&f.vault()).unwrap();
        assert_eq!(report.copied, 3);
        assert!(report.failed.is_empty());

        // Readable at the other end without this app, which is the whole point.
        let copied = target
            .root()
            .join("Rahim-Uddin/2025/2025-03-14_Rahim-Uddin_CBC.jpg");
        assert_eq!(std::fs::read(copied).unwrap(), b"scan bytes");
        assert!(target.snapshot().is_some(), "the snapshot travels with the files");
    }

    #[test]
    fn exports_are_left_behind_because_they_can_be_rebuilt() {
        let f = Fx::new("skip");
        f.write("Rahim-Uddin/2025/report.jpg", b"x");
        f.write("Exports/Rahim Uddin — Medical History.pdf", b"big merged pdf");

        let target = f.target();
        let report = target.push(&f.vault()).unwrap();

        assert_eq!(report.copied, 1);
        assert!(
            !target.root().join("Exports").exists(),
            "an export is a derivative — copying it spends the user's storage twice",
        );
    }

    #[test]
    fn a_second_backup_copies_only_what_changed() {
        let f = Fx::new("incremental");
        f.write("a.jpg", b"one");
        f.write("b.jpg", b"two");

        let target = f.target();
        assert_eq!(target.push(&f.vault()).unwrap().copied, 2);

        let second = target.push(&f.vault()).unwrap();
        assert_eq!(second.copied, 0, "nothing changed");
        assert_eq!(second.unchanged, 2);

        // A document edited in place has a new length, and must go again.
        f.write("b.jpg", b"two, corrected");
        let third = target.push(&f.vault()).unwrap();
        assert_eq!(third.copied, 1);
        assert_eq!(third.unchanged, 1);
    }

    #[test]
    fn a_wal_sidecar_is_never_copied() {
        let f = Fx::new("wal");
        f.write("medicine-report-tracker.backup.db", b"snapshot");
        f.write("medicine-report-tracker.backup.db-wal", b"half a transaction");
        f.write("medicine-report-tracker.backup.db.partial", b"interrupted");

        let target = f.target();
        assert_eq!(
            target.push(&f.vault()).unwrap().copied,
            1,
            "only the consistent snapshot may travel",
        );
    }

    #[test]
    fn restoring_brings_back_a_file_that_vanished() {
        let f = Fx::new("pullmissing");
        let doc = f.write("Rahim-Uddin/2025/report.jpg", b"the only copy");
        let target = f.target();
        target.push(&f.vault()).unwrap();

        // The disk lost it, or someone deleted it in Explorer.
        std::fs::remove_file(&doc).unwrap();

        let report = target.pull(&f.vault()).unwrap();
        assert_eq!(report.copied, 1);
        assert_eq!(std::fs::read(&doc).unwrap(), b"the only copy");
    }

    #[test]
    fn restoring_replaces_a_file_that_was_damaged() {
        let f = Fx::new("pullcorrupt");
        let doc = f.write("Rahim-Uddin/2025/report.jpg", b"the whole scan");
        let target = f.target();
        target.push(&f.vault()).unwrap();

        // Truncated — the shape a half-finished write or a failing disk leaves.
        std::fs::write(&doc, b"the wh").unwrap();

        let report = target.pull(&f.vault()).unwrap();
        assert_eq!(report.copied, 1, "a shorter file is not the same file");
        assert_eq!(std::fs::read(&doc).unwrap(), b"the whole scan");
    }

    #[test]
    fn restoring_leaves_untouched_files_alone() {
        let f = Fx::new("pullsame");
        f.write("a.jpg", b"one");
        f.write("b.jpg", b"two");
        let target = f.target();
        target.push(&f.vault()).unwrap();

        let report = target.pull(&f.vault()).unwrap();
        assert_eq!(report.copied, 0);
        assert_eq!(report.unchanged, 2);
    }

    #[test]
    fn restoring_onto_an_empty_machine_rebuilds_the_whole_vault() {
        let f = Fx::new("freshmachine");
        f.write("Rahim-Uddin/2024/a.pdf", b"one");
        f.write("Rahim-Uddin/2025/b.jpg", b"two");
        f.write("medicine-report-tracker.backup.db", b"snapshot");
        let target = f.target();
        target.push(&f.vault()).unwrap();

        // A new computer: Drive is there, the vault is not.
        std::fs::remove_dir_all(f.vault()).unwrap();
        std::fs::create_dir_all(f.vault()).unwrap();

        let report = target.pull(&f.vault()).unwrap();
        assert_eq!(report.copied, 3);
        assert!(f.vault().join("Rahim-Uddin/2024/a.pdf").exists());
        assert!(f.vault().join("medicine-report-tracker.backup.db").exists());
    }

    #[test]
    fn a_target_whose_drive_is_not_mounted_says_so_plainly() {
        let f = Fx::new("unmounted");
        let target = FolderTarget::new(&f.dir.join("no-such-drive"));
        assert!(!target.available());

        let err = target.push(&f.vault()).unwrap_err();
        assert!(err.contains("not there"), "{err}");
        assert!(err.contains("Google Drive"), "it should say what to check: {err}");
    }

    #[test]
    fn restoring_from_a_target_that_has_never_been_written_says_so() {
        let f = Fx::new("noBackup");
        let err = f.target().pull(&f.vault()).unwrap_err();
        assert!(err.contains("no backup"), "{err}");
    }

    #[test]
    fn the_backup_folder_is_named_so_it_is_obvious_in_drive() {
        let f = Fx::new("naming");
        assert!(f.target().describe().ends_with("MedicineReportTracker"));
    }
}
