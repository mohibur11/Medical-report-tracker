/**
 * Typed wrappers over the Tauri command surface.
 *
 * Every call into Rust goes through here so the invoke() string names exist in
 * exactly one place — a typo in an invoke name is otherwise a runtime-only failure
 * that no amount of TypeScript strictness catches.
 */

import { invoke } from '@tauri-apps/api/core';

export interface DbHealth {
  sqliteVersion: string;
  fts5: boolean;
  journalMode: string;
  schemaVersion: number;
  dbPath: string;
  vaultPath: string;
  patientCount: number;
  documentCount: number;
  pendingJournalOps: number;
}

export type IngestStatus =
  | 'pending'
  | 'extracted'
  | 'needs_date'
  /** A password-protected PDF: staged and kept, but unreadable until unlocked. */
  | 'locked'
  | 'duplicate'
  | 'failed';

export type FileKind = 'jpeg' | 'png' | 'pdf' | 'heic' | 'tiff' | 'webp' | 'unknown';

export interface IngestItem {
  id: string;
  batchId: string;
  srcPath: string;
  fileName: string;
  status: IngestStatus;
  error: string | null;
  fileKind: FileKind;
  sha256: string | null;
  byteSize: number;
  width: number | null;
  height: number | null;
  exifOrientation: number | null;
  orientationBaked: boolean;
  thumbPath: string | null;
}

export const health = (): Promise<DbHealth> => invoke('db_health');

/** Stage dropped files. Returns one row per input, failures included. */
export const importFiles = (paths: string[]): Promise<IngestItem[]> =>
  invoke('import_files', { pathsIn: paths });

export const stagedThumb = (id: string): Promise<string | null> =>
  invoke('staged_thumb', { id });

/**
 * The staged file at a size worth looking at.
 *
 * The thumbnail tells two reports apart; this is for checking a date against the
 * page it was read from. PDFs come back as their first-page picture.
 */
export const stagedPreview = (id: string): Promise<string | null> =>
  invoke('staged_preview', { id });

/**
 * The review queue as stored, not as remembered.
 *
 * Staging is durable, so a backlog import that was closed halfway through comes
 * back on the next launch instead of leaving files stranded with nothing
 * pointing at them.
 */
export const listStaged = (): Promise<IngestItem[]> => invoke('list_staged');

/**
 * Take a file back out of the review queue. Resolves to the name it was removed
 * under, so the confirmation can say which one went.
 *
 * The file the user picked from is untouched; only the app's own staged copy and
 * its thumbnail go. A file that has already been filed is refused — that one is
 * a document now, and the library's trash is where it belongs.
 */
export const discardStaged = (id: string): Promise<string> =>
  invoke('discard_staged', { id });

/**
 * Import anything shared to the app since it last looked.
 *
 * Android only in practice: a photo shared from the camera roll or WhatsApp is
 * copied into the app's storage by the activity, and this is what picks it up.
 */
export const takeShared = (): Promise<IngestItem[]> => invoke('take_shared');

/**
 * Choose files on a phone and import them.
 *
 * Android only: the file dialog answers with a content:// URI that Rust cannot
 * open, so the picker copies the bytes across first.
 */
export const pickAndImport = (): Promise<IngestItem[]> => invoke('pick_and_import');

/**
 * Remove password protection from a staged PDF.
 *
 * pdfcpu repairs structural damage on its own, so a password is the one thing
 * that genuinely stops a PDF being read. On success the staged file is replaced
 * by an unlocked copy and nothing downstream needs to know it was ever locked.
 */
export const unlockPdf = (ingestId: string, password: string): Promise<void> =>
  invoke('unlock_pdf', { ingestId, password });

export interface Patient {
  id: string;
  displayName: string;
  folderSlug: string;
  dob: string | null;
  documentCount: number;
}

export interface CommittedDocument {
  id: string;
  relPath: string;
  fileName: string;
  titleTruncated: boolean;
}

export const listPatients = (): Promise<Patient[]> => invoke('list_patients');

export const createPatient = (displayName: string, dob?: string): Promise<Patient> =>
  invoke('create_patient', { displayName, dob: dob || null });

export interface RenameReport {
  displayName: string;
  folderSlug: string;
  moved: number;
  missing: number;
  /** Files a lock kept in place, each with the reason. */
  leftBehind: string[];
}

/**
 * Rename a patient, or correct their date of birth.
 *
 * The name is in the folder and in every filename, so a rename moves every one of
 * their documents. Partial success is possible — a file open in a viewer or held
 * by a sync client cannot be moved — and is reported rather than hidden.
 */
export const renamePatient = (
  patientId: string,
  displayName: string,
  dob: string | null,
): Promise<RenameReport> => invoke('rename_patient', { patientId, displayName, dob });

