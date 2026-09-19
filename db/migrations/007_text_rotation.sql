-- Migration 007 — remember how far a staged image was turned to make its text
-- read upright.
--
-- EXIF orientation only covers a phone held sideways. A report lying sideways on
-- the table, a page fed upside down into a scanner, or a screenshot that never
-- had a tag at all, all arrive with pixels the tag calls upright. The recognizer
-- is run at each quarter turn and the staged file is rewritten at the turn that
-- read best; the value here is the turn that was applied, clockwise, in degrees.
--
-- NULL means it has not been decided yet, which is what makes recognition try.
-- Once set — by the detector or by the user turning the page by hand — it is
-- never second-guessed, or a hand correction would be undone on the next read.

ALTER TABLE ingest_items ADD COLUMN text_rotation INTEGER;

INSERT INTO schema_migrations (version, applied_at) VALUES (7, datetime('now'));
