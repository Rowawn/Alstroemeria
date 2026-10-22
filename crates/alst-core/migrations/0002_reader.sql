-- Imported dictionary data (SPEC §7). Local to the device; never synced.
CREATE TABLE local_dict_dicts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  title TEXT NOT NULL,
  revision TEXT,
  language TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  entry_count INTEGER NOT NULL DEFAULT 0,
  imported_at INTEGER NOT NULL
);

CREATE TABLE local_dict_entries (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  dict_id INTEGER NOT NULL REFERENCES local_dict_dicts(id) ON DELETE CASCADE,
  term TEXT NOT NULL,
  term_lc TEXT NOT NULL,
  reading TEXT NOT NULL DEFAULT '',
  tags TEXT NOT NULL DEFAULT '',
  score INTEGER NOT NULL DEFAULT 0,
  glossary TEXT NOT NULL  -- JSON array of plain-text definitions
);
CREATE INDEX local_dict_entries_term ON local_dict_entries(term_lc);
CREATE INDEX local_dict_entries_reading ON local_dict_entries(reading) WHERE reading <> '';
CREATE INDEX local_dict_entries_dict ON local_dict_entries(dict_id);

CREATE INDEX term_parents_term ON term_parents(term_id);
CREATE INDEX events_type ON events(type, hlc);
