-- M2 Plan A: durable scheduler authority. Queue selection, lease grant, and
-- outbox append commit in one transaction (design doc §M2; AD-E2 matrix).

CREATE TABLE execution_task_queue (
    id                 TEXT PRIMARY KEY,
    execution_task_id  TEXT NOT NULL REFERENCES task_runs(id),
    status             TEXT NOT NULL DEFAULT 'queued'
                       CHECK (status IN ('queued','leased','completed','dead_letter','cancelled')),
    attempts           INTEGER NOT NULL DEFAULT 0,
    max_attempts       INTEGER NOT NULL DEFAULT 3,
    available_at       INTEGER NOT NULL,
    current_lease_id   TEXT,
    last_reason        TEXT,
    request_id         TEXT NOT NULL,
    enqueued_at        INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL
);
-- One open queue row per task; terminal rows do not block a re-enqueue.
CREATE UNIQUE INDEX idx_queue_open_task ON execution_task_queue(execution_task_id)
    WHERE status IN ('queued','leased');
CREATE UNIQUE INDEX idx_queue_request ON execution_task_queue(request_id);
CREATE INDEX idx_queue_eligible ON execution_task_queue(status, available_at, enqueued_at);

CREATE TABLE scheduler_acquisitions (
    request_id             TEXT PRIMARY KEY,
    queue_id               TEXT NOT NULL REFERENCES execution_task_queue(id),
    lease_id               TEXT NOT NULL,
    worker_incarnation_id  TEXT NOT NULL,
    acquired_at            INTEGER NOT NULL
);

CREATE TABLE execution_scheduler_outbox (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id   TEXT NOT NULL UNIQUE,
    kind       TEXT NOT NULL,
    queue_id   TEXT NOT NULL,
    task_id    TEXT NOT NULL,
    lease_id   TEXT,
    payload    TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    drained_at INTEGER
);
CREATE INDEX idx_scheduler_outbox_pending ON execution_scheduler_outbox(drained_at, seq)
    WHERE drained_at IS NULL;

UPDATE schema_meta SET value = '23' WHERE key = 'version';
