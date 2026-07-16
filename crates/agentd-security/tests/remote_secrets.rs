use std::{sync::Mutex, time::Duration};

use agentd_core::ports::{SecretBrokerPort, SecurityError};
use agentd_core::types::{
    AttemptCapabilityId, AuthenticatedWorkload, AuthorityKey, CapabilityAdmission, FencingToken,
    LeaseId, OrganizationRef, ProjectExecutionSnapshotRef, ProjectRef, ProtectedAction,
    ProtectedResource, ProtectedResourceKind, RbacPolicyVersionRef, RunId, SecretCheckoutRequest,
    SecretMaterial, SecretSelector, SecurityAuditContext, SecurityDenialReason, TaskLeaseClaim,
    TaskRunId, WorkerId, WorkerIncarnationId, WorkloadRole,
};
use agentd_security::remote_secrets::{
    RemoteSecretBroker, RemoteSecretRequest, RemoteSecretResponse, SecretBrokerTransport,
    SecretBrokerTransportError,
};

#[derive(Debug)]
struct FakeTransport {
    requests: Mutex<Vec<RemoteSecretRequest>>,
    response: Mutex<Option<ScriptedResponse>>,
}

#[derive(Debug)]
enum ScriptedResponse {
    Valid {
        material: &'static [u8],
        expires_at: i64,
    },
    ForeignScope,
    ExcessExpiry,
    Transport(SecretBrokerTransportError),
}

#[async_trait::async_trait]
impl SecretBrokerTransport for FakeTransport {
    async fn checkout(
        &self,
        request: &RemoteSecretRequest,
    ) -> Result<RemoteSecretResponse, SecretBrokerTransportError> {
        self.requests
            .lock()
            .expect("requests")
            .push(request.clone());
        let response = self
            .response
            .lock()
            .expect("response")
            .take()
            .expect("one scripted response");
        match response {
            ScriptedResponse::Valid {
                material,
                expires_at,
            } => Ok(RemoteSecretResponse {
                scope: request.scope.clone(),
                secret_version: "v1".to_string(),
                material: SecretMaterial::new(material.to_vec()),
                expires_at,
            }),
            ScriptedResponse::ForeignScope => {
                let mut scope = request.scope.clone();
                scope.selector = SecretSelector::new("repository/foreign-token").expect("selector");
                Ok(RemoteSecretResponse {
                    scope,
                    secret_version: "v1".to_string(),
                    material: SecretMaterial::new(b"scope-leak".to_vec()),
                    expires_at: 250,
                })
            }
            ScriptedResponse::ExcessExpiry => Ok(RemoteSecretResponse {
                scope: request.scope.clone(),
                secret_version: "v1".to_string(),
                material: SecretMaterial::new(b"expiry-leak".to_vec()),
                expires_at: request.scope.requested_expires_at + 1,
            }),
            ScriptedResponse::Transport(error) => Err(error),
        }
    }
}

#[derive(Debug)]
struct NeverTransport;

#[async_trait::async_trait]
impl SecretBrokerTransport for NeverTransport {
    async fn checkout(
        &self,
        _request: &RemoteSecretRequest,
    ) -> Result<RemoteSecretResponse, SecretBrokerTransportError> {
        std::future::pending().await
    }
}

fn authority() -> AuthorityKey {
    AuthorityKey::new("specify:remote-secret-test").expect("authority")
}

fn selector() -> SecretSelector {
    SecretSelector::new("repository/app-token").expect("selector")
}

