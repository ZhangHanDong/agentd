use agentd_core::ports::SecurityError;
use agentd_core::types::{WorkerId, WorkerIncarnationId, WorkloadRole};
use agentd_store::SqliteStore;
use agentd_store::enrollment_repo::{WorkerWorkloadEnrollment, enroll_worker_workload_identity};
use agentd_store::security_repo::{WorkloadIdentityBindingCreate, get_workload_identity_binding};
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use serde_json::json;

const ROLLOUT_ID: &str = "ir_01ARZ3NDEKTSV4RRFFQ69G5FAV";
const SECOND_ROLLOUT_ID: &str = "ir_01ARZ3NDEKTSV4RRFFQ69G5FAW";

async fn store() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    sqlx::query(
        "INSERT INTO enterprise_worker_image_rollouts \
         (rollout_id, image_digest, signature_bundle_sha256, policy_sha256, \
          required_zones_json, declaration_sha256, status, declared_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 'declared', 1, 1)",
    )
    .bind(ROLLOUT_ID)
    .bind(format!("sha256:{}", "a".repeat(64)))
    .bind("b".repeat(64))
    .bind("c".repeat(64))
    .bind(json!(["zone-a"]).to_string())
    .bind("d".repeat(64))
    .execute(store.pool())
    .await
    .expect("rollout");
    sqlx::query(
        "INSERT INTO enterprise_zone_pool_policies \
         (pool_id, region, zone, resource_class, trust_domain, rollout_id, \
          minimum_replicas, maximum_replicas, target_queue_per_slot, \
          scale_down_cooldown_seconds, enabled, policy_sha256, updated_at) \
         VALUES ('zp_01ARZ3NDEKTSV4RRFFQ69G5FAV', 'cn-east-1', 'zone-a', \
                 'restricted', 'agents.example', ?, 1, 10, 1, 60, 1, ?, 1)",
    )
    .bind(ROLLOUT_ID)
    .bind("f".repeat(64))
    .execute(store.pool())
    .await
    .expect("zone policy");
    (store, dir)
}

fn enrollment(
    worker_id: WorkerId,
    incarnation_id: WorkerIncarnationId,
    certificate_sha256: &str,
    created_at: i64,
) -> WorkerWorkloadEnrollment {
    WorkerWorkloadEnrollment {
        worker: WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "agents.example".to_string(),
            labels: json!({
                "pool": "restricted",
                "agentd_attestation": {
                    "rollout_id": ROLLOUT_ID,
                    "image_digest": format!("sha256:{}", "a".repeat(64)),
                    "signature_bundle_sha256": "b".repeat(64),
                    "signature_policy_sha256": "c".repeat(64),
                    "region": "cn-east-1",
                    "zone": "zone-a",
                    "resource_class": "restricted"
                }
            }),
        },
        incarnation: WorkerRegistration {
            id: incarnation_id.clone(),
            daemon_version: "0.0.0-ad-e7".to_string(),
            host_name: "worker-0".to_string(),
            network_zone: Some("zone-a".to_string()),
            capabilities: json!({"runtime": ["codex"]}),
        },
        binding: WorkloadIdentityBindingCreate {
            certificate_sha256: certificate_sha256.to_string(),
            spiffe_uri: format!("spiffe://agents.example/worker/{incarnation_id}"),
            role: WorkloadRole::Worker,
            trust_domain: "agents.example".to_string(),
            worker_id: Some(worker_id),
            worker_incarnation_id: Some(incarnation_id),
            not_before: 1_700_000_000,
            not_after: 1_900_000_000,
            created_at,
        },
    }
}

