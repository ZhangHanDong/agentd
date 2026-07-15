//! Scoped secret-broker adapter.

use std::collections::HashMap;
use std::fmt;

use agentd_core::ports::{
    AuditActorKind, ExecutionAuditAppend, ExecutionAuditPort, ExecutionEvidenceLinks,
    ExecutionSnapshotLink, SecretBrokerPort, SecurityError,
};
use agentd_core::types::{
    ProtectedAction, ProtectedResourceKind, SecretCheckoutRequest, SecretLease, SecretMaterial,
    SecretSelector, SecurityDenialReason,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

struct BrokerSecret {
    material: Zeroizing<Vec<u8>>,
    expires_at: i64,
}

pub struct ScopedSecretBroker<A> {
    secrets: HashMap<SecretSelector, BrokerSecret>,
    audit: A,
}

impl<A> ScopedSecretBroker<A> {
    #[must_use]
    pub fn new(audit: A) -> Self {
        Self {
            secrets: HashMap::new(),
            audit,
        }
    }

    pub fn insert(&mut self, selector: SecretSelector, material: Vec<u8>, expires_at: i64) {
        self.secrets.insert(
            selector,
            BrokerSecret {
                material: Zeroizing::new(material),
                expires_at,
            },
        );
    }

    #[must_use]
    pub const fn audit(&self) -> &A {
        &self.audit
    }
}

impl<A> fmt::Debug for ScopedSecretBroker<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScopedSecretBroker")
            .field("secret_count", &self.secrets.len())
            .field("audit", &"[REDACTED]")
            .finish()
    }
}

#[async_trait::async_trait]
impl<A> SecretBrokerPort for ScopedSecretBroker<A>
where
    A: ExecutionAuditPort,
{
    async fn checkout_secret(
        &self,
        request: &SecretCheckoutRequest,
    ) -> Result<SecretLease, SecurityError> {
        let valid_admission = request.admission.action == ProtectedAction::SecretCheckout
            && request.observed_at >= request.admission.issued_at
            && request.observed_at < request.admission.expires_at
            && matches!(
                &request.admission.resource.kind,
                ProtectedResourceKind::Secret(selector) if selector == &request.selector
            )
            && request
                .admission
                .scope
                .authorize_resource(&request.admission.resource)
                .is_ok();
        let secret = valid_admission
            .then(|| self.secrets.get(&request.selector))
            .flatten()
            .filter(|secret| request.observed_at < secret.expires_at);
        let Some(secret) = secret else {
            self.audit_decision(request, "denied").await?;
            return Err(SecurityError::Denied(
                SecurityDenialReason::SecretUnavailable,
            ));
        };
        self.audit_decision(request, "accepted").await?;
        Ok(SecretLease {
            selector: request.selector.clone(),
            material: SecretMaterial::new(secret.material.to_vec()),
            expires_at: secret.expires_at.min(request.admission.expires_at),
        })
    }
}

impl<A> ScopedSecretBroker<A>
where
    A: ExecutionAuditPort,
{
    async fn audit_decision(
        &self,
        request: &SecretCheckoutRequest,
        decision: &str,
    ) -> Result<(), SecurityError> {
        let payload = json!({
            "capability_id": request.admission.id,
            "decision": decision,
            "action": "secret.checkout",
            "execution_task_id": request.admission.scope.task_lease_claim.execution_task_id,
            "worker_incarnation_id": request.admission.scope.worker_incarnation_id,
        });
        let audit_id = agentd_core::types::AuditEventId::new();
        let event = ExecutionAuditAppend {
            id: audit_id.clone(),
            idempotency_scope: format!(
                "secret-checkout:{}",
                request.admission.scope.task_lease_claim.execution_task_id
            ),
            idempotency_key: audit_id.to_string(),
            event_type: format!("execution.secret_{decision}"),
            actor_kind: AuditActorKind::Worker,
            actor_ref: request.admission.scope.worker_incarnation_id.to_string(),
            payload_sha256: sha256_json(&payload)?,
            payload,
            links: audit_links(request),
            execution_artifact_id: None,
            occurred_at: request.observed_at,
        };
        self.audit
            .append_audit(&event)
            .await
            .map(|_| ())
            .map_err(|error| {
                SecurityError::Unavailable(format!("required secret audit failed: {error}"))
            })
    }
}

fn audit_links(request: &SecretCheckoutRequest) -> ExecutionEvidenceLinks {
    let scope = &request.admission.scope;
    ExecutionEvidenceLinks {
        execution_run_id: scope.audit_context.execution_run_id.clone(),
        execution_task_id: Some(scope.task_lease_claim.execution_task_id.clone()),
        runtime_session_id: None,
        runtime_attempt_id: None,
        worker_incarnation_id: Some(scope.worker_incarnation_id.clone()),
        snapshot: ExecutionSnapshotLink {
            authority_key: scope.authority_key.to_string(),
            resource_kind: "execution_snapshot".to_string(),
            resource_id: scope.execution_snapshot_ref.resource_id().to_string(),
            resource_version: scope.execution_snapshot_ref.resource_version().to_string(),
            content_sha256: scope.audit_context.snapshot_content_sha256.clone(),
        },
        target_repository_id: scope.audit_context.target_repository_id.clone(),
        target_base_commit: scope.audit_context.target_base_commit.clone(),
    }
}

fn sha256_json(value: &Value) -> Result<String, SecurityError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| SecurityError::Invalid(format!("invalid secret audit: {error}")))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
