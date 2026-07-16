-- AD-E2 durable enterprise queue, worker availability, fenced reports, and outbox.
-- Additive only: agent_scheduler_* remains a compatibility surface and is not
-- referenced by canonical enterprise scheduling state.

CREATE TABLE enterprise_fleet_queue (
    execution_task_id            TEXT PRIMARY KEY REFERENCES task_runs(id) ON DELETE RESTRICT,
    idempotency_key              TEXT NOT NULL UNIQUE CHECK (length(trim(idempotency_key)) > 0),
    submission_sha256            TEXT NOT NULL CHECK (
                                     length(submission_sha256) = 64
                                     AND submission_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                 ),
    status                       TEXT NOT NULL CHECK (
                                     status IN (
                                         'queued', 'acquired', 'retry_wait',
                                         'completed', 'cancelled', 'dead_letter'
                                     )
                                 ),
    snapshot_authority_key       TEXT NOT NULL CHECK (length(trim(snapshot_authority_key)) > 0),
    snapshot_resource_id         TEXT NOT NULL CHECK (length(trim(snapshot_resource_id)) > 0),
    snapshot_resource_version    TEXT NOT NULL CHECK (length(trim(snapshot_resource_version)) > 0),
    snapshot_content_sha256      TEXT NOT NULL CHECK (
                                     length(snapshot_content_sha256) = 64
                                     AND snapshot_content_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                 ),
    snapshot_valid_until         INTEGER NOT NULL,
    organization_authority_key   TEXT NOT NULL CHECK (length(trim(organization_authority_key)) > 0),
    organization_resource_id     TEXT NOT NULL CHECK (length(trim(organization_resource_id)) > 0),
    organization_resource_version TEXT NOT NULL CHECK (length(trim(organization_resource_version)) > 0),
    project_authority_key        TEXT NOT NULL CHECK (length(trim(project_authority_key)) > 0),
    project_resource_id          TEXT NOT NULL CHECK (length(trim(project_resource_id)) > 0),
    project_resource_version     TEXT NOT NULL CHECK (length(trim(project_resource_version)) > 0),
    rbac_policy_resource_id      TEXT NOT NULL CHECK (length(trim(rbac_policy_resource_id)) > 0),
    rbac_policy_resource_version TEXT NOT NULL CHECK (length(trim(rbac_policy_resource_version)) > 0),
    quota_policy_resource_id     TEXT NOT NULL CHECK (length(trim(quota_policy_resource_id)) > 0),
    quota_policy_resource_version TEXT NOT NULL CHECK (length(trim(quota_policy_resource_version)) > 0),
    policy_revocation_epoch      INTEGER NOT NULL CHECK (policy_revocation_epoch > 0),
    placement_policy_json        TEXT NOT NULL CHECK (length(trim(placement_policy_json)) > 0),
    resource_class               TEXT NOT NULL CHECK (length(trim(resource_class)) > 0),
    required_capabilities_json   TEXT NOT NULL CHECK (length(trim(required_capabilities_json)) > 0),
    quota_max_active             INTEGER NOT NULL CHECK (quota_max_active > 0),
    priority                     INTEGER NOT NULL,
    max_attempts                 INTEGER NOT NULL CHECK (max_attempts > 0),
    attempt_count                INTEGER NOT NULL DEFAULT 0 CHECK (
                                     attempt_count >= 0 AND attempt_count <= max_attempts
                                 ),
    assigned_lease_id            TEXT REFERENCES execution_task_leases(id) ON DELETE RESTRICT,
    assigned_worker_incarnation_id TEXT REFERENCES worker_incarnations(id) ON DELETE RESTRICT,
    next_eligible_at             INTEGER,
    outcome_sha256               TEXT CHECK (
                                     outcome_sha256 IS NULL
                                     OR (
                                         length(outcome_sha256) = 64
                                         AND outcome_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                     )
                                 ),
    block_code                   TEXT CHECK (
                                     block_code IS NULL OR length(trim(block_code)) > 0
                                 ),
    created_at                   INTEGER NOT NULL,
    updated_at                   INTEGER NOT NULL,
    CHECK (snapshot_valid_until > created_at),
    CHECK (
        (status = 'acquired' AND assigned_lease_id IS NOT NULL AND assigned_worker_incarnation_id IS NOT NULL)
        OR status <> 'acquired'
    ),
    CHECK (
        (status = 'retry_wait' AND next_eligible_at IS NOT NULL)
        OR status <> 'retry_wait'
    ),
    CHECK (
        (status = 'completed' AND outcome_sha256 IS NOT NULL)
        OR status <> 'completed'
    )
);

