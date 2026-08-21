mod auth;
mod backup;
mod categories;
mod db;
mod documents;
mod drive;
mod export;
mod google;
mod imaging;
mod ingest;
mod naming;
mod ocr;
mod patients;
mod paths;
mod presets;
mod reconcile;
mod search;
mod secret;
mod sniff;
mod sync;
mod vault;

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::sync::BackupTarget;

use crate::db::{Db, DbHealth};
use crate::ingest::IngestItem;

/// The id of the single local user, resolved once at startup.
pub struct CurrentUser(pub String);

#[tauri::command]
fn db_health(app: AppHandle, state: State<'_, Db>) -> Result<DbHealth, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    db::health(
        &conn,
        paths::db_path(&app)?.display().to_string(),
        paths::vault_root(&app)?.display().to_string(),
    )
}

/// Stage dropped files. Returns one row per input including failures, so the
/// review grid can show every file the user dropped rather than silently losing
/// the ones that could not be read.
#[tauri::command]
async fn import_files(
    app: AppHandle,
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    paths_in: Vec<String>,
) -> Result<Vec<IngestItem>, String> {
    let staging = paths::staging_dir(&app)?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    ingest::stage_batch(&conn, &user.0, &staging, &paths_in)
}

/// Ask the phone for files and import them.
///
/// The desktop file dialog cannot be used here: Android answers it with a
/// content:// URI, which nothing on the Rust side can open, so the Add button
/// appeared to do nothing at all. The picker copies the bytes into the same inbox
/// a shared file lands in, and this then imports them the same way.
#[tauri::command]
fn pick_and_import(
    app: AppHandle,
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
) -> Result<Vec<IngestItem>, String> {
    #[cfg(target_os = "android")]
    {
        ocr::pick_files()?;
        let local = app
            .path()
            .app_local_data_dir()
            .map_err(|e| format!("cannot resolve local app data folder: {e}"))?;
        let staging = paths::staging_dir(&app)?;
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        return ingest::take_shared(&conn, &user.0, &local, &staging);
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = (&app, &state, &user);
        Err("The desktop app uses its own file dialog.".into())
    }
}


/// Import anything that was shared to the app since it last looked.
///
/// Called by the phone front end when it opens and whenever it comes back to the
/// foreground, which is exactly when a share has just happened.
#[tauri::command]
fn take_shared(
    app: AppHandle,
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
) -> Result<Vec<IngestItem>, String> {
    let local = app
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("cannot resolve local app data folder: {e}"))?;
    let staging = paths::staging_dir(&app)?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    ingest::take_shared(&conn, &user.0, &local, &staging)
}


/// The review queue as the database has it, so a half-finished import survives
/// closing the app.
/// Unlock a password-protected PDF that is waiting in the queue.
#[tauri::command]
fn unlock_pdf(
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    ingest_id: String,
    password: String,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    ingest::unlock(&conn, &user.0, &ingest_id, &password)
}

/// The bytes of a staged PDF, when it is small enough to be worth reading in the
/// window.
///
/// A PDF emailed by a lab carries its text exactly, and reading that beats
/// recognising pixels on both speed and accuracy. Whether a given file has such
/// text cannot be decided cheaply from outside — measured: a text layer can sit
/// inside a form XObject where neither the font list nor the page content stream
/// mentions it — so the only gate is size. A text-layer PDF is small; anything
/// larger is a scan, and shipping tens of megabytes across the bridge to learn
/// that would cost more than the recognition it hoped to avoid.
#[tauri::command]
fn staged_pdf_text_source(
    state: State<'_, Db>,
    ingest_id: String,
) -> Result<Option<String>, String> {
    use base64::Engine;

    const MAX_BYTES: u64 = 12 * 1024 * 1024;

    let staged: Option<String> = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT staged_path FROM ingest_items WHERE id = ?1 AND file_kind = 'pdf'",
            rusqlite::params![ingest_id],
            |r| r.get(0),
        )
        .unwrap_or(None)
    };
    let Some(staged) = staged else { return Ok(None) };
    let path = std::path::PathBuf::from(staged);

    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(u64::MAX) > MAX_BYTES {
        return Ok(None);
    }

    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read the staged file: {e}"))?;
    Ok(Some(base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[tauri::command]
fn list_staged(state: State<'_, Db>, user: State<'_, CurrentUser>) -> Result<Vec<IngestItem>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    ingest::list_staged(&conn, &user.0)
}

/// Rename a patient, or correct their date of birth.
///
/// Renaming moves every one of their files, so this is not the cheap operation
/// its UI suggests; the report says exactly what moved and what did not.
#[tauri::command]
fn rename_patient(
    app: AppHandle,
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    patient_id: String,
    display_name: String,
    dob: Option<String>,
) -> Result<vault::RenameReport, String> {
    let vault_root = paths::vault_root(&app)?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    vault::rename_patient(
        &conn,
        &user.0,
        &vault_root,
        &patient_id,
        &display_name,
        dob.as_deref(),
    )
}

#[tauri::command]
fn list_export_presets(
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
) -> Result<Vec<presets::ExportPreset>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    presets::list(&conn, &user.0)
}

