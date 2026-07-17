use std::collections::BTreeSet;

use agentd_core::ports::{
    CapacityObservation, ControlPlaneHeartbeatRequest, ControlPlaneLeadershipRequest,
    ControlPlaneMember, ControlPlaneMemberStatus, EnterpriseScalePort, LegalHold,
    RetentionDisposition, RetentionPolicy, WorkerImageRollout, WorkerImageRolloutStatus,
    WorkerImageZoneObservation, ZonePoolPolicy,
};
use agentd_core::types::{
    ControlPlaneInstanceId, LegalHoldId, WorkerImageRolloutId, ZonePoolId,
};
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
async fn rollout_and_autoscaling_are_zone_and_policy_bounded() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect(&directory.path().join("agentd.db"))
        .await
        .unwrap();
    let scale = SqliteEnterpriseScaleControlPlane::new(store.pool().clone());
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
    scale.declare_worker_image_rollout(&rollout).await.unwrap();
    for zone in ["cn-east-a", "cn-east-b"] {
        let current = scale
            .observe_worker_image_zone(&WorkerImageZoneObservation {
                rollout_id: rollout_id.clone(),
                zone: zone.to_string(),
                observed_image_digest: rollout.image_digest.clone(),
                signature_verified: true,
                ready_workers: 2,
                desired_workers: 2,
                observed_at: 110,
            })
            .await
            .unwrap();
        if zone == "cn-east-b" {
            assert_eq!(WorkerImageRolloutStatus::Healthy, current.status);
        }
    }
    let pool_id = ZonePoolId::from_string("zp_01ARZ3NDEKTSV4RRFFQ69G5FAV");
    scale
        .upsert_zone_pool(&ZonePoolPolicy {
            pool_id: pool_id.clone(),
            region: "cn-east".to_string(),
            zone: "cn-east-a".to_string(),
            resource_class: "general".to_string(),
            trust_domain: "factory.example".to_string(),
            rollout_id,
            minimum_replicas: 2,
            maximum_replicas: 20,
            target_queue_per_slot: 2,
            scale_down_cooldown_seconds: 300,
            enabled: true,
            policy_sha256: digest('e'),
            updated_at: 120,
        })
        .await
        .unwrap();
    let recommendation = scale
        .recommend_capacity(&CapacityObservation {
            pool_id,
            queue_depth: 40,
            running_tasks: 4,
            ready_replicas: 2,
            total_slots: 4,
            available_slots: 0,
            last_scale_at: None,
            observed_at: 130,
        })
        .await
        .unwrap();
    assert_eq!(12, recommendation.desired_replicas);
    assert_eq!("queue_pressure", recommendation.reason_code);
}

#[tokio::test]
async fn legal_hold_precedes_expiry_and_replication() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect(&directory.path().join("agentd.db"))
        .await
        .unwrap();
    let scale = SqliteEnterpriseScaleControlPlane::new(store.pool().clone());
    let tenant = digest('1');
    let subject = digest('2');
    scale
        .set_retention_policy(&RetentionPolicy {
            tenant_scope_sha256: tenant.clone(),
            policy_version_sha256: digest('3'),
            artifact_retention_seconds: 10,
            transcript_retention_seconds: 10,
            audit_retention_seconds: 20,
            minimum_replica_regions: 2,
            updated_at: 1,
        })
        .await
        .unwrap();
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
    scale.place_legal_hold(&hold).await.unwrap();
    let held = scale
        .decide_retention(&tenant, "artifact", &subject, 1, 100)
        .await
        .unwrap();
    assert_eq!(RetentionDisposition::LegalHold, held.disposition);
    scale
        .release_legal_hold(&hold.legal_hold_id, 101)
        .await
        .unwrap();
    let pending = scale
        .decide_retention(&tenant, "artifact", &subject, 1, 102)
        .await
        .unwrap();
    assert_eq!(RetentionDisposition::ReplicationPending, pending.disposition);
}
