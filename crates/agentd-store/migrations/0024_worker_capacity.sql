-- M2 Plan B: declared concurrent-task capacity per worker incarnation. The
-- durable scheduler's acquire never grants beyond an incarnation's open
-- active leases (design doc §M2 item 2). Existing rows default to 1.
ALTER TABLE worker_incarnations ADD COLUMN capacity INTEGER NOT NULL DEFAULT 1;

UPDATE schema_meta SET value = '24' WHERE key = 'version';
