-- Comment snapshots are identity-bearing summary evidence. Keep the expected summary input on the
-- per-post state so dashboard reads cannot surface a summary derived from stale/removed comments.
ALTER TABLE source_sync_metadata ADD COLUMN page_finality TEXT NOT NULL DEFAULT 'complete'
  CHECK (page_finality IN ('complete', 'partial'));

ALTER TABLE post_comment_state ADD COLUMN evidence_hash TEXT NOT NULL DEFAULT 'unavailable';
ALTER TABLE post_comment_state ADD COLUMN summary_input_hash TEXT NOT NULL DEFAULT '';
UPDATE post_comment_state
SET evidence_hash='unavailable',
    summary_input_hash=(SELECT p.content_hash FROM posts p WHERE p.id=post_comment_state.post_id);

-- Sync jobs distinguish partial provider pages from complete outcomes instead of overloading a
-- successful terminal state. No table references jobs, so rebuilding is safe and preserves rows.
ALTER TABLE jobs RENAME TO jobs_before_comment_finality;
CREATE TABLE jobs (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  dedupe_key TEXT NOT NULL UNIQUE,
  state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'complete', 'partial', 'failed', 'scheduled')),
  attempts INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL DEFAULT 3,
  run_after INTEGER NOT NULL,
  lease_expires_at INTEGER,
  last_error_code TEXT,
  message TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  lease_owner TEXT,
  lease_token INTEGER
);
INSERT INTO jobs(id, kind, dedupe_key, state, attempts, max_attempts, run_after, lease_expires_at,
                 last_error_code, message, created_at, updated_at, lease_owner, lease_token)
SELECT id, kind, dedupe_key, state, attempts, max_attempts, run_after, lease_expires_at,
       last_error_code, message, created_at, updated_at, lease_owner, lease_token
FROM jobs_before_comment_finality;
DROP TABLE jobs_before_comment_finality;
CREATE INDEX jobs_due_idx ON jobs(state, run_after);
