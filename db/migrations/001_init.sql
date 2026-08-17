-- Migration 001 — initial schema.
--
-- Design rules this schema commits to, each chosen because retrofitting it later
-- is either a data migration or a rewrite:
--
--  * The DATABASE owns identity and metadata. The <Patient>/<Year>/ folder tree is
--    a rendered PROJECTION of these rows, never the source of truth. A single tree
--    encodes exactly one hierarchy, and categories are a second, many-to-many axis.
--  * Identity is an immutable ULID everywhere. Never a name, never a path, never an
--    email — all three change, and two of them are user-facing.
--  * owner_user_id sits on every top-level row from day one, with one user. Adding a
--    tenant column to a populated, synced database is the worst migration in this design.
--  * doc_date is bare TEXT 'YYYY-MM-DD'. No time, no timezone, never an epoch —
--    parsing DD/MM/YYYY into a UTC instant produces off-by-one-day bugs immediately.

PRAGMA foreign_keys = ON;

CREATE TABLE schema_migrations (
  version     INTEGER PRIMARY KEY,
  applied_at  TEXT NOT NULL
);

-- ---------------------------------------------------------------------------
-- users
-- ---------------------------------------------------------------------------
-- v1 is single-user and offline. The password is an honest SCREEN LOCK: files and
-- this database are plaintext, protected at rest by BitLocker, and the UI says so.
--
-- wrapped_dek is the ~40-line seam that keeps "encrypt the metadata DB later" or
-- "add a real cloud account" a one-row change instead of a re-encryption migration.
-- Nothing is encrypted with it in v1.
CREATE TABLE users (
  id             TEXT PRIMARY KEY,           -- ULID
  email          TEXT NOT NULL,              -- decorative in v1; the join key when accounts become real
  pw_hash        TEXT NOT NULL,              -- Argon2id PHC string, params embedded
  wrapped_dek    BLOB,                       -- reserved; NULL in v1
  dek_dpapi      BLOB,                       -- reserved; "remember this PC"
  created_at     TEXT NOT NULL,
  updated_at     TEXT NOT NULL
);

-- Email is UNIQUE but deliberately NOT the primary key.
CREATE UNIQUE INDEX ux_users_email ON users (lower(email));

-- ---------------------------------------------------------------------------
-- patients
-- ---------------------------------------------------------------------------
CREATE TABLE patients (
  id             TEXT PRIMARY KEY,           -- ULID; the folder slug is a mutable label
  owner_user_id  TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  display_name   TEXT NOT NULL,              -- NFC-normalized; keeps characters the slug strips
  folder_slug    TEXT NOT NULL,              -- path-safe projection of display_name
  dob            TEXT,                       -- 'YYYY-MM-DD'; hard-rejects this date during extraction
  notes          TEXT,
  archived_at    TEXT,
  created_at     TEXT NOT NULL,
  updated_at     TEXT NOT NULL
);

-- NTFS is case-insensitive but case-preserving: without this, 'Rahim' and 'rahim'
-- become two rows mapping to ONE physical directory and their files silently interleave.
CREATE UNIQUE INDEX ux_patients_slug ON patients (owner_user_id, upper(folder_slug));

-- ---------------------------------------------------------------------------
-- categories
-- ---------------------------------------------------------------------------
-- Global rather than per-patient: "thyroid" spans family members, and requirement 5
-- exports a category across everyone.
CREATE TABLE categories (
  id             TEXT PRIMARY KEY,           -- ULID; documents reference this, never the name,
  owner_user_id  TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name           TEXT NOT NULL,              -- so rename is a one-row UPDATE with zero disk I/O
  color          TEXT,
  archived_at    TEXT,                       -- archive over hard delete, so old exports stay explicable
  created_at     TEXT NOT NULL,
  updated_at     TEXT NOT NULL
);

CREATE UNIQUE INDEX ux_categories_name ON categories (owner_user_id, lower(name));

