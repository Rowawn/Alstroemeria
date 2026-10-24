-- LAN sync (SPEC §9.2): vector clock, pending rows, peers, and the synced dictionary/extension package tables.
CREATE TABLE local_sync_clock (
  site_id TEXT PRIMARY KEY,
  hlc INTEGER NOT NULL
);
-- Everything this device has written or applied, per site. Not derived from _changes: superseded entries are gone there.
INSERT INTO local_sync_clock (site_id, hlc)
  SELECT site_id, MAX(hlc) FROM (SELECT site_id, hlc FROM _changes UNION ALL SELECT site_id, hlc FROM events) GROUP BY site_id;

CREATE TABLE local_sync_pending (
  tbl TEXT NOT NULL,
  row_id TEXT NOT NULL,
  PRIMARY KEY (tbl, row_id)
);

ALTER TABLE local_peers ADD COLUMN public_key TEXT;
ALTER TABLE local_peers ADD COLUMN addr TEXT;
ALTER TABLE local_peers ADD COLUMN port INTEGER;
ALTER TABLE local_peers ADD COLUMN media_policy TEXT NOT NULL DEFAULT '{}';
ALTER TABLE local_peers ADD COLUMN paired_at INTEGER;

ALTER TABLE local_dict_dicts ADD COLUMN source_id TEXT;

CREATE TABLE dictionaries (
  id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  title TEXT NOT NULL,
  revision TEXT,
  language TEXT NOT NULL,
  media_hash TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE extension_packages (
  id TEXT PRIMARY KEY, -- extension id
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  version TEXT NOT NULL,
  media_hash TEXT NOT NULL
);

-- Ordered scans for changes_since / events_since.
CREATE INDEX _changes_order ON _changes(hlc, site_id, tbl, row_id, col);
CREATE INDEX events_order ON events(hlc, site_id, id);
