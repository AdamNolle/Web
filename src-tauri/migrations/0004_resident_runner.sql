-- Digest schedule timestamps from pre-runner builds were placeholders. They are retained only
-- for schema compatibility and are never authoritative; zero makes that explicit.
UPDATE digests SET next_edition_at = 0;
-- Version 3 could not distinguish legacy fallback timestamps. Understate rather than falsely
-- asserting publication provenance; newly synchronized rows carry an explicit kind.
UPDATE posts SET published_time_kind = 'fetched';

CREATE TABLE IF NOT EXISTS runner_state (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  last_attempt_at INTEGER,
  last_success_at INTEGER,
  last_scheduled_for INTEGER,
  next_scheduled_at INTEGER,
  lease_owner TEXT,
  lease_expires_at INTEGER,
  last_outcome TEXT NOT NULL DEFAULT 'idle',
  detail TEXT NOT NULL DEFAULT 'The resident runner has not attempted work yet.'
);
INSERT OR IGNORE INTO runner_state(singleton) VALUES(1);

CREATE TABLE IF NOT EXISTS receipt_sources (
  request_id TEXT PRIMARY KEY REFERENCES request_receipts(request_id) ON DELETE CASCADE,
  source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS receipt_sources_source_idx ON receipt_sources(source_id);
