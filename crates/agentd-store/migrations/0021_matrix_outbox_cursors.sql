CREATE TABLE IF NOT EXISTS matrix_outbox_cursors (
    bridge_id TEXT PRIMARY KEY,
    last_seq INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
UPDATE schema_meta SET value = '21' WHERE key = 'version';
