CREATE TABLE IF NOT EXISTS app_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS clipboard_items (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('text','image','files')),
  fingerprint TEXT NOT NULL UNIQUE,
  payload_json TEXT NOT NULL,
  preview TEXT NOT NULL,
  byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
  is_favorite INTEGER NOT NULL DEFAULT 0 CHECK (is_favorite IN (0,1)),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS clipboard_items_order
  ON clipboard_items(is_favorite DESC, updated_at_ms DESC);
CREATE INDEX IF NOT EXISTS clipboard_items_kind
  ON clipboard_items(kind);
