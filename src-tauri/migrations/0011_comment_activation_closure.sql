-- Migration 10 could not reconstruct comment evidence hashes for pre-existing social rows.
-- Fail closed: remove legacy summaries that were not bound to an exact retained comment snapshot,
-- and require a provider refresh before those rows can become current again.
DELETE FROM summaries
WHERE post_id IN (
  SELECT post_id FROM post_comment_state WHERE status != 'unavailable'
);

UPDATE post_comment_state
SET status='partial',
    truncated=1,
    evidence_hash='migration-unverified',
    summary_input_hash='migration-unverified:' || post_id
WHERE status != 'unavailable';

UPDATE source_sync_metadata
SET comments_status='partial',
    comments_truncated=1,
    page_finality='partial',
    safe_detail='Comment evidence requires refresh after migration.'
WHERE source_id IN (
  SELECT DISTINCT p.source_id
  FROM posts p
  JOIN post_comment_state pcs ON pcs.post_id=p.id
  WHERE pcs.evidence_hash='migration-unverified'
);
