-- P236 / Phase G: backend-facing Matrix bridge compatibility contract.

CREATE TABLE matrix_bridge_rooms (
    room_id TEXT PRIMARY KEY,
    group_name TEXT,
    agent_name TEXT,
    trusted INTEGER NOT NULL,
    trust_reason TEXT NOT NULL,
    inviter_mxid TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_matrix_bridge_rooms_group
ON matrix_bridge_rooms(group_name);

CREATE INDEX idx_matrix_bridge_rooms_agent
ON matrix_bridge_rooms(agent_name);

CREATE TABLE matrix_bridge_events (
    event_id TEXT PRIMARY KEY,
    room_id TEXT NOT NULL,
    sender_mxid TEXT NOT NULL,
    message_id TEXT,
    route TEXT NOT NULL,
    ignored INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX idx_matrix_bridge_events_room_created
ON matrix_bridge_events(room_id, created_at);

UPDATE schema_meta SET value = '12' WHERE key = 'version';
