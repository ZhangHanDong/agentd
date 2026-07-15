use std::sync::atomic::{AtomicUsize, Ordering};

use agentd_core::ports::{
    AttemptCapabilityPort, AuditPage, AuditReadRequest, ExecutionAuditAppend, ExecutionAuditPort,
    ExecutionAuditRecord, ExecutionEvidenceError, SecurityError, TaskLeaseCloseRequest,
    TaskLeaseDispatchRequest, TaskLeasePort,
};
use agentd_core::types::{
    AuthenticatedWorkload, AuthorityKey, CapabilityIssueRequest, CapabilityValidationRequest,
    NodeId, OrganizationRef, ProjectExecutionSnapshotRef, ProjectRef, ProtectedAction,
    ProtectedResource, ProtectedResourceKind, RbacPolicyVersionRef, RunId, SecurityAuditContext,
    SecurityDenialReason, TaskRunId, TenantAuthorization, WorkerId, WorkerIncarnationId,
    WorkloadRole,
};
use agentd_store::execution_evidence_control_plane::SqliteExecutionEvidenceControlPlane;
use agentd_store::security_repo::{self, SqliteAttemptCapabilityRepository};
use agentd_store::task_lease_control_plane::SqliteTaskLeaseControlPlane;
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, run_repo, task_repo};
use serde_json::json;
use sqlx::Row;

struct Fixture {
    store: SqliteStore,
    _dir: tempfile::TempDir,
    run_id: RunId,
    task_id: TaskRunId,
    worker_id: WorkerId,
    incarnation_id: WorkerIncarnationId,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let run_id = RunId::new();
    run_repo::insert_run(store.pool(), &run_id, "security-workflow")
        .await
        .expect("run");
    let task_id = task_repo::insert_task_run(store.pool(), &run_id, &NodeId::parsed("secure"))
        .await
        .expect("task");
    let worker_id = WorkerId::new();
    worker_repo::create_worker(
        store.pool(),
        WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "agents.example".to_string(),
            labels: json!({"zone": "security-test"}),
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
            daemon_version: "0.0.0-ad-e1".to_string(),
            host_name: "security-host".to_string(),
            network_zone: Some("test".to_string()),
            capabilities: json!({"sandbox": "oci"}),
        },
    )
    .await
    .expect("incarnation");
    Fixture {
        store,
        _dir: dir,
        run_id,
        task_id,
        worker_id,
        incarnation_id,
    }
}

fn authority_key() -> AuthorityKey {
    AuthorityKey::new("specify:security-test").expect("authority")
}

fn organization(id: &str) -> OrganizationRef {
    OrganizationRef::new(authority_key(), id, "4").expect("organization")
}

fn project(id: &str) -> ProjectRef {
    ProjectRef::new(authority_key(), id, "9").expect("project")
}

fn snapshot(id: &str) -> ProjectExecutionSnapshotRef {
    ProjectExecutionSnapshotRef::new(authority_key(), id, "12").expect("snapshot")
}

async fn dispatch(
    fixture: &Fixture,
    observed_at: i64,
    expires_at: i64,
) -> agentd_core::types::TaskLeaseGrant {
    SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone())
        .dispatch(&TaskLeaseDispatchRequest {
            execution_task_id: fixture.task_id.clone(),
            worker_incarnation_id: fixture.incarnation_id.clone(),
            observed_at,
            expires_at,
        })
        .await
        .expect("dispatch")
}

fn workload(fixture: &Fixture) -> AuthenticatedWorkload {
    AuthenticatedWorkload {
        spiffe_uri: format!("spiffe://agents.example/worker/{}", fixture.incarnation_id),
        role: WorkloadRole::Worker,
        trust_domain: "agents.example".to_string(),
        certificate_sha256: "a".repeat(64),
        not_before: 50,
        not_after: 500,
        worker_id: Some(fixture.worker_id.clone()),
        worker_incarnation_id: Some(fixture.incarnation_id.clone()),
    }
}

fn scope(
    fixture: &Fixture,
    claim: agentd_core::types::TaskLeaseClaim,
    project_id: &str,
    epoch: u64,
) -> agentd_core::types::ExecutionSecurityScope {
    agentd_core::types::ExecutionSecurityScope {
        authority_key: authority_key(),
        organization_ref: organization("org-a"),
        project_ref: project(project_id),
        execution_snapshot_ref: snapshot("snapshot-a"),
        rbac_policy_version_ref: RbacPolicyVersionRef::new(authority_key(), "rbac-a", "3")
            .expect("rbac"),
        worker_incarnation_id: fixture.incarnation_id.clone(),
        task_lease_claim: claim,
        sandbox_profile_id: "oci-restricted-v1".to_string(),
        egress_profile_id: "deny-all-v1".to_string(),
        policy_revocation_epoch: epoch,
        valid_until: 450,
        audit_context: SecurityAuditContext {
            execution_run_id: fixture.run_id.clone(),
            snapshot_content_sha256: "b".repeat(64),
            target_repository_id: "repository-a".to_string(),
            target_base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        },
    }
}

