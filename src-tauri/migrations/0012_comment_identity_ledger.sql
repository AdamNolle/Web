-- Source-owned first-seen provider comment identity. This local mapping deliberately survives
-- comment/post reconciliation and retention so a provider ID cannot later move between posts.
-- It is never copied to receipts or audit events, and source deletion erases it by cascade.
CREATE TABLE comment_identity_ledger (
  source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  remote_id TEXT NOT NULL,
  post_remote_id TEXT NOT NULL,
  first_seen_generation INTEGER NOT NULL CHECK (first_seen_generation > 0),
  PRIMARY KEY(source_id, remote_id)
);

INSERT INTO comment_identity_ledger(source_id, remote_id, post_remote_id, first_seen_generation)
SELECT c.source_id, c.remote_id, p.remote_id, s.generation
FROM comments c
JOIN posts p ON p.id=c.post_id
JOIN sources s ON s.id=c.source_id
ORDER BY c.source_id, c.remote_id;