#[tauri::command]
fn save_export_preset(
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    name: String,
    preset: presets::ExportPreset,
) -> Result<presets::ExportPreset, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    presets::save(&conn, &user.0, &name, &preset)
}

#[tauri::command]
fn delete_export_preset(
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    id: String,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    presets::remove(&conn, &user.0, &id)
}

/// Record that a preset was used, so the list stays ordered by habit.
#[tauri::command]
fn use_export_preset(state: State<'_, Db>, user: State<'_, CurrentUser>, id: String) {
    if let Ok(conn) = state.0.lock() {
        presets::touch(&conn, &user.0, &id);
    }
}

#[tauri::command]
fn list_patients(state: State<'_, Db>, user: State<'_, CurrentUser>) -> Result<Vec<patients::Patient>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    patients::list(&conn, &user.0)
}

#[tauri::command]
fn create_patient(
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    display_name: String,
    dob: Option<String>,
) -> Result<patients::Patient, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    patients::create(&conn, &user.0, &display_name, dob.as_deref())
}

/// Move a staged file into the vault under its canonical name.
#[tauri::command]
fn commit_item(
    app: AppHandle,
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    ingest_id: String,
    patient_id: String,
    doc_date: String,
    title: String,
    doc_type: String,
) -> Result<vault::CommittedDocument, String> {
    let vault_root = paths::vault_root(&app)?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    vault::commit(
        &conn,
        &user.0,
        &vault_root,
        vault::CommitRequest {
            ingest_id: &ingest_id,
            patient_id: &patient_id,
            doc_date: &doc_date,
            title: &title,
            doc_type: &doc_type,
        },
    )
}

/// Recognise text on a staged file. Returns what was read; ranking the dates out
/// of it happens in the review grid, where the user can see and correct the choice.
#[tauri::command]
async fn run_ocr(state: State<'_, Db>, ingest_id: String) -> Result<Vec<ocr::OcrPage>, String> {
    // Recognition costs roughly a third of a second per page and must not hold the
    // database lock while it runs. A backlog import opens a queue of hundreds of
    // rows at once, and every other command — list, commit, search — would sit
    // behind them.
    let target = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        ocr::staged_target(&conn, &ingest_id)?
    };
    if let Some(pages) = target.cached {
        return Ok(pages);
    }

    let pages = ocr::recognize_file(&target.path, &target.kind)?;

    let conn = state.0.lock().map_err(|e| e.to_string())?;
    ocr::store_ocr(&conn, &ingest_id, &pages)?;
    Ok(pages)
}

#[tauri::command]
fn ocr_available() -> bool {
    ocr::available()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LockState {
    enabled: bool,
    email: String,
}

#[tauri::command]
fn lock_state(state: State<'_, Db>) -> Result<LockState, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    Ok(LockState {
        enabled: auth::is_enabled(&conn)?,
        email: auth::email(&conn).unwrap_or_default(),
    })
}

#[tauri::command]
fn unlock(state: State<'_, Db>, password: String) -> Result<bool, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    auth::verify(&conn, &password)
}

#[tauri::command]
fn set_password(
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    email: String,
    password: String,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    auth::set_password(&conn, &user.0, &email, &password)
}

#[tauri::command]
fn change_password(
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    current: String,
    next: String,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    auth::change_password(&conn, &user.0, &current, &next)
}

#[tauri::command]
fn disable_password(
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    current: String,
) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    auth::disable(&conn, &user.0, &current)
}

