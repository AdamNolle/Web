-- Monotonic local-only invalidation marker. Renderer may defer harmless edition additions/reordering,
-- but must immediately purge content after retention, deletion, mute, or not-relevant changes.
CREATE TABLE app_state (
  singleton INTEGER PRIMARY KEY CHECK(singleton=1),
  privacy_epoch INTEGER NOT NULL DEFAULT 0
);
INSERT INTO app_state(singleton, privacy_epoch) VALUES(1, 0);
