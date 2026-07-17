use std::collections::BTreeSet;

use agentd_core::ports::{
    ArtifactReplicaAcknowledgement, ArtifactReplicationPlan, CapacityObservation,
    ControlPlaneHeartbeatRequest, ControlPlaneLeadershipRequest, ControlPlaneMember,
    ControlPlaneMemberStatus, DisasterRecoveryCheckpoint, DisasterRecoveryDrill,
    DisasterRecoveryDrillStatus, EnterpriseMutationFence, EnterpriseScalePort, LegalHold,
    ReplicaStatus, RetentionDisposition, RetentionPolicy, TenantKeyStatus, TenantKeyTransition,
    TenantKeyVersion, WorkerImageRollback, WorkerImageRollout, WorkerImageRolloutStatus,
    WorkerImageZoneObservation, ZonePoolPolicy,
};
use agentd_core::types::{
    ArtifactReplicationId, ControlPlaneInstanceId, DisasterRecoveryCheckpointId,
    DisasterRecoveryDrillId, ExecutionArtifactId, LegalHoldId, TenantKeyId, WorkerImageRolloutId,
    ZonePoolId,
};
use agentd_store::operator::{DoctorStatus, run_doctor};
use agentd_store::{SqliteEnterpriseScaleControlPlane, SqliteStore};

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn instance(value: &str, sequence: u64, observed_at: i64) -> ControlPlaneHeartbeatRequest {
    ControlPlaneHeartbeatRequest {
        idempotency_key: format!("heartbeat-{value}-{sequence}"),
        member: ControlPlaneMember {
            instance_id: ControlPlaneInstanceId::from_string(value),
            heartbeat_sequence: sequence,
            region: "cn-east".to_string(),
            zone: "cn-east-a".to_string(),
            daemon_version: "0.0.0-test".to_string(),
            endpoint_sha256: digest('a'),
            status: ControlPlaneMemberStatus::Ready,
            started_at: 1,
            observed_at,
        },
    }
}

async fn mutation_fence(scale: &SqliteEnterpriseScaleControlPlane) -> EnterpriseMutationFence {
    let instance_id = "ci_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    scale
        .heartbeat_control_plane(&instance(instance_id, 1, 80))
        .await
        .unwrap();
    let lease = scale
        .acquire_leadership(&ControlPlaneLeadershipRequest {
            instance_id: ControlPlaneInstanceId::from_string(instance_id),
            idempotency_key: "test-mutation-leader".to_string(),
            observed_at: 80,
            expires_at: 300,
        })
        .await
        .unwrap();
    EnterpriseMutationFence {
        instance_id: lease.instance_id,
        term: lease.term,
        fencing_token: lease.fencing_token,
        observed_at: 90,
    }
}

#[tokio::test]
async fn enterprise_store_rejects_untyped_external_ids() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect(&directory.path().join("agentd.db"))
        .await
        .unwrap();
    let scale = SqliteEnterpriseScaleControlPlane::new(store.pool().clone());

    let error = scale
        .heartbeat_control_plane(&instance("not-a-control-plane-id", 1, 100))
        .await
        .expect_err("untyped control-plane id");
    assert!(matches!(
        error,
        agentd_core::ports::EnterpriseScaleError::Invalid(_)
    ));

    let fence = mutation_fence(&scale).await;
    let error = scale
        .declare_worker_image_rollout(
            &fence,
            &WorkerImageRollout {
                rollout_id: WorkerImageRolloutId::from_string("not-a-rollout-id"),
                image_digest: format!("sha256:{}", digest('a')),
                signature_bundle_sha256: digest('b'),
                policy_sha256: digest('c'),
                required_zones: BTreeSet::from(["cn-east-a".to_string()]),
                status: WorkerImageRolloutStatus::Declared,
                declared_at: 100,
                updated_at: 100,
            },
        )
        .await
        .expect_err("untyped rollout id");
    assert!(matches!(
        error,
        agentd_core::ports::EnterpriseScaleError::Invalid(_)
    ));
}

