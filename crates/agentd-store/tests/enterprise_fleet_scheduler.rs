use std::collections::BTreeSet;
use std::sync::Arc;

use agentd_core::ports::{
    ArtifactUploadAckRequest, FleetCompletionReport, FleetDenialReason, FleetFailureReport,
    FleetHeartbeatRequest, FleetPullRequest, FleetQueueStatus, FleetReapRequest,
    FleetSchedulerError, FleetSchedulerPort, FleetSideEffectRequest, FleetSubmitRequest,
    FleetTaskRequirements, PolicyRevocationPort, SecurityError, WorkerAvailability,
};
use agentd_core::types::{
    ArtifactUploadId, AuthenticatedWorkload, AuthorityKey, CertificationGate,
    CertificationPolicyVersionRef, DataClassification, ExecutionArtifactId, FrozenSpecVersionRef,
    MatrixRoomRef, NodeId, OfflineRecoveryPolicy, OrganizationRef, PlacementPolicy,
    ProductWorkflowRef, ProjectExecutionSnapshot, ProjectExecutionSnapshotRef, ProjectRef,
    ProjectRoomBindingRef, ProtectedAction, QuotaPolicyVersionRef, RbacPolicyVersionRef,
    RepositoryBinding, RepositoryRef, RepositoryRole, RoomBinding, RoomBindingRole, RunId,
    SecurityCheckpoint, SecurityEpochRequest, SecurityEpochStatus, TaskRunId, TeamRef, WorkerId,
    WorkerIncarnationId, WorkerStatus, WorkloadRole,
};
use agentd_store::fleet_scheduler::SqliteFleetScheduler;
use agentd_store::security_repo::{
    WorkloadIdentityBindingCreate, bind_workload_identity, revoke_workload_identity,
};
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, run_repo, task_repo};
use serde_json::json;
use sqlx::Row;

const ROLLOUT_ID: &str = "ir_01ARZ3NDEKTSV4RRFFQ69G5FAV";

#[derive(Debug)]
struct CurrentEpoch;

#[async_trait::async_trait]
impl PolicyRevocationPort for CurrentEpoch {
    async fn check_security_epoch(
        &self,
        request: &SecurityEpochRequest,
    ) -> Result<SecurityEpochStatus, SecurityError> {
        Ok(SecurityEpochStatus {
            checkpoint: request.checkpoint,
            organization_ref: request.organization_ref.clone(),
            project_ref: request.project_ref.clone(),
            execution_snapshot_ref: request.execution_snapshot_ref.clone(),
            current_epoch: request.pinned_epoch,
            observed_at: request.observed_at,
        })
    }
}

#[tokio::test]
async fn heartbeat_must_match_operator_enrolled_image_attestation() {
    let fixture = fixture().await;
    let labels = json!({
        "agentd_attestation": {
            "rollout_id": ROLLOUT_ID,
            "image_digest": format!("sha256:{}", "d".repeat(64)),
            "signature_bundle_sha256": "e".repeat(64),
            "signature_policy_sha256": "f".repeat(64),
            "region": "eu-west-1",
            "zone": "zone-a",
            "resource_class": "standard"
        }
    });
    sqlx::query("UPDATE workers SET labels_json = ? WHERE id = ?")
        .bind(serde_json::to_string(&labels).expect("labels"))
        .bind(fixture.worker_id.as_str())
        .execute(fixture._store.pool())
        .await
        .expect("attestation labels");

    fixture
        .scheduler
        .heartbeat(&heartbeat(&fixture, 1, 160))
        .await
        .expect("matching heartbeat");
    let mut mismatch = heartbeat(&fixture, 2, 170);
    mismatch.availability.zone = "zone-b".to_string();
    assert_eq!(
        fixture.scheduler.heartbeat(&mismatch).await,
        Err(FleetSchedulerError::Denied(
            FleetDenialReason::PlacementDenied
        ))
    );
    sqlx::query("UPDATE workers SET labels_json = '{}' WHERE id = ?")
        .bind(fixture.worker_id.as_str())
        .execute(fixture._store.pool())
        .await
        .expect("remove attestation labels");
    assert_eq!(
        fixture
            .scheduler
            .heartbeat(&heartbeat(&fixture, 3, 180))
            .await,
        Err(FleetSchedulerError::Denied(
            FleetDenialReason::PlacementDenied
        ))
    );
    sqlx::query("UPDATE workers SET labels_json = ? WHERE id = ?")
        .bind(serde_json::to_string(&labels).expect("labels"))
        .bind(fixture.worker_id.as_str())
        .execute(fixture._store.pool())
        .await
        .expect("restore attestation labels");
    sqlx::query(
        "UPDATE enterprise_worker_image_rollouts SET status = 'rolled_back', updated_at = 185 \
         WHERE rollout_id = ?",
    )
    .bind(ROLLOUT_ID)
    .execute(fixture._store.pool())
    .await
    .expect("roll back rollout");
    assert_eq!(
        fixture
            .scheduler
            .heartbeat(&heartbeat(&fixture, 4, 190))
            .await,
        Err(FleetSchedulerError::Denied(
            FleetDenialReason::PlacementDenied
        ))
    );
}

