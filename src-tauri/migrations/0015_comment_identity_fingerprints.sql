-- The identity ledger must outlive ordinary comment retention so a provider cannot later reuse a
-- comment identifier for another post. It must not, however, retain raw provider identifiers once
-- the underlying comment has expired. Rust migrates existing rows to source-scoped fingerprints
-- inside this migration transaction, then removes the legacy table.
ALTER TABLE comment_identity_ledger RENAME TO comment_identity_ledger_v12_raw;

CREATE TABLE comment_identity_ledger (
  source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  comment_fingerprint TEXT NOT NULL CHECK (length(comment_fingerprint) = 64),
  post_fingerprint TEXT NOT NULL CHECK (length(post_fingerprint) = 64),
  first_seen_generation INTEGER NOT NULL CHECK (first_seen_generation > 0),
  PRIMARY KEY(source_id, comment_fingerprint)
);
