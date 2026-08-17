//! Where things live on disk, and why.

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

pub const VAULT_DIR_NAME: &str = "MedicineReportTracker";

/// The browsable archive: `%USERPROFILE%\Documents\MedicineReportTracker\`.
///
/// Deliberately somewhere the user can open in Explorer, copy to a USB stick and
/// read without this app ever running again. That portability is the whole reason
/// requirement 3.2 exists, and the reason the vault is not encrypted.
pub fn vault_root(app: &AppHandle) -> Result<PathBuf, String> {
    let docs = app
        .path()
        .document_dir()
        .map_err(|e| format!("cannot resolve Documents folder: {e}"))?;
    Ok(docs.join(VAULT_DIR_NAME))
}

/// The live database: `%LOCALAPPDATA%\<identifier>\app.db`.
///
/// NEVER inside the vault. The vault sits in Documents, which is routinely
/// OneDrive-synced, and a WAL sidecar synced mid-transaction is a documented
/// corruption path. The backup story is a `VACUUM INTO` snapshot written into the
/// vault on close, not the live file.
pub fn db_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("cannot resolve local app data folder: {e}"))?;
    Ok(dir.join("app.db"))
}

/// Staging area for files that have been imported but not yet committed to a
/// patient. Lives beside the DB rather than in the vault so a half-finished import
/// never appears in the browsable tree.
pub fn staging_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("cannot resolve local app data folder: {e}"))?;
    Ok(dir.join("staging"))
}
