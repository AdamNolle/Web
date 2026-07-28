-- Source identity is versioned so a fetch that began before deletion cannot recreate data.
ALTER TABLE sources ADD COLUMN generation INTEGER NOT NULL DEFAULT 1;
ALTER TABLE sources ADD COLUMN validator_url TEXT;
-- v4 validators may have been issued by an unrecorded redirect target. They cannot be
-- safely replayed to the requested URL, so the first v5 fetch is deliberately unconditional.
UPDATE sources SET etag=NULL, last_modified=NULL;
CREATE TABLE source_tombstones (
  source_id TEXT PRIMARY KEY,
  generation INTEGER NOT NULL,
  deleted_at INTEGER NOT NULL
);

-- A runner owner and monotonically increasing fencing token must both match for renewal/finish.
ALTER TABLE runner_state ADD COLUMN lease_token INTEGER NOT NULL DEFAULT 0;
ALTER TABLE jobs ADD COLUMN lease_owner TEXT;
ALTER TABLE jobs ADD COLUMN lease_token INTEGER;
