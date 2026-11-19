-- Every tool action that changed something, for "what did you just do?" and undo.
CREATE TABLE activity (
  id INTEGER PRIMARY KEY,
  at INTEGER NOT NULL,
  tool TEXT NOT NULL,
  target TEXT NOT NULL DEFAULT '',
  result TEXT NOT NULL DEFAULT '',
  ok INTEGER NOT NULL DEFAULT 1,
  undo_json TEXT,
  undone INTEGER NOT NULL DEFAULT 0
);
