use std::collections::BTreeMap;
use std::sync::Mutex;

use agentd_core::ports::{
    AttemptCapabilityPort, ExecutionSandboxPort, SecretBrokerPort, SecurityError,
    TenantAuthorizationPort, WorkloadIdentityPort,
};
use agentd_core::types::{
    AttemptCapabilityId, AuthenticatedWorkload, AuthorityKey, CapabilityAdmission,
    CapabilityIssueRequest, CapabilityToken, CapabilityValidationRequest, EgressPolicy,
    ExecutionSandboxProfile, ExecutionSecurityScope, FencingToken, LeaseId, OciSandboxRuntime,
    OrganizationRef, PreparedSandbox, ProjectExecutionSnapshotRef, ProjectRef, ProtectedAction,
    ProtectedResource, ProtectedResourceKind, RbacPolicyVersionRef, SandboxCleanupRequest,
    SandboxExecuteRequest, SandboxExecution, SandboxLimits, SandboxMount, SandboxMountAccess,
    SandboxPrepareRequest, SecretCheckoutRequest, SecretLease, SecretMaterial, SecretSelector,
    TaskLeaseClaim, TaskRunId, TenantAuthorization, TenantAuthorizationRequest, WorkerId,
    WorkerIncarnationId, WorkloadIdentityRequest, WorkloadRole,
};

fn authority_key() -> AuthorityKey {
    AuthorityKey::new("specify:security-test").expect("authority key")
}

fn organization(id: &str) -> OrganizationRef {
    OrganizationRef::new(authority_key(), id, "4").expect("organization ref")
}

fn project(id: &str) -> ProjectRef {
    ProjectRef::new(authority_key(), id, "9").expect("project ref")
}

fn snapshot(id: &str) -> ProjectExecutionSnapshotRef {
    ProjectExecutionSnapshotRef::new(authority_key(), id, "12").expect("snapshot ref")
}

fn worker_incarnation() -> WorkerIncarnationId {
    WorkerIncarnationId::from_string("wi_01ARZ3NDEKTSV4RRFFQ69G5FAV")
}

fn claim() -> TaskLeaseClaim {
    TaskLeaseClaim {
        execution_task_id: TaskRunId::from_string("tr_01ARZ3NDEKTSV4RRFFQ69G5FAW"),
        worker_incarnation_id: worker_incarnation(),
        lease_id: LeaseId::from_string("ls_01ARZ3NDEKTSV4RRFFQ69G5FAX"),
        fencing_token: FencingToken::new(7).expect("fencing token"),
    }
}

fn workload() -> AuthenticatedWorkload {
    AuthenticatedWorkload {
        spiffe_uri: "spiffe://agents.example/worker/wi_01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        role: WorkloadRole::Worker,
        trust_domain: "agents.example".to_string(),
        certificate_sha256: "a".repeat(64),
        not_before: 100,
        not_after: 500,
        worker_id: Some(WorkerId::from_string("wk_01ARZ3NDEKTSV4RRFFQ69G5FAY")),
        worker_incarnation_id: Some(worker_incarnation()),
    }
}

fn scope() -> ExecutionSecurityScope {
    ExecutionSecurityScope {
        authority_key: authority_key(),
        organization_ref: organization("org-a"),
        project_ref: project("project-a"),
        execution_snapshot_ref: snapshot("snapshot-a"),
        rbac_policy_version_ref: RbacPolicyVersionRef::new(authority_key(), "rbac-a", "3")
            .expect("rbac ref"),
        worker_incarnation_id: worker_incarnation(),
        task_lease_claim: claim(),
        sandbox_profile_id: "oci-restricted-v1".to_string(),
        egress_profile_id: "deny-all-v1".to_string(),
        policy_revocation_epoch: 11,
        valid_until: 450,
    }
}

fn resource() -> ProtectedResource {
    ProtectedResource {
        organization_ref: organization("org-a"),
        project_ref: project("project-a"),
        execution_snapshot_ref: snapshot("snapshot-a"),
        kind: ProtectedResourceKind::Artifact("artifact-output".to_string()),
    }
}

fn authorization() -> TenantAuthorization {
    TenantAuthorization {
        workload: workload(),
        scope: scope(),
        action: ProtectedAction::ArtifactWrite,
        resource: resource(),
        authorized_at: 120,
        expires_at: 400,
    }
}

fn admission() -> CapabilityAdmission {
    CapabilityAdmission {
        id: AttemptCapabilityId::from_string("cp_01ARZ3NDEKTSV4RRFFQ69G5FAZ"),
        workload: workload(),
        scope: scope(),
        action: ProtectedAction::ArtifactWrite,
        resource: resource(),
        issued_at: 125,
        expires_at: 350,
    }
}

