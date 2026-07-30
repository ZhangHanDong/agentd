-- M4 Plan A: the canonical Matrix command record.
--
-- `command_id` is derived deterministically from (room_id, event_id), so a
-- replayed Matrix event recomputes the identical id without a read. The
-- partial unique index is the room/project dedup constraint: at most one OPEN
-- command per (room, project, payload). It is partial on purpose — a full
-- unique index would make an ordinary chat room reject the second "ok" anyone
-- types, because plain chat messages are recorded here as `settled` and must
-- never occupy the slot.
--
-- `project_key` is NOT NULL DEFAULT '' rather than a nullable `project_id`:
-- SQLite treats NULLs as distinct inside a unique index, so an unbound room
-- would otherwise escape the constraint entirely.
CREATE TABLE matrix_commands (
    command_id     TEXT PRIMARY KEY CHECK (length(trim(command_id)) > 0),
    event_id       TEXT NOT NULL UNIQUE CHECK (length(trim(event_id)) > 0),
    room_id        TEXT NOT NULL CHECK (length(trim(room_id)) > 0),
    project_key    TEXT NOT NULL DEFAULT '',
    dedup_key      TEXT NOT NULL CHECK (length(trim(dedup_key)) > 0),
    sender_mxid    TEXT NOT NULL CHECK (length(trim(sender_mxid)) > 0),
    route          TEXT NOT NULL CHECK (length(trim(route)) > 0),
    status         TEXT NOT NULL CHECK (status IN ('accepted', 'running', 'settled', 'rejected')),
    message_id     TEXT,
    run_id         TEXT,
    -- The run this command asks agentd to create, as JSON. NULL for plain
    -- chat. Task 6's sweep reads it; nothing else does.
    run_request_json TEXT,
    record_version INTEGER NOT NULL DEFAULT 1 CHECK (record_version > 0),
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);

CREATE UNIQUE INDEX idx_matrix_commands_open_room_project
    ON matrix_commands(room_id, project_key, dedup_key)
    WHERE status IN ('accepted', 'running');

CREATE INDEX idx_matrix_commands_room_created
    ON matrix_commands(room_id, created_at);

CREATE INDEX idx_matrix_commands_status_created
    ON matrix_commands(status, created_at);

UPDATE schema_meta SET value = '29' WHERE key = 'version';
