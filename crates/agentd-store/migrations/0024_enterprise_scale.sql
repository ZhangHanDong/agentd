-- AD-E7 enterprise scale, multi-zone, multi-region, compliance, and DR state.

CREATE TABLE enterprise_control_plane_members (
    instance_id          TEXT PRIMARY KEY CHECK (length(instance_id) = 29 AND substr(instance_id, 1, 3) = 'ci_'),
    heartbeat_sequence   INTEGER NOT NULL CHECK (heartbeat_sequence > 0),
    region               TEXT NOT NULL CHECK (length(trim(region)) > 0),
    zone                 TEXT NOT NULL CHECK (length(trim(zone)) > 0),
    daemon_version       TEXT NOT NULL CHECK (length(trim(daemon_version)) > 0),
    endpoint_sha256      TEXT NOT NULL CHECK (length(endpoint_sha256) = 64 AND endpoint_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    status               TEXT NOT NULL CHECK (status IN ('ready', 'draining', 'offline')),
    started_at           INTEGER NOT NULL,
    observed_at          INTEGER NOT NULL,
    updated_at           INTEGER NOT NULL CHECK (updated_at >= started_at)
);

CREATE INDEX idx_enterprise_control_plane_members_health
    ON enterprise_control_plane_members(status, observed_at, region, zone);

CREATE TABLE enterprise_control_plane_leadership (
    singleton            INTEGER PRIMARY KEY CHECK (singleton = 1),
    instance_id          TEXT NOT NULL REFERENCES enterprise_control_plane_members(instance_id) ON DELETE RESTRICT,
    term                 INTEGER NOT NULL CHECK (term > 0),
    fencing_token        INTEGER NOT NULL CHECK (fencing_token > 0),
    acquired_at          INTEGER NOT NULL,
    renewed_at           INTEGER NOT NULL,
    expires_at           INTEGER NOT NULL CHECK (expires_at > renewed_at)
);

CREATE TABLE enterprise_scale_receipts (
    operation            TEXT NOT NULL CHECK (length(trim(operation)) > 0),
    idempotency_key      TEXT NOT NULL CHECK (length(trim(idempotency_key)) > 0),
    request_sha256       TEXT NOT NULL CHECK (length(request_sha256) = 64 AND request_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    response_json        TEXT NOT NULL CHECK (json_valid(response_json)),
    recorded_at          INTEGER NOT NULL,
    PRIMARY KEY (operation, idempotency_key)
);

CREATE TABLE enterprise_worker_image_rollouts (
    rollout_id              TEXT PRIMARY KEY CHECK (length(rollout_id) = 29 AND substr(rollout_id, 1, 3) = 'ir_'),
    image_digest            TEXT NOT NULL CHECK (length(image_digest) = 71 AND substr(image_digest, 1, 7) = 'sha256:'),
    signature_bundle_sha256 TEXT NOT NULL CHECK (length(signature_bundle_sha256) = 64 AND signature_bundle_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    policy_sha256           TEXT NOT NULL CHECK (length(policy_sha256) = 64 AND policy_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    required_zones_json     TEXT NOT NULL CHECK (json_valid(required_zones_json)),
    declaration_sha256      TEXT NOT NULL CHECK (length(declaration_sha256) = 64 AND declaration_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    status                  TEXT NOT NULL CHECK (status IN ('declared', 'progressing', 'healthy', 'degraded', 'rolled_back')),
    declared_at             INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL CHECK (updated_at >= declared_at)
);

CREATE TABLE enterprise_worker_image_zone_observations (
    rollout_id              TEXT NOT NULL REFERENCES enterprise_worker_image_rollouts(rollout_id) ON DELETE RESTRICT,
    zone                    TEXT NOT NULL CHECK (length(trim(zone)) > 0),
    observed_image_digest   TEXT NOT NULL CHECK (length(observed_image_digest) = 71 AND substr(observed_image_digest, 1, 7) = 'sha256:'),
    signature_verified      INTEGER NOT NULL CHECK (signature_verified IN (0, 1)),
    ready_workers           INTEGER NOT NULL CHECK (ready_workers >= 0),
    desired_workers         INTEGER NOT NULL CHECK (desired_workers > 0),
    observed_at             INTEGER NOT NULL,
    observation_sha256      TEXT NOT NULL CHECK (length(observation_sha256) = 64 AND observation_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    PRIMARY KEY (rollout_id, zone)
);

CREATE TABLE enterprise_zone_pool_policies (
    pool_id                         TEXT PRIMARY KEY CHECK (length(pool_id) = 29 AND substr(pool_id, 1, 3) = 'zp_'),
    region                          TEXT NOT NULL CHECK (length(trim(region)) > 0),
    zone                            TEXT NOT NULL CHECK (length(trim(zone)) > 0),
    resource_class                  TEXT NOT NULL CHECK (length(trim(resource_class)) > 0),
    trust_domain                    TEXT NOT NULL CHECK (length(trim(trust_domain)) > 0),
    rollout_id                      TEXT NOT NULL REFERENCES enterprise_worker_image_rollouts(rollout_id) ON DELETE RESTRICT,
    minimum_replicas                INTEGER NOT NULL CHECK (minimum_replicas > 0),
    maximum_replicas                INTEGER NOT NULL CHECK (maximum_replicas >= minimum_replicas),
    target_queue_per_slot           INTEGER NOT NULL CHECK (target_queue_per_slot > 0),
    scale_down_cooldown_seconds     INTEGER NOT NULL CHECK (scale_down_cooldown_seconds > 0),
    enabled                         INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    policy_sha256                   TEXT NOT NULL CHECK (length(policy_sha256) = 64 AND policy_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    updated_at                      INTEGER NOT NULL,
    UNIQUE (region, zone, resource_class, trust_domain)
);

CREATE TABLE enterprise_autoscaling_recommendations (
    sequence              INTEGER PRIMARY KEY AUTOINCREMENT,
    pool_id               TEXT NOT NULL REFERENCES enterprise_zone_pool_policies(pool_id) ON DELETE RESTRICT,
    current_replicas      INTEGER NOT NULL CHECK (current_replicas >= 0),
    desired_replicas      INTEGER NOT NULL CHECK (desired_replicas >= 0),
    queue_depth           INTEGER NOT NULL CHECK (queue_depth >= 0),
    running_tasks         INTEGER NOT NULL CHECK (running_tasks >= 0),
    total_slots           INTEGER NOT NULL CHECK (total_slots >= 0),
    available_slots       INTEGER NOT NULL CHECK (available_slots >= 0 AND available_slots <= total_slots),
    reason_code           TEXT NOT NULL CHECK (length(trim(reason_code)) > 0),
    recommendation_sha256 TEXT NOT NULL UNIQUE CHECK (length(recommendation_sha256) = 64 AND recommendation_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    observed_at           INTEGER NOT NULL
);

CREATE INDEX idx_enterprise_autoscaling_pool
    ON enterprise_autoscaling_recommendations(pool_id, observed_at DESC, sequence DESC);

CREATE TABLE enterprise_tenant_keys (
    tenant_key_id          TEXT PRIMARY KEY CHECK (length(tenant_key_id) = 29 AND substr(tenant_key_id, 1, 3) = 'tk_'),
    tenant_scope_sha256    TEXT NOT NULL CHECK (length(tenant_scope_sha256) = 64 AND tenant_scope_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    region                 TEXT NOT NULL CHECK (length(trim(region)) > 0),
    kms_key_ref_sha256     TEXT NOT NULL CHECK (length(kms_key_ref_sha256) = 64 AND kms_key_ref_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    key_version_ref_sha256 TEXT NOT NULL CHECK (length(key_version_ref_sha256) = 64 AND key_version_ref_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    status                 TEXT NOT NULL CHECK (status IN ('active', 'retiring', 'retired')),
    registration_sha256    TEXT NOT NULL CHECK (length(registration_sha256) = 64 AND registration_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    activated_at           INTEGER NOT NULL,
    retired_at             INTEGER,
    CHECK ((status = 'retired' AND retired_at IS NOT NULL) OR (status <> 'retired' AND retired_at IS NULL)),
    UNIQUE (tenant_scope_sha256, region, key_version_ref_sha256)
);

CREATE UNIQUE INDEX idx_enterprise_tenant_keys_active
    ON enterprise_tenant_keys(tenant_scope_sha256, region)
    WHERE status = 'active';

CREATE TABLE enterprise_artifact_replication_plans (
    replication_id        TEXT PRIMARY KEY CHECK (length(replication_id) = 29 AND substr(replication_id, 1, 3) = 'rp_'),
    execution_artifact_id TEXT NOT NULL,
    tenant_scope_sha256   TEXT NOT NULL CHECK (length(tenant_scope_sha256) = 64 AND tenant_scope_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    artifact_sha256       TEXT NOT NULL CHECK (length(artifact_sha256) = 64 AND artifact_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    source_region         TEXT NOT NULL CHECK (length(trim(source_region)) > 0),
    required_regions_json TEXT NOT NULL CHECK (json_valid(required_regions_json)),
    plan_sha256           TEXT NOT NULL CHECK (length(plan_sha256) = 64 AND plan_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    created_at            INTEGER NOT NULL,
    UNIQUE (execution_artifact_id, artifact_sha256)
);

CREATE TABLE enterprise_artifact_replica_acknowledgements (
    replication_id        TEXT NOT NULL REFERENCES enterprise_artifact_replication_plans(replication_id) ON DELETE RESTRICT,
    region                TEXT NOT NULL CHECK (length(trim(region)) > 0),
    artifact_sha256       TEXT NOT NULL CHECK (length(artifact_sha256) = 64 AND artifact_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    object_ref_sha256     TEXT NOT NULL CHECK (length(object_ref_sha256) = 64 AND object_ref_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    tenant_key_id         TEXT NOT NULL REFERENCES enterprise_tenant_keys(tenant_key_id) ON DELETE RESTRICT,
    status                TEXT NOT NULL CHECK (status IN ('pending', 'available', 'failed')),
    acknowledgement_sha256 TEXT NOT NULL CHECK (length(acknowledgement_sha256) = 64 AND acknowledgement_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    acknowledged_at       INTEGER NOT NULL,
    PRIMARY KEY (replication_id, region)
);

CREATE TABLE enterprise_retention_policies (
    tenant_scope_sha256          TEXT PRIMARY KEY CHECK (length(tenant_scope_sha256) = 64 AND tenant_scope_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    policy_version_sha256        TEXT NOT NULL CHECK (length(policy_version_sha256) = 64 AND policy_version_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    artifact_retention_seconds   INTEGER NOT NULL CHECK (artifact_retention_seconds > 0),
    transcript_retention_seconds INTEGER NOT NULL CHECK (transcript_retention_seconds > 0),
    audit_retention_seconds      INTEGER NOT NULL CHECK (audit_retention_seconds >= artifact_retention_seconds),
    minimum_replica_regions      INTEGER NOT NULL CHECK (minimum_replica_regions > 0),
    updated_at                   INTEGER NOT NULL
);

CREATE TABLE enterprise_legal_holds (
    legal_hold_id       TEXT PRIMARY KEY CHECK (length(legal_hold_id) = 29 AND substr(legal_hold_id, 1, 3) = 'lh_'),
    tenant_scope_sha256 TEXT NOT NULL CHECK (length(tenant_scope_sha256) = 64 AND tenant_scope_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    subject_kind        TEXT NOT NULL CHECK (length(trim(subject_kind)) > 0),
    subject_sha256      TEXT NOT NULL CHECK (length(subject_sha256) = 64 AND subject_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    reason_sha256       TEXT NOT NULL CHECK (length(reason_sha256) = 64 AND reason_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    hold_sha256         TEXT NOT NULL CHECK (length(hold_sha256) = 64 AND hold_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    active              INTEGER NOT NULL CHECK (active IN (0, 1)),
    placed_at           INTEGER NOT NULL,
    released_at         INTEGER,
    CHECK ((active = 1 AND released_at IS NULL) OR (active = 0 AND released_at IS NOT NULL))
);

CREATE INDEX idx_enterprise_legal_holds_subject
    ON enterprise_legal_holds(tenant_scope_sha256, subject_kind, subject_sha256, active);

CREATE TABLE enterprise_dr_checkpoints (
    checkpoint_id              TEXT PRIMARY KEY CHECK (length(checkpoint_id) = 29 AND substr(checkpoint_id, 1, 3) = 'dr_'),
    region                     TEXT NOT NULL CHECK (length(trim(region)) > 0),
    database_sha256            TEXT NOT NULL CHECK (length(database_sha256) = 64 AND database_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    artifact_index_sha256      TEXT NOT NULL CHECK (length(artifact_index_sha256) = 64 AND artifact_index_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    audit_head_sha256          TEXT NOT NULL CHECK (length(audit_head_sha256) = 64 AND audit_head_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    matrix_cursor_sha256       TEXT NOT NULL CHECK (length(matrix_cursor_sha256) = 64 AND matrix_cursor_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    certification_head_sha256 TEXT NOT NULL CHECK (length(certification_head_sha256) = 64 AND certification_head_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    checkpoint_sha256          TEXT NOT NULL CHECK (length(checkpoint_sha256) = 64 AND checkpoint_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    maximum_rpo_seconds        INTEGER NOT NULL CHECK (maximum_rpo_seconds > 0),
    maximum_rto_seconds        INTEGER NOT NULL CHECK (maximum_rto_seconds > 0),
    created_at                 INTEGER NOT NULL
);

CREATE TABLE enterprise_dr_drills (
    drill_id                 TEXT PRIMARY KEY CHECK (length(drill_id) = 29 AND substr(drill_id, 1, 3) = 'dd_'),
    checkpoint_id            TEXT NOT NULL REFERENCES enterprise_dr_checkpoints(checkpoint_id) ON DELETE RESTRICT,
    recovery_region          TEXT NOT NULL CHECK (length(trim(recovery_region)) > 0),
    measured_rpo_seconds     INTEGER NOT NULL CHECK (measured_rpo_seconds >= 0),
    measured_rto_seconds     INTEGER NOT NULL CHECK (measured_rto_seconds >= 0),
    lease_fencing_verified   INTEGER NOT NULL CHECK (lease_fencing_verified IN (0, 1)),
    accepted_state_verified INTEGER NOT NULL CHECK (accepted_state_verified IN (0, 1)),
    status                   TEXT NOT NULL CHECK (status IN ('passed', 'failed')),
    evidence_sha256          TEXT NOT NULL CHECK (length(evidence_sha256) = 64 AND evidence_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    drill_sha256             TEXT NOT NULL CHECK (length(drill_sha256) = 64 AND drill_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    completed_at             INTEGER NOT NULL
);

CREATE TABLE enterprise_load_models (
    load_model_id                    TEXT PRIMARY KEY CHECK (length(load_model_id) = 29 AND substr(load_model_id, 1, 3) = 'lm_'),
    version                          TEXT NOT NULL UNIQUE CHECK (length(trim(version)) > 0),
    content_sha256                   TEXT NOT NULL CHECK (length(content_sha256) = 64 AND content_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    dimensions_json                  TEXT NOT NULL CHECK (json_valid(dimensions_json)),
    test_window_seconds              INTEGER NOT NULL CHECK (test_window_seconds > 0),
    tenant_count                     INTEGER NOT NULL CHECK (tenant_count > 0),
    project_count                    INTEGER NOT NULL CHECK (project_count > 0),
    room_count                       INTEGER NOT NULL CHECK (room_count > 0),
    matrix_events_per_second         INTEGER NOT NULL CHECK (matrix_events_per_second > 0),
    maximum_queue_depth              INTEGER NOT NULL CHECK (maximum_queue_depth > 0),
    noisy_neighbor_ratio_basis_points INTEGER NOT NULL CHECK (noisy_neighbor_ratio_basis_points BETWEEN 0 AND 10000),
    registration_sha256              TEXT NOT NULL CHECK (length(registration_sha256) = 64 AND registration_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    registered_at                    INTEGER NOT NULL
);

CREATE TABLE enterprise_service_level_measurements (
    sequence                    INTEGER PRIMARY KEY AUTOINCREMENT,
    idempotency_key             TEXT NOT NULL UNIQUE CHECK (length(trim(idempotency_key)) > 0),
    tenant_scope_sha256         TEXT NOT NULL CHECK (length(tenant_scope_sha256) = 64 AND tenant_scope_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    metric                      TEXT NOT NULL CHECK (length(trim(metric)) > 0),
    target_units                INTEGER NOT NULL CHECK (target_units > 0),
    observed_units              INTEGER NOT NULL CHECK (observed_units >= 0),
    error_budget_units          INTEGER NOT NULL CHECK (error_budget_units >= 0),
    consumed_budget_units       INTEGER NOT NULL CHECK (consumed_budget_units >= 0),
    window_started_at           INTEGER NOT NULL,
    window_ends_at              INTEGER NOT NULL CHECK (window_ends_at > window_started_at),
    measured_at                 INTEGER NOT NULL CHECK (measured_at >= window_started_at),
    status                      TEXT NOT NULL CHECK (status IN ('within_objective', 'budget_warning', 'breached')),
    measurement_sha256          TEXT NOT NULL CHECK (length(measurement_sha256) = 64 AND measurement_sha256 NOT GLOB '*[^0123456789abcdef]*')
);

CREATE INDEX idx_enterprise_service_level_status
    ON enterprise_service_level_measurements(status, measured_at DESC);

CREATE TRIGGER trg_enterprise_rollout_identity_immutable
BEFORE UPDATE OF rollout_id, image_digest, signature_bundle_sha256, policy_sha256,
                 required_zones_json, declaration_sha256, declared_at
ON enterprise_worker_image_rollouts
BEGIN SELECT RAISE(ABORT, 'worker image rollout declaration is immutable'); END;

CREATE TRIGGER trg_enterprise_replication_plan_immutable
BEFORE UPDATE ON enterprise_artifact_replication_plans
BEGIN SELECT RAISE(ABORT, 'artifact replication plan is immutable'); END;

CREATE TRIGGER trg_enterprise_dr_checkpoint_immutable
BEFORE UPDATE ON enterprise_dr_checkpoints
BEGIN SELECT RAISE(ABORT, 'disaster recovery checkpoint is immutable'); END;

CREATE TRIGGER trg_enterprise_dr_drill_immutable
BEFORE UPDATE ON enterprise_dr_drills
BEGIN SELECT RAISE(ABORT, 'disaster recovery drill is immutable'); END;

UPDATE schema_meta SET value = '24' WHERE key = 'version';
