-- Provider-neutral, non-secret sync health and comment completeness. OAuth/session secrets remain
-- exclusively in the OS vault and no social connector is enabled by this migration.
CREATE TABLE source_sync_metadata (
  source_id TEXT PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,
  health_state TEXT NOT NULL DEFAULT 'healthy'
    CHECK (health_state IN ('healthy', 'rate_limited', 'auth_required', 'transient', 'paused')),
  safe_detail TEXT NOT NULL DEFAULT '',
  comments_status TEXT NOT NULL DEFAULT 'unavailable'
    CHECK (comments_status IN ('unavailable', 'complete', 'partial')),
  comments_truncated INTEGER NOT NULL DEFAULT 0 CHECK (comments_truncated IN (0, 1)),
  retry_at INTEGER,
  updated_at INTEGER NOT NULL
);

ALTER TABLE comments ADD COLUMN depth INTEGER NOT NULL DEFAULT 0 CHECK (depth BETWEEN 0 AND 8);
ALTER TABLE comments ADD COLUMN position INTEGER NOT NULL DEFAULT 0 CHECK (position >= 0);
CREATE INDEX comments_post_order_idx ON comments(post_id, position, published_at, remote_id);
CREATE TABLE post_comment_state (
  post_id TEXT PRIMARY KEY REFERENCES posts(id) ON DELETE CASCADE,
  status TEXT NOT NULL CHECK (status IN ('unavailable', 'complete', 'partial')),
  truncated INTEGER NOT NULL DEFAULT 0 CHECK (truncated IN (0, 1)),
  fetched_at INTEGER NOT NULL
);

INSERT INTO post_comment_state(post_id, status, truncated, fetched_at)
SELECT id, 'unavailable', 0, fetched_at FROM posts;

INSERT INTO source_sync_metadata(source_id, health_state, safe_detail, comments_status, comments_truncated, retry_at, updated_at)
SELECT id,
       CASE status WHEN 'paused' THEN 'paused' WHEN 'attention' THEN 'transient' ELSE 'healthy' END,
       CASE connector_kind WHEN 'rss' THEN 'RSS comments are unavailable.' ELSE '' END,
       'unavailable',
       0,
       next_poll_at,
       updated_at
FROM sources;
