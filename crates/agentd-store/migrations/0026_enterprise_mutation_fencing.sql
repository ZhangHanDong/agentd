-- AD-E7 corrective migration: every enterprise mutation records and validates
-- the current leadership fence inside the same SQLite write transaction.

CREATE TABLE enterprise_mutation_fences (
    sequence          INTEGER PRIMARY KEY AUTOINCREMENT,
    operation         TEXT NOT NULL CHECK (length(trim(operation)) > 0),
    resource_key      TEXT NOT NULL CHECK (length(trim(resource_key)) > 0),
    mutation_sha256   TEXT NOT NULL CHECK (length(mutation_sha256) = 64 AND mutation_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    instance_id       TEXT NOT NULL REFERENCES enterprise_control_plane_members(instance_id) ON DELETE RESTRICT,
    term              INTEGER NOT NULL CHECK (term > 0),
    fencing_token     INTEGER NOT NULL CHECK (fencing_token > 0),
    observed_at       INTEGER NOT NULL
);

CREATE INDEX idx_enterprise_mutation_fences_resource
    ON enterprise_mutation_fences(operation, resource_key, sequence DESC);

CREATE INDEX idx_enterprise_mutation_fences_leader
    ON enterprise_mutation_fences(instance_id, term, fencing_token, sequence DESC);

CREATE TRIGGER trg_enterprise_mutation_fences_no_update
BEFORE UPDATE ON enterprise_mutation_fences
BEGIN SELECT RAISE(ABORT, 'enterprise mutation fence history is immutable'); END;

CREATE TRIGGER trg_enterprise_mutation_fences_no_delete
BEFORE DELETE ON enterprise_mutation_fences
BEGIN SELECT RAISE(ABORT, 'enterprise mutation fence history is immutable'); END;

UPDATE schema_meta SET value = '26' WHERE key = 'version';
