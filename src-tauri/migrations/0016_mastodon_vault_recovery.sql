-- A vault write can have indeterminate finality if the operating-system credential
-- provider returns an error after accepting it. Keep only the non-secret reference
-- until the source and request receipt commit together, so a later app launch can
-- remove an orphaned credential without ever persisting token material in SQLite.
CREATE TABLE IF NOT EXISTS pending_vault_cleanup (
  request_id TEXT PRIMARY KEY REFERENCES request_receipts(request_id) ON DELETE CASCADE,
  secret_ref TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL
);