fn resource(project_id: &str) -> ProtectedResource {
    ProtectedResource {
        organization_ref: organization("org-a"),
        project_ref: project(project_id),
        execution_snapshot_ref: snapshot("snapshot-a"),
        kind: ProtectedResourceKind::Artifact("artifact-output".to_string()),
    }
}

fn authorization(
    fixture: &Fixture,
    claim: agentd_core::types::TaskLeaseClaim,
    project_id: &str,
    epoch: u64,
    authorized_at: i64,
) -> TenantAuthorization {
    TenantAuthorization {
        workload: workload(fixture),
        scope: scope(fixture, claim, project_id, epoch),
        action: ProtectedAction::ArtifactWrite,
        resource: resource(project_id),
        authorized_at,
        expires_at: 400,
    }
}

fn repository(
    fixture: &Fixture,
) -> SqliteAttemptCapabilityRepository<
    SqliteTaskLeaseControlPlane,
    SqliteExecutionEvidenceControlPlane<SqliteTaskLeaseControlPlane>,
> {
    let lease = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());
    let audit = SqliteExecutionEvidenceControlPlane::new(
        fixture.store.pool().clone(),
        SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone()),
    );
    SqliteAttemptCapabilityRepository::new(fixture.store.pool().clone(), lease, audit)
}

async fn set_epoch(
    fixture: &Fixture,
    scope: &agentd_core::types::ExecutionSecurityScope,
    epoch: u64,
) {
    security_repo::set_policy_revocation_epoch(fixture.store.pool(), scope, epoch, 90)
        .await
        .expect("policy epoch");
}

async fn issue(
    fixture: &Fixture,
    claim: agentd_core::types::TaskLeaseClaim,
    expires_at: i64,
) -> (
    agentd_core::types::CapabilityToken,
    agentd_core::types::CapabilityAdmission,
) {
    let authorization = authorization(fixture, claim, "project-a", 11, 110);
    set_epoch(fixture, &authorization.scope, 11).await;
    repository(fixture)
        .issue_capability(&CapabilityIssueRequest {
            authorization,
            requested_expires_at: expires_at,
        })
        .await
        .expect("issue capability")
}

fn validation(
    token: agentd_core::types::CapabilityToken,
    admission: &agentd_core::types::CapabilityAdmission,
    observed_at: i64,
) -> CapabilityValidationRequest {
    CapabilityValidationRequest {
        token,
        scope: admission.scope.clone(),
        action: admission.action,
        resource: admission.resource.clone(),
        observed_at,
    }
}

fn assert_denied(error: &SecurityError, expected: SecurityDenialReason) {
    assert_eq!(error.denial_reason(), Some(expected), "got {error:?}");
}

#[tokio::test]
async fn attempt_capability_binds_exact_scope_and_persists_only_digest() {
    let fixture = fixture().await;
    let lease = dispatch(&fixture, 100, 300).await;
    let (token, admission) = issue(&fixture, lease.claim(), 250).await;

    assert_eq!(format!("{token:?}"), "CapabilityToken([REDACTED])");
    assert_eq!(admission.scope.task_lease_claim, lease.claim());
    assert_eq!(admission.scope.project_ref, project("project-a"));
    assert_eq!(admission.action, ProtectedAction::ArtifactWrite);
    assert_eq!(admission.expires_at, 250);

    let row = sqlx::query(
        "SELECT token_sha256, action, resource_json, lease_id, fencing_token, expires_at \
         FROM attempt_capabilities WHERE id = ?",
    )
    .bind(admission.id.as_str())
    .fetch_one(fixture.store.pool())
    .await
    .expect("capability row");
    let digest: String = row.get("token_sha256");
    assert_eq!(digest.len(), 64);
    assert_eq!(row.get::<String, _>("action"), "artifact.write");
    assert_eq!(row.get::<String, _>("lease_id"), lease.lease_id.to_string());
    assert_eq!(row.get::<i64, _>("fencing_token"), 1);
    assert_eq!(row.get::<i64, _>("expires_at"), 250);
    let resource_json: String = row.get("resource_json");
    assert!(!resource_json.contains("secret-value"));

    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('attempt_capabilities') ORDER BY cid",
    )
    .fetch_all(fixture.store.pool())
    .await
    .expect("capability columns");
    assert!(!columns.iter().any(|column| {
        let lower = column.to_ascii_lowercase();
        lower == "token" || lower.contains("secret") || lower.contains("private_key")
    }));

    let validated = repository(&fixture)
        .validate_capability(&validation(token, &admission, 130))
        .await
        .expect("validate capability");
    assert_eq!(validated, admission);
    assert_eq!(
        validated.scope.task_lease_claim.fencing_token,
        lease.fencing_token
    );
}

