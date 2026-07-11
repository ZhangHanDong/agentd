-- P228 / Phase E: durable agent-chat pool scheduler baseline.
CREATE TABLE agent_scheduler_reservations (
    id TEXT PRIMARY KEY,
    role TEXT NOT NULL,
    tier TEXT NOT NULL,
    agent TEXT,
    provisioned_name TEXT,
    status TEXT NOT NULL,
    task_json TEXT,
    room TEXT,
    runtime_json TEXT,
    ticket TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    released_at INTEGER
);

CREATE INDEX idx_agent_scheduler_reservations_agent_status
ON agent_scheduler_reservations(agent, status);

CREATE INDEX idx_agent_scheduler_reservations_cell_status
ON agent_scheduler_reservations(role, tier, status);

CREATE TABLE agent_scheduler_queue (
    ticket TEXT PRIMARY KEY,
    role TEXT NOT NULL,
    tier TEXT NOT NULL,
    task_json TEXT,
    room TEXT,
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    drained_at INTEGER,
    reservation_id TEXT
);

CREATE INDEX idx_agent_scheduler_queue_cell_status
ON agent_scheduler_queue(role, tier, status, created_at);

UPDATE schema_meta SET value = '10' WHERE key = 'version';