#[tokio::test]
async fn stable_control_plane_identity_resumes_sequence_without_location_drift() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect(&directory.path().join("agentd.db"))
        .await
        .unwrap();
    let scale = SqliteEnterpriseScaleControlPlane::new(store.pool().clone());
    let instance_id = "ci_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    scale
        .heartbeat_control_plane(&instance(instance_id, 1, 100))
        .await
        .unwrap();

    let mut restarted = instance(instance_id, 2, 120);
    restarted.member.started_at = 110;
    restarted.member.daemon_version = "0.0.1-test".to_string();
    let durable = scale.heartbeat_control_plane(&restarted).await.unwrap();
    assert_eq!(2, durable.heartbeat_sequence);
    assert_eq!(110, durable.started_at);

    let mut moved = instance(instance_id, 3, 130);
    moved.member.started_at = 110;
    moved.member.zone = "cn-east-b".to_string();
    assert!(scale.heartbeat_control_plane(&moved).await.is_err());

    let mut rewound = instance(instance_id, 3, 105);
    rewound.member.started_at = 105;
    assert!(scale.heartbeat_control_plane(&rewound).await.is_err());
}

#[tokio::test]
async fn leadership_fences_expired_control_plane_instances() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect(&directory.path().join("agentd.db"))
        .await
        .unwrap();
    let scale = SqliteEnterpriseScaleControlPlane::new(store.pool().clone());
    let first_id = "ci_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let second_id = "ci_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    scale
        .heartbeat_control_plane(&instance(first_id, 1, 100))
        .await
        .unwrap();
    scale
        .heartbeat_control_plane(&instance(second_id, 1, 105))
        .await
        .unwrap();
    let first = scale
        .acquire_leadership(&ControlPlaneLeadershipRequest {
            instance_id: ControlPlaneInstanceId::from_string(first_id),
            idempotency_key: "leader-first".to_string(),
            observed_at: 100,
            expires_at: 120,
        })
        .await
        .unwrap();
    assert_eq!(1, first.term);
    assert!(
        scale
            .acquire_leadership(&ControlPlaneLeadershipRequest {
                instance_id: ControlPlaneInstanceId::from_string(second_id),
                idempotency_key: "leader-second-early".to_string(),
                observed_at: 110,
                expires_at: 130,
            })
            .await
            .is_err()
    );
    let second = scale
        .acquire_leadership(&ControlPlaneLeadershipRequest {
            instance_id: ControlPlaneInstanceId::from_string(second_id),
            idempotency_key: "leader-second".to_string(),
            observed_at: 121,
            expires_at: 141,
        })
        .await
        .unwrap();
    assert_eq!(2, second.term);
    assert_eq!(2, second.fencing_token);
}

#[tokio::test]
async fn stale_leader_cannot_commit_enterprise_mutations() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect(&directory.path().join("agentd.db"))
        .await
        .unwrap();
    let scale = SqliteEnterpriseScaleControlPlane::new(store.pool().clone());
    let stale_fence = mutation_fence(&scale).await;
    let second_id = "ci_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    scale
        .heartbeat_control_plane(&instance(second_id, 1, 1_001))
        .await
        .unwrap();
    scale
        .acquire_leadership(&ControlPlaneLeadershipRequest {
            instance_id: ControlPlaneInstanceId::from_string(second_id),
            idempotency_key: "replacement-leader".to_string(),
            observed_at: 1_001,
            expires_at: 1_200,
        })
        .await
        .unwrap();

    let error = scale
        .set_retention_policy(
            &stale_fence,
            &RetentionPolicy {
                tenant_scope_sha256: digest('d'),
                policy_version_sha256: digest('e'),
                artifact_retention_seconds: 10,
                transcript_retention_seconds: 10,
                audit_retention_seconds: 20,
                minimum_replica_regions: 1,
                updated_at: 1_002,
            },
        )
        .await
        .expect_err("stale leader mutation");
    assert!(
        matches!(error, agentd_core::ports::EnterpriseScaleError::Denied(_)),
        "got {error:?}"
    );
}

