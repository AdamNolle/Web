-- Daily-use, local-only reading features. Search is contentful so source deletion
-- and retention remove private data without relying on SQLite rowids.
CREATE TABLE IF NOT EXISTS saved_posts (
  post_id TEXT PRIMARY KEY REFERENCES posts(id) ON DELETE CASCADE,
  saved_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS saved_posts_saved_at_idx ON saved_posts(saved_at DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS post_search USING fts5(
  post_id UNINDEXED, title, body_text, author, source,
  tokenize='unicode61 remove_diacritics 2'
);

INSERT INTO post_search(post_id, title, body_text, author, source)
SELECT p.id, p.title, p.body_text, COALESCE(a.display_name, s.account_label), s.account_label
FROM posts p JOIN sources s ON s.id=p.source_id LEFT JOIN actors a ON a.id=p.actor_id
WHERE p.deleted_at IS NULL AND NOT EXISTS (SELECT 1 FROM post_search ps WHERE ps.post_id=p.id);

CREATE TRIGGER IF NOT EXISTS post_search_posts_insert
AFTER INSERT ON posts WHEN NEW.deleted_at IS NULL
BEGIN
  INSERT INTO post_search(post_id, title, body_text, author, source)
  SELECT NEW.id, NEW.title, NEW.body_text, COALESCE(a.display_name, s.account_label), s.account_label
  FROM sources s LEFT JOIN actors a ON a.id=NEW.actor_id WHERE s.id=NEW.source_id;
END;

CREATE TRIGGER IF NOT EXISTS post_search_posts_update
AFTER UPDATE OF title, body_text, actor_id, source_id, deleted_at ON posts
BEGIN
  DELETE FROM post_search WHERE post_id=OLD.id;
  INSERT INTO post_search(post_id, title, body_text, author, source)
  SELECT NEW.id, NEW.title, NEW.body_text, COALESCE(a.display_name, s.account_label), s.account_label
  FROM sources s LEFT JOIN actors a ON a.id=NEW.actor_id
  WHERE s.id=NEW.source_id AND NEW.deleted_at IS NULL;
END;

CREATE TRIGGER IF NOT EXISTS post_search_posts_delete
AFTER DELETE ON posts
BEGIN
  DELETE FROM post_search WHERE post_id=OLD.id;
END;

CREATE TRIGGER IF NOT EXISTS post_search_source_rename
AFTER UPDATE OF account_label ON sources
BEGIN
  UPDATE post_search SET source=NEW.account_label
  WHERE post_id IN (SELECT id FROM posts WHERE source_id=NEW.id);
END;