-- ---------------------------------------------------------------------------
-- documents
-- ---------------------------------------------------------------------------
-- One row per FILE, not per page: a 12-page PDF is one document with page_count=12.
CREATE TABLE documents (
  id              TEXT PRIMARY KEY,          -- ULID
  owner_user_id   TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  patient_id      TEXT NOT NULL REFERENCES patients(id) ON DELETE RESTRICT,

  -- Bare text. '2026-03-00' = month known, day not. '0000-00-00' = unknown.
  -- Both sort correctly as plain strings and feed the "undated" queue.
  doc_date        TEXT NOT NULL,
  date_source     TEXT NOT NULL,             -- pdf_text | ocr | exif | manual | mtime
  date_confidence REAL,                      -- so the app knows whether it has already asked

  title           TEXT NOT NULL,             -- the test/report name; drives the filename
  doc_type        TEXT NOT NULL,             -- report | prescription | invoice | other
  notes           TEXT,

  page_count      INTEGER NOT NULL DEFAULT 1,
  rel_path        TEXT NOT NULL,             -- vault-relative; a PROJECTION, not identity
  sha256          TEXT NOT NULL,
  byte_size       INTEGER NOT NULL,
  file_kind       TEXT NOT NULL,             -- from magic bytes, never the source extension
  exif_orientation INTEGER,                  -- as found; pixels are baked upright at ingest

  missing_at      TEXT,                      -- set by the reconciler; rows are NEVER auto-purged
  trashed_at      TEXT,                      -- delete moves to a trash folder, never unlinks
  created_at      TEXT NOT NULL,
  updated_at      TEXT NOT NULL
);

CREATE INDEX ix_documents_patient_date ON documents (patient_id, doc_date);
CREATE INDEX ix_documents_date         ON documents (owner_user_id, doc_date);
CREATE INDEX ix_documents_sha          ON documents (owner_user_id, sha256);
CREATE UNIQUE INDEX ux_documents_relpath ON documents (owner_user_id, rel_path);

-- ---------------------------------------------------------------------------
-- document_category  (many-to-many)
-- ---------------------------------------------------------------------------
-- A thyroid panel for a diabetic patient legitimately belongs to both categories,
-- which is exactly the case requirement 5 exercises. A single category_id column
-- cannot express it, and the folder tree cannot either.
CREATE TABLE document_category (
  document_id  TEXT NOT NULL REFERENCES documents(id)  ON DELETE CASCADE,
  category_id  TEXT NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
  created_at   TEXT NOT NULL,
  PRIMARY KEY (document_id, category_id)
);

CREATE INDEX ix_doccat_category ON document_category (category_id);

-- ---------------------------------------------------------------------------
-- document_page
-- ---------------------------------------------------------------------------
-- OCR text lives here, not on documents, so large blobs never bloat a list query.
CREATE TABLE document_page (
  document_id  TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  page_no      INTEGER NOT NULL,
  ocr_text     TEXT,
  ocr_json     TEXT,                         -- word boxes + confidences; OcrResult.Text
  width_px     INTEGER,                      -- flattens layout, so geometry must be kept
  height_px    INTEGER,
  PRIMARY KEY (document_id, page_no)
);

-- ---------------------------------------------------------------------------
-- extraction_candidate
-- ---------------------------------------------------------------------------
-- Runner-up guesses with their bounding boxes. This is what makes "wrong date,
-- use the other one" a single click, and lets the review grid show a cropped
-- snippet of exactly where a date was read from.
CREATE TABLE extraction_candidate (
  id           TEXT PRIMARY KEY,
  document_id  TEXT REFERENCES documents(id) ON DELETE CASCADE,
  ingest_id    TEXT,                         -- set while still staged, before a document exists
  kind         TEXT NOT NULL,                -- date | title | patient
  value        TEXT NOT NULL,
  raw          TEXT,                         -- as it appeared, for display
  score        REAL NOT NULL,
  anchor       TEXT,                         -- the label that justified the score
  page_no      INTEGER,
  bbox         TEXT,                         -- JSON [x,y,w,h] in page pixels
  created_at   TEXT NOT NULL
);

