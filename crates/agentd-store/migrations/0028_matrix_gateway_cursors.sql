-- M4 Plan A: the AgentdMatrixGateway-owned durable inbound cursor.
--
-- Before this table there was no inbound cursor anywhere: `SdkMatrixClient`
-- syncs with no token and the bridge's JSON state file holds only the outbox
-- sequence, so a restarted gateway re-delivered whatever the homeserver's
-- initial sync returned. The daemon owns this record; the remote bridge reads
-- and advances it over HTTP and never opens this database.
CREATE TABLE matrix_gateway_cursors (
    gateway_id     TEXT PRIMARY KEY CHECK (length(trim(gateway_id)) > 0),
    sync_token     TEXT,
    last_event_id  TEXT,
    record_version INTEGER NOT NULL DEFAULT 1 CHECK (record_version > 0),
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);

UPDATE schema_meta SET value = '28' WHERE key = 'version';
