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
 * The review queue as stored, not as remembered.
 *
 * Staging is durable, so a backlog import that was closed halfway through comes
 * back on the next launch instead of leaving files stranded with nothing
 * pointing at them.
 */
export const listStaged = (): Promise<IngestItem[]> => invoke('list_staged');

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
}): Promise<CommittedDocument> => invoke('update_document', args);

/** Move a document to the vault's Trash folder. Nothing is unlinked. */
export const trashDocument = (documentId: string): Promise<void> =>
  invoke('trash_document', { documentId });

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

export const revealInExplorer = (path: string): Promise<void> =>
  invoke('reveal_in_explorer', { path });

export const commitItem = (args: {
  ingestId: string;
  patientId: string;
  docDate: string;
  title: string;
  docType: string;
}): Promise<CommittedDocument> => invoke('commit_item', args);
