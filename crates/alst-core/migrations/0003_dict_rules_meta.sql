-- Deinflection rules per dictionary entry and Yomitan term-meta banks (SPEC §16.5, §16.7).
-- Device-local like the rest of `local_dict_*`. Entries imported before this migration have
-- rules = '' and their dictionary has rules_imported = 0 (re-import to enable conjugation lookup).
ALTER TABLE local_dict_entries ADD COLUMN rules TEXT NOT NULL DEFAULT '';
ALTER TABLE local_dict_dicts ADD COLUMN rules_imported INTEGER NOT NULL DEFAULT 0;

CREATE TABLE local_dict_meta (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  dict_id INTEGER NOT NULL REFERENCES local_dict_dicts(id) ON DELETE CASCADE,
  term TEXT NOT NULL,
  reading TEXT NOT NULL DEFAULT '',
  mode TEXT NOT NULL,      -- 'freq' | 'pitch'
  data TEXT NOT NULL       -- JSON, see SPEC §16.7
);
CREATE INDEX local_dict_meta_term ON local_dict_meta(term, mode);
CREATE INDEX local_dict_meta_dict ON local_dict_meta(dict_id);