fn profile() -> ExecutionSandboxProfile {
    ExecutionSandboxProfile {
        profile_id: "oci-restricted-v1".to_string(),
        runtime: OciSandboxRuntime::Docker,
        image_digest: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .to_string(),
        read_only_root: true,
        ephemeral_workspace: true,
        mounts: vec![SandboxMount {
            source_id: "input-bundle".to_string(),
            target: "/workspace/input".to_string(),
            access: SandboxMountAccess::ReadOnly,
        }],
        drop_all_capabilities: true,
        no_new_privileges: true,
        seccomp_profile: "runtime-default".to_string(),
        limits: SandboxLimits {
            pids: 64,
            memory_bytes: 512 * 1024 * 1024,
            cpu_millis: 1_000,
        },
        tenant_cache_namespace: "specify:security-test/org-a/project-a".to_string(),
        shared_cache: false,
        egress: EgressPolicy::DenyAll,
    }
}

#[derive(Default)]
struct RecordingSecurityPorts {
    calls: Mutex<Vec<&'static str>>,
}

impl RecordingSecurityPorts {
    fn record(&self, call: &'static str) {
        self.calls.lock().expect("calls lock").push(call);
    }
}

#[async_trait::async_trait]
impl WorkloadIdentityPort for RecordingSecurityPorts {
    async fn authenticate_workload(
        &self,
        request: &WorkloadIdentityRequest,
    ) -> Result<AuthenticatedWorkload, SecurityError> {
        assert_eq!(request.observed_at, 110);
        assert_eq!(request.peer_certificates_der, vec![vec![1, 2, 3]]);
        self.record("identity");
        Ok(workload())
    }
}

#[async_trait::async_trait]
impl TenantAuthorizationPort for RecordingSecurityPorts {
    async fn authorize_tenant(
        &self,
        request: &TenantAuthorizationRequest,
    ) -> Result<TenantAuthorization, SecurityError> {
        assert_eq!(request.workload, workload());
        assert_eq!(request.scope, scope());
        assert_eq!(request.action, ProtectedAction::ArtifactWrite);
        assert_eq!(request.resource, resource());
        self.record("authorization");
        Ok(authorization())
    }
}

#[async_trait::async_trait]
impl AttemptCapabilityPort for RecordingSecurityPorts {
    async fn issue_capability(
        &self,
        request: &CapabilityIssueRequest,
    ) -> Result<(CapabilityToken, CapabilityAdmission), SecurityError> {
        assert_eq!(request.authorization, authorization());
        assert_eq!(request.requested_expires_at, 350);
        self.record("capability.issue");
        Ok((CapabilityToken::new([7_u8; 32]), admission()))
    }

    async fn validate_capability(
        &self,
        request: &CapabilityValidationRequest,
    ) -> Result<CapabilityAdmission, SecurityError> {
        assert_eq!(request.observed_at, 130);
        assert_eq!(request.scope, scope());
        assert_eq!(request.action, ProtectedAction::ArtifactWrite);
        assert_eq!(request.resource, resource());
        self.record("capability.validate");
        Ok(admission())
    }

    async fn revoke_capability(
        &self,
        id: &AttemptCapabilityId,
        observed_at: i64,
    ) -> Result<(), SecurityError> {
        assert_eq!(id.as_str(), "cp_01ARZ3NDEKTSV4RRFFQ69G5FAZ");
        assert_eq!(observed_at, 140);
        self.record("capability.revoke");
        Ok(())
    }
}

#[async_trait::async_trait]
impl SecretBrokerPort for RecordingSecurityPorts {
    async fn checkout_secret(
        &self,
        request: &SecretCheckoutRequest,
    ) -> Result<SecretLease, SecurityError> {
        assert_eq!(request.admission, admission());
        assert_eq!(request.selector.as_str(), "repository/app-token");
        self.record("secret");
        Ok(SecretLease {
            selector: request.selector.clone(),
            material: SecretMaterial::new(b"secret-value".to_vec()),
            expires_at: 300,
        })
    }
}

#[async_trait::async_trait]
impl ExecutionSandboxPort for RecordingSecurityPorts {
    async fn prepare_sandbox(
        &self,
        request: &SandboxPrepareRequest,
    ) -> Result<PreparedSandbox, SecurityError> {
        assert_eq!(request.admission, admission());
        assert_eq!(request.profile, profile());
        self.record("sandbox.prepare");
        Ok(PreparedSandbox {
            sandbox_id: "sandbox-1".to_string(),
            profile: request.profile.clone(),
            created_at: 150,
            expires_at: 300,
        })
    }