#[tokio::test]
async fn pull_rejects_an_overlong_worker_selected_lease() {
    let fixture = fixture().await;
    fixture
        .scheduler
        .heartbeat(&heartbeat(&fixture, 1, 160))
        .await
        .expect("heartbeat");

    assert!(matches!(
        fixture.scheduler.pull(&pull(&fixture, 170, 471)).await,
        Err(FleetSchedulerError::Invalid(_))
    ));
}

struct Fixture {
    _store: SqliteStore,
    _dir: tempfile::TempDir,
    scheduler: SqliteFleetScheduler,
    task_id: TaskRunId,
    worker_id: WorkerId,
    incarnation_id: WorkerIncarnationId,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    sqlx::query(
        "INSERT INTO enterprise_worker_image_rollouts \
         (rollout_id, image_digest, signature_bundle_sha256, policy_sha256, \
          required_zones_json, declaration_sha256, status, declared_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 'declared', 1, 1)",
    )
    .bind(ROLLOUT_ID)
    .bind(format!("sha256:{}", "d".repeat(64)))
    .bind("e".repeat(64))
    .bind("f".repeat(64))
    .bind(json!(["zone-a"]).to_string())
    .bind("a".repeat(64))
    .execute(store.pool())
    .await
    .expect("rollout");
    sqlx::query(
        "INSERT INTO enterprise_zone_pool_policies \
         (pool_id, region, zone, resource_class, trust_domain, rollout_id, \
          minimum_replicas, maximum_replicas, target_queue_per_slot, \
          scale_down_cooldown_seconds, enabled, policy_sha256, updated_at) \
         VALUES ('zp_01ARZ3NDEKTSV4RRFFQ69G5FAV', 'eu-west-1', 'zone-a', \
                 'standard', 'workers.example', ?, 1, 10, 1, 60, 1, ?, 1)",
    )
    .bind(ROLLOUT_ID)
    .bind("b".repeat(64))
    .execute(store.pool())
    .await
    .expect("zone policy");
    let run_id = RunId::new();
    run_repo::insert_run(store.pool(), &run_id, "fleet-workflow")
        .await
        .expect("run");
    let task_id = task_repo::insert_task_run(store.pool(), &run_id, &NodeId::parsed("impl"))
        .await
        .expect("task");
    let worker_id = WorkerId::new();
    worker_repo::create_worker(
        store.pool(),
        WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "workers.example".to_string(),
            labels: json!({
                "fleet": "enterprise",
                "agentd_attestation": {
                    "rollout_id": ROLLOUT_ID,
                    "image_digest": format!("sha256:{}", "d".repeat(64)),
                    "signature_bundle_sha256": "e".repeat(64),
                    "signature_policy_sha256": "f".repeat(64),
                    "region": "eu-west-1",
                    "zone": "zone-a",
                    "resource_class": "standard"
                }
            }),
        },
    )
    .await
    .expect("worker");
    let incarnation_id = WorkerIncarnationId::new();
    worker_repo::register_incarnation(
        store.pool(),
        &worker_id,
        WorkerRegistration {
            id: incarnation_id.clone(),
            daemon_version: "0.0.0-ad-e2".to_string(),
            host_name: "fleet-host-a".to_string(),
            network_zone: Some("zone-a".to_string()),
            capabilities: json!({"runtime": ["codex"]}),
        },
    )
    .await
    .expect("incarnation");
    bind_workload_identity(
        store.pool(),
        WorkloadIdentityBindingCreate {
            certificate_sha256: "c".repeat(64),
            spiffe_uri: format!("spiffe://workers.example/worker/{incarnation_id}"),
            role: WorkloadRole::Worker,
            trust_domain: "workers.example".to_string(),
            worker_id: Some(worker_id.clone()),
            worker_incarnation_id: Some(incarnation_id.clone()),
            not_before: 100,
            not_after: 1_000,
            created_at: 100,
        },
    )
    .await
    .expect("identity binding");
    let scheduler = SqliteFleetScheduler::new(store.pool().clone(), Arc::new(CurrentEpoch));
    Fixture {
        _store: store,
        _dir: dir,
        scheduler,
        task_id,
        worker_id,
        incarnation_id,
    }
}

