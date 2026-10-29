-- Content packs (SPEC §10.3): the packs that were imported, and which pack item a work came from.
CREATE TABLE packs (
  id TEXT PRIMARY KEY, -- the pack's own id (manifest.id)
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  title TEXT NOT NULL,
  language TEXT NOT NULL,
  topic TEXT,
  level TEXT,
  description TEXT,
  created_by TEXT,
  version_at TEXT, -- manifest createdAt
  manifest TEXT NOT NULL DEFAULT '{}' -- the manifest json, for the pack view and attributions
);

ALTER TABLE works ADD COLUMN pack_item TEXT;
CREATE INDEX works_pack ON works(pack_id, pack_item);
