-- Cached tokenizer output for morphological languages (SPEC §5.3). Device-local; safe to delete any time.
CREATE TABLE local_token_cache (
  part_id TEXT PRIMARY KEY,
  fingerprint TEXT NOT NULL,   -- language definition + engine + text hash; a mismatch means "recompute"
  tokens TEXT NOT NULL,        -- JSON: sentences of tokens
  used_at INTEGER NOT NULL
);
