-- Small, durable settings that belong to the installation rather than to a
-- document: where the Google Drive folder is, when the last sync ran.
--
-- A table rather than a config file because the database is already the thing
-- that is backed up, restored and reasoned about, and a second source of truth
-- that can disagree with it is a bug waiting to be written.

CREATE TABLE app_setting (
  key        TEXT PRIMARY KEY,
  value      TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

INSERT INTO schema_migrations (version, applied_at) VALUES (5, datetime('now'));
