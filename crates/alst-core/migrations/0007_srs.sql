-- SRS module (SPEC §11.2). All tables are synced (SPEC §4.1) except local_example_pool.
-- Card scheduling columns (queue, due, stability, difficulty, step, reps, lapses, last_review) are a cache of
-- replaying the card's review events (SPEC §11.6): the store writes them without change capture, and every
-- device derives the same values. They have defaults so a card row can be created from its identity columns.
CREATE TABLE deck_configs (
  id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  name TEXT NOT NULL,
  new_per_day INTEGER NOT NULL DEFAULT 20,
  reviews_per_day INTEGER NOT NULL DEFAULT 200,
  desired_retention REAL NOT NULL DEFAULT 0.9,
  learning_steps TEXT NOT NULL DEFAULT '["1m","10m"]',
  relearning_steps TEXT NOT NULL DEFAULT '["10m"]',
  max_interval_days INTEGER NOT NULL DEFAULT 36500,
  fsrs_params TEXT, -- json, 21 numbers; null = defaults
  bury_siblings INTEGER NOT NULL DEFAULT 1,
  day_starts_hour INTEGER NOT NULL DEFAULT 4
);

CREATE TABLE decks (
  id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  name TEXT NOT NULL, -- "A::B" nests, as Anki
  kind TEXT NOT NULL DEFAULT 'classic', -- classic | concept
  language TEXT,
  config_id TEXT,
  anki_id INTEGER,
  description TEXT NOT NULL DEFAULT ''
);

CREATE TABLE note_types (
  id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  name TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'standard', -- standard | cloze
  fields TEXT NOT NULL DEFAULT '[]', -- json [name]
  templates TEXT NOT NULL DEFAULT '[]', -- json [{name, qfmt, afmt}]
  css TEXT NOT NULL DEFAULT '',
  anki_id INTEGER,
  builtin TEXT -- key of a built-in type, null for imported or user-made
);

CREATE TABLE notes (
  id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  note_type_id TEXT NOT NULL,
  deck_id TEXT NOT NULL,
  fields TEXT NOT NULL DEFAULT '[]', -- json [string]
  tags TEXT NOT NULL DEFAULT '[]',
  guid TEXT,
  anki_id INTEGER,
  concept_id TEXT,
  term_id TEXT,
  source TEXT -- json {workId, partId, start, end, cue?}
);
CREATE INDEX notes_deck ON notes(deck_id);
CREATE INDEX notes_guid ON notes(guid);

CREATE TABLE cards (
  id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  note_id TEXT,
  concept_id TEXT,
  deck_id TEXT NOT NULL,
  ord INTEGER NOT NULL DEFAULT 0,
  direction TEXT, -- recognition | production (concept cards)
  suspended INTEGER NOT NULL DEFAULT 0,
  buried_until INTEGER NOT NULL DEFAULT 0,
  anki_id INTEGER,
  -- cache of replaying review events (not in _changes)
  queue TEXT NOT NULL DEFAULT 'new', -- new | learning | review | relearning
  due INTEGER NOT NULL DEFAULT 0, -- ms
  stability REAL,
  difficulty REAL,
  step INTEGER NOT NULL DEFAULT 0,
  reps INTEGER NOT NULL DEFAULT 0,
  lapses INTEGER NOT NULL DEFAULT 0,
  last_review INTEGER
);
CREATE INDEX cards_deck ON cards(deck_id, queue, due);
CREATE INDEX cards_note ON cards(note_id);
CREATE INDEX cards_concept ON cards(concept_id);

CREATE TABLE note_media (
  id TEXT PRIMARY KEY, -- derived from the name (Anki's flat media namespace)
  created_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  name TEXT NOT NULL,
  media_hash TEXT NOT NULL
);

CREATE TABLE local_example_pool (
  concept_id TEXT NOT NULL,
  part_id TEXT NOT NULL,
  start INTEGER NOT NULL,
  end INTEGER NOT NULL,
  score REAL NOT NULL,
  last_shown INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (concept_id, part_id, start)
);