/** Move a staged file into the vault under its canonical name. */
export interface OcrWord {
  text: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface OcrPage {
  /** Reading-order text. Layout is flattened; use `words` when geometry matters. */
  text: string;
  words: OcrWord[];
  engine: string;
  millis: number;
  /** 1-based. Images are always page 1; PDFs carry their real page number. */
  pageNo: number;
}

/**
 * Recognise text on a staged file — one entry per page, so a multi-page scanned
 * PDF comes back whole. Ranking the dates happens in the review grid.
 */
export const runOcr = (ingestId: string): Promise<OcrPage[]> => invoke('run_ocr', { ingestId });

export const ocrAvailable = (): Promise<boolean> => invoke('ocr_available');

/**
 * Base64 of a staged PDF, but only when it has embedded fonts and so plausibly
 * carries real text. Scans come back null and take the recognition path.
 */
export const stagedPdfTextSource = (ingestId: string): Promise<string | null> =>
  invoke('staged_pdf_text_source', { ingestId });

export interface LockState {
  enabled: boolean;
  email: string;
}

export const lockState = (): Promise<LockState> => invoke('lock_state');
export const unlock = (password: string): Promise<boolean> => invoke('unlock', { password });

export const setPassword = (email: string, password: string): Promise<void> =>
  invoke('set_password', { email, password });

export const changePassword = (current: string, next: string): Promise<void> =>
  invoke('change_password', { current, next });

export const disablePassword = (current: string): Promise<void> =>
  invoke('disable_password', { current });

export interface SearchHit {
  documentId: string;
  title: string;
  patient: string;
  docDate: string;
  /** Matched text with the hit wrapped in [ ]. */
  snippet: string;
}

export const searchDocuments = (query: string): Promise<SearchHit[]> =>
  invoke('search_documents', { query });

export const reindex = (): Promise<number> => invoke('reindex');

export interface ReconcileReport {
  ok: number;
  relinked: string[];
  adopted: string[];
  missing: string[];
  unknown: string[];
  recovered: string[];
}

/** Compare the vault with the database and repair what can be repaired. */
export const rescanVault = (): Promise<ReconcileReport> => invoke('rescan_vault');

/**
 * Change a filed document's details.
 *
 * The date, patient and title are all part of the canonical filename, and the
 * patient and year are its folders — so an edit can move the file across the
 * vault, journalled the same way filing is.
 */
export const updateDocument = (args: {
  documentId: string;
  patientId: string;
  docDate: string;
  title: string;
  docType: string;
  notes: string;
}): Promise<CommittedDocument> => invoke('update_document', args);

export interface TrashedDocument {
  id: string;
  title: string;
  patient: string;
  docDate: string;
  relPath: string;
  trashedAt: string;
  /** False when the file has left the Trash folder — moved or emptied by hand. */
  recoverable: boolean;
}

export const listTrashed = (): Promise<TrashedDocument[]> => invoke('list_trashed');

/** Put a trashed document back exactly where it was. */
export const restoreDocument = (documentId: string): Promise<void> =>
  invoke('restore_document', { documentId });

/** Move a document to the vault's Trash folder. Nothing is unlinked. */
export const trashDocument = (documentId: string): Promise<void> =>
  invoke('trash_document', { documentId });

export interface GoogleAccount {
  email: string | null;
  connected: boolean;
  /** False until an OAuth client ID has been configured. */
  configured: boolean;
}

export interface DriveStatus {
  account: GoogleAccount;
  /** 'api' once an account is connected, otherwise the synced folder. */
  method: 'api' | 'folder';
  folder: string | null;
  backupPath: string | null;
  /** False when Drive for desktop is stopped or signed out. */
  available: boolean;
  suggestions: string[];
  lastSync: string | null;
  hasBackup: boolean;
}

export interface SyncReport {
  location: string;
  copied: number;
  unchanged: number;
  bytes: number;
  failed: string[];
}

export const driveStatus = (): Promise<DriveStatus> => invoke('drive_status');

/**
 * Store the OAuth client this installation signs in with.
 *
 * Asked of the user rather than shipped in the binary: this repository is public,
 * and a client ID committed to it would be spent by strangers against the quota
 * and would put someone else's app name on the consent screen.
 */
export const setGoogleClient = (
  clientId: string,
  clientSecret: string,
): Promise<GoogleAccount> => invoke('set_google_client', { clientId, clientSecret });

/** Opens the browser and waits — up to three minutes — for the account choice. */
export const connectGoogle = (): Promise<GoogleAccount> => invoke('connect_google');

export const disconnectGoogle = (): Promise<GoogleAccount> => invoke('disconnect_google');

export const setDriveFolder = (folder: string): Promise<void> =>
  invoke('set_drive_folder', { folder });

/**
 * Write the vault as it is now into the Drive folder.
 *
 * Refreshes the sidecars and the database snapshot first, so what lands in Drive
 * is the library as it stands rather than as it was at the last close.
 */
export const backupToDrive = (): Promise<SyncReport> => invoke('backup_to_drive');

/** Copy back anything missing or different locally. */
export const restoreFromDrive = (): Promise<SyncReport> => invoke('restore_from_drive');

/** Database snapshot into the vault, plus a metadata sidecar per document. */
export const backupNow = (): Promise<string> => invoke('backup_now');

export interface Category {
  id: string;
  name: string;
  color: string | null;
  documentCount: number;
}

export const listCategories = (): Promise<Category[]> => invoke('list_categories');

export const createCategory = (name: string, color?: string): Promise<Category> =>
  invoke('create_category', { name, color: color ?? null });

export const renameCategory = (id: string, name: string): Promise<void> =>
  invoke('rename_category', { id, name });

/** Archive, not delete — old exports stay explicable. */
export const archiveCategory = (id: string): Promise<void> => invoke('archive_category', { id });

export const setDocumentCategories = (documentId: string, categoryIds: string[]): Promise<void> =>
  invoke('set_document_categories', { documentId, categoryIds });

export const tagDocuments = (documentIds: string[], categoryId: string): Promise<number> =>
  invoke('tag_documents', { documentIds, categoryId });

/** Take a category off many documents at once — the undo for a bulk tag. */
export const untagDocuments = (documentIds: string[], categoryId: string): Promise<number> =>
  invoke('untag_documents', { documentIds, categoryId });

/** (documentId, categoryId) pairs, joined client-side. */
export const documentTags = (documentIds: string[]): Promise<Array<[string, string]>> =>
  invoke('document_tags', { documentIds });

export interface DocumentRow {
  id: string;
  patientId: string;
  patient: string;
  docDate: string;
  title: string;
  docType: string;
  fileKind: string;
  relPath: string;
  pageCount: number;
  byteSize: number;
  /** What the paper does not say. Searchable, and kept in the sidecar. */
  notes: string | null;
  missing: boolean;
}

export const listDocuments = (): Promise<DocumentRow[]> => invoke('list_documents');
export const listYears = (): Promise<string[]> => invoke('list_years');

export type Preset = 'original' | 'standard' | 'emailSafe';

export interface ExportRequest {
  patientIds: string[];
  years: string[];
  categoryIds: string[];
  docTypes: string[];
  preset: Preset;
  /** Split into parts above this size. null disables splitting. */
  maxBytes: number | null;
  outDir: string;
  baseName: string;
}

export interface ExportPart {
  path: string;
  bytes: number;
  pages: number;
  documents: number;
}

export interface ExportResult {
  parts: ExportPart[];
  totalDocuments: number;
  undated: number;
  missing: string[];
}

export const exportPdf = (request: ExportRequest): Promise<ExportResult> =>
  invoke('export_pdf', { request });

/**
 * A saved set of export filters.
 *
 * The filters are stored, never the result — a preset opened a year from now
 * picks up everything filed since, which is what "all thyroid reports" means to
 * whoever asked for it.
 */
export interface ExportPreset {
  id: string;
  name: string;
  patientIds: string[];
  years: string[];
  categoryIds: string[];
  docTypes: string[];
  preset: Preset;
  /** null means never split. */
  maxBytes: number | null;
}

export const listExportPresets = (): Promise<ExportPreset[]> => invoke('list_export_presets');

/** Saving over an existing name replaces it. */
export const saveExportPreset = (name: string, preset: ExportPreset): Promise<ExportPreset> =>
  invoke('save_export_preset', { name, preset });

export const deleteExportPreset = (id: string): Promise<void> =>
  invoke('delete_export_preset', { id });

/** Keeps the list ordered by habit rather than alphabet. */
export const useExportPreset = (id: string): Promise<void> => invoke('use_export_preset', { id });

export interface FolderExport {
  outDir: string;
  copied: number;
  missing: string[];
}

/**
 * The same filtered slice as loose, numbered files.
 *
 * The escape hatch for when merging cannot work — an unreadable source, no room
 * for the intermediates. The canonical filenames already carry date, patient and
 * title, and the numbering keeps the order the merged PDF would have had.
 */
export const exportToFolder = (request: ExportRequest): Promise<FolderExport> =>
  invoke('export_to_folder', { request });

export const revealInExplorer = (path: string): Promise<void> =>
  invoke('reveal_in_explorer', { path });

export const commitItem = (args: {
  ingestId: string;
  patientId: string;
  docDate: string;
  title: string;
  docType: string;
}): Promise<CommittedDocument> => invoke('commit_item', args);