fn admission() -> CapabilityAdmission {
    let organization_ref = OrganizationRef::new(authority(), "org-a", "1").expect("organization");
    let project_ref = ProjectRef::new(authority(), "project-a", "2").expect("project");
    let execution_snapshot_ref =
        ProjectExecutionSnapshotRef::new(authority(), "snapshot-a", "3").expect("snapshot");
    let worker_incarnation_id = WorkerIncarnationId::from_string("wi_01ARZ3NDEKTSV4RRFFQ69G5FAV");
    let resource = ProtectedResource {
        organization_ref: organization_ref.clone(),
        project_ref: project_ref.clone(),
        execution_snapshot_ref: execution_snapshot_ref.clone(),
        kind: ProtectedResourceKind::Secret(selector()),
    };
    CapabilityAdmission {
        id: AttemptCapabilityId::from_string("cp_01ARZ3NDEKTSV4RRFFQ69G5FAW"),
        workload: AuthenticatedWorkload {
            spiffe_uri: format!("spiffe://agents.example/worker/{worker_incarnation_id}"),
            role: WorkloadRole::Worker,
            trust_domain: "agents.example".to_string(),
            certificate_sha256: "a".repeat(64),
            not_before: 100,
            not_after: 400,
            worker_id: Some(WorkerId::from_string("wk_01ARZ3NDEKTSV4RRFFQ69G5FAX")),
            worker_incarnation_id: Some(worker_incarnation_id.clone()),
        },
        scope: agentd_core::types::ExecutionSecurityScope {
            authority_key: authority(),
            organization_ref,
            project_ref,
            execution_snapshot_ref,
            rbac_policy_version_ref: RbacPolicyVersionRef::new(authority(), "rbac-a", "4")
                .expect("rbac"),
            worker_incarnation_id: worker_incarnation_id.clone(),
            task_lease_claim: TaskLeaseClaim {
                execution_task_id: TaskRunId::from_string("tr_01ARZ3NDEKTSV4RRFFQ69G5FAY"),
                worker_incarnation_id,
                lease_id: LeaseId::from_string("ls_01ARZ3NDEKTSV4RRFFQ69G5FAZ"),
                fencing_token: FencingToken::new(8).expect("fencing"),
            },
            sandbox_profile_id: "oci-restricted-v1".to_string(),
            egress_profile_id: "deny-all-v1".to_string(),
            policy_revocation_epoch: 7,
            valid_until: 330,
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
        expires_at: 300,
    }
}

fn request() -> SecretCheckoutRequest {
    SecretCheckoutRequest {
        admission: admission(),
        selector: selector(),
        observed_at: 200,
    }
}

#[tokio::test]
async fn remote_broker_sends_exact_immutable_scope_and_returns_transient_material() {
    let checkout = request();
    let transport = FakeTransport {
        requests: Mutex::new(Vec::new()),
        response: Mutex::new(Some(ScriptedResponse::Valid {
            material: b"remote-secret-value",
            expires_at: 280,
        })),
    };
    let broker = RemoteSecretBroker::new(transport, Duration::from_secs(1)).expect("broker");

    let lease = broker
        .checkout_secret(&checkout)
        .await
        .expect("remote checkout");
    assert_eq!(lease.selector, selector());
    assert_eq!(lease.material.expose_secret(), b"remote-secret-value");
    assert_eq!(lease.expires_at, 280);
    let requests = broker.transport().requests.lock().expect("requests");
    let remote_request = requests.first().expect("remote request");
    assert_eq!(
        remote_request.scope.rbac_policy_version_ref,
        checkout.admission.scope.rbac_policy_version_ref
    );
    assert_eq!(remote_request.scope.policy_revocation_epoch, 7);
    assert!(remote_request.scope.checkout_id.starts_with("sc_"));
    assert_eq!(remote_request.scope.checkout_id.len(), 29);
    assert!(!format!("{broker:?} {lease:?}").contains("remote-secret-value"));
}

#[tokio::test]
async fn remote_broker_rejects_scope_expiry_and_transport_failures_without_disclosure() {
    let checkout = request();
    for response in [
        ScriptedResponse::ForeignScope,
        ScriptedResponse::ExcessExpiry,
        ScriptedResponse::Transport(SecretBrokerTransportError::TimedOut),
        ScriptedResponse::Transport(SecretBrokerTransportError::Unavailable),
    ] {
        let transport = FakeTransport {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(Some(response)),
        };
        let error = RemoteSecretBroker::new(transport, Duration::from_secs(1))
            .expect("broker")
            .checkout_secret(&checkout)
            .await
            .expect_err("invalid remote response must fail closed");
        assert_eq!(
            error,
            SecurityError::Denied(SecurityDenialReason::SecretUnavailable)
        );
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("scope-leak"));
        assert!(!rendered.contains("expiry-leak"));
        assert!(!rendered.contains("foreign-token"));
    }
}

#[tokio::test]
async fn remote_broker_enforces_a_local_timeout() {
    let error = RemoteSecretBroker::new(NeverTransport, Duration::from_millis(1))
        .expect("broker")
        .checkout_secret(&request())
        .await
        .expect_err("local timeout must fail closed");

    assert_eq!(
        error,
        SecurityError::Denied(SecurityDenialReason::SecretUnavailable)
    );
}
