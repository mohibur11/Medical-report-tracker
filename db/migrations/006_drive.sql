-- What has been uploaded to Google Drive, and where it landed.
--
-- Drive has no paths. A file is an id, and folders are files whose children point
-- at them, so the vault's shape has to be remembered here or every sync would
-- have to walk the whole remote tree to find out what already exists.
--
-- The recorded size and modification time are what make a second sync cheap: a
-- file whose length and mtime still match what was uploaded is not uploaded
-- again. Being wrong only costs a re-upload; it can never skip a real change.

CREATE TABLE drive_file (
  rel_path    TEXT PRIMARY KEY,        -- vault-relative, backslashes as stored
  file_id     TEXT NOT NULL,           -- Drive's id for it
  is_folder   INTEGER NOT NULL DEFAULT 0,
  size        INTEGER,
  mtime       INTEGER,                 -- seconds since the epoch, local file
  uploaded_at TEXT NOT NULL
);

CREATE INDEX ix_drive_file_id ON drive_file (file_id);

INSERT INTO schema_migrations (version, applied_at) VALUES (6, datetime('now'));