#[tokio::test]
async fn offline_leader_cannot_commit_before_its_lease_expires() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect(&directory.path().join("agentd.db"))
        .await
        .unwrap();
    let scale = SqliteEnterpriseScaleControlPlane::new(store.pool().clone());
    let fence = mutation_fence(&scale).await;
    let mut offline = instance(fence.instance_id.as_str(), 2, 91);
    offline.member.status = ControlPlaneMemberStatus::Offline;
    scale.heartbeat_control_plane(&offline).await.unwrap();

    let policy = RetentionPolicy {
        tenant_scope_sha256: digest('1'),
        policy_version_sha256: digest('2'),
        artifact_retention_seconds: 60,
        transcript_retention_seconds: 60,
        audit_retention_seconds: 120,
        minimum_replica_regions: 1,
        updated_at: 92,
    };
    let mut current_fence = fence;
    current_fence.observed_at = 92;
    assert!(
        scale
            .set_retention_policy(&current_fence, &policy)
            .await
            .is_err()
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn rollout_and_autoscaling_are_zone_and_policy_bounded() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect(&directory.path().join("agentd.db"))
        .await
        .unwrap();
    let scale = SqliteEnterpriseScaleControlPlane::new(store.pool().clone());
    let fence = mutation_fence(&scale).await;
    let rollout_id = WorkerImageRolloutId::from_string("ir_01ARZ3NDEKTSV4RRFFQ69G5FAV");
    let rollout = WorkerImageRollout {
        rollout_id: rollout_id.clone(),
        image_digest: format!("sha256:{}", digest('b')),
        signature_bundle_sha256: digest('c'),
        policy_sha256: digest('d'),
        required_zones: BTreeSet::from(["cn-east-a".to_string(), "cn-east-b".to_string()]),
        status: WorkerImageRolloutStatus::Declared,
        declared_at: 100,
        updated_at: 100,
    };
    scale
        .declare_worker_image_rollout(&fence, &rollout)
        .await
        .unwrap();
    for zone in ["cn-east-a", "cn-east-b"] {
        let current = scale
            .observe_worker_image_zone(
                &fence,
                &WorkerImageZoneObservation {
                    rollout_id: rollout_id.clone(),
                    zone: zone.to_string(),
                    observed_image_digest: rollout.image_digest.clone(),
                    signature_verified: true,
                    ready_workers: 2,
                    desired_workers: 2,
                    observed_at: 110,
                },
            )
            .await
            .unwrap();
        if zone == "cn-east-b" {
            assert_eq!(WorkerImageRolloutStatus::Healthy, current.status);
        }
    }
    let pool_id = ZonePoolId::from_string("zp_01ARZ3NDEKTSV4RRFFQ69G5FAV");
    let policy_v1 = ZonePoolPolicy {
        pool_id: pool_id.clone(),
        region: "cn-east".to_string(),
        zone: "cn-east-a".to_string(),
        resource_class: "general".to_string(),
        trust_domain: "factory.example".to_string(),
        rollout_id: rollout_id.clone(),
        minimum_replicas: 2,
        maximum_replicas: 20,
        target_queue_per_slot: 2,
        scale_down_cooldown_seconds: 300,
        enabled: true,
        policy_sha256: digest('e'),
        updated_at: 120,
    };
    scale.upsert_zone_pool(&fence, &policy_v1).await.unwrap();
    let policy_v2 = ZonePoolPolicy {
        maximum_replicas: 30,
        policy_sha256: digest('f'),
        updated_at: 125,
        ..policy_v1.clone()
    };
    scale.upsert_zone_pool(&fence, &policy_v2).await.unwrap();
    assert_eq!(
        policy_v2,
        scale.upsert_zone_pool(&fence, &policy_v1).await.unwrap()
    );
    let recommendation = scale
        .recommend_capacity(
            &fence,
            &CapacityObservation {
                pool_id,
                queue_depth: 40,
                running_tasks: 4,
                ready_replicas: 2,
                total_slots: 4,
                available_slots: 0,
                last_scale_at: None,
                observed_at: 130,
            },
        )
        .await
        .unwrap();
    assert_eq!(12, recommendation.desired_replicas);
    assert_eq!("queue_pressure", recommendation.reason_code);

    let rollback = WorkerImageRollback {
        rollout_id: rollout_id.clone(),
        reason_sha256: digest('9'),
        rolled_back_at: 140,
    };
    sqlx::query(
        "INSERT INTO workers (id, status, trust_domain, labels_json, created_at, updated_at) \
         VALUES ('wk_01ARZ3NDEKTSV4RRFFQ69G5FAV', 'online', 'factory.example', \
                 json_object('agentd_attestation', json_object('rollout_id', ?)), 130, 130)",
    )
    .bind(rollout_id.as_str())
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO worker_incarnations \
         (id, worker_id, daemon_version, host_name, capabilities_json, is_current, \
          registered_at, last_seen_at) \
         VALUES ('wi_01ARZ3NDEKTSV4RRFFQ69G5FAV', 'wk_01ARZ3NDEKTSV4RRFFQ69G5FAV', \
                 '0.0.0-test', 'worker-a', '{}', 1, 130, 130)",
    )
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO enterprise_worker_availability \
         (worker_incarnation_id, worker_id, heartbeat_sequence, worker_status, daemon_version, \
          protocol_min, protocol_max, region, zone, resource_class, capabilities_json, \
          total_slots, available_slots, data_classifications_json, image_digest, \
          image_signature_verified, dedicated_pool, egress_profile_ids_json, \
          tenant_cache_namespaces_json, observed_at, updated_at) \
         VALUES ('wi_01ARZ3NDEKTSV4RRFFQ69G5FAV', 'wk_01ARZ3NDEKTSV4RRFFQ69G5FAV', \
                 1, 'online', '0.0.0-test', 1, 1, 'cn-east', 'cn-east-a', 'general', \
                 '[]', 2, 2, '[\"internal\"]', ?, 1, 0, '[]', '[]', 130, 130)",
    )
    .bind(&rollout.image_digest)
    .execute(store.pool())
    .await
    .unwrap();
    assert_eq!(
        WorkerImageRolloutStatus::RolledBack,
        scale
            .rollback_worker_image_rollout(&fence, &rollback)
            .await
            .unwrap()
            .status
    );
    let worker_state = sqlx::query_as::<_, (String, i64, String)>(
        "SELECT availability.worker_status, availability.available_slots, worker.status \
         FROM enterprise_worker_availability AS availability \
         JOIN workers AS worker ON worker.id = availability.worker_id \
         WHERE availability.worker_incarnation_id = 'wi_01ARZ3NDEKTSV4RRFFQ69G5FAV'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(
        ("offline".to_string(), 0, "offline".to_string()),
        worker_state
    );
    assert_eq!(
        WorkerImageRolloutStatus::RolledBack,
        scale
            .rollback_worker_image_rollout(&fence, &rollback)
            .await
            .unwrap()
            .status
    );
    assert_eq!(
        WorkerImageRolloutStatus::RolledBack,
        scale
            .observe_worker_image_zone(
                &fence,
                &WorkerImageZoneObservation {
                    rollout_id: rollout_id.clone(),
                    zone: "cn-east-a".to_string(),
                    observed_image_digest: rollout.image_digest.clone(),
                    signature_verified: true,
                    ready_workers: 2,
                    desired_workers: 2,
                    observed_at: 110,
                }
            )
            .await
            .unwrap()
            .status
    );
    assert_eq!(
        policy_v2,
        scale.upsert_zone_pool(&fence, &policy_v1).await.unwrap()
    );
    assert!(
        scale
            .observe_worker_image_zone(
                &fence,
                &WorkerImageZoneObservation {
                    rollout_id,
                    zone: "cn-east-a".to_string(),
                    observed_image_digest: rollout.image_digest,
                    signature_verified: true,
                    ready_workers: 2,
                    desired_workers: 2,
                    observed_at: 150,
                }
            )
            .await
            .is_err()
    );
    sqlx::query(
        "UPDATE enterprise_zone_pool_policies SET maximum_replicas = maximum_replicas + 1 \
         WHERE pool_id = ?",
    )
    .bind(policy_v2.pool_id.as_str())
    .execute(store.pool())
    .await
    .unwrap();
    let doctor = run_doctor(store.pool(), 151).await.unwrap();
    let history_check = doctor
        .checks
        .iter()
        .find(|check| check.name == "enterprise_audit_history")
        .unwrap();
    assert_eq!(DoctorStatus::Fail, history_check.status);
    assert_eq!(Some(1), history_check.count);
    for statement in [
        "UPDATE enterprise_worker_image_zone_observation_history SET observed_at = observed_at + 1",
        "DELETE FROM enterprise_worker_image_zone_observation_history",
        "UPDATE enterprise_worker_image_rollout_rollbacks SET rolled_back_at = rolled_back_at + 1",
        "DELETE FROM enterprise_worker_image_rollout_rollbacks",
        "UPDATE enterprise_zone_pool_policy_versions SET updated_at = updated_at + 1",
        "DELETE FROM enterprise_zone_pool_policy_versions",
        "UPDATE enterprise_worker_image_rollouts SET image_digest = image_digest",
        "DELETE FROM enterprise_worker_image_rollouts",
        "DELETE FROM enterprise_zone_pool_policies",
    ] {
        assert!(
            sqlx::query(statement).execute(store.pool()).await.is_err(),
            "enterprise rollout history accepted mutation: {statement}"
        );
    }
}