CREATE INDEX ix_candidate_document ON extraction_candidate (document_id, kind, score);
CREATE INDEX ix_candidate_ingest   ON extraction_candidate (ingest_id, kind, score);

-- ---------------------------------------------------------------------------
-- ingest_items
-- ---------------------------------------------------------------------------
-- Durable staging with an explicit status machine, so closing the app 40 files into
-- a 60-photo import does not discard completed OCR work and typed corrections.
CREATE TABLE ingest_items (
  id                TEXT PRIMARY KEY,        -- ULID
  owner_user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  batch_id          TEXT NOT NULL,
  src_path          TEXT NOT NULL,
  staged_path       TEXT,                    -- normalized copy inside the vault's staging area
  status            TEXT NOT NULL,           -- pending|extracted|needs_date|needs_patient|committed|failed|duplicate
  error             TEXT,

  file_kind         TEXT,
  sha256            TEXT,
  byte_size         INTEGER,
  page_count        INTEGER,
  exif_orientation  INTEGER,

  guessed_date      TEXT,
  guessed_title     TEXT,
  guessed_type      TEXT,
  chosen_patient_id TEXT REFERENCES patients(id) ON DELETE SET NULL,
  chosen_date       TEXT,                    -- what the user typed; wins over guessed_*
  chosen_title      TEXT,
  chosen_type       TEXT,

  document_id       TEXT REFERENCES documents(id) ON DELETE SET NULL,
  created_at        TEXT NOT NULL,
  updated_at        TEXT NOT NULL
);

CREATE INDEX ix_ingest_batch  ON ingest_items (batch_id, status);
CREATE INDEX ix_ingest_status ON ingest_items (owner_user_id, status);
CREATE INDEX ix_ingest_sha    ON ingest_items (sha256);

-- ---------------------------------------------------------------------------
-- fs_journal
-- ---------------------------------------------------------------------------
-- Windows has no transactional multi-file rename (TxF is deprecated) and denies
-- rename on any file with an open handle. Renaming a patient with 500 files must
-- therefore commit intent HERE first, then apply idempotently and resume on restart.
-- Without this, one open PDF viewer or one power cut leaves a half-renamed tree.
CREATE TABLE fs_journal (
  id           TEXT PRIMARY KEY,             -- ULID; ULIDs sort by creation, so this is the replay order
  op           TEXT NOT NULL,                -- move | rename_dir | write | trash
  intent_json  TEXT NOT NULL,                -- {from, to, ...}
  applied_at   TEXT,                         -- NULL = not yet applied; replayed on next start
  failed_at    TEXT,
  error        TEXT,
  created_at   TEXT NOT NULL
);

CREATE INDEX ix_journal_pending ON fs_journal (applied_at, id);

-- ---------------------------------------------------------------------------
-- name_reservation
-- ---------------------------------------------------------------------------
-- Collision suffixes (__02, __03) are allocated monotonically and NEVER reused, so
-- a deleted file's suffix cannot be handed to a different document later and make
-- two exports disagree about what "__02" meant.
CREATE TABLE name_reservation (
  dir_rel     TEXT NOT NULL,                 -- '<PatientSlug>\<Year>'
  base_name   TEXT NOT NULL,                 -- 'YYYY-MM-DD_Patient_Title'
  next_seq    INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (dir_rel, base_name)
);

-- ---------------------------------------------------------------------------
-- full-text search
-- ---------------------------------------------------------------------------
-- Searching "creatinine" across five years is worth more than perfect field
-- extraction, and degrades gracefully at a 6% character error rate.
-- FTS5 is available from the `bundled` rusqlite feature alone — there is no `fts5`
-- feature in rusqlite 0.37, and requesting one fails the build.
CREATE VIRTUAL TABLE document_fts USING fts5 (
  title,
  notes,
  ocr_text,
  content = '',                              -- external-content: we own the writes
  tokenize = 'unicode61 remove_diacritics 2'
);

INSERT INTO schema_migrations (version, applied_at) VALUES (1, datetime('now'));
