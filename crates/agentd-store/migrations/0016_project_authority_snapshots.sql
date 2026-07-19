-- Immutable local projection of external project-authority snapshots.
CREATE TABLE project_authority_snapshots (
    snapshot_ref TEXT PRIMARY KEY,
    authority_key TEXT NOT NULL CHECK (length(trim(authority_key)) > 0),
    project_ref TEXT NOT NULL CHECK (length(trim(project_ref)) > 0),
    authority_revision INTEGER NOT NULL CHECK (authority_revision > 0),
    issued_at INTEGER NOT NULL,
    valid_until INTEGER NOT NULL CHECK (valid_until > issued_at),
    content_sha256 TEXT NOT NULL CHECK (length(content_sha256) = 64),
    snapshot_json TEXT NOT NULL CHECK (json_valid(snapshot_json)),
    recorded_at INTEGER NOT NULL
);

CREATE INDEX idx_project_authority_snapshots_project
    ON project_authority_snapshots(authority_key, project_ref, authority_revision);

CREATE INDEX idx_project_authority_snapshots_expiry
    ON project_authority_snapshots(valid_until);

UPDATE schema_meta SET value = '16' WHERE key = 'version';
