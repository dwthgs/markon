-- One row per finished turn, on this Mac only: where its time went.
CREATE TABLE traces (
  id INTEGER PRIMARY KEY,
  at INTEGER NOT NULL,
  user_text TEXT NOT NULL DEFAULT '',
  provider TEXT NOT NULL DEFAULT '',
  model TEXT NOT NULL DEFAULT '',
  stt_ms INTEGER,
  first_text_ms INTEGER,
  first_audio_ms INTEGER,
  total_ms INTEGER NOT NULL DEFAULT 0,
  tokens_in INTEGER NOT NULL DEFAULT 0,
  tokens_out INTEGER NOT NULL DEFAULT 0,
  tools_json TEXT NOT NULL DEFAULT '[]',
  ok INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX traces_at ON traces(at);