#[tokio::test]
async fn legal_hold_precedes_expiry_and_replication() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect(&directory.path().join("agentd.db"))
        .await
        .unwrap();
    let scale = SqliteEnterpriseScaleControlPlane::new(store.pool().clone());
    let fence = mutation_fence(&scale).await;
    let tenant = digest('1');
    let subject = digest('2');
    let retention_v1 = RetentionPolicy {
        tenant_scope_sha256: tenant.clone(),
        policy_version_sha256: digest('3'),
        artifact_retention_seconds: 10,
        transcript_retention_seconds: 10,
        audit_retention_seconds: 20,
        minimum_replica_regions: 2,
        updated_at: 1,
    };
    scale
        .set_retention_policy(&fence, &retention_v1)
        .await
        .unwrap();
    let retention_v2 = RetentionPolicy {
        policy_version_sha256: digest('5'),
        audit_retention_seconds: 30,
        updated_at: 2,
        ..retention_v1.clone()
    };
    scale
        .set_retention_policy(&fence, &retention_v2)
        .await
        .unwrap();
    assert_eq!(
        retention_v2,
        scale
            .set_retention_policy(&fence, &retention_v1)
            .await
            .unwrap()
    );
    let hold = LegalHold {
        legal_hold_id: LegalHoldId::from_string("lh_01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        tenant_scope_sha256: tenant.clone(),
        subject_kind: "artifact".to_string(),
        subject_sha256: subject.clone(),
        reason_sha256: digest('4'),
        active: true,
        placed_at: 5,
        released_at: None,
    };
    scale.place_legal_hold(&fence, &hold).await.unwrap();
    let held_decision = scale
        .decide_retention(&tenant, "artifact", &subject, 1, 100)
        .await
        .unwrap();
    assert_eq!(RetentionDisposition::LegalHold, held_decision.disposition);
    scale
        .release_legal_hold(&fence, &hold.legal_hold_id, 101)
        .await
        .unwrap();
    let pending = scale
        .decide_retention(&tenant, "artifact", &subject, 1, 102)
        .await
        .unwrap();
    assert_eq!(
        RetentionDisposition::ReplicationPending,
        pending.disposition
    );
    for statement in [
        "UPDATE enterprise_retention_policy_versions SET updated_at = updated_at + 1",
        "DELETE FROM enterprise_retention_policy_versions",
        "DELETE FROM enterprise_retention_policies",
        "DELETE FROM enterprise_legal_holds",
    ] {
        assert!(
            sqlx::query(statement).execute(store.pool()).await.is_err(),
            "enterprise retention history accepted mutation: {statement}"
        );
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn tenant_key_rotation_and_replica_retry_preserve_state_transitions() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect(&directory.path().join("agentd.db"))
        .await
        .unwrap();
    let scale = SqliteEnterpriseScaleControlPlane::new(store.pool().clone());
    let fence = mutation_fence(&scale).await;
    let tenant = digest('5');
    let old_key_id = TenantKeyId::from_string("tk_01ARZ3NDEKTSV4RRFFQ69G5FAV");
    let old_key = TenantKeyVersion {
        tenant_key_id: old_key_id.clone(),
        tenant_scope_sha256: tenant.clone(),
        region: "cn-east".to_string(),
        kms_key_ref_sha256: digest('6'),
        key_version_ref_sha256: digest('7'),
        status: TenantKeyStatus::Active,
        activated_at: 10,
        retired_at: None,
    };
    let mut bypassed_transition = old_key.clone();
    bypassed_transition.tenant_key_id = TenantKeyId::from_string("tk_01ARZ3NDEKTSV4RRFFQ69G5FAX");
    bypassed_transition.status = TenantKeyStatus::Retired;
    bypassed_transition.retired_at = Some(10);
    assert!(
        scale
            .register_tenant_key(&fence, &bypassed_transition)
            .await
            .is_err()
    );
    scale.register_tenant_key(&fence, &old_key).await.unwrap();
    let retiring = TenantKeyTransition {
        tenant_key_id: old_key_id.clone(),
        target_status: TenantKeyStatus::Retiring,
        transitioned_at: 20,
    };
    assert_eq!(
        TenantKeyStatus::Retiring,
        scale
            .transition_tenant_key(&fence, &retiring)
            .await
            .unwrap()
            .status
    );
    assert_eq!(
        TenantKeyStatus::Retiring,
        scale
            .transition_tenant_key(&fence, &retiring)
            .await
            .unwrap()
            .status
    );

    let new_key_id = TenantKeyId::from_string("tk_01ARZ3NDEKTSV4RRFFQ69G5FAW");
    scale
        .register_tenant_key(
            &fence,
            &TenantKeyVersion {
                tenant_key_id: new_key_id.clone(),
                tenant_scope_sha256: tenant.clone(),
                region: "cn-east".to_string(),
                kms_key_ref_sha256: digest('8'),
                key_version_ref_sha256: digest('9'),
                status: TenantKeyStatus::Active,
                activated_at: 21,
                retired_at: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        TenantKeyStatus::Retired,
        scale
            .transition_tenant_key(
                &fence,
                &TenantKeyTransition {
                    tenant_key_id: old_key_id.clone(),
                    target_status: TenantKeyStatus::Retired,
                    transitioned_at: 30,
                }
            )
            .await
            .unwrap()
            .status
    );
    assert_eq!(
        TenantKeyStatus::Retired,
        scale
            .transition_tenant_key(&fence, &retiring)
            .await
            .unwrap()
            .status
    );

    let replication_id = ArtifactReplicationId::from_string("rp_01ARZ3NDEKTSV4RRFFQ69G5FAV");
    let artifact_sha256 = digest('a');
    let replication_plan = ArtifactReplicationPlan {
        replication_id: replication_id.clone(),
        execution_artifact_id: ExecutionArtifactId::from_string("ar_01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        tenant_scope_sha256: tenant.clone(),
        artifact_sha256: artifact_sha256.clone(),
        source_region: "cn-north".to_string(),
        required_regions: BTreeSet::from(["cn-east".to_string(), "cn-north".to_string()]),
        created_at: 40,
    };
    assert!(
        scale
            .create_replication_plan(&fence, &replication_plan)
            .await
            .is_err()
    );
    sqlx::query(
        "INSERT INTO runs (id, workflow_sha, status, started_at, last_heartbeat) \
         VALUES ('r_scale_replication', 'scale', 'running', 1, 1)",
    )
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO execution_artifacts \
         (id, kind, content_sha256, size_bytes, media_type, storage_ref, provenance_json, \
          execution_run_id, snapshot_authority_key, snapshot_resource_kind, \
          snapshot_resource_id, snapshot_resource_version, snapshot_content_sha256, \
          target_repository_id, target_base_commit, created_at) \
         VALUES (?, 'log', ?, 0, 'application/octet-stream', 'object://scale-test', '{}', \
                 'r_scale_replication', 'specify:scale-test', 'execution_snapshot', \
                 'snapshot-scale', '1', ?, 'repository-scale', 'base', 39)",
    )
    .bind(replication_plan.execution_artifact_id.as_str())
    .bind(&artifact_sha256)
    .bind(digest('0'))
    .execute(store.pool())
    .await
    .unwrap();
    scale
        .create_replication_plan(&fence, &replication_plan)
        .await
        .unwrap();
    let pending = ArtifactReplicaAcknowledgement {
        replication_id: replication_id.clone(),
        region: "cn-east".to_string(),
        artifact_sha256: artifact_sha256.clone(),
        object_ref_sha256: digest('b'),
        tenant_key_id: new_key_id.clone(),
        status: ReplicaStatus::Pending,
        acknowledged_at: 41,
    };
    scale
        .acknowledge_artifact_replica(&fence, &pending)
        .await
        .unwrap();
    let available = ArtifactReplicaAcknowledgement {
        replication_id,
        region: "cn-east".to_string(),
        artifact_sha256,
        object_ref_sha256: digest('c'),
        tenant_key_id: new_key_id,
        status: ReplicaStatus::Available,
        acknowledged_at: 42,
    };
    assert_eq!(
        ReplicaStatus::Available,
        scale
            .acknowledge_artifact_replica(&fence, &available)
            .await
            .unwrap()
            .status
    );
    assert_eq!(
        ReplicaStatus::Available,
        scale
            .acknowledge_artifact_replica(&fence, &pending)
            .await
            .unwrap()
            .status
    );
    for statement in [
        "UPDATE enterprise_tenant_key_transitions SET transitioned_at = transitioned_at + 1",
        "DELETE FROM enterprise_tenant_key_transitions",
        "UPDATE enterprise_artifact_replica_transitions SET acknowledged_at = acknowledged_at + 1",
        "DELETE FROM enterprise_artifact_replica_transitions",
        "UPDATE enterprise_artifact_replication_plans SET created_at = created_at + 1",
        "UPDATE enterprise_mutation_fences SET observed_at = observed_at + 1",
        "DELETE FROM enterprise_mutation_fences",
    ] {
        assert!(
            sqlx::query(statement).execute(store.pool()).await.is_err(),
            "immutable enterprise ledger accepted: {statement}"
        );
    }
}

#[tokio::test]
async fn disaster_recovery_drills_bind_status_to_objectives_and_integrity() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect(&directory.path().join("agentd.db"))
        .await
        .unwrap();
    let scale = SqliteEnterpriseScaleControlPlane::new(store.pool().clone());
    let fence = mutation_fence(&scale).await;
    let checkpoint = DisasterRecoveryCheckpoint {
        checkpoint_id: DisasterRecoveryCheckpointId::from_string("dr_01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        region: "cn-east".to_string(),
        database_sha256: digest('1'),
        artifact_index_sha256: digest('2'),
        audit_head_sha256: digest('3'),
        matrix_cursor_sha256: digest('4'),
        certification_head_sha256: digest('5'),
        maximum_rpo_seconds: 60,
        maximum_rto_seconds: 300,
        created_at: 100,
    };
    scale
        .record_dr_checkpoint(&fence, &checkpoint)
        .await
        .unwrap();

    let passed = DisasterRecoveryDrill {
        drill_id: DisasterRecoveryDrillId::from_string("dd_01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        checkpoint_id: checkpoint.checkpoint_id.clone(),
        recovery_region: "cn-north".to_string(),
        measured_rpo_seconds: 60,
        measured_rto_seconds: 300,
        lease_fencing_verified: true,
        accepted_state_verified: true,
        status: DisasterRecoveryDrillStatus::Passed,
        evidence_sha256: digest('6'),
        completed_at: 400,
    };
    assert_eq!(
        passed,
        scale.record_dr_drill(&fence, &passed).await.unwrap()
    );

    let failed = DisasterRecoveryDrill {
        drill_id: DisasterRecoveryDrillId::from_string("dd_01ARZ3NDEKTSV4RRFFQ69G5FAW"),
        lease_fencing_verified: false,
        status: DisasterRecoveryDrillStatus::Failed,
        evidence_sha256: digest('7'),
        ..passed.clone()
    };
    assert_eq!(
        failed,
        scale.record_dr_drill(&fence, &failed).await.unwrap()
    );

    let contradictory = DisasterRecoveryDrill {
        drill_id: DisasterRecoveryDrillId::from_string("dd_01ARZ3NDEKTSV4RRFFQ69G5FAX"),
        status: DisasterRecoveryDrillStatus::Passed,
        evidence_sha256: digest('8'),
        ..failed
    };
    assert!(scale.record_dr_drill(&fence, &contradictory).await.is_err());

    let predating = DisasterRecoveryDrill {
        drill_id: DisasterRecoveryDrillId::from_string("dd_01ARZ3NDEKTSV4RRFFQ69G5FAY"),
        lease_fencing_verified: true,
        accepted_state_verified: true,
        status: DisasterRecoveryDrillStatus::Passed,
        evidence_sha256: digest('9'),
        completed_at: 99,
        ..passed
    };
    assert!(scale.record_dr_drill(&fence, &predating).await.is_err());
    for statement in [
        "UPDATE enterprise_dr_checkpoints SET created_at = created_at + 1",
        "DELETE FROM enterprise_dr_checkpoints",
        "UPDATE enterprise_dr_drills SET completed_at = completed_at + 1",
        "DELETE FROM enterprise_dr_drills",
    ] {
        assert!(
            sqlx::query(statement).execute(store.pool()).await.is_err(),
            "immutable disaster recovery ledger accepted: {statement}"
        );
    }
}
