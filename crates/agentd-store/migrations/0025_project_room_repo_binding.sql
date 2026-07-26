-- M3 Plan A: the project ↔ room ↔ repository binding as a durable,
-- agentd-owned first-class record. `projects.repo_path`/`github_repo`
-- ("locator") and `projects.matrix_room_id` ("transport hint") are classified
-- non-authoritative by the P266 contract, and 0022's
-- `matrix_bridge_rooms.project_id` carries no repository — so neither can host
-- this. No FK to `projects`: this table is the authority and a binding may be
-- declared before the legacy `projects` import alias row exists.
CREATE TABLE project_room_repo_bindings (
    project_id     TEXT PRIMARY KEY CHECK (length(trim(project_id)) > 0),
    room_id        TEXT NOT NULL UNIQUE CHECK (length(trim(room_id)) > 0),
    repository_id  TEXT NOT NULL CHECK (length(trim(repository_id)) > 0),
    repository_url TEXT NOT NULL CHECK (length(trim(repository_url)) > 0),
    default_branch TEXT NOT NULL CHECK (length(trim(default_branch)) > 0),
    record_version INTEGER NOT NULL DEFAULT 1 CHECK (record_version > 0),
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);

CREATE INDEX idx_project_room_repo_bindings_repository
    ON project_room_repo_bindings(repository_id);

UPDATE schema_meta SET value = '25' WHERE key = 'version';
