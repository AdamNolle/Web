PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_migrations (
  version INTEGER PRIMARY KEY,
  applied_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sources (
  id TEXT PRIMARY KEY,
  connector_kind TEXT NOT NULL CHECK (connector_kind IN ('demo', 'rss', 'bluesky', 'mastodon')),
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
  UNIQUE(connector_kind, account_label)
);

CREATE TABLE IF NOT EXISTS actors (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  remote_id TEXT NOT NULL,
  display_name TEXT NOT NULL,
  profile_url TEXT,
  UNIQUE(source_id, remote_id)
);

CREATE TABLE IF NOT EXISTS posts (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  remote_id TEXT NOT NULL,
  canonical_url TEXT NOT NULL,
  actor_id TEXT REFERENCES actors(id) ON DELETE SET NULL,
  title TEXT NOT NULL,
  body_text TEXT NOT NULL,
  published_at INTEGER NOT NULL,
  fetched_at INTEGER NOT NULL,
  content_hash TEXT NOT NULL,
  deleted_at INTEGER,
  UNIQUE(source_id, remote_id)
);
CREATE INDEX IF NOT EXISTS posts_published_idx ON posts(published_at DESC);

CREATE TABLE IF NOT EXISTS comments (
  id TEXT PRIMARY KEY,
  post_id TEXT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
  source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  remote_id TEXT NOT NULL,
  parent_remote_id TEXT,
  actor_id TEXT REFERENCES actors(id) ON DELETE SET NULL,
  body_text TEXT NOT NULL,
  published_at INTEGER NOT NULL,
  fetched_at INTEGER NOT NULL,
  content_hash TEXT NOT NULL,
  deleted_at INTEGER,
  UNIQUE(source_id, remote_id)
);

CREATE TABLE IF NOT EXISTS summaries (
  id TEXT PRIMARY KEY,
  post_id TEXT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
  summary_text TEXT NOT NULL,
  comment_overview TEXT NOT NULL,
  provenance_json TEXT NOT NULL,
  provider TEXT NOT NULL,
  model_id TEXT,
  prompt_version TEXT NOT NULL,
  input_hash TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  UNIQUE(post_id, input_hash, prompt_version)
);

CREATE TABLE IF NOT EXISTS digests (
  id TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  period_start INTEGER NOT NULL,
  period_end INTEGER NOT NULL,
  generated_at INTEGER NOT NULL,
  next_edition_at INTEGER NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('building', 'ready', 'failed')),
  overview TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS digest_items (
  digest_id TEXT NOT NULL REFERENCES digests(id) ON DELETE CASCADE,
  post_id TEXT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
  rank INTEGER NOT NULL,
  reason TEXT NOT NULL,
  topic TEXT NOT NULL,
  importance REAL NOT NULL CHECK (importance >= 0 AND importance <= 1),
  PRIMARY KEY(digest_id, post_id)
);

CREATE TABLE IF NOT EXISTS trend_clusters (
  id TEXT PRIMARY KEY,
  digest_id TEXT NOT NULL REFERENCES digests(id) ON DELETE CASCADE,
  label TEXT NOT NULL,
  summary TEXT NOT NULL,
  confidence TEXT NOT NULL CHECK (confidence IN ('emerging', 'supported')),
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS trend_members (
  cluster_id TEXT NOT NULL REFERENCES trend_clusters(id) ON DELETE CASCADE,
  post_id TEXT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
  PRIMARY KEY(cluster_id, post_id)
);

CREATE TABLE IF NOT EXISTS feedback (
  id TEXT PRIMARY KEY,
  request_id TEXT NOT NULL UNIQUE,
  post_id TEXT REFERENCES posts(id) ON DELETE CASCADE,
  source_id TEXT REFERENCES sources(id) ON DELETE CASCADE,
  signal TEXT NOT NULL CHECK (signal IN ('more_like_this', 'less_like_this', 'not_relevant', 'mute_source')),
  created_at INTEGER NOT NULL,
  retracted_at INTEGER
);

CREATE TABLE IF NOT EXISTS jobs (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  dedupe_key TEXT NOT NULL UNIQUE,
  state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'complete', 'failed', 'scheduled')),
  attempts INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL DEFAULT 3,
  run_after INTEGER NOT NULL,
  lease_expires_at INTEGER,
  last_error_code TEXT,
  message TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS jobs_due_idx ON jobs(state, run_after);

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS audit_events (
  id TEXT PRIMARY KEY,
  occurred_at INTEGER NOT NULL,
  category TEXT NOT NULL,
  action TEXT NOT NULL,
  subject_id TEXT,
  outcome TEXT NOT NULL,
  detail_json TEXT NOT NULL DEFAULT '{}'
);
