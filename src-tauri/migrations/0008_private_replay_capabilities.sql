-- A completed source deletion keeps its replay identity outside request_receipts.
-- The random capability is not derived from the enumerable source identifier; the receipt stores
-- only the capability, while the durable source tombstone owns the source-to-capability mapping.
ALTER TABLE source_tombstones ADD COLUMN replay_capability TEXT;
CREATE UNIQUE INDEX source_tombstones_replay_capability_idx
ON source_tombstones(replay_capability)
WHERE replay_capability IS NOT NULL;

-- Older command-only completion tombstones cannot prove a payload after logical erasure. Keep them
-- fail-closed as unknown rather than claiming that an arbitrary replay completed.
UPDATE request_receipts
SET payload_hash = 'tombstone-unknown:' || command
WHERE payload_hash = 'tombstone:' || command;
