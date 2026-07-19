CREATE TABLE cutover_project_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL,
    phase TEXT NOT NULL,
    authority_revision TEXT NOT NULL,
    matrix_cursor INTEGER NOT NULL,
    lease_epoch INTEGER NOT NULL,
    recorded_at INTEGER NOT NULL
);

CREATE INDEX idx_cutover_project_history_project
    ON cutover_project_history(project_id, id DESC);
UPDATE schema_meta SET value = '20' WHERE key = 'version';
