-- P267 / enterprise agent, worker, and runtime identity model.
-- Additive only: legacy agents/projects/runs/task_runs remain unchanged and
-- readable while new execution-control records use P265 canonical identities.

CREATE TABLE agent_profiles (
    id             TEXT PRIMARY KEY
                   CHECK (
                       length(id) = 29
                       AND substr(id, 1, 3) = 'ap_'
                       AND substr(id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                   ),
    role           TEXT NOT NULL CHECK (length(trim(role)) > 0),
    capability     TEXT,
    runtime        TEXT NOT NULL CHECK (length(trim(runtime)) > 0),
    model          TEXT,
    prompt_profile TEXT,
    status         TEXT NOT NULL CHECK (status IN ('active', 'disabled', 'retired')),
    record_version INTEGER NOT NULL DEFAULT 1 CHECK (record_version >= 1),
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);

CREATE TABLE legacy_agent_aliases (
    legacy_agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE RESTRICT,
    agent_profile_id TEXT NOT NULL REFERENCES agent_profiles(id) ON DELETE RESTRICT,
    created_at       INTEGER NOT NULL
);

CREATE INDEX idx_legacy_agent_aliases_profile
    ON legacy_agent_aliases(agent_profile_id, legacy_agent_id);

CREATE TABLE workers (
    id             TEXT PRIMARY KEY
                   CHECK (
                       length(id) = 29
                       AND substr(id, 1, 3) = 'wk_'
                       AND substr(id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                   ),
    status         TEXT NOT NULL CHECK (status IN ('online', 'draining', 'offline', 'retired')),
    trust_domain   TEXT NOT NULL CHECK (length(trim(trust_domain)) > 0),
    labels_json    TEXT NOT NULL DEFAULT '{}',
    record_version INTEGER NOT NULL DEFAULT 1 CHECK (record_version >= 1),
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    retired_at     INTEGER
);

CREATE TABLE worker_incarnations (
    id                TEXT PRIMARY KEY
                      CHECK (
                          length(id) = 29
                          AND substr(id, 1, 3) = 'wi_'
                          AND substr(id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                      ),
    worker_id         TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
    daemon_version    TEXT NOT NULL CHECK (length(trim(daemon_version)) > 0),
    host_name         TEXT NOT NULL CHECK (length(trim(host_name)) > 0),
    network_zone      TEXT,
    capabilities_json TEXT NOT NULL DEFAULT '{}',
    is_current        INTEGER NOT NULL CHECK (is_current IN (0, 1)),
    registered_at     INTEGER NOT NULL,
    last_seen_at      INTEGER NOT NULL,
    superseded_at     INTEGER,
    CHECK (
        (is_current = 1 AND superseded_at IS NULL)
        OR (is_current = 0 AND superseded_at IS NOT NULL)
    )
);

CREATE UNIQUE INDEX idx_worker_incarnations_one_current
    ON worker_incarnations(worker_id)
    WHERE is_current = 1;

CREATE INDEX idx_worker_incarnations_worker_registered
    ON worker_incarnations(worker_id, registered_at, id);

CREATE TABLE runtime_sessions (
    id                        TEXT PRIMARY KEY
                              CHECK (
                                  length(id) = 29
                                  AND substr(id, 1, 3) = 'rs_'
                                  AND substr(id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                              ),
    execution_task_id         TEXT NOT NULL REFERENCES task_runs(id) ON DELETE RESTRICT,
    agent_profile_id          TEXT NOT NULL REFERENCES agent_profiles(id) ON DELETE RESTRICT,
    snapshot_authority_key    TEXT NOT NULL CHECK (length(trim(snapshot_authority_key)) > 0),
    snapshot_resource_kind    TEXT NOT NULL DEFAULT 'execution_snapshot'
                              CHECK (snapshot_resource_kind = 'execution_snapshot'),
    snapshot_resource_id      TEXT NOT NULL CHECK (length(trim(snapshot_resource_id)) > 0),
    snapshot_resource_version TEXT NOT NULL CHECK (length(trim(snapshot_resource_version)) > 0),
    snapshot_content_sha256   TEXT NOT NULL
                              CHECK (
                                  length(snapshot_content_sha256) = 64
                                  AND snapshot_content_sha256 NOT GLOB '*[^0123456789abcdef]*'
                              ),
    status                    TEXT NOT NULL CHECK (
                                  status IN (
                                      'requested', 'starting', 'running', 'resume_pending',
                                      'completed', 'failed', 'cancelled', 'lost'
                                  )
                              ),
    record_version            INTEGER NOT NULL DEFAULT 1 CHECK (record_version >= 1),
    terminal_reason           TEXT,
    created_at                INTEGER NOT NULL,
    updated_at                INTEGER NOT NULL
);

CREATE INDEX idx_runtime_sessions_task
    ON runtime_sessions(execution_task_id, created_at, id);

CREATE TABLE runtime_attempts (
    id                    TEXT PRIMARY KEY
                          CHECK (
                              length(id) = 29
                              AND substr(id, 1, 3) = 'ra_'
                              AND substr(id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                          ),
    runtime_session_id    TEXT NOT NULL REFERENCES runtime_sessions(id) ON DELETE RESTRICT,
    worker_incarnation_id TEXT NOT NULL REFERENCES worker_incarnations(id) ON DELETE RESTRICT,
    status                TEXT NOT NULL CHECK (status IN ('starting', 'running', 'exited', 'gone')),
    backend_target        TEXT,
    session_name          TEXT,
    pane_id               TEXT,
    pid                   INTEGER CHECK (pid IS NULL OR pid > 0),
    native_session_ref    TEXT,
    workdir               TEXT,
    is_current            INTEGER NOT NULL CHECK (is_current IN (0, 1)),
    started_at            INTEGER NOT NULL,
    finished_at           INTEGER,
    superseded_at         INTEGER,
    CHECK (
        (is_current = 1 AND superseded_at IS NULL)
        OR (is_current = 0 AND superseded_at IS NOT NULL)
    )
);

CREATE UNIQUE INDEX idx_runtime_attempts_one_current
    ON runtime_attempts(runtime_session_id)
    WHERE is_current = 1;

CREATE INDEX idx_runtime_attempts_session_started
    ON runtime_attempts(runtime_session_id, started_at, id);

CREATE INDEX idx_runtime_attempts_worker
    ON runtime_attempts(worker_incarnation_id, started_at, id);

UPDATE schema_meta SET value = '13' WHERE key = 'version';
