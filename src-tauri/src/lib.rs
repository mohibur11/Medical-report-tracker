mod auth;
mod backup;
mod categories;
mod db;
mod documents;
mod export;
mod imaging;
mod ingest;
mod naming;
mod ocr;
mod patients;
mod paths;
mod reconcile;
mod search;
mod sniff;
mod vault;

use std::sync::Mutex;

use tauri::{AppHandle, Manager, State};

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

/// The review queue as the database has it, so a half-finished import survives
/// closing the app.
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
        },
    )
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be registered before anything else: a second launch has to be
        // turned away before it opens the database, replays the journal, or
        // starts moving files the first copy is already moving.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            use tauri::Manager;
            // Someone tried to open the app again — almost always by
            // double-clicking the shortcut. Show them the window they already
            // have rather than doing nothing.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle();

            // Created eagerly so the health screen reports paths that exist.
            let vault = paths::vault_root(handle)?;
            std::fs::create_dir_all(&vault)?;
            std::fs::create_dir_all(paths::staging_dir(handle)?)?;

            let conn = db::open(&paths::db_path(handle)?)?;
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
            staged_thumb,
            list_patients,
            create_patient,
            rename_patient,
            commit_item,
            list_documents,
            list_years,
            export_pdf,
            reveal_in_explorer,
            list_categories,
            create_category,
            rename_category,
            archive_category,
            set_document_categories,
            tag_documents,
            document_tags,
            search_documents,
            reindex,
            rescan_vault,
            trash_document,
            update_document,
            backup_now,
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
