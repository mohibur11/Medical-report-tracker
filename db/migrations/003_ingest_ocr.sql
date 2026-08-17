-- Migration 003 — hold recognised text on the staged item.
--
-- OCR runs while a file is still in the inbox, before a patient and a date are
-- confirmed, so there is no document row to attach it to yet. Keeping it on
-- `ingest_items` means a crash mid-review does not throw away recognition work
-- that took ~350 ms per page — which is the whole point of the durable staging
-- table.
--
-- On commit the text moves to `document_page`, where it becomes searchable.

ALTER TABLE ingest_items ADD COLUMN ocr_text TEXT;

-- Word boxes and confidences. OcrResult.Text flattens layout, so anything that
-- later needs geometry — cropping the snippet that a date was read from, or
-- reading a results table — needs this rather than the flat string.
ALTER TABLE ingest_items ADD COLUMN ocr_json TEXT;

-- When recognition ran, so a re-import does not silently reuse a stale result.
ALTER TABLE ingest_items ADD COLUMN ocr_at TEXT;

INSERT INTO schema_migrations (version, applied_at) VALUES (3, datetime('now'));