#[tokio::test]
async fn revoked_identity_is_offlined_and_cannot_commit_with_cached_authentication() {
    let fixture = fixture().await;
    fixture
        .scheduler
        .heartbeat(&heartbeat(&fixture, 1, 160))
        .await
        .expect("initial heartbeat");

    revoke_workload_identity(
        fixture._store.pool(),
        &"c".repeat(64),
        165,
        "operator_revocation",
    )
    .await
    .expect("revoke identity");

    let availability = sqlx::query(
        "SELECT worker_status, available_slots FROM enterprise_worker_availability \
         WHERE worker_incarnation_id = ?",
    )
    .bind(fixture.incarnation_id.as_str())
    .fetch_one(fixture._store.pool())
    .await
    .expect("availability");
    assert_eq!(availability.get::<String, _>("worker_status"), "offline");
    assert_eq!(availability.get::<i64, _>("available_slots"), 0);

    assert_eq!(
        fixture
            .scheduler
            .heartbeat(&heartbeat(&fixture, 2, 170))
            .await,
        Err(FleetSchedulerError::Denied(
            FleetDenialReason::IdentityMismatch
        ))
    );
}

fn authority() -> AuthorityKey {
    AuthorityKey::new("specify:fleet-test").expect("authority")
}

fn snapshot() -> ProjectExecutionSnapshot {
    let authority = authority();
    let project = ProjectRef::new(authority.clone(), "project-a", "2").expect("project");
    let rbac = RbacPolicyVersionRef::new(authority.clone(), "rbac-a", "3").expect("rbac");
    ProjectExecutionSnapshot {
        snapshot_ref: ProjectExecutionSnapshotRef::new(authority.clone(), "snapshot-a", "4")
            .expect("snapshot"),
        authority_key: authority.clone(),
        authority_revision: 4,
        organization_ref: OrganizationRef::new(authority.clone(), "org-a", "1").expect("org"),
        team_refs: vec![TeamRef::new(authority.clone(), "team-a", "1").expect("team")],
        project_ref: project.clone(),
        repository_bindings: vec![RepositoryBinding {
            repository_ref: RepositoryRef::new(authority.clone(), "repo-a", "1").expect("repo"),
            role: RepositoryRole::Target,
            forge_locator: Some("github:org/repo".to_string()),
            base_commit: "a".repeat(40),
        }],
        room_bindings: vec![RoomBinding {
            binding_ref: ProjectRoomBindingRef::new(authority.clone(), "room-binding-a", "1")
                .expect("binding"),
            project_ref: project,
            matrix_room_ref: MatrixRoomRef::new(
                AuthorityKey::new("matrix:fleet-test").expect("matrix authority"),
                "!room:example",
                "1",
            )
            .expect("room"),
            roles: vec![RoomBindingRole::Command],
            allowed_command_classes: vec!["execute".to_string()],
            rbac_policy_version_ref: rbac.clone(),
        }],
        issue_ref: None,
        requirement_refs: Vec::new(),
        frozen_spec_version_ref: FrozenSpecVersionRef::new(authority.clone(), "spec-a", "1")
            .expect("spec"),
        product_workflow_ref: ProductWorkflowRef::new(authority.clone(), "workflow-a", "1")
            .expect("workflow"),
        rbac_policy_version_ref: rbac,
        quota_policy_version_ref: QuotaPolicyVersionRef::new(authority.clone(), "quota-a", "1")
            .expect("quota"),
        certification_policy_version_ref: Some(
            CertificationPolicyVersionRef::new(authority, "cert-a", "1").expect("cert"),
        ),
        certification_gate: CertificationGate::Machine,
        skill_packages: Vec::new(),
        placement_policy: PlacementPolicy {
            data_classification: DataClassification::Restricted,
            allowed_regions: BTreeSet::from(["eu-west-1".to_string()]),
            allowed_worker_trust_domains: BTreeSet::from(["workers.example".to_string()]),
            require_signed_image: true,
            require_dedicated_pool: true,
            egress_profile_id: "restricted-egress-v1".to_string(),
            tenant_cache_namespace: "org-a/project-a".to_string(),
        },
        policy_revocation_epoch: 9,
        issued_at: 100,
        valid_until: 1_000,
        content_sha256: "b".repeat(64),
        offline_recovery_policy: OfflineRecoveryPolicy::Deny,
    }
}

