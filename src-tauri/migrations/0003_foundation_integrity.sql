ALTER TABLE posts ADD COLUMN canonical_http_url TEXT;
-- The legacy schema had no provenance bit, so never assert that its timestamp was published.
ALTER TABLE posts ADD COLUMN published_time_kind TEXT NOT NULL DEFAULT 'fetched'
  CHECK (published_time_kind IN ('published', 'updated', 'fetched'));
ALTER TABLE trend_clusters ADD COLUMN cluster_method TEXT NOT NULL DEFAULT 'fixture'
  CHECK (cluster_method IN ('fixture', 'lexical', 'embedding'));

-- Rust performs the canonical URL backfill in the same migration transaction. SQL prefix
-- matching is deliberately insufficient for credentials, fragments, malformed hosts, and ports.

-- Desktop production data must never be populated with browser-preview fixtures.
DELETE FROM sources
WHERE id IN ('source-rss-ai', 'source-mastodon', 'source-bluesky', 'source-rss-design');
DELETE FROM digests WHERE id = 'edition-demo';
DELETE FROM jobs WHERE dedupe_key IN ('seed:digest', 'seed:sync');
DELETE FROM app_meta WHERE key = 'demo_seed_v1';
DELETE FROM request_receipts WHERE command = 'delete_source';
