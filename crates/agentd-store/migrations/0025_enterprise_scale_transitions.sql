-- AD-E7 corrective migration: preserve key-rotation and replica-retry history.

CREATE TABLE enterprise_tenant_key_transitions (
    sequence          INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_key_id     TEXT NOT NULL REFERENCES enterprise_tenant_keys(tenant_key_id) ON DELETE RESTRICT,
    previous_status   TEXT NOT NULL CHECK (previous_status IN ('active', 'retiring', 'retired')),
    target_status     TEXT NOT NULL CHECK (target_status IN ('active', 'retiring', 'retired')),
    transition_sha256 TEXT NOT NULL UNIQUE CHECK (length(transition_sha256) = 64 AND transition_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    transitioned_at   INTEGER NOT NULL,
    UNIQUE (tenant_key_id, target_status)
);

CREATE INDEX idx_enterprise_tenant_key_transitions_key
    ON enterprise_tenant_key_transitions(tenant_key_id, sequence);

CREATE TABLE enterprise_artifact_replica_transitions (
    sequence               INTEGER PRIMARY KEY AUTOINCREMENT,
    replication_id         TEXT NOT NULL,
    region                 TEXT NOT NULL,
    previous_status        TEXT,
    target_status          TEXT NOT NULL CHECK (target_status IN ('pending', 'available', 'failed')),
    acknowledgement_sha256 TEXT NOT NULL UNIQUE CHECK (length(acknowledgement_sha256) = 64 AND acknowledgement_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    acknowledged_at        INTEGER NOT NULL,
    FOREIGN KEY (replication_id, region)
        REFERENCES enterprise_artifact_replica_acknowledgements(replication_id, region)
        ON DELETE RESTRICT
);

CREATE INDEX idx_enterprise_artifact_replica_transitions_replica
    ON enterprise_artifact_replica_transitions(replication_id, region, sequence);

CREATE TRIGGER trg_enterprise_tenant_key_transitions_no_update
BEFORE UPDATE ON enterprise_tenant_key_transitions
BEGIN SELECT RAISE(ABORT, 'enterprise tenant key transition history is immutable'); END;

CREATE TRIGGER trg_enterprise_tenant_key_transitions_no_delete
BEFORE DELETE ON enterprise_tenant_key_transitions
BEGIN SELECT RAISE(ABORT, 'enterprise tenant key transition history is immutable'); END;

CREATE TRIGGER trg_enterprise_artifact_replica_transitions_no_update
BEFORE UPDATE ON enterprise_artifact_replica_transitions
BEGIN SELECT RAISE(ABORT, 'enterprise artifact replica transition history is immutable'); END;

CREATE TRIGGER trg_enterprise_artifact_replica_transitions_no_delete
BEFORE DELETE ON enterprise_artifact_replica_transitions
BEGIN SELECT RAISE(ABORT, 'enterprise artifact replica transition history is immutable'); END;

UPDATE schema_meta SET value = '25' WHERE key = 'version';
