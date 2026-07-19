CREATE TABLE cutover_projects (
    project_id TEXT PRIMARY KEY NOT NULL,
    phase TEXT NOT NULL CHECK (phase IN ('observe', 'shadow', 'canary', 'cutover', 'drain', 'retired', 'rollback')),
    authority_revision TEXT NOT NULL,
    matrix_cursor INTEGER NOT NULL DEFAULT 0,
    lease_epoch INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_cutover_projects_phase ON cutover_projects(phase);
UPDATE schema_meta SET value = '19' WHERE key = 'version';
