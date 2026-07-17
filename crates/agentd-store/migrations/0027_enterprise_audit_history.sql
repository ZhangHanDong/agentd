-- AD-E7 corrective migration: retain every mutable policy/observation version
-- and make audit-relevant enterprise ledgers non-destructive.

CREATE TABLE enterprise_worker_image_zone_observation_history (
    sequence                  INTEGER PRIMARY KEY AUTOINCREMENT,
    rollout_id                TEXT NOT NULL REFERENCES enterprise_worker_image_rollouts(rollout_id) ON DELETE RESTRICT,
    zone                      TEXT NOT NULL,
    observed_image_digest     TEXT NOT NULL,
    signature_verified        INTEGER NOT NULL CHECK (signature_verified IN (0, 1)),
    ready_workers             INTEGER NOT NULL CHECK (ready_workers >= 0),
    desired_workers           INTEGER NOT NULL CHECK (desired_workers > 0),
    observed_at               INTEGER NOT NULL,
    observation_sha256        TEXT NOT NULL UNIQUE CHECK (length(observation_sha256) = 64 AND observation_sha256 NOT GLOB '*[^0123456789abcdef]*')
);

INSERT INTO enterprise_worker_image_zone_observation_history
    (rollout_id, zone, observed_image_digest, signature_verified, ready_workers,
     desired_workers, observed_at, observation_sha256)
SELECT rollout_id, zone, observed_image_digest, signature_verified, ready_workers,
       desired_workers, observed_at, observation_sha256
FROM enterprise_worker_image_zone_observations;

CREATE INDEX idx_enterprise_rollout_observation_history
    ON enterprise_worker_image_zone_observation_history(rollout_id, zone, sequence);