/// Change a filed document's details. Moves the file if its canonical name or
/// folder changes, which correcting a date or a patient always does.
#[tauri::command]
fn update_document(
    app: AppHandle,
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    document_id: String,
    patient_id: String,
    doc_date: String,
    title: String,
    doc_type: String,
    notes: Option<String>,
) -> Result<vault::CommittedDocument, String> {
    let vault_root = paths::vault_root(&app)?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    vault::update(
        &conn,
        &user.0,
        &vault_root,
        vault::UpdateRequest {
            document_id: &document_id,
            patient_id: &patient_id,
            doc_date: &doc_date,
            title: &title,
            doc_type: &doc_type,
            notes: notes.as_deref().unwrap_or_default(),
        },
    )
}

/// What is in the Trash, and whether each one can still be put back.
#[tauri::command]
fn list_trashed(
    app: AppHandle,
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
) -> Result<Vec<vault::TrashedDocument>, String> {
    let vault_root = paths::vault_root(&app)?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    vault::list_trashed(&conn, &user.0, &vault_root)
}

/// Put a trashed document back where it was.
#[tauri::command]
fn restore_document(
    app: AppHandle,
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    document_id: String,
) -> Result<(), String> {
    let vault_root = paths::vault_root(&app)?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    vault::restore(&conn, &user.0, &vault_root, &document_id)
}

/// Move a document to the vault's Trash folder. Nothing is unlinked.
#[tauri::command]
fn trash_document(
    app: AppHandle,
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    document_id: String,
) -> Result<(), String> {
    let vault_root = paths::vault_root(&app)?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    vault::trash(&conn, &user.0, &vault_root, &document_id)
}

/// Write a database snapshot into the vault, plus a metadata sidecar per document.
/// Setting key for the Google Drive (or other synced) folder.
const DRIVE_FOLDER: &str = "drive_folder";
const LAST_SYNC: &str = "drive_last_sync";

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DriveStatus {
    /// The signed-in Google account, when the API is being used.
    account: google::Account,
    /// Which of the two ways a backup would currently be written.
    method: &'static str,
    /// The chosen folder, if one has been chosen or found.
    folder: Option<String>,
    /// Where the copy is written inside it.
    backup_path: Option<String>,
    /// Is it there right now? A Drive folder disappears when the client stops.
    available: bool,
    /// Folders that look like Drive mounts, for the "choose one" case.
    suggestions: Vec<String>,
    last_sync: Option<String>,
    /// Does the target already hold a backup that could be restored?
    has_backup: bool,
}

fn drive_target(conn: &rusqlite::Connection) -> Option<sync::FolderTarget> {
    let folder = db::setting(conn, DRIVE_FOLDER)
        .map(std::path::PathBuf::from)
        .or_else(|| sync::likely_drive_roots().into_iter().next())?;
    Some(sync::FolderTarget::new(&folder))
}

#[tauri::command]
fn drive_status(state: State<'_, Db>) -> Result<DriveStatus, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let chosen = db::setting(&conn, DRIVE_FOLDER);
    let suggestions: Vec<String> = sync::likely_drive_roots()
        .into_iter()
        .map(|p| p.display().to_string())
        .collect();

    let account = google::account(&conn);
    let folder = drive_target(&conn);

    // The API wins when an account is connected: it works whether or not Drive
    // for desktop is installed, and it is what the user asked to sign in to.
    let (method, backup_path, available, has_backup) = if account.connected {
        let api = drive::DriveApiTarget::new(&conn);
        (
            "api",
            Some(api.describe()),
            true,
            // Only a real listing could answer this, which costs a round trip on
            // every status refresh. Restore checks for itself and says so.
            true,
        )
    } else {
        (
            "folder",
            folder.as_ref().map(|t| BackupTarget::describe(t)),
            folder.as_ref().map(|t| t.available()).unwrap_or(false),
            folder.as_ref().and_then(|t| t.snapshot()).is_some(),
        )
    };

    Ok(DriveStatus {
        account,
        method,
        folder: chosen.or_else(|| suggestions.first().cloned()),
        backup_path,
        available,
        has_backup,
        suggestions,
        last_sync: db::setting(&conn, LAST_SYNC),
    })
}

#[tauri::command]
fn set_drive_folder(state: State<'_, Db>, folder: String) -> Result<(), String> {
    let path = std::path::PathBuf::from(&folder);
    if !path.is_dir() {
        return Err(format!("{folder} is not a folder on this computer."));
    }
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    db::set_setting(&conn, DRIVE_FOLDER, &folder)
}

