-- Adds a non-live "archive import" source kind for official personal-data-export files (X,
-- Instagram; Facebook deferred). These sources never make a network call and are never selected
-- by the resident/manual RSS runner (both existing sync queries filter on connector_kind='rss').
-- SQLite cannot ALTER a CHECK constraint in place, so the sources table is rebuilt with every
-- existing column preserved exactly. The Rust migration runner disables the `foreign_keys` pragma
-- for exactly this migration (see db.rs) before running it: with enforcement on, `DROP TABLE
-- sources` fires every child table's `ON DELETE CASCADE` action and silently deletes all posts,
-- comments, actors, and sync metadata, even with `defer_foreign_keys=ON` (that pragma only defers
-- the orphan *check*, not the cascade *action*). No child table or row is otherwise touched here.
CREATE TABLE sources_new (
  id TEXT PRIMARY KEY,
  connector_kind TEXT NOT NULL CHECK (connector_kind IN ('demo', 'rss', 'bluesky', 'mastodon', 'archive_import')),
  account_label TEXT NOT NULL,
  detail TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL CHECK (status IN ('healthy', 'attention', 'paused')),
  config_json TEXT NOT NULL DEFAULT '{}',
  secret_ref TEXT,
  sync_cursor TEXT,
  etag TEXT,
  last_modified TEXT,
  last_success_at INTEGER,
  next_poll_at INTEGER,
  failure_count INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  generation INTEGER NOT NULL DEFAULT 1,
  validator_url TEXT,
  UNIQUE(connector_kind, account_label)
);

INSERT INTO sources_new(id, connector_kind, account_label, detail, status, config_json, secret_ref,
                         sync_cursor, etag, last_modified, last_success_at, next_poll_at,
                         failure_count, created_at, updated_at, generation, validator_url)
SELECT id, connector_kind, account_label, detail, status, config_json, secret_ref,
       sync_cursor, etag, last_modified, last_success_at, next_poll_at,
       failure_count, created_at, updated_at, generation, validator_url
FROM sources;

DROP TABLE sources;
ALTER TABLE sources_new RENAME TO sources;