CREATE TABLE enterprise_worker_image_rollout_rollbacks (
    sequence          INTEGER PRIMARY KEY AUTOINCREMENT,
    rollout_id        TEXT NOT NULL REFERENCES enterprise_worker_image_rollouts(rollout_id) ON DELETE RESTRICT,
    previous_status   TEXT NOT NULL CHECK (previous_status IN ('declared', 'progressing', 'healthy', 'degraded')),
    reason_sha256     TEXT NOT NULL CHECK (length(reason_sha256) = 64 AND reason_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    rollback_sha256   TEXT NOT NULL UNIQUE CHECK (length(rollback_sha256) = 64 AND rollback_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    rolled_back_at    INTEGER NOT NULL,
    UNIQUE (rollout_id)
);

CREATE TABLE enterprise_zone_pool_policy_versions (
    sequence                         INTEGER PRIMARY KEY AUTOINCREMENT,
    pool_id                          TEXT NOT NULL REFERENCES enterprise_zone_pool_policies(pool_id) ON DELETE RESTRICT,
    region                           TEXT NOT NULL,
    zone                             TEXT NOT NULL,
    resource_class                   TEXT NOT NULL,
    trust_domain                     TEXT NOT NULL,
    rollout_id                       TEXT NOT NULL REFERENCES enterprise_worker_image_rollouts(rollout_id) ON DELETE RESTRICT,
    minimum_replicas                 INTEGER NOT NULL CHECK (minimum_replicas > 0),
    maximum_replicas                 INTEGER NOT NULL CHECK (maximum_replicas >= minimum_replicas),
    target_queue_per_slot            INTEGER NOT NULL CHECK (target_queue_per_slot > 0),
    scale_down_cooldown_seconds      INTEGER NOT NULL CHECK (scale_down_cooldown_seconds > 0),
    enabled                          INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    policy_sha256                    TEXT NOT NULL CHECK (length(policy_sha256) = 64 AND policy_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    policy_record_sha256             TEXT NOT NULL CHECK (length(policy_record_sha256) = 64 AND policy_record_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    updated_at                       INTEGER NOT NULL,
    UNIQUE (pool_id, policy_sha256)
);

INSERT INTO enterprise_zone_pool_policy_versions
    (pool_id, region, zone, resource_class, trust_domain, rollout_id,
     minimum_replicas, maximum_replicas, target_queue_per_slot,
     scale_down_cooldown_seconds, enabled, policy_sha256,
     policy_record_sha256, updated_at)
SELECT pool_id, region, zone, resource_class, trust_domain, rollout_id,
       minimum_replicas, maximum_replicas, target_queue_per_slot,
       scale_down_cooldown_seconds, enabled, policy_sha256,
       policy_sha256, updated_at
FROM enterprise_zone_pool_policies;

CREATE INDEX idx_enterprise_zone_pool_policy_versions
    ON enterprise_zone_pool_policy_versions(pool_id, sequence);

CREATE TABLE enterprise_retention_policy_versions (
    sequence                       INTEGER PRIMARY KEY AUTOINCREMENT,
    tenant_scope_sha256            TEXT NOT NULL,
    policy_version_sha256          TEXT NOT NULL,
    policy_record_sha256           TEXT NOT NULL CHECK (length(policy_record_sha256) = 64 AND policy_record_sha256 NOT GLOB '*[^0123456789abcdef]*'),
    artifact_retention_seconds     INTEGER NOT NULL CHECK (artifact_retention_seconds > 0),
    transcript_retention_seconds   INTEGER NOT NULL CHECK (transcript_retention_seconds > 0),
    audit_retention_seconds        INTEGER NOT NULL CHECK (audit_retention_seconds >= artifact_retention_seconds),
    minimum_replica_regions        INTEGER NOT NULL CHECK (minimum_replica_regions > 0),
    updated_at                     INTEGER NOT NULL,
    UNIQUE (tenant_scope_sha256, policy_version_sha256)
);

INSERT INTO enterprise_retention_policy_versions
    (tenant_scope_sha256, policy_version_sha256, policy_record_sha256,
     artifact_retention_seconds, transcript_retention_seconds,
     audit_retention_seconds, minimum_replica_regions, updated_at)
SELECT tenant_scope_sha256, policy_version_sha256, policy_version_sha256,
       artifact_retention_seconds, transcript_retention_seconds,
       audit_retention_seconds, minimum_replica_regions, updated_at
FROM enterprise_retention_policies;

CREATE INDEX idx_enterprise_retention_policy_versions
    ON enterprise_retention_policy_versions(tenant_scope_sha256, sequence);

-- Schema 24 allowed transcript retention to exceed audit retention. Preserve
-- those immutable legacy versions during backfill, but reject every new
-- non-compliant history record after this trigger exists.
CREATE TRIGGER trg_enterprise_retention_policy_versions_audit_floor_insert
BEFORE INSERT ON enterprise_retention_policy_versions
WHEN NEW.audit_retention_seconds < NEW.transcript_retention_seconds
BEGIN SELECT RAISE(ABORT, 'audit retention cannot be shorter than transcript retention'); END;

CREATE TRIGGER trg_enterprise_rollout_observation_history_no_update
BEFORE UPDATE ON enterprise_worker_image_zone_observation_history
BEGIN SELECT RAISE(ABORT, 'enterprise rollout observation history is immutable'); END;

CREATE TRIGGER trg_enterprise_rollout_observation_history_no_delete
BEFORE DELETE ON enterprise_worker_image_zone_observation_history
BEGIN SELECT RAISE(ABORT, 'enterprise rollout observation history is immutable'); END;

CREATE TRIGGER trg_enterprise_worker_image_rollout_rollbacks_no_update
BEFORE UPDATE ON enterprise_worker_image_rollout_rollbacks
BEGIN SELECT RAISE(ABORT, 'enterprise rollout rollback history is immutable'); END;

CREATE TRIGGER trg_enterprise_worker_image_rollout_rollbacks_no_delete
BEFORE DELETE ON enterprise_worker_image_rollout_rollbacks
BEGIN SELECT RAISE(ABORT, 'enterprise rollout rollback history is immutable'); END;

CREATE TRIGGER trg_enterprise_zone_pool_policy_versions_no_update
BEFORE UPDATE ON enterprise_zone_pool_policy_versions
BEGIN SELECT RAISE(ABORT, 'enterprise zone pool policy history is immutable'); END;

CREATE TRIGGER trg_enterprise_zone_pool_policy_versions_no_delete
BEFORE DELETE ON enterprise_zone_pool_policy_versions
BEGIN SELECT RAISE(ABORT, 'enterprise zone pool policy history is immutable'); END;

CREATE TRIGGER trg_enterprise_retention_policy_versions_no_update
BEFORE UPDATE ON enterprise_retention_policy_versions
BEGIN SELECT RAISE(ABORT, 'enterprise retention policy history is immutable'); END;

CREATE TRIGGER trg_enterprise_retention_policy_versions_no_delete
BEFORE DELETE ON enterprise_retention_policy_versions
BEGIN SELECT RAISE(ABORT, 'enterprise retention policy history is immutable'); END;

CREATE TRIGGER trg_enterprise_scale_receipts_no_update
BEFORE UPDATE ON enterprise_scale_receipts
BEGIN SELECT RAISE(ABORT, 'enterprise scale receipts are immutable'); END;

CREATE TRIGGER trg_enterprise_scale_receipts_no_delete
BEFORE DELETE ON enterprise_scale_receipts
BEGIN SELECT RAISE(ABORT, 'enterprise scale receipts are immutable'); END;

CREATE TRIGGER trg_enterprise_worker_image_rollouts_no_delete
BEFORE DELETE ON enterprise_worker_image_rollouts
BEGIN SELECT RAISE(ABORT, 'enterprise worker image rollouts are immutable'); END;

CREATE TRIGGER trg_enterprise_worker_image_rollouts_immutable_fields
BEFORE UPDATE OF rollout_id, image_digest, signature_bundle_sha256, policy_sha256,
                 required_zones_json, declaration_sha256, declared_at
ON enterprise_worker_image_rollouts
BEGIN SELECT RAISE(ABORT, 'enterprise worker image rollout identity is immutable'); END;

CREATE TRIGGER trg_enterprise_worker_image_zone_observations_no_delete
BEFORE DELETE ON enterprise_worker_image_zone_observations
BEGIN SELECT RAISE(ABORT, 'enterprise worker image observations cannot be deleted'); END;

CREATE TRIGGER trg_enterprise_zone_pool_policies_no_delete
BEFORE DELETE ON enterprise_zone_pool_policies
BEGIN SELECT RAISE(ABORT, 'enterprise zone pool policies cannot be deleted'); END;

CREATE TRIGGER trg_enterprise_autoscaling_recommendations_no_update
BEFORE UPDATE ON enterprise_autoscaling_recommendations
BEGIN SELECT RAISE(ABORT, 'enterprise autoscaling recommendations are immutable'); END;

CREATE TRIGGER trg_enterprise_autoscaling_recommendations_no_delete
BEFORE DELETE ON enterprise_autoscaling_recommendations
BEGIN SELECT RAISE(ABORT, 'enterprise autoscaling recommendations are immutable'); END;

CREATE TRIGGER trg_enterprise_tenant_keys_immutable_fields
BEFORE UPDATE OF tenant_key_id, tenant_scope_sha256, region, kms_key_ref_sha256,
                 key_version_ref_sha256, registration_sha256, activated_at
ON enterprise_tenant_keys
BEGIN SELECT RAISE(ABORT, 'enterprise tenant key identity is immutable'); END;

CREATE TRIGGER trg_enterprise_tenant_keys_no_delete
BEFORE DELETE ON enterprise_tenant_keys
BEGIN SELECT RAISE(ABORT, 'enterprise tenant keys cannot be deleted'); END;

CREATE TRIGGER trg_enterprise_replication_plans_no_delete
BEFORE DELETE ON enterprise_artifact_replication_plans
BEGIN SELECT RAISE(ABORT, 'enterprise replication plans are immutable'); END;

CREATE TRIGGER trg_enterprise_replication_plans_no_update
BEFORE UPDATE ON enterprise_artifact_replication_plans
BEGIN SELECT RAISE(ABORT, 'enterprise replication plans are immutable'); END;

CREATE TRIGGER trg_enterprise_replication_plans_artifact_insert
BEFORE INSERT ON enterprise_artifact_replication_plans
WHEN NOT EXISTS (
    SELECT 1 FROM execution_artifacts
    WHERE id = NEW.execution_artifact_id
      AND content_sha256 = NEW.artifact_sha256
)
BEGIN SELECT RAISE(ABORT, 'enterprise replication plan requires its exact execution artifact'); END;

CREATE TRIGGER trg_enterprise_replica_ack_immutable_fields
BEFORE UPDATE OF replication_id, region, artifact_sha256
ON enterprise_artifact_replica_acknowledgements
BEGIN SELECT RAISE(ABORT, 'enterprise artifact replica identity is immutable'); END;

CREATE TRIGGER trg_enterprise_replica_ack_no_delete
BEFORE DELETE ON enterprise_artifact_replica_acknowledgements
BEGIN SELECT RAISE(ABORT, 'enterprise artifact replica acknowledgements cannot be deleted'); END;

CREATE TRIGGER trg_enterprise_retention_policies_no_delete
BEFORE DELETE ON enterprise_retention_policies
BEGIN SELECT RAISE(ABORT, 'enterprise retention policies cannot be deleted'); END;

CREATE TRIGGER trg_enterprise_retention_policy_audit_floor_insert
BEFORE INSERT ON enterprise_retention_policies
WHEN NEW.audit_retention_seconds < NEW.transcript_retention_seconds
BEGIN SELECT RAISE(ABORT, 'audit retention cannot be shorter than transcript retention'); END;

CREATE TRIGGER trg_enterprise_retention_policy_audit_floor_update
BEFORE UPDATE ON enterprise_retention_policies
WHEN NEW.audit_retention_seconds < NEW.transcript_retention_seconds
BEGIN SELECT RAISE(ABORT, 'audit retention cannot be shorter than transcript retention'); END;

CREATE TRIGGER trg_enterprise_legal_holds_immutable_fields
BEFORE UPDATE OF legal_hold_id, tenant_scope_sha256, subject_kind, subject_sha256,
                 reason_sha256, hold_sha256, placed_at
ON enterprise_legal_holds
BEGIN SELECT RAISE(ABORT, 'enterprise legal hold identity is immutable'); END;

CREATE TRIGGER trg_enterprise_legal_holds_no_delete
BEFORE DELETE ON enterprise_legal_holds
BEGIN SELECT RAISE(ABORT, 'enterprise legal holds cannot be deleted'); END;

CREATE TRIGGER trg_enterprise_dr_checkpoints_no_delete
BEFORE DELETE ON enterprise_dr_checkpoints
BEGIN SELECT RAISE(ABORT, 'enterprise disaster recovery checkpoints are immutable'); END;

CREATE TRIGGER trg_enterprise_dr_checkpoints_no_update
BEFORE UPDATE ON enterprise_dr_checkpoints
BEGIN SELECT RAISE(ABORT, 'enterprise disaster recovery checkpoints are immutable'); END;

CREATE TRIGGER trg_enterprise_dr_drills_no_delete
BEFORE DELETE ON enterprise_dr_drills
BEGIN SELECT RAISE(ABORT, 'enterprise disaster recovery drills are immutable'); END;

CREATE TRIGGER trg_enterprise_dr_drills_no_update
BEFORE UPDATE ON enterprise_dr_drills
BEGIN SELECT RAISE(ABORT, 'enterprise disaster recovery drills are immutable'); END;

CREATE TRIGGER trg_enterprise_load_models_no_update
BEFORE UPDATE ON enterprise_load_models
BEGIN SELECT RAISE(ABORT, 'enterprise load models are immutable'); END;

CREATE TRIGGER trg_enterprise_load_models_no_delete
BEFORE DELETE ON enterprise_load_models
BEGIN SELECT RAISE(ABORT, 'enterprise load models are immutable'); END;

CREATE TRIGGER trg_enterprise_service_level_measurements_no_update
BEFORE UPDATE ON enterprise_service_level_measurements
BEGIN SELECT RAISE(ABORT, 'enterprise service-level measurements are immutable'); END;

CREATE TRIGGER trg_enterprise_service_level_measurements_no_delete
BEFORE DELETE ON enterprise_service_level_measurements
BEGIN SELECT RAISE(ABORT, 'enterprise service-level measurements are immutable'); END;

UPDATE schema_meta SET value = '27' WHERE key = 'version';
