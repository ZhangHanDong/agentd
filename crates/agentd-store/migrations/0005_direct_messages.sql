-- P217 / Phase D: durable direct-message inbox baseline.
CREATE TABLE direct_messages (
    id           TEXT PRIMARY KEY,
    ts           INTEGER NOT NULL,
    from_agent   TEXT NOT NULL,
    to_agent     TEXT NOT NULL,
    message_type TEXT NOT NULL,
    priority     TEXT NOT NULL,
    summary      TEXT NOT NULL,
    full         TEXT NOT NULL,
    reply_to     TEXT,
    source       TEXT NOT NULL,
    source_room  TEXT,
    sender_mxid  TEXT,
    trust_level  TEXT,
    from_id      TEXT,
    raw_json     TEXT NOT NULL DEFAULT '{}',
    created_at   INTEGER NOT NULL,
    read_at      INTEGER
);

CREATE INDEX idx_direct_messages_to_unread ON direct_messages(to_agent, read_at, ts, id);

UPDATE schema_meta SET value = '5' WHERE key = 'version';
