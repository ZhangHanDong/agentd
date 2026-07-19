-- AD-E1 opaque capability metadata. Secret/token bytes are never persisted.
CREATE TABLE execution_capabilities (
    id TEXT PRIMARY KEY,
    token_digest TEXT NOT NULL UNIQUE CHECK (length(token_digest) = 64),
    worker_incarnation_id TEXT NOT NULL REFERENCES worker_incarnations(id) ON DELETE RESTRICT,
    lease_id TEXT NOT NULL REFERENCES execution_task_leases(id) ON DELETE RESTRICT,
    fencing_token INTEGER NOT NULL CHECK (fencing_token > 0),
    action TEXT NOT NULL CHECK (length(trim(action)) > 0),
    resource_json TEXT NOT NULL CHECK (json_valid(resource_json)),
    scope_json TEXT NOT NULL CHECK (json_valid(scope_json)),
    issued_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL CHECK (expires_at > issued_at),
    revoked_at INTEGER,
    revocation_epoch INTEGER NOT NULL CHECK (revocation_epoch >= 0)
);
CREATE INDEX idx_execution_capabilities_worker_lease
    ON execution_capabilities(worker_incarnation_id, lease_id, fencing_token);
CREATE INDEX idx_execution_capabilities_expiry
    ON execution_capabilities(expires_at, revoked_at);
UPDATE schema_meta SET value = '17' WHERE key = 'version';
