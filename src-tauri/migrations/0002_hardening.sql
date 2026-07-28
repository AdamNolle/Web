ALTER TABLE summaries ADD COLUMN summary_method TEXT NOT NULL DEFAULT 'extractive';
ALTER TABLE summaries ADD COLUMN uncertainty TEXT NOT NULL DEFAULT '';

CREATE TABLE IF NOT EXISTS app_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS request_receipts (
  request_id TEXT PRIMARY KEY,
  command TEXT NOT NULL,
  payload_hash TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('pending', 'complete')),
  created_at INTEGER NOT NULL,
  completed_at INTEGER
);