    async fn execute_sandbox(
        &self,
        request: &SandboxExecuteRequest,
    ) -> Result<SandboxExecution, SecurityError> {
        assert_eq!(request.sandbox.sandbox_id, "sandbox-1");
        assert_eq!(request.argv, vec!["cargo", "test"]);
        assert_eq!(
            request.env,
            BTreeMap::from([("CI".to_string(), "1".to_string())])
        );
        self.record("sandbox.execute");
        Ok(SandboxExecution {
            exit_code: 0,
            stdout: b"ok".to_vec(),
            stderr: Vec::new(),
        })
    }

    async fn cleanup_sandbox(&self, request: &SandboxCleanupRequest) -> Result<(), SecurityError> {
        assert_eq!(request.sandbox_id, "sandbox-1");
        assert_eq!(request.observed_at, 170);
        self.record("sandbox.cleanup");
        Ok(())
    }
}

#[tokio::test]
async fn security_ports_preserve_separate_ordered_boundaries() {
    let ports = RecordingSecurityPorts::default();

    let identity_request = WorkloadIdentityRequest {
        peer_certificates_der: vec![vec![1, 2, 3]],
        observed_at: 110,
    };
    ports
        .authenticate_workload(&identity_request)
        .await
        .expect("identity");

    let authorization_request = TenantAuthorizationRequest {
        workload: workload(),
        scope: scope(),
        action: ProtectedAction::ArtifactWrite,
        resource: resource(),
    };
    ports
        .authorize_tenant(&authorization_request)
        .await
        .expect("authorization");

    let issue_request = CapabilityIssueRequest {
        authorization: authorization(),
        requested_expires_at: 350,
    };
    let (token, _) = ports
        .issue_capability(&issue_request)
        .await
        .expect("capability issue");
    assert_eq!(format!("{token:?}"), "CapabilityToken([REDACTED])");

    let validation_request = CapabilityValidationRequest {
        token,
        scope: scope(),
        action: ProtectedAction::ArtifactWrite,
        resource: resource(),
        observed_at: 130,
    };
    ports
        .validate_capability(&validation_request)
        .await
        .expect("capability validation");
    ports
        .revoke_capability(
            &AttemptCapabilityId::from_string("cp_01ARZ3NDEKTSV4RRFFQ69G5FAZ"),
            140,
        )
        .await
        .expect("capability revocation");

    let checkout = SecretCheckoutRequest {
        admission: admission(),
        selector: SecretSelector::new("repository/app-token").expect("secret selector"),
    };
    let secret = ports
        .checkout_secret(&checkout)
        .await
        .expect("secret checkout");
    assert_eq!(
        format!("{:?}", secret.material),
        "SecretMaterial([REDACTED])"
    );
    assert_eq!(secret.expires_at, 300);

    let prepare = SandboxPrepareRequest {
        admission: admission(),
        profile: profile(),
    };
    let sandbox = ports
        .prepare_sandbox(&prepare)
        .await
        .expect("prepare sandbox");
    let execute = SandboxExecuteRequest {
        sandbox: sandbox.clone(),
        argv: vec!["cargo".to_string(), "test".to_string()],
        env: BTreeMap::from([("CI".to_string(), "1".to_string())]),
    };
    ports
        .execute_sandbox(&execute)
        .await
        .expect("execute sandbox");
    ports
        .cleanup_sandbox(&SandboxCleanupRequest {
            sandbox_id: sandbox.sandbox_id,
            observed_at: 170,
        })
        .await
        .expect("cleanup sandbox");

    assert_eq!(
        *ports.calls.lock().expect("calls lock"),
        [
            "identity",
            "authorization",
            "capability.issue",
            "capability.validate",
            "capability.revoke",
            "secret",
            "sandbox.prepare",
            "sandbox.execute",
            "sandbox.cleanup",
        ]
    );
}

#[test]
fn security_scope_uses_authority_refs_and_rejects_cross_tenant_resources() {
    let scope = scope();

    let authorized = scope
        .authorize_resource(&resource())
        .expect("matching resource");
    assert_eq!(authorized.scope, scope);
    assert_eq!(authorized.resource, resource());

    let wrong_organization = ProtectedResource {
        organization_ref: organization("org-b"),
        ..resource()
    };
    assert_eq!(
        scope
            .authorize_resource(&wrong_organization)
            .expect_err("cross-tenant resource"),
        agentd_core::types::SecurityDenialReason::TenantMismatch
    );

    let wrong_project = ProtectedResource {
        project_ref: project("project-b"),
        ..resource()
    };
    assert_eq!(
        scope
            .authorize_resource(&wrong_project)
            .expect_err("cross-project resource"),
        agentd_core::types::SecurityDenialReason::ProjectMismatch
    );

    let wrong_snapshot = ProtectedResource {
        execution_snapshot_ref: snapshot("snapshot-b"),
        ..resource()
    };
    assert_eq!(
        scope
            .authorize_resource(&wrong_snapshot)
            .expect_err("cross-snapshot resource"),
        agentd_core::types::SecurityDenialReason::SnapshotMismatch
    );
}
