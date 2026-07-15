-- P235 / Phase G: backend-facing remote relay compatibility baseline.

CREATE TABLE relay_servers (
    id TEXT PRIMARY KEY,
    instance_id TEXT,
    boot_ts INTEGER,
    agents_json TEXT NOT NULL,
    sessions_json TEXT NOT NULL,
    agent_count INTEGER NOT NULL,
    online INTEGER NOT NULL,
    maintenance INTEGER NOT NULL DEFAULT 0,
    last_seen_at INTEGER NOT NULL,
    heartbeat_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE delivery_events (
    id TEXT PRIMARY KEY,
    seq INTEGER NOT NULL UNIQUE,
    type TEXT NOT NULL,
    message_id TEXT,
    queue_entry_id TEXT,
    agent TEXT,
    target TEXT,
    reason TEXT,
    source TEXT,
    context_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX idx_delivery_events_agent_seq
ON delivery_events(agent, seq);

CREATE TABLE relay_stream_events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    event TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX idx_relay_stream_events_seq
ON relay_stream_events(seq);

UPDATE schema_meta SET value = '11' WHERE key = 'version';
