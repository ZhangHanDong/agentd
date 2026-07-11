-- P268 / immutable enterprise execution artifact index and audit log.
-- Additive only: legacy artifacts/events/delivery_events remain unchanged and
-- require explicit mapping into the enterprise records.

CREATE TABLE execution_artifacts (
    id                             TEXT PRIMARY KEY
                                   CHECK (
                                       length(id) = 29
                                       AND substr(id, 1, 3) = 'ar_'
                                       AND substr(id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                                   ),
    kind                           TEXT NOT NULL CHECK (
                                       kind IN (
                                           'requirements', 'spec', 'plan', 'review',
                                           'runtime_summary', 'transcript', 'log',
                                           'patch', 'commit', 'test_report'
                                       )
                                   ),
    content_sha256                 TEXT NOT NULL
                                   CHECK (
                                       length(content_sha256) = 64
                                       AND content_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                   ),
    size_bytes                     INTEGER NOT NULL CHECK (size_bytes >= 0),
    media_type                     TEXT NOT NULL CHECK (length(trim(media_type)) > 0),
    storage_ref                    TEXT NOT NULL CHECK (length(trim(storage_ref)) > 0),
    provenance_json                TEXT NOT NULL CHECK (json_valid(provenance_json)),
    execution_run_id               TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    execution_task_id              TEXT REFERENCES task_runs(id) ON DELETE RESTRICT,
    runtime_session_id             TEXT REFERENCES runtime_sessions(id) ON DELETE RESTRICT,
    runtime_attempt_id             TEXT REFERENCES runtime_attempts(id) ON DELETE RESTRICT,
    snapshot_authority_key         TEXT NOT NULL CHECK (length(trim(snapshot_authority_key)) > 0),
    snapshot_resource_kind         TEXT NOT NULL DEFAULT 'execution_snapshot'
                                   CHECK (snapshot_resource_kind = 'execution_snapshot'),
    snapshot_resource_id           TEXT NOT NULL CHECK (length(trim(snapshot_resource_id)) > 0),
    snapshot_resource_version      TEXT NOT NULL CHECK (length(trim(snapshot_resource_version)) > 0),
    snapshot_content_sha256        TEXT NOT NULL
                                   CHECK (
                                       length(snapshot_content_sha256) = 64
                                       AND snapshot_content_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                   ),
    target_repository_id           TEXT NOT NULL CHECK (length(trim(target_repository_id)) > 0),
    target_base_commit             TEXT NOT NULL CHECK (length(trim(target_base_commit)) > 0),
    producer_worker_incarnation_id TEXT REFERENCES worker_incarnations(id) ON DELETE RESTRICT,
    created_at                     INTEGER NOT NULL,
    CHECK (runtime_session_id IS NULL OR execution_task_id IS NOT NULL),
    CHECK (runtime_attempt_id IS NULL OR runtime_session_id IS NOT NULL)
);

CREATE INDEX idx_execution_artifacts_run_created
    ON execution_artifacts(execution_run_id, created_at, id);

CREATE INDEX idx_execution_artifacts_task_created
    ON execution_artifacts(execution_task_id, created_at, id)
    WHERE execution_task_id IS NOT NULL;

CREATE INDEX idx_execution_artifacts_session_created
    ON execution_artifacts(runtime_session_id, created_at, id)
    WHERE runtime_session_id IS NOT NULL;

CREATE INDEX idx_execution_artifacts_content
    ON execution_artifacts(content_sha256, size_bytes, id);

CREATE TABLE legacy_artifact_mappings (
    legacy_sha256         TEXT PRIMARY KEY REFERENCES artifacts(sha256) ON DELETE RESTRICT,
    execution_artifact_id TEXT NOT NULL UNIQUE REFERENCES execution_artifacts(id) ON DELETE RESTRICT,
    created_at            INTEGER NOT NULL
);

CREATE TABLE artifact_certification_refs (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    execution_artifact_id TEXT NOT NULL REFERENCES execution_artifacts(id) ON DELETE RESTRICT,
    authority_key         TEXT NOT NULL CHECK (length(trim(authority_key)) > 0),
    ref_kind              TEXT NOT NULL CHECK (
                              ref_kind IN ('request', 'result', 'signature', 'attestation')
                          ),
    external_ref          TEXT NOT NULL CHECK (length(trim(external_ref)) > 0),
    recorded_at           INTEGER NOT NULL,
    UNIQUE (execution_artifact_id, authority_key, ref_kind),
    UNIQUE (authority_key, ref_kind, external_ref)
);

CREATE INDEX idx_artifact_certification_refs_artifact
    ON artifact_certification_refs(execution_artifact_id, id);

CREATE TABLE execution_audit_events (
    sequence                       INTEGER PRIMARY KEY AUTOINCREMENT,
    id                             TEXT NOT NULL UNIQUE
                                   CHECK (
                                       length(id) = 29
                                       AND substr(id, 1, 3) = 'ae_'
                                       AND substr(id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                                   ),
    idempotency_scope              TEXT NOT NULL CHECK (length(trim(idempotency_scope)) > 0),
    idempotency_key                TEXT NOT NULL CHECK (length(trim(idempotency_key)) > 0),
    event_type                     TEXT NOT NULL CHECK (length(trim(event_type)) > 0),
    actor_kind                     TEXT NOT NULL CHECK (
                                       actor_kind IN (
                                           'control_plane', 'worker', 'agent_profile', 'operator',
                                           'project_authority', 'certification_authority',
                                           'system', 'import'
                                       )
                                   ),
    actor_ref                      TEXT NOT NULL CHECK (length(trim(actor_ref)) > 0),
    payload_sha256                 TEXT NOT NULL
                                   CHECK (
                                       length(payload_sha256) = 64
                                       AND payload_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                   ),
    payload_json                   TEXT NOT NULL CHECK (json_valid(payload_json)),
    execution_run_id               TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    execution_task_id              TEXT REFERENCES task_runs(id) ON DELETE RESTRICT,
    runtime_session_id             TEXT REFERENCES runtime_sessions(id) ON DELETE RESTRICT,
    runtime_attempt_id             TEXT REFERENCES runtime_attempts(id) ON DELETE RESTRICT,
    execution_artifact_id          TEXT REFERENCES execution_artifacts(id) ON DELETE RESTRICT,
    worker_incarnation_id          TEXT REFERENCES worker_incarnations(id) ON DELETE RESTRICT,
    snapshot_authority_key         TEXT NOT NULL CHECK (length(trim(snapshot_authority_key)) > 0),
    snapshot_resource_kind         TEXT NOT NULL DEFAULT 'execution_snapshot'
                                   CHECK (snapshot_resource_kind = 'execution_snapshot'),
    snapshot_resource_id           TEXT NOT NULL CHECK (length(trim(snapshot_resource_id)) > 0),
    snapshot_resource_version      TEXT NOT NULL CHECK (length(trim(snapshot_resource_version)) > 0),
    snapshot_content_sha256        TEXT NOT NULL
                                   CHECK (
                                       length(snapshot_content_sha256) = 64
                                       AND snapshot_content_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                   ),
    target_repository_id           TEXT NOT NULL CHECK (length(trim(target_repository_id)) > 0),
    target_base_commit             TEXT NOT NULL CHECK (length(trim(target_base_commit)) > 0),
    occurred_at                    INTEGER NOT NULL,
    recorded_at                    INTEGER NOT NULL,
    UNIQUE (idempotency_scope, idempotency_key),
    CHECK (runtime_session_id IS NULL OR execution_task_id IS NOT NULL),
    CHECK (runtime_attempt_id IS NULL OR runtime_session_id IS NOT NULL)
);

CREATE INDEX idx_execution_audit_events_run_sequence
    ON execution_audit_events(execution_run_id, sequence);

CREATE INDEX idx_execution_audit_events_task_sequence
    ON execution_audit_events(execution_task_id, sequence)
    WHERE execution_task_id IS NOT NULL;

CREATE INDEX idx_execution_audit_events_artifact_sequence
    ON execution_audit_events(execution_artifact_id, sequence)
    WHERE execution_artifact_id IS NOT NULL;

CREATE TRIGGER trg_execution_artifacts_no_update
BEFORE UPDATE ON execution_artifacts
BEGIN
    SELECT RAISE(ABORT, 'execution_artifacts are immutable');
END;

CREATE TRIGGER trg_execution_artifacts_no_delete
BEFORE DELETE ON execution_artifacts
BEGIN
    SELECT RAISE(ABORT, 'execution_artifacts are immutable');
END;

CREATE TRIGGER trg_legacy_artifact_mappings_no_update
BEFORE UPDATE ON legacy_artifact_mappings
BEGIN
    SELECT RAISE(ABORT, 'legacy_artifact_mappings are immutable');
END;

CREATE TRIGGER trg_legacy_artifact_mappings_no_delete
BEFORE DELETE ON legacy_artifact_mappings
BEGIN
    SELECT RAISE(ABORT, 'legacy_artifact_mappings are immutable');
END;

CREATE TRIGGER trg_artifact_certification_refs_no_update
BEFORE UPDATE ON artifact_certification_refs
BEGIN
    SELECT RAISE(ABORT, 'artifact_certification_refs are immutable');
END;

CREATE TRIGGER trg_artifact_certification_refs_no_delete
BEFORE DELETE ON artifact_certification_refs
BEGIN
    SELECT RAISE(ABORT, 'artifact_certification_refs are immutable');
END;

CREATE TRIGGER trg_execution_audit_events_no_update
BEFORE UPDATE ON execution_audit_events
BEGIN
    SELECT RAISE(ABORT, 'execution_audit_events are append-only');
END;

CREATE TRIGGER trg_execution_audit_events_no_delete
BEFORE DELETE ON execution_audit_events
BEGIN
    SELECT RAISE(ABORT, 'execution_audit_events are append-only');
END;

UPDATE schema_meta SET value = '14' WHERE key = 'version';