CREATE INDEX idx_enterprise_fleet_queue_pull
    ON enterprise_fleet_queue(status, next_eligible_at, priority DESC, created_at, execution_task_id);

CREATE INDEX idx_enterprise_fleet_queue_quota
    ON enterprise_fleet_queue(
        project_authority_key, project_resource_id, project_resource_version, status
    );

CREATE INDEX idx_enterprise_fleet_queue_worker
    ON enterprise_fleet_queue(assigned_worker_incarnation_id, status, updated_at);

CREATE TABLE enterprise_worker_availability (
    worker_incarnation_id        TEXT PRIMARY KEY REFERENCES worker_incarnations(id) ON DELETE RESTRICT,
    worker_id                    TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
    heartbeat_sequence           INTEGER NOT NULL CHECK (heartbeat_sequence > 0),
    worker_status                TEXT NOT NULL CHECK (worker_status IN ('online', 'draining', 'offline')),
    daemon_version               TEXT NOT NULL CHECK (length(trim(daemon_version)) > 0),
    protocol_min                 INTEGER NOT NULL CHECK (protocol_min > 0),
    protocol_max                 INTEGER NOT NULL CHECK (protocol_max >= protocol_min),
    region                       TEXT NOT NULL CHECK (length(trim(region)) > 0),
    zone                         TEXT NOT NULL CHECK (length(trim(zone)) > 0),
    resource_class               TEXT NOT NULL CHECK (length(trim(resource_class)) > 0),
    capabilities_json            TEXT NOT NULL CHECK (length(trim(capabilities_json)) > 0),
    total_slots                  INTEGER NOT NULL CHECK (total_slots > 0),
    available_slots              INTEGER NOT NULL CHECK (
                                     available_slots >= 0 AND available_slots <= total_slots
                                 ),
    data_classifications_json    TEXT NOT NULL CHECK (length(trim(data_classifications_json)) > 0),
    image_digest                 TEXT NOT NULL CHECK (
                                     length(image_digest) = 71
                                     AND substr(image_digest, 1, 7) = 'sha256:'
                                     AND substr(image_digest, 8) NOT GLOB '*[^0123456789abcdef]*'
                                 ),
    image_signature_verified     INTEGER NOT NULL CHECK (image_signature_verified IN (0, 1)),
    dedicated_pool               INTEGER NOT NULL CHECK (dedicated_pool IN (0, 1)),
    egress_profile_ids_json      TEXT NOT NULL CHECK (length(trim(egress_profile_ids_json)) > 0),
    tenant_cache_namespaces_json TEXT NOT NULL CHECK (length(trim(tenant_cache_namespaces_json)) > 0),
    observed_at                  INTEGER NOT NULL,
    updated_at                   INTEGER NOT NULL
);

CREATE INDEX idx_enterprise_worker_availability_pull
    ON enterprise_worker_availability(
        worker_status, resource_class, available_slots, observed_at, worker_incarnation_id
    );

