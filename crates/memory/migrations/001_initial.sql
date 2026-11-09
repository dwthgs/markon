-- Spec section 5 core tables.
CREATE TABLE conversations (
  id INTEGER PRIMARY KEY,
  started_at INTEGER NOT NULL,
  title TEXT NOT NULL DEFAULT '',
  source TEXT NOT NULL CHECK (source IN ('voice', 'text', 'schedule'))
);
CREATE TABLE turns (
  id INTEGER PRIMARY KEY,
  conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  text TEXT NOT NULL,
  audio_ms INTEGER NOT NULL DEFAULT 0,
  tokens_in INTEGER NOT NULL DEFAULT 0,
  tokens_out INTEGER NOT NULL DEFAULT 0,
  provider TEXT NOT NULL DEFAULT '',
  model TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
);
CREATE TABLE tool_calls (
  id INTEGER PRIMARY KEY,
  turn_id INTEGER REFERENCES turns(id) ON DELETE CASCADE,
  tool TEXT NOT NULL,
  args_json TEXT NOT NULL,
  result_ref TEXT,
  status TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  ended_at INTEGER
);
CREATE TABLE cards (
  id TEXT NOT NULL,
  conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  layout_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL,
  PRIMARY KEY (conversation_id, id)
);
CREATE TABLE facts (
  id INTEGER PRIMARY KEY,
  subject TEXT NOT NULL,
  statement TEXT NOT NULL,
  source_turn_id INTEGER REFERENCES turns(id) ON DELETE SET NULL,
  derived_from INTEGER REFERENCES facts(id) ON DELETE CASCADE,
  origin TEXT NOT NULL CHECK (origin IN ('user_stated', 'tool_confirmed', 'derived')),
  confidence REAL NOT NULL,
  created_at INTEGER NOT NULL,
  superseded_by INTEGER REFERENCES facts(id) ON DELETE SET NULL
);
CREATE TABLE preferences (key TEXT PRIMARY KEY, value_json TEXT NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE schedules (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  cron TEXT NOT NULL,
  agent TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  last_run INTEGER,
  next_run INTEGER
);
CREATE TABLE usage (
  day TEXT NOT NULL,
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  tokens_in INTEGER NOT NULL DEFAULT 0,
  tokens_out INTEGER NOT NULL DEFAULT 0,
  est_cost REAL NOT NULL DEFAULT 0,
  PRIMARY KEY (day, provider, model)
);

CREATE VIRTUAL TABLE facts_fts USING fts5(statement, subject, content='facts', content_rowid='id', tokenize='porter');
CREATE TRIGGER facts_ai AFTER INSERT ON facts BEGIN
  INSERT INTO facts_fts(rowid, statement, subject) VALUES (new.id, new.statement, new.subject);
END;
CREATE TRIGGER facts_ad AFTER DELETE ON facts BEGIN
  INSERT INTO facts_fts(facts_fts, rowid, statement, subject) VALUES ('delete', old.id, old.statement, old.subject);
END;
CREATE VIRTUAL TABLE turns_fts USING fts5(text, content='turns', content_rowid='id', tokenize='porter');
CREATE TRIGGER turns_ai AFTER INSERT ON turns BEGIN
  INSERT INTO turns_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER turns_ad AFTER DELETE ON turns BEGIN
  INSERT INTO turns_fts(turns_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE INDEX facts_subject ON facts(subject);
CREATE INDEX turns_conv ON turns(conversation_id);