/// Store the OAuth client this installation should sign in with.
///
/// Asked of the user rather than shipped in the binary: this repository is
/// public, and a client ID committed to it would be used by strangers against
/// the quota — and, worse, would show their app's name on the consent screen.
#[tauri::command]
fn set_google_client(
    state: State<'_, Db>,
    client_id: String,
    client_secret: String,
) -> Result<google::Account, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    google::set_client(&conn, &client_id, &client_secret)?;
    Ok(google::account(&conn))
}

/// Sign in to Google. Opens the browser and waits for the answer.
#[tauri::command]
async fn connect_google(
    app: AppHandle,
    state: State<'_, Db>,
) -> Result<google::Account, String> {
    // The database lock is taken twice, briefly, and never while the browser is
    // open: signing in takes as long as the person takes, and holding it would
    // freeze every other part of the app until they finished.
    let (client_id, client_secret) = {
        // Android signs in with the client compiled into the app, because the
        // redirect scheme derived from it is declared in the manifest and the two
        // cannot be allowed to disagree.
        #[cfg(target_os = "android")]
        {
            (String::new(), String::new())
        }

        #[cfg(not(target_os = "android"))]
        {
            let conn = state.0.lock().map_err(|e| e.to_string())?;
            google::client_credentials(&conn)?
        }
    };

    // The browser is opened through the opener plugin, which knows how to do it
    // on each platform — a process on Windows, an intent on Android.
    let opener = app.clone();
    let tokens = tauri::async_runtime::spawn_blocking(move || {
        google::run_flow(&client_id, &client_secret, |url| {
            // On a phone the page opens inside this app. Sending it to the system
            // browser backgrounds the app, Android freezes the process, and the
            // listener waiting for Google's redirect stops accepting.
            #[cfg(target_os = "android")]
            {
                let _ = &opener;
                return ocr::open_in_app(url);
            }

            #[cfg(not(target_os = "android"))]
            {
                use tauri_plugin_opener::OpenerExt;
                opener
                    .opener()
                    .open_url(url, None::<&str>)
                    .map_err(|e| format!("cannot open the browser: {e}"))
            }
        })
    })
    .await
    .map_err(|e| format!("the sign-in did not finish: {e}"))??;

    let conn = state.0.lock().map_err(|e| e.to_string())?;
    google::store(&conn, tokens)
}

#[tauri::command]
fn disconnect_google(state: State<'_, Db>) -> Result<google::Account, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    google::disconnect(&conn)?;
    Ok(google::account(&conn))
}

/// Write the current state of the vault into the Drive folder.
///
/// The database snapshot is refreshed first, so what lands in Drive is the
/// library as it is now rather than as it was at the last close.
#[tauri::command]
fn backup_to_drive(
    app: AppHandle,
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
) -> Result<sync::SyncReport, String> {
    let vault_root = paths::vault_root(&app)?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;

    // What travels must be current, so the sidecars and the snapshot are
    // rewritten before anything is copied.
    backup::write_all_sidecars(&conn, &vault_root, &user.0)?;
    backup::snapshot(&conn, &vault_root)?;

    let report = if google::account(&conn).connected {
        drive::DriveApiTarget::new(&conn).push(&vault_root)?
    } else {
        drive_target(&conn)
            .ok_or(
                "Not connected to Google Drive, and no synced folder was found. \
                 Connect a Google account, or choose the folder Drive syncs.",
            )?
            .push(&vault_root)?
    };

    db::set_setting(&conn, LAST_SYNC, &chrono_now(&conn))?;
    Ok(report)
}

/// Copy back anything missing or different locally.
#[tauri::command]
fn restore_from_drive(
    app: AppHandle,
    state: State<'_, Db>,
) -> Result<sync::SyncReport, String> {
    let vault_root = paths::vault_root(&app)?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    if google::account(&conn).connected {
        return drive::DriveApiTarget::new(&conn).pull(&vault_root);
    }
    drive_target(&conn)
        .ok_or("No Google Drive folder is set, and no Google account is connected.")?
        .pull(&vault_root)
}

/// SQLite owns the clock here, so the timestamp matches every other one stored.
fn chrono_now(conn: &rusqlite::Connection) -> String {
    conn.query_row("SELECT datetime('now')", [], |r| r.get::<_, String>(0))
        .unwrap_or_default()
}

