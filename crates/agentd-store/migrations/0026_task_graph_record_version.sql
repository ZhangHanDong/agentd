-- M3 Plan C: optimistic concurrency for the live task-graph row. The whole
-- graph is one `raw_json` blob, so two node results committed at the same
-- moment previously blind-overwrote each other. Every write now carries
-- `WHERE record_version = ?` with a rows_affected guard; the advance path
-- cannot run inside one BEGIN IMMEDIATE because it calls pool-based scheduler
-- and message repos, so the version predicate is what serializes it.
ALTER TABLE agent_chat_task_graphs ADD COLUMN record_version INTEGER NOT NULL DEFAULT 1;

UPDATE schema_meta SET value = '26' WHERE key = 'version';
