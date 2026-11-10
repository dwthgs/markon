-- Chunks of the user's documents and sources, searched by keyword (FTS5) and meaning
-- (a vector per chunk), and one person across every source.
CREATE TABLE chunks (
  id INTEGER PRIMARY KEY,
  source TEXT NOT NULL,
  reference TEXT NOT NULL,
  title TEXT NOT NULL DEFAULT '',
  text TEXT NOT NULL,
  mtime INTEGER NOT NULL DEFAULT 0,
  vector BLOB
);
CREATE INDEX chunks_reference ON chunks(reference);
CREATE VIRTUAL TABLE chunks_fts USING fts5(text, title, content='chunks', content_rowid='id', tokenize='porter');
CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
  INSERT INTO chunks_fts(rowid, text, title) VALUES (new.id, new.text, new.title);
END;
CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
  INSERT INTO chunks_fts(chunks_fts, rowid, text, title) VALUES ('delete', old.id, old.text, old.title);
END;
CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE);
CREATE TABLE identities (
  person_id INTEGER NOT NULL REFERENCES people(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY (kind, value)
);