#[tauri::command]
fn backup_now(
    app: AppHandle,
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
) -> Result<String, String> {
    let vault_root = paths::vault_root(&app)?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let sidecars = backup::write_all_sidecars(&conn, &vault_root, &user.0)?;
    let path = backup::snapshot(&conn, &vault_root)?;
    Ok(format!("{} ({sidecars} sidecars)", path.display()))
}

/// Compare the vault with the database and repair what can be repaired.
/// The answer to "I moved some files in Explorer".
#[tauri::command]
fn rescan_vault(
    app: AppHandle,
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
) -> Result<reconcile::ReconcileReport, String> {
    let vault_root = paths::vault_root(&app)?;
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    reconcile::run(&mut conn, &user.0, &vault_root)
}

#[tauri::command]
fn search_documents(
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    query: String,
) -> Result<Vec<search::SearchHit>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    search::search(&conn, &user.0, &query, 50)
}

/// Rebuild the whole index. The repair path when the index and the library
/// disagree, and what a vault rescan will call.
#[tauri::command]
fn reindex(state: State<'_, Db>, user: State<'_, CurrentUser>) -> Result<usize, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    search::reindex_all(&conn, &user.0)
}

#[tauri::command]
fn list_categories(state: State<'_, Db>, user: State<'_, CurrentUser>) -> Result<Vec<categories::Category>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    categories::list(&conn, &user.0)
}

#[tauri::command]
fn create_category(
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    name: String,
    color: Option<String>,
) -> Result<categories::Category, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    categories::create(&conn, &user.0, &name, color.as_deref())
}

#[tauri::command]
fn rename_category(state: State<'_, Db>, user: State<'_, CurrentUser>, id: String, name: String) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    categories::rename(&conn, &user.0, &id, &name)
}

#[tauri::command]
fn archive_category(state: State<'_, Db>, user: State<'_, CurrentUser>, id: String) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    categories::archive(&conn, &user.0, &id)
}

#[tauri::command]
fn set_document_categories(
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    document_id: String,
    category_ids: Vec<String>,
) -> Result<(), String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    categories::set_for_document(&mut conn, &user.0, &document_id, &category_ids)
}

#[tauri::command]
fn tag_documents(
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    document_ids: Vec<String>,
    category_id: String,
) -> Result<usize, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    categories::tag_many(&mut conn, &user.0, &document_ids, &category_id)
}

#[tauri::command]
fn untag_documents(
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    document_ids: Vec<String>,
    category_id: String,
) -> Result<usize, String> {
    let mut conn = state.0.lock().map_err(|e| e.to_string())?;
    categories::untag_many(&mut conn, &user.0, &document_ids, &category_id)
}

/// (documentId, categoryId) pairs for the given documents, joined client-side so
/// the library list does not issue a query per row.
#[tauri::command]
fn document_tags(
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    document_ids: Vec<String>,
) -> Result<Vec<(String, String)>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    categories::for_documents(&conn, &user.0, &document_ids)
}

#[tauri::command]
fn list_documents(state: State<'_, Db>, user: State<'_, CurrentUser>) -> Result<Vec<documents::DocumentRow>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    documents::list(&conn, &user.0)
}

#[tauri::command]
fn list_years(state: State<'_, Db>, user: State<'_, CurrentUser>) -> Result<Vec<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    documents::years(&conn, &user.0)
}

/// Build the merged PDF. Runs on a blocking thread: a large export is seconds of
/// image re-encoding and several pdfcpu invocations, and must not freeze the UI.
#[tauri::command]
async fn export_pdf(
    app: AppHandle,
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    request: export::ExportRequest,
) -> Result<export::ExportResult, String> {
    let vault_root = paths::vault_root(&app)?;
    let work = paths::staging_dir(&app)?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    export::build(&conn, &user.0, &vault_root, &work, &request)
}

/// Reveal a finished export in Explorer, selecting the file.
/// The escape hatch: the same filtered slice as loose, numbered files.
#[tauri::command]
fn export_to_folder(
    app: AppHandle,
    state: State<'_, Db>,
    user: State<'_, CurrentUser>,
    request: export::ExportRequest,
) -> Result<export::FolderExport, String> {
    let vault_root = paths::vault_root(&app)?;
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    export::to_folder(&conn, &user.0, &vault_root, &request)
}

