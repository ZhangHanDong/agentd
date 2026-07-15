use std::sync::Mutex;

use agentd_core::ports::{
    AuditPage, AuditReadRequest, ExecutionAuditAppend, ExecutionAuditPort, ExecutionAuditRecord,
    ExecutionEvidenceError, SecretBrokerPort,
};
use agentd_core::types::{
    AttemptCapabilityId, AuthenticatedWorkload, AuthorityKey, CapabilityAdmission, FencingToken,
    LeaseId, OrganizationRef, ProjectExecutionSnapshotRef, ProjectRef, ProtectedAction,
    ProtectedResource, ProtectedResourceKind, RbacPolicyVersionRef, RunId, SecretCheckoutRequest,
    SecretSelector, SecurityAuditContext, SecurityDenialReason, TaskLeaseClaim, TaskRunId,
    WorkerId, WorkerIncarnationId, WorkloadRole,
};
use agentd_security::secrets::ScopedSecretBroker;

#[derive(Debug, Default)]
struct RecordingAudit {
    events: Mutex<Vec<ExecutionAuditAppend>>,
}

#[async_trait::async_trait]
impl ExecutionAuditPort for RecordingAudit {
    async fn append_audit(
        &self,
        request: &ExecutionAuditAppend,
    ) -> Result<ExecutionAuditRecord, ExecutionEvidenceError> {
        self.events
            .lock()
            .expect("events lock")
            .push(request.clone());
        Ok(ExecutionAuditRecord {
            append: request.clone(),
            sequence: 1,
            recorded_at: request.occurred_at,
        })
    }

    async fn read_audit(
        &self,
        _request: &AuditReadRequest,
    ) -> Result<AuditPage, ExecutionEvidenceError> {
        Ok(AuditPage {
            records: Vec::new(),
            next_after_sequence: None,
        })
    }
}

fn authority_key() -> AuthorityKey {
    AuthorityKey::new("specify:secret-test").expect("authority")
}

fn selector(value: &str) -> SecretSelector {
    SecretSelector::new(value).expect("selector")
}

fn admission() -> CapabilityAdmission {
    let worker_incarnation_id = WorkerIncarnationId::from_string("wi_01ARZ3NDEKTSV4RRFFQ69G5FAV");
    let organization_ref =
        OrganizationRef::new(authority_key(), "org-a", "1").expect("organization");
    let project_ref = ProjectRef::new(authority_key(), "project-a", "2").expect("project");
    let execution_snapshot_ref =
        ProjectExecutionSnapshotRef::new(authority_key(), "snapshot-a", "3").expect("snapshot");
    let resource = ProtectedResource {
        organization_ref: organization_ref.clone(),
        project_ref: project_ref.clone(),
        execution_snapshot_ref: execution_snapshot_ref.clone(),
        kind: ProtectedResourceKind::Secret(selector("repository/app-token")),
    };
    CapabilityAdmission {
        id: AttemptCapabilityId::from_string("cp_01ARZ3NDEKTSV4RRFFQ69G5FAW"),
        workload: AuthenticatedWorkload {
            spiffe_uri: format!("spiffe://agents.example/worker/{worker_incarnation_id}"),
            role: WorkloadRole::Worker,
            trust_domain: "agents.example".to_string(),
            certificate_sha256: "a".repeat(64),
            not_before: 100,
            not_after: 500,
            worker_id: Some(WorkerId::from_string("wk_01ARZ3NDEKTSV4RRFFQ69G5FAX")),
            worker_incarnation_id: Some(worker_incarnation_id.clone()),
        },
        scope: agentd_core::types::ExecutionSecurityScope {
            authority_key: authority_key(),
            organization_ref,
            project_ref,
            execution_snapshot_ref,
            rbac_policy_version_ref: RbacPolicyVersionRef::new(authority_key(), "rbac-a", "4")
                .expect("rbac"),
            worker_incarnation_id: worker_incarnation_id.clone(),
            task_lease_claim: TaskLeaseClaim {
                execution_task_id: TaskRunId::from_string("tr_01ARZ3NDEKTSV4RRFFQ69G5FAY"),
                worker_incarnation_id,
                lease_id: LeaseId::from_string("ls_01ARZ3NDEKTSV4RRFFQ69G5FAZ"),
                fencing_token: FencingToken::new(8).expect("fencing token"),
            },
            sandbox_profile_id: "oci-restricted-v1".to_string(),
            egress_profile_id: "deny-all-v1".to_string(),
            policy_revocation_epoch: 1,
            valid_until: 450,
            audit_context: SecurityAuditContext {
                execution_run_id: RunId::from_string("r_01ARZ3NDEKTSV4RRFFQ69G5FB0"),
                snapshot_content_sha256: "b".repeat(64),
                target_repository_id: "repository-a".to_string(),
                target_base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            },
        },
        action: ProtectedAction::SecretCheckout,
        resource,
        issued_at: 120,
        expires_at: 350,
    }
}

#[tokio::test]
async fn secret_broker_requires_checkout_admission_and_redacts_material() {
    let audit = RecordingAudit::default();
    let mut broker = ScopedSecretBroker::new(audit);
    broker.insert(
        selector("repository/app-token"),
        b"super-secret-value".to_vec(),
        300,
    );

    let request = SecretCheckoutRequest {
        admission: admission(),
        selector: selector("repository/app-token"),
        observed_at: 200,
    };
    let mut lease = broker
        .checkout_secret(&request)
        .await
        .expect("secret checkout");
    assert_eq!(lease.material.expose_secret(), b"super-secret-value");
    assert_eq!(lease.expires_at, 300);
    assert!(!format!("{lease:?}").contains("super-secret-value"));
    lease.material.zeroize();
    assert!(lease.material.expose_secret().iter().all(|byte| *byte == 0));

    let serialized_admission = serde_json::to_string(&request.admission).expect("admission JSON");
    assert!(!serialized_admission.contains("super-secret-value"));

    let mut wrong_action = request.clone();
    wrong_action.admission.action = ProtectedAction::ArtifactWrite;
    let action_error = broker
        .checkout_secret(&wrong_action)
        .await
        .expect_err("wrong action");
    assert_eq!(
        action_error.denial_reason(),
        Some(SecurityDenialReason::SecretUnavailable)
    );

    let mut wrong_resource = request.clone();
    wrong_resource.selector = selector("repository/other-token");
    let resource_error = broker
        .checkout_secret(&wrong_resource)
        .await
        .expect_err("wrong resource");
    assert_eq!(
        resource_error.denial_reason(),
        Some(SecurityDenialReason::SecretUnavailable)
    );
    assert_eq!(format!("{action_error}"), format!("{resource_error}"));

    let events = broker.audit().events.lock().expect("events lock");
    assert_eq!(events.len(), 3);
    let audit_json = serde_json::to_string(&*events).expect("audit JSON");
    assert!(!audit_json.contains("super-secret-value"));
    assert!(!audit_json.contains("repository/app-token"));
}