CREATE TABLE enterprise_scheduler_outbox (
    sequence                 INTEGER PRIMARY KEY AUTOINCREMENT,
    id                       TEXT NOT NULL UNIQUE CHECK (
                                 length(id) = 29
                                 AND substr(id, 1, 3) = 'fo_'
                                 AND substr(id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                             ),
    event_type               TEXT NOT NULL CHECK (length(trim(event_type)) > 0),
    execution_task_id        TEXT NOT NULL REFERENCES task_runs(id) ON DELETE RESTRICT,
    worker_incarnation_id    TEXT REFERENCES worker_incarnations(id) ON DELETE RESTRICT,
    lease_id                 TEXT REFERENCES execution_task_leases(id) ON DELETE RESTRICT,
    fencing_token            INTEGER CHECK (fencing_token IS NULL OR fencing_token > 0),
    payload_sha256           TEXT NOT NULL CHECK (
                                 length(payload_sha256) = 64
                                 AND payload_sha256 NOT GLOB '*[^0123456789abcdef]*'
                             ),
    created_at               INTEGER NOT NULL,
    delivered_at             INTEGER,
    CHECK (
        (worker_incarnation_id IS NULL AND lease_id IS NULL AND fencing_token IS NULL)
        OR (worker_incarnation_id IS NOT NULL AND lease_id IS NOT NULL AND fencing_token IS NOT NULL)
    )
);

CREATE INDEX idx_enterprise_scheduler_outbox_pending
    ON enterprise_scheduler_outbox(delivered_at, sequence)
    WHERE delivered_at IS NULL;

CREATE TABLE enterprise_scheduler_report_receipts (
    report_kind         TEXT NOT NULL CHECK (
                            report_kind IN ('complete', 'fail', 'cancel', 'artifact', 'side_effect')
                        ),
    idempotency_key     TEXT NOT NULL CHECK (length(trim(idempotency_key)) > 0),
    execution_task_id   TEXT NOT NULL REFERENCES task_runs(id) ON DELETE RESTRICT,
    worker_incarnation_id TEXT NOT NULL REFERENCES worker_incarnations(id) ON DELETE RESTRICT,
    lease_id            TEXT NOT NULL REFERENCES execution_task_leases(id) ON DELETE RESTRICT,
    fencing_token       INTEGER NOT NULL CHECK (fencing_token > 0),
    request_sha256      TEXT NOT NULL CHECK (
                            length(request_sha256) = 64
                            AND request_sha256 NOT GLOB '*[^0123456789abcdef]*'
                        ),
    recorded_at         INTEGER NOT NULL,
    PRIMARY KEY (report_kind, idempotency_key)
);

CREATE TABLE enterprise_artifact_upload_acknowledgements (
    upload_id               TEXT PRIMARY KEY CHECK (
                                length(upload_id) = 29
                                AND substr(upload_id, 1, 3) = 'au_'
                                AND substr(upload_id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                            ),
    execution_artifact_id   TEXT NOT NULL CHECK (
                                length(execution_artifact_id) = 29
                                AND substr(execution_artifact_id, 1, 3) = 'ar_'
                                AND substr(execution_artifact_id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                            ),
    execution_task_id       TEXT NOT NULL REFERENCES task_runs(id) ON DELETE RESTRICT,
    worker_incarnation_id   TEXT NOT NULL REFERENCES worker_incarnations(id) ON DELETE RESTRICT,
    lease_id                TEXT NOT NULL REFERENCES execution_task_leases(id) ON DELETE RESTRICT,
    fencing_token           INTEGER NOT NULL CHECK (fencing_token > 0),
    idempotency_key         TEXT NOT NULL UNIQUE CHECK (length(trim(idempotency_key)) > 0),
    artifact_sha256         TEXT NOT NULL CHECK (
                                length(artifact_sha256) = 64
                                AND artifact_sha256 NOT GLOB '*[^0123456789abcdef]*'
                            ),
    upload_attempt          INTEGER NOT NULL CHECK (upload_attempt > 0),
    part_count              INTEGER NOT NULL CHECK (part_count > 0),
    acknowledged_at         INTEGER NOT NULL
);

CREATE INDEX idx_enterprise_artifact_upload_task
    ON enterprise_artifact_upload_acknowledgements(execution_task_id, acknowledged_at, upload_id);

CREATE TABLE enterprise_side_effect_admissions (
    idempotency_key         TEXT PRIMARY KEY CHECK (length(trim(idempotency_key)) > 0),
    execution_task_id       TEXT NOT NULL REFERENCES task_runs(id) ON DELETE RESTRICT,
    worker_incarnation_id   TEXT NOT NULL REFERENCES worker_incarnations(id) ON DELETE RESTRICT,
    lease_id                TEXT NOT NULL REFERENCES execution_task_leases(id) ON DELETE RESTRICT,
    fencing_token           INTEGER NOT NULL CHECK (fencing_token > 0),
    checkpoint              TEXT NOT NULL CHECK (
                                checkpoint IN (
                                    'artifact_acceptance', 'delivery', 'release'
                                )
                            ),
    action                  TEXT NOT NULL CHECK (length(trim(action)) > 0),
    admitted_at             INTEGER NOT NULL
);

CREATE TABLE enterprise_fencing_rejections (
    sequence                INTEGER PRIMARY KEY AUTOINCREMENT,
    report_kind             TEXT NOT NULL CHECK (length(trim(report_kind)) > 0),
    execution_task_id       TEXT NOT NULL REFERENCES task_runs(id) ON DELETE RESTRICT,
    worker_incarnation_id   TEXT NOT NULL,
    lease_id                TEXT NOT NULL,
    fencing_token           INTEGER NOT NULL CHECK (fencing_token > 0),
    denial_code             TEXT NOT NULL CHECK (length(trim(denial_code)) > 0),
    observed_at             INTEGER NOT NULL
);

CREATE INDEX idx_enterprise_fencing_rejections_task
    ON enterprise_fencing_rejections(execution_task_id, sequence);

CREATE TRIGGER trg_enterprise_fleet_queue_identity_immutable
BEFORE UPDATE OF execution_task_id, idempotency_key, submission_sha256, snapshot_authority_key,
    snapshot_resource_id, snapshot_resource_version, snapshot_content_sha256,
    organization_authority_key, organization_resource_id, organization_resource_version,
    project_authority_key, project_resource_id, project_resource_version,
    rbac_policy_resource_id, rbac_policy_resource_version,
    quota_policy_resource_id, quota_policy_resource_version,
    policy_revocation_epoch, placement_policy_json, resource_class,
    required_capabilities_json, quota_max_active, priority, max_attempts, created_at
ON enterprise_fleet_queue
BEGIN
    SELECT RAISE(ABORT, 'enterprise fleet queue authority and requirements are immutable');
END;

CREATE TRIGGER trg_enterprise_fleet_queue_terminal_immutable
BEFORE UPDATE ON enterprise_fleet_queue
WHEN OLD.status IN ('completed', 'cancelled', 'dead_letter')
BEGIN
    SELECT RAISE(ABORT, 'terminal enterprise fleet queue record is immutable');
END;

CREATE TRIGGER trg_enterprise_scheduler_outbox_no_update
BEFORE UPDATE OF id, event_type, execution_task_id, worker_incarnation_id, lease_id,
    fencing_token, payload_sha256, created_at
ON enterprise_scheduler_outbox
BEGIN
    SELECT RAISE(ABORT, 'scheduler outbox event is immutable except delivery acknowledgement');
END;

CREATE TRIGGER trg_enterprise_scheduler_outbox_no_delete
BEFORE DELETE ON enterprise_scheduler_outbox
BEGIN
    SELECT RAISE(ABORT, 'scheduler outbox history is immutable');
END;

CREATE TRIGGER trg_enterprise_scheduler_receipts_no_update
BEFORE UPDATE ON enterprise_scheduler_report_receipts
BEGIN
    SELECT RAISE(ABORT, 'scheduler report receipt is immutable');
END;

CREATE TRIGGER trg_enterprise_scheduler_receipts_no_delete
BEFORE DELETE ON enterprise_scheduler_report_receipts
BEGIN
    SELECT RAISE(ABORT, 'scheduler report receipt history is immutable');
END;

CREATE TRIGGER trg_enterprise_artifact_upload_ack_no_update
BEFORE UPDATE ON enterprise_artifact_upload_acknowledgements
BEGIN
    SELECT RAISE(ABORT, 'artifact upload acknowledgement is immutable');
END;

CREATE TRIGGER trg_enterprise_artifact_upload_ack_no_delete
BEFORE DELETE ON enterprise_artifact_upload_acknowledgements
BEGIN
    SELECT RAISE(ABORT, 'artifact upload acknowledgement history is immutable');
END;

CREATE TRIGGER trg_enterprise_side_effect_admission_no_update
BEFORE UPDATE ON enterprise_side_effect_admissions
BEGIN
    SELECT RAISE(ABORT, 'side effect admission is immutable');
END;

CREATE TRIGGER trg_enterprise_side_effect_admission_no_delete
BEFORE DELETE ON enterprise_side_effect_admissions
BEGIN
    SELECT RAISE(ABORT, 'side effect admission history is immutable');
END;

CREATE TRIGGER trg_enterprise_fencing_rejections_no_update
BEFORE UPDATE ON enterprise_fencing_rejections
BEGIN
    SELECT RAISE(ABORT, 'fencing rejection evidence is immutable');
END;

CREATE TRIGGER trg_enterprise_fencing_rejections_no_delete
BEFORE DELETE ON enterprise_fencing_rejections
BEGIN
    SELECT RAISE(ABORT, 'fencing rejection evidence is immutable');
END;

UPDATE schema_meta SET value = '18' WHERE key = 'version';