#[tauri::command]
fn reveal_in_explorer(path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    if !p.exists() {
        return Err("That file is no longer there.".into());
    }
    std::process::Command::new("explorer")
        .arg(format!("/select,{}", p.display()))
        .spawn()
        .map_err(|e| format!("cannot open Explorer: {e}"))?;
    Ok(())
}

/// Thumbnail for one staged item, as a data URL.
///
/// Fetched lazily per row rather than bundled into the import response: a
/// 200-file batch would otherwise ship several megabytes of base64 through the
/// IPC bridge before the grid had rendered a single row.
#[tauri::command]
fn staged_thumb(app: AppHandle, id: String) -> Result<Option<String>, String> {
    use base64::Engine;

    // `id` reaches the filesystem, so it must be a ULID and nothing else —
    // otherwise this command is a path traversal into any file on the machine.
    if id.len() != 26 || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("invalid id".into());
    }

    let path = paths::staging_dir(&app)?.join(format!("{id}.thumb.jpg"));
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ))),
        Err(_) => Ok(None),
    }
}

/// Files handed to the app on the command line — by "Open with", by dropping a
/// file on the icon, or by a second launch while one is already running.
///
/// Anything that is not an existing file is ignored rather than reported: the
/// command line also carries flags, and a launch that fails because of one is
/// worse than a launch that quietly imports nothing.
///
/// Desktop only: Android hands files over as intents, not as a command line.
#[cfg(desktop)]
fn stage_arguments(app: &AppHandle, argv: &[String]) -> usize {
    let paths: Vec<String> = argv
        .iter()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .filter(|a| std::path::Path::new(a).is_file())
        .cloned()
        .collect();

    if paths.is_empty() {
        return 0;
    }

    let (Ok(staging), Some(db), Some(user)) = (
        paths::staging_dir(app),
        app.try_state::<Db>(),
        app.try_state::<CurrentUser>(),
    ) else {
        return 0;
    };
    let Ok(conn) = db.0.lock() else { return 0 };

    match ingest::stage_batch(&conn, &user.0, &staging, &paths) {
        Ok(items) => {
            // The queue is read from the database, so the window only needs to be
            // told to look again.
            let _ = app.emit("queue-changed", items.len());
            items.len()
        }
        Err(_) => 0,
    }
}

/// Turn away a second launch, and take whatever file it was carrying.
///
/// Desktop only, and separated out because the whole idea does not exist on
/// Android: the system runs one instance of an app, and there is no command line
/// for a second one to carry.
#[cfg(desktop)]
fn guard_single_instance(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
        use tauri::Manager;
        // Someone tried to open the app again — by double-clicking the shortcut,
        // or by sending it a file with "Open with". Show them the window they
        // already have, and take the file rather than dropping it.
        stage_arguments(app, &argv);
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
    }))
}

#[cfg(not(desktop))]
fn guard_single_instance(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    builder
}

/// Wire up the Kotlin text recogniser on Android, and nothing anywhere else.
///
/// Registered as a plugin because that is how Tauri reaches Kotlin; the handle is
/// then handed to the ocr module, which is where the rest of the app already
/// asks for text.
#[cfg(target_os = "android")]
fn android_ocr() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("mlkit-ocr")
        .setup(|_app, api| {
            let handle = api.register_android_plugin(
                "com.mohibur.medicinereporttracker",
                "OcrPlugin",
            )?;
            ocr::set_android_handle(handle);
            Ok(())
        })
        .build()
}

