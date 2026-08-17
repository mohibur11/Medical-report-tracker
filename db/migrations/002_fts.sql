-- Migration 002 — rebuild the full-text index so it can return snippets.
--
-- 001 declared `document_fts` with `content = ''` (contentless). A contentless
-- FTS5 table stores no column values, so `snippet()` and `highlight()` return
-- nothing — and a search result with no context is barely a search result.
-- Contentless tables also cannot be UPDATEd, which the re-index path needs.
--
-- External-content was the other option, but it keys on an INTEGER rowid and
-- documents are identified by a TEXT ULID, so it does not fit either.
--
-- This stores the searchable text in the index and carries `doc_id` as an
-- UNINDEXED column to map hits back. The duplication is a few hundred bytes per
-- document and buys working snippets.

DROP TABLE IF EXISTS document_fts;

CREATE VIRTUAL TABLE document_fts USING fts5 (
  doc_id UNINDEXED,
  title,
  notes,
  ocr_text,
  tokenize = 'unicode61 remove_diacritics 2'
);

INSERT INTO schema_migrations (version, applied_at) VALUES (2, datetime('now'));