#[tokio::test]
async fn worker_workload_enrollment_is_atomic_and_idempotent() {
    let (store, _dir) = store().await;
    let worker_id = WorkerId::new();
    let incarnation_id = WorkerIncarnationId::new();
    let fingerprint = "a".repeat(64);

    let first = enroll_worker_workload_identity(
        store.pool(),
        enrollment(
            worker_id.clone(),
            incarnation_id.clone(),
            &fingerprint,
            1_800_000_000,
        ),
    )
    .await
    .expect("first enrollment");
    let replay = enroll_worker_workload_identity(
        store.pool(),
        enrollment(
            worker_id.clone(),
            incarnation_id.clone(),
            &fingerprint,
            1_800_000_100,
        ),
    )
    .await
    .expect("idempotent replay");

    assert_eq!(first.worker.id, worker_id);
    assert_eq!(first.incarnation.id, incarnation_id);
    assert_eq!(first.identity, replay.identity);
    assert_eq!(replay.identity.binding.created_at, 1_800_000_000);
    assert!(replay.incarnation.is_current);

    let second_worker_id = WorkerId::new();
    let error = enroll_worker_workload_identity(
        store.pool(),
        enrollment(
            second_worker_id.clone(),
            WorkerIncarnationId::new(),
            &fingerprint,
            1_800_000_200,
        ),
    )
    .await
    .expect_err("fingerprint reuse must fail");
    assert!(matches!(error, SecurityError::Invalid(_)), "got {error:?}");
    assert!(
        worker_repo::get_worker(store.pool(), &second_worker_id)
            .await
            .expect("worker lookup")
            .is_none(),
        "failed enrollment must not leave a worker row"
    );
    assert!(
        get_workload_identity_binding(store.pool(), &fingerprint)
            .await
            .expect("binding lookup")
            .is_some()
    );
}

#[tokio::test]
async fn new_incarnation_atomically_replaces_current_attestation_labels() {
    let (store, _dir) = store().await;
    let worker_id = WorkerId::new();
    let first_incarnation = WorkerIncarnationId::new();
    let first = enrollment(
        worker_id.clone(),
        first_incarnation.clone(),
        &"a".repeat(64),
        1_800_000_000,
    );
    enroll_worker_workload_identity(store.pool(), first)
        .await
        .expect("first enrollment");

    sqlx::query(
        "INSERT INTO enterprise_worker_image_rollouts \
         (rollout_id, image_digest, signature_bundle_sha256, policy_sha256, \
          required_zones_json, declaration_sha256, status, declared_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 'declared', 2, 2)",
    )
    .bind(SECOND_ROLLOUT_ID)
    .bind(format!("sha256:{}", "e".repeat(64)))
    .bind("b".repeat(64))
    .bind("c".repeat(64))
    .bind(json!(["zone-a"]).to_string())
    .bind("e".repeat(64))
    .execute(store.pool())
    .await
    .expect("replacement rollout");
    sqlx::query(
        "UPDATE enterprise_zone_pool_policies SET rollout_id = ?, updated_at = 2 \
         WHERE pool_id = 'zp_01ARZ3NDEKTSV4RRFFQ69G5FAV'",
    )
    .bind(SECOND_ROLLOUT_ID)
    .execute(store.pool())
    .await
    .expect("replacement zone policy");

    let second_incarnation = WorkerIncarnationId::new();
    let mut second = enrollment(
        worker_id.clone(),
        second_incarnation.clone(),
        &"b".repeat(64),
        1_800_000_100,
    );
    second.worker.labels["agentd_attestation"]["rollout_id"] = json!(SECOND_ROLLOUT_ID);
    second.worker.labels["agentd_attestation"]["image_digest"] =
        json!(format!("sha256:{}", "e".repeat(64)));
    let expected_labels = second.worker.labels.clone();
    let replaced = enroll_worker_workload_identity(store.pool(), second)
        .await
        .expect("replacement enrollment");

    assert_eq!(replaced.worker.labels, expected_labels);
    assert!(replaced.incarnation.is_current);
    assert!(
        !worker_repo::get_incarnation(store.pool(), &first_incarnation)
            .await
            .expect("old incarnation")
            .expect("old incarnation record")
            .is_current
    );
}