#[cfg(not(target_os = "android"))]
fn android_ocr() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("mlkit-ocr").build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Registered before anything else: a second launch has to be turned away
    // before it opens the database, replays the journal, or starts moving files
    // the first copy is already moving.
    guard_single_instance(tauri::Builder::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(android_ocr())
        .setup(|app| {
            let handle = app.handle();

            // Created eagerly so the health screen reports paths that exist.
            let vault = paths::vault_root(handle)?;
            std::fs::create_dir_all(&vault)?;
            std::fs::create_dir_all(paths::staging_dir(handle)?)?;

            // A snapshot sits beside the vault, and another in the Drive folder
            // if one has ever been written. Either can stand in for a database
            // that is missing or damaged — which is what makes a fresh machine
            // work: install, let Drive sync, launch.
            let mut snapshots = vec![vault.join(backup::SNAPSHOT_NAME)];
            snapshots.extend(
                sync::likely_drive_roots()
                    .iter()
                    .filter_map(|root| sync::FolderTarget::new(root).snapshot()),
            );

            let (conn, outcome) = db::open_or_restore(&paths::db_path(handle)?, &snapshots)?;
            if let db::OpenOutcome::Restored { from, kept } = &outcome {
                eprintln!("database restored from {from}");
                if !kept.is_empty() {
                    eprintln!("the unreadable one was kept at {kept}");
                }
            }
            let user_id = db::ensure_user(&conn)?;

            // Finish any filesystem move a previous run died in the middle of,
            // BEFORE anything else is allowed to touch the vault. Otherwise the
            // tree and the database disagree for the rest of the session.
            match vault::replay_journal(&conn) {
                Ok(0) => {}
                Ok(n) => eprintln!("replayed {n} pending file operation(s)"),
                Err(e) => eprintln!("journal replay failed: {e}"),
            }

            // Migration 002 rebuilt the search index, and a future one may too.
            // An empty index alongside a non-empty library means search would
            // silently return nothing, so repopulate it rather than wait to be asked.
            let stale_index: bool = conn
                .query_row(
                    "SELECT (SELECT count(*) FROM documents WHERE trashed_at IS NULL) > 0
                        AND (SELECT count(*) FROM document_fts) = 0",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .map(|v| v == 1)
                .unwrap_or(false);
            if stale_index {
                match search::reindex_all(&conn, &user_id) {
                    Ok(n) => eprintln!("rebuilt search index for {n} document(s)"),
                    Err(e) => eprintln!("reindex failed: {e}"),
                }
            }

            app.manage(Db(Mutex::new(conn)));
            app.manage(CurrentUser(user_id));

            // Opened by double-clicking a scan, or by dropping files on the icon.
            // Done after the state is managed, because staging needs both.
            #[cfg(desktop)]
            {
                let argv: Vec<String> = std::env::args().collect();
                match stage_arguments(handle, &argv) {
                    0 => {}
                    n => eprintln!("staged {n} file(s) from the command line"),
                }
            }

            // Debug builds open devtools automatically. Frontend failures in a
            // desktop webview are otherwise invisible — there is no address bar
            // and no console unless one is asked for.
            #[cfg(debug_assertions)]
            if let Some(w) = app.get_webview_window("main") {
                w.open_devtools();
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Snapshot on close. A backup the user has to remember to take is a
            // backup that does not exist, and this is the single most likely bad
            // outcome for the app — a lost disk, not an attacker.
            if let tauri::WindowEvent::Destroyed = event {
                let app = window.app_handle().clone();

                if let (Ok(vault_root), Some(state)) = (paths::vault_root(&app), app.try_state::<Db>()) {
                    if let Ok(conn) = state.0.lock() {
                        match backup::snapshot(&conn, &vault_root) {
                            Ok(p) => eprintln!("backup written to {}", p.display()),
                            Err(e) => eprintln!("backup failed: {e}"),
                        }
                    }
                }

                // Exit explicitly. Closing the window otherwise leaves the process
                // running with no window — invisible to the user, and it holds the
                // database lock so the next launch cannot open it.
                app.exit(0);
            }
        })
        .invoke_handler(tauri::generate_handler![
            db_health,
            import_files,
            list_staged,
            take_shared,
            pick_and_import,
            staged_pdf_text_source,
            unlock_pdf,
            staged_thumb,
            list_patients,
            create_patient,
            list_export_presets,
            save_export_preset,
            delete_export_preset,
            use_export_preset,
            rename_patient,
            commit_item,
            list_documents,
            list_years,
            export_pdf,
            export_to_folder,
            reveal_in_explorer,
            list_categories,
            create_category,
            rename_category,
            archive_category,
            set_document_categories,
            tag_documents,
            untag_documents,
            document_tags,
            search_documents,
            reindex,
            rescan_vault,
            trash_document,
            list_trashed,
            restore_document,
            update_document,
            backup_now,
            drive_status,
            set_google_client,
            connect_google,
            disconnect_google,
            set_drive_folder,
            backup_to_drive,
            restore_from_drive,
            run_ocr,
            ocr_available,
            lock_state,
            unlock,
            set_password,
            change_password,
            disable_password
        ])
        .run(tauri::generate_context!())
        .expect("error while running Medicine Report Tracker");
}
