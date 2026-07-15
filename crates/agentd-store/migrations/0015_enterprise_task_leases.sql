-- P270 / durable control-plane task leases and task-scoped fencing tokens.
-- Additive only: compatibility scheduler tickets/reservations remain unchanged
-- and are never promoted into canonical lease identity.

CREATE TABLE execution_task_leases (
    id                    TEXT PRIMARY KEY
                          CHECK (
                              length(id) = 29
                              AND substr(id, 1, 3) = 'ls_'
                              AND substr(id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                          ),
    execution_task_id     TEXT NOT NULL REFERENCES task_runs(id) ON DELETE RESTRICT,
    worker_incarnation_id TEXT NOT NULL REFERENCES worker_incarnations(id) ON DELETE RESTRICT,
    fencing_token         INTEGER NOT NULL CHECK (fencing_token > 0),
    status                TEXT NOT NULL CHECK (
                              status IN ('active', 'released', 'expired', 'cancelled', 'superseded')
                          ),
    acquired_at           INTEGER NOT NULL,
    expires_at            INTEGER NOT NULL CHECK (expires_at > acquired_at),
    renewed_at            INTEGER CHECK (
                              renewed_at IS NULL
                              OR (renewed_at >= acquired_at AND renewed_at < expires_at)
                          ),
    terminal_at           INTEGER,
    terminal_reason       TEXT,
    record_version        INTEGER NOT NULL DEFAULT 1 CHECK (record_version >= 1),
    CHECK (
        (status = 'active' AND terminal_at IS NULL AND terminal_reason IS NULL)
        OR (
            status IN ('released', 'expired', 'cancelled', 'superseded')
            AND terminal_at IS NOT NULL
            AND length(trim(terminal_reason)) > 0
        )
    )
);

CREATE UNIQUE INDEX idx_execution_task_leases_task_token
    ON execution_task_leases(execution_task_id, fencing_token);

CREATE UNIQUE INDEX idx_execution_task_leases_one_active
    ON execution_task_leases(execution_task_id)
    WHERE status = 'active';

CREATE INDEX idx_execution_task_leases_worker_status
    ON execution_task_leases(worker_incarnation_id, status, expires_at, id);

CREATE INDEX idx_execution_task_leases_due
    ON execution_task_leases(status, expires_at, execution_task_id)
    WHERE status = 'active';

CREATE TABLE execution_task_lease_heads (
    execution_task_id  TEXT PRIMARY KEY REFERENCES task_runs(id) ON DELETE RESTRICT,
    last_fencing_token INTEGER NOT NULL CHECK (last_fencing_token > 0),
    current_lease_id   TEXT REFERENCES execution_task_leases(id) ON DELETE RESTRICT,
    updated_at         INTEGER NOT NULL
);

CREATE TRIGGER trg_execution_task_leases_identity_immutable
BEFORE UPDATE OF id, execution_task_id, worker_incarnation_id, fencing_token, acquired_at
ON execution_task_leases
BEGIN
    SELECT RAISE(ABORT, 'execution task lease identity is immutable');
END;

CREATE TRIGGER trg_execution_task_leases_terminal_immutable
BEFORE UPDATE ON execution_task_leases
WHEN OLD.status <> 'active'
BEGIN
    SELECT RAISE(ABORT, 'terminal execution task lease is immutable');
END;

CREATE TRIGGER trg_execution_task_leases_no_delete
BEFORE DELETE ON execution_task_leases
BEGIN
    SELECT RAISE(ABORT, 'execution task lease history is immutable');
END;

CREATE TRIGGER trg_execution_task_lease_heads_identity_immutable
BEFORE UPDATE OF execution_task_id ON execution_task_lease_heads
BEGIN
    SELECT RAISE(ABORT, 'execution task lease head identity is immutable');
END;

CREATE TRIGGER trg_execution_task_lease_heads_token_monotonic
BEFORE UPDATE OF last_fencing_token ON execution_task_lease_heads
WHEN NEW.last_fencing_token <= OLD.last_fencing_token
BEGIN
    SELECT RAISE(ABORT, 'execution task fencing token must increase');
END;

CREATE TRIGGER trg_execution_task_lease_heads_current_valid_insert
BEFORE INSERT ON execution_task_lease_heads
WHEN NEW.current_lease_id IS NOT NULL
     AND NOT EXISTS (
         SELECT 1
         FROM execution_task_leases AS lease
         WHERE lease.id = NEW.current_lease_id
           AND lease.execution_task_id = NEW.execution_task_id
           AND lease.fencing_token = NEW.last_fencing_token
           AND lease.status = 'active'
     )
BEGIN
    SELECT RAISE(ABORT, 'execution task lease head current pointer is invalid');
END;

CREATE TRIGGER trg_execution_task_lease_heads_current_valid
BEFORE UPDATE OF current_lease_id, last_fencing_token ON execution_task_lease_heads
WHEN NEW.current_lease_id IS NOT NULL
     AND NOT EXISTS (
         SELECT 1
         FROM execution_task_leases AS lease
         WHERE lease.id = NEW.current_lease_id
           AND lease.execution_task_id = NEW.execution_task_id
           AND lease.fencing_token = NEW.last_fencing_token
           AND lease.status = 'active'
     )
BEGIN
    SELECT RAISE(ABORT, 'execution task lease head current pointer is invalid');
END;

CREATE TRIGGER trg_execution_task_lease_heads_no_delete
BEFORE DELETE ON execution_task_lease_heads
BEGIN
    SELECT RAISE(ABORT, 'execution task lease head history is immutable');
END;

UPDATE schema_meta SET value = '15' WHERE key = 'version';
