-- AD-E6 native agent authority and workflow-runtime bindings. Legacy runtime
-- address columns remain readable only for offline import compatibility.

ALTER TABLE agents ADD COLUMN native_runtime_ref TEXT;

CREATE TABLE native_runtime_authority (
    singleton       INTEGER PRIMARY KEY CHECK (singleton = 1),
    worker_id       TEXT NOT NULL UNIQUE REFERENCES workers(id) ON DELETE RESTRICT,
    created_at      INTEGER NOT NULL
);

CREATE TABLE native_agent_runtime_bindings (
    runtime_session_id     TEXT PRIMARY KEY REFERENCES runtime_sessions(id) ON DELETE RESTRICT,
    runtime_attempt_id     TEXT NOT NULL UNIQUE REFERENCES runtime_attempts(id) ON DELETE RESTRICT,
    agent_id               TEXT NOT NULL CHECK (length(trim(agent_id)) > 0),
    execution_task_id      TEXT NOT NULL REFERENCES task_runs(id) ON DELETE RESTRICT,
    synthetic_task         INTEGER NOT NULL CHECK (synthetic_task IN (0, 1)),
    capability_json        TEXT NOT NULL CHECK (json_valid(capability_json)),
    worktree               TEXT NOT NULL CHECK (length(trim(worktree)) > 0),
    status                 TEXT NOT NULL CHECK (status IN ('active', 'finished')),
    created_at             INTEGER NOT NULL,
    finished_at            INTEGER,
    CHECK (
        (status = 'active' AND finished_at IS NULL)
        OR (status = 'finished' AND finished_at IS NOT NULL)
    )
);

CREATE INDEX idx_native_agent_bindings_agent_active
    ON native_agent_runtime_bindings(agent_id, status, created_at DESC);

CREATE TRIGGER trg_native_agent_binding_identity_immutable
BEFORE UPDATE OF runtime_session_id, runtime_attempt_id, agent_id, execution_task_id,
                 synthetic_task, capability_json, worktree, created_at
ON native_agent_runtime_bindings
BEGIN SELECT RAISE(ABORT, 'native agent runtime binding identity is immutable'); END;

UPDATE schema_meta SET value = '23' WHERE key = 'version';
