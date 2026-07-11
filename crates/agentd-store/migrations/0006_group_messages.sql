-- P220 / agent-chat parity Phase D: durable group membership, group messages,
-- per-agent group-history reads, and group mention inbox drains.

CREATE TABLE groups (
    name       TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL
);

CREATE TABLE group_members (
    group_name TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (group_name, agent_name),
    FOREIGN KEY (group_name) REFERENCES groups(name) ON DELETE CASCADE
);

CREATE TABLE group_messages (
    id            TEXT PRIMARY KEY,
    ts            INTEGER NOT NULL,
    from_agent    TEXT NOT NULL,
    group_name    TEXT NOT NULL,
    message_type  TEXT NOT NULL,
    priority      TEXT NOT NULL,
    summary       TEXT NOT NULL,
    full          TEXT NOT NULL,
    mentions_json TEXT NOT NULL DEFAULT '[]',
    reply_to      TEXT,
    schema_json   TEXT,
    source        TEXT NOT NULL DEFAULT 'api',
    created_at    INTEGER NOT NULL,
    FOREIGN KEY (group_name) REFERENCES groups(name) ON DELETE CASCADE
);

CREATE TABLE group_message_reads (
    agent_name TEXT NOT NULL,
    group_name TEXT NOT NULL,
    message_id TEXT NOT NULL,
    read_at    INTEGER NOT NULL,
    PRIMARY KEY (agent_name, group_name, message_id),
    FOREIGN KEY (group_name) REFERENCES groups(name) ON DELETE CASCADE,
    FOREIGN KEY (message_id) REFERENCES group_messages(id) ON DELETE CASCADE
);

CREATE TABLE group_mention_reads (
    agent_name TEXT NOT NULL,
    message_id TEXT NOT NULL,
    read_at    INTEGER NOT NULL,
    PRIMARY KEY (agent_name, message_id),
    FOREIGN KEY (message_id) REFERENCES group_messages(id) ON DELETE CASCADE
);

CREATE INDEX idx_group_members_agent ON group_members(agent_name, group_name);
CREATE INDEX idx_group_messages_order ON group_messages(group_name, ts, id);
CREATE INDEX idx_group_message_reads_agent ON group_message_reads(agent_name, group_name);
CREATE INDEX idx_group_mention_reads_agent ON group_mention_reads(agent_name);

UPDATE schema_meta SET value = '6' WHERE key = 'version';
