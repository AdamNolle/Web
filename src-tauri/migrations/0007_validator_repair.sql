-- Iteration-5 databases may already record migration 5 while retaining validators issued by an
-- unrecorded redirect representation. Only unbound validators are unsafe; preserve validators
-- whose effective representation URL is known.
UPDATE sources
SET etag = NULL,
    last_modified = NULL
WHERE validator_url IS NULL
  AND (etag IS NOT NULL OR last_modified IS NOT NULL);