#[derive(Debug, Default)]
struct FailingAudit {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl ExecutionAuditPort for FailingAudit {
    async fn append_audit(
        &self,
        _request: &ExecutionAuditAppend,
    ) -> Result<ExecutionAuditRecord, ExecutionEvidenceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ExecutionEvidenceError::Unavailable(
            "injected audit outage".to_string(),
        ))
    }

    async fn read_audit(
        &self,
        _request: &AuditReadRequest,
    ) -> Result<AuditPage, ExecutionEvidenceError> {
        Err(ExecutionEvidenceError::Unavailable(
            "injected audit outage".to_string(),
        ))
    }
}

#[tokio::test]
async fn attempt_capability_rejects_stale_expired_revoked_and_wrong_scope() {
    let fixture = fixture().await;
    let lease_port = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());
    let first_lease = dispatch(&fixture, 100, 200).await;
    let (stale_token, stale_admission) = issue(&fixture, first_lease.claim(), 190).await;
    lease_port
        .release(&TaskLeaseCloseRequest {
            claim: first_lease.claim(),
            observed_at: 120,
            reason: "superseded_for_test".to_string(),
        })
        .await
        .expect("release first lease");
    let current_lease = dispatch(&fixture, 121, 400).await;

    let stale = repository(&fixture)
        .validate_capability(&validation(stale_token, &stale_admission, 130))
        .await
        .expect_err("stale lease");
    assert_denied(&stale, SecurityDenialReason::LeaseRejected);

    let (expired_token, expired_admission) = issue(&fixture, current_lease.claim(), 150).await;
    let expired = repository(&fixture)
        .validate_capability(&validation(expired_token, &expired_admission, 151))
        .await
        .expect_err("expired capability");
    assert_denied(&expired, SecurityDenialReason::CapabilityExpired);

    let (revoked_token, revoked_admission) = issue(&fixture, current_lease.claim(), 300).await;
    repository(&fixture)
        .revoke_capability(&revoked_admission.id, 140)
        .await
        .expect("revoke capability");
    let revoked = repository(&fixture)
        .validate_capability(&validation(revoked_token, &revoked_admission, 141))
        .await
        .expect_err("revoked capability");
    assert_denied(&revoked, SecurityDenialReason::CapabilityRevoked);

    let (action_token, action_admission) = issue(&fixture, current_lease.claim(), 300).await;
    let mut wrong_action = validation(action_token, &action_admission, 145);
    wrong_action.action = ProtectedAction::ArtifactRead;
    let action_error = repository(&fixture)
        .validate_capability(&wrong_action)
        .await
        .expect_err("wrong action");
    assert_denied(&action_error, SecurityDenialReason::CapabilityScopeMismatch);

    let (project_token, project_admission) = issue(&fixture, current_lease.claim(), 300).await;
    let mut wrong_project = validation(project_token, &project_admission, 146);
    wrong_project.scope.project_ref = project("project-b");
    wrong_project.resource = resource("project-b");
    let project_error = repository(&fixture)
        .validate_capability(&wrong_project)
        .await
        .expect_err("wrong project");
    assert_denied(
        &project_error,
        SecurityDenialReason::CapabilityScopeMismatch,
    );

    let audit_rows = sqlx::query(
        "SELECT payload_json FROM execution_audit_events \
         WHERE event_type = 'execution.security_denied' ORDER BY sequence",
    )
    .fetch_all(fixture.store.pool())
    .await
    .expect("denial audits");
    assert_eq!(audit_rows.len(), 5);
    for row in audit_rows {
        let payload: String = row.get("payload_json");
        assert!(!payload.contains("secret-value"));
        assert!(!payload.contains("CapabilityToken"));
        assert!(!payload.contains("token_sha256"));
    }

    let (audit_token, audit_admission) = issue(&fixture, current_lease.claim(), 300).await;
    let failing = SqliteAttemptCapabilityRepository::new(
        fixture.store.pool().clone(),
        SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone()),
        FailingAudit::default(),
    );
    let mut rejected = validation(audit_token, &audit_admission, 147);
    rejected.action = ProtectedAction::ArtifactRead;
    let unavailable = failing
        .validate_capability(&rejected)
        .await
        .expect_err("required denial audit failure");
    assert!(matches!(unavailable, SecurityError::Unavailable(_)));
}
