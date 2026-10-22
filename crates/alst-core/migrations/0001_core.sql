CREATE TABLE works (
  id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  language TEXT NOT NULL,
  title TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'text',
  author TEXT,
  source_url TEXT,
  source_license TEXT,
  pack_id TEXT,
  tags TEXT NOT NULL DEFAULT '[]',
  archived INTEGER NOT NULL DEFAULT 0,
  settings TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE parts (
  id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  work_id TEXT NOT NULL REFERENCES works(id),
  idx INTEGER NOT NULL,
  title TEXT,
  text TEXT NOT NULL DEFAULT '',
  media_hash TEXT,
  timing TEXT,
  word_count INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX parts_work ON parts(work_id, idx);

CREATE TABLE concepts (
  id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  language TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'lexeme',
  key TEXT NOT NULL,
  meta TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE terms (
  id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  language TEXT NOT NULL,
  text TEXT NOT NULL,
  text_lc TEXT NOT NULL,
  lemma TEXT,
  concept_id TEXT REFERENCES concepts(id),
  status INTEGER NOT NULL DEFAULT 0,
  translation TEXT,
  reading TEXT,
  notes TEXT,
  tags TEXT NOT NULL DEFAULT '[]',
  word_count INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX terms_lookup ON terms(language, text_lc);

CREATE TABLE term_parents (
  id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  term_id TEXT NOT NULL REFERENCES terms(id),
  parent_id TEXT NOT NULL REFERENCES terms(id)
);

CREATE TABLE media (
  id TEXT PRIMARY KEY, -- sha256
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  mime TEXT NOT NULL,
  size INTEGER NOT NULL,
  original_name TEXT
);

CREATE TABLE extension_state (
  id TEXT PRIMARY KEY, -- extension id
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  enabled INTEGER NOT NULL DEFAULT 1,
  settings TEXT NOT NULL DEFAULT '{}',
  languages TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE ext_data (
  id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  extension_id TEXT NOT NULL,
  key TEXT NOT NULL,
  value TEXT NOT NULL DEFAULT 'null'
);
CREATE UNIQUE INDEX ext_data_key ON ext_data(extension_id, key);

CREATE TABLE settings (
  id TEXT PRIMARY KEY, -- key
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  value TEXT NOT NULL DEFAULT 'null'
);

CREATE TABLE events (
  id TEXT PRIMARY KEY,
  hlc INTEGER NOT NULL,
  site_id TEXT NOT NULL,
  type TEXT NOT NULL,
  language TEXT,
  subject_type TEXT,
  subject_id TEXT,
  payload TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX events_subject ON events(subject_type, subject_id);

CREATE TABLE _changes (
  tbl TEXT NOT NULL,
  row_id TEXT NOT NULL,
  col TEXT NOT NULL,
  value TEXT,
  hlc INTEGER NOT NULL,
  site_id TEXT NOT NULL,
  PRIMARY KEY (tbl, row_id, col)
);
CREATE INDEX _changes_hlc ON _changes(hlc, site_id);

CREATE TABLE local_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE local_peers (
  site_id TEXT PRIMARY KEY,
  name TEXT,
  cert_fingerprint TEXT,
  last_seen INTEGER,
  last_vector TEXT NOT NULL DEFAULT '{}'
);