fn workload(fixture: &Fixture) -> AuthenticatedWorkload {
    AuthenticatedWorkload {
        spiffe_uri: format!("spiffe://workers.example/worker/{}", fixture.incarnation_id),
        role: WorkloadRole::Worker,
        trust_domain: "workers.example".to_string(),
        certificate_sha256: "c".repeat(64),
        not_before: 100,
        not_after: 1_000,
        worker_id: Some(fixture.worker_id.clone()),
        worker_incarnation_id: Some(fixture.incarnation_id.clone()),
    }
}

fn submit(fixture: &Fixture, max_attempts: u32) -> FleetSubmitRequest {
    FleetSubmitRequest {
        idempotency_key: format!("submit:{}", fixture.task_id),
        execution_task_id: fixture.task_id.clone(),
        snapshot: snapshot(),
        requirements: FleetTaskRequirements {
            resource_class: "standard".to_string(),
            required_capabilities: BTreeSet::from(["runtime:codex".to_string()]),
            quota_max_active: 2,
            priority: 10,
            max_attempts,
        },
        submitted_at: 150,
    }
}

fn heartbeat(fixture: &Fixture, sequence: u64, observed_at: i64) -> FleetHeartbeatRequest {
    FleetHeartbeatRequest {
        workload: workload(fixture),
        availability: WorkerAvailability {
            worker_id: fixture.worker_id.clone(),
            worker_incarnation_id: fixture.incarnation_id.clone(),
            heartbeat_sequence: sequence,
            worker_status: WorkerStatus::Online,
            daemon_version: "0.0.0-ad-e2".to_string(),
            protocol_min: 1,
            protocol_max: 2,
            region: "eu-west-1".to_string(),
            zone: "zone-a".to_string(),
            resource_class: "standard".to_string(),
            capabilities: BTreeSet::from(["runtime:codex".to_string()]),
            total_slots: 2,
            available_slots: 2,
            data_classifications: BTreeSet::from([DataClassification::Restricted]),
            image_digest: format!("sha256:{}", "d".repeat(64)),
            image_signature_verified: true,
            dedicated_pool: true,
            egress_profile_ids: BTreeSet::from(["restricted-egress-v1".to_string()]),
            tenant_cache_namespaces: BTreeSet::from(["org-a/project-a".to_string()]),
        },
        observed_at,
    }
}

fn pull(fixture: &Fixture, observed_at: i64, expires_at: i64) -> FleetPullRequest {
    FleetPullRequest {
        workload: workload(fixture),
        protocol_version: 1,
        observed_at,
        heartbeat_max_age_seconds: 60,
        lease_expires_at: expires_at,
    }
}

