-- Saved export filters.
--
-- The export screen asks the same five questions every time, and the answers
-- rarely change: this doctor wants the thyroid history, that one wants everything
-- from last year. Retyping them is friction on the one action the whole app
-- exists to perform.
--
-- The filters are stored, not the result. A preset applied a year from now should
-- pick up everything filed since, which is exactly what "all thyroid reports"
-- means to the person asking for it.

CREATE TABLE export_preset (
  id            TEXT PRIMARY KEY,        -- ULID
  owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name          TEXT NOT NULL,

  -- JSON arrays of ids/values. Deliberately not foreign keys: a preset that
  -- mentions a category the user later archives should still open, minus that
  -- category, rather than failing or cascading itself away.
  patient_ids   TEXT NOT NULL DEFAULT '[]',
  years         TEXT NOT NULL DEFAULT '[]',
  category_ids  TEXT NOT NULL DEFAULT '[]',
  doc_types     TEXT NOT NULL DEFAULT '[]',

  preset        TEXT NOT NULL,           -- original | standard | emailSafe
  max_bytes     INTEGER,                 -- NULL disables splitting

  created_at    TEXT NOT NULL,
  used_at       TEXT
);

-- Names are how the user picks one, so two with the same name is a trap. NTFS
-- case rules do not apply here, but people do not distinguish them either.
CREATE UNIQUE INDEX ux_preset_name ON export_preset (owner_user_id, lower(name));

INSERT INTO schema_migrations (version, applied_at) VALUES (4, datetime('now'));