#[tokio::test]
async fn queue_pull_lease_outbox_reports_and_artifact_ack_are_fenced_and_idempotent() {
    let fixture = fixture().await;
    let submitted = fixture
        .scheduler
        .submit_task(&submit(&fixture, 3))
        .await
        .expect("submit");
    assert_eq!(submitted.status, FleetQueueStatus::Queued);
    let duplicate = fixture
        .scheduler
        .submit_task(&submit(&fixture, 3))
        .await
        .expect("idempotent submit");
    assert_eq!(duplicate, submitted);

    fixture
        .scheduler
        .heartbeat(&heartbeat(&fixture, 1, 160))
        .await
        .expect("heartbeat");
    let assignment = fixture
        .scheduler
        .pull(&pull(&fixture, 170, 300))
        .await
        .expect("pull")
        .expect("assignment");
    assert_eq!(assignment.task.status, FleetQueueStatus::Acquired);
    assert_eq!(
        assignment.task.current_claim,
        Some(assignment.lease.claim())
    );

    let artifact = ArtifactUploadAckRequest {
        workload: workload(&fixture),
        claim: assignment.lease.claim(),
        upload_id: ArtifactUploadId::new(),
        execution_artifact_id: ExecutionArtifactId::new(),
        idempotency_key: "artifact-upload-1".to_string(),
        artifact_sha256: "e".repeat(64),
        upload_attempt: 1,
        part_count: 2,
        observed_at: 180,
    };
    let first_ack = fixture
        .scheduler
        .acknowledge_artifact_upload(&artifact)
        .await
        .expect("artifact ack");
    let duplicate_ack = fixture
        .scheduler
        .acknowledge_artifact_upload(&artifact)
        .await
        .expect("duplicate artifact ack");
    assert_eq!(duplicate_ack, first_ack);

    fixture
        .scheduler
        .admit_side_effect(&FleetSideEffectRequest {
            workload: workload(&fixture),
            claim: assignment.lease.claim(),
            checkpoint: SecurityCheckpoint::Delivery,
            action: ProtectedAction::ForgeWrite,
            idempotency_key: "delivery-1".to_string(),
            observed_at: 190,
        })
        .await
        .expect("side effect");
    let completion = FleetCompletionReport {
        workload: workload(&fixture),
        claim: assignment.lease.claim(),
        idempotency_key: "completion-1".to_string(),
        outcome_sha256: "f".repeat(64),
        observed_at: 200,
    };
    let completed = fixture
        .scheduler
        .complete(&completion)
        .await
        .expect("complete");
    assert_eq!(completed.status, FleetQueueStatus::Completed);
    assert_eq!(
        fixture
            .scheduler
            .complete(&completion)
            .await
            .expect("duplicate completion"),
        completed
    );
    assert!(
        fixture
            .scheduler
            .outbox_after(None, 100)
            .await
            .expect("outbox")
            .len()
            >= 5
    );
}

#[tokio::test]
async fn retry_reassignment_reaper_and_dead_letter_never_reuse_fencing_tokens() {
    let fixture = fixture().await;
    fixture
        .scheduler
        .submit_task(&submit(&fixture, 2))
        .await
        .expect("submit");
    fixture
        .scheduler
        .heartbeat(&heartbeat(&fixture, 1, 160))
        .await
        .expect("heartbeat");
    let first = fixture
        .scheduler
        .pull(&pull(&fixture, 170, 180))
        .await
        .expect("pull")
        .expect("first assignment");
    let retry = fixture
        .scheduler
        .fail(&FleetFailureReport {
            workload: workload(&fixture),
            claim: first.lease.claim(),
            idempotency_key: "failure-1".to_string(),
            failure_code: "provider_unavailable".to_string(),
            retryable: true,
            observed_at: 175,
        })
        .await
        .expect("retry");
    assert_eq!(retry.status, FleetQueueStatus::RetryWait);
    let eligible_at = retry.next_eligible_at.expect("retry time");
    fixture
        .scheduler
        .heartbeat(&heartbeat(&fixture, 2, eligible_at))
        .await
        .expect("heartbeat 2");
    let second = fixture
        .scheduler
        .pull(&pull(&fixture, eligible_at, eligible_at + 5))
        .await
        .expect("repull")
        .expect("second assignment");
    assert!(second.lease.fencing_token > first.lease.fencing_token);

    let summary = fixture
        .scheduler
        .reap(&FleetReapRequest {
            observed_at: eligible_at + 10,
            heartbeat_stale_before: eligible_at + 1,
            lease_expired_before: eligible_at + 10,
        })
        .await
        .expect("reap");
    assert_eq!(summary.tasks_dead_lettered, 1);
    let explain = fixture
        .scheduler
        .explain(&fixture.task_id)
        .await
        .expect("explain")
        .expect("task");
    assert_eq!(explain.status, FleetQueueStatus::DeadLetter);
    assert_eq!(explain.block_code.as_deref(), Some("worker_or_lease_stale"));
}

#[tokio::test]
async fn enterprise_scheduler_schema_contains_no_compatibility_or_sensitive_payload_fields() {
    let migration = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("migrations/0018_enterprise_fleet_scheduler.sql"),
    )
    .expect("migration");
    let lower = migration.to_ascii_lowercase();
    for forbidden in [
        "agent_scheduler_queue references",
        "tmux",
        "matrix_room_id",
        "transcript",
        "workdir",
        "secret_bytes",
        "raw_error",
    ] {
        assert!(!lower.contains(forbidden), "migration contains {forbidden}");
    }
}
