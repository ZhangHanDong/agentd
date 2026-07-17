//! Digest-only attempt capabilities bound to immutable authority scope and task fencing.

use agentd_core::ports::{
    AttemptCapabilityPort, AuditActorKind, ExecutionAuditAppend, ExecutionAuditPort,
    ExecutionEvidenceLinks, ExecutionSnapshotLink, SecurityError, TaskLeaseError, TaskLeasePort,
};
use agentd_core::types::{
    AttemptCapabilityId, AuthenticatedWorkload, AuthorityKey, CapabilityAdmission,
    CapabilityIssueRequest, CapabilityToken, CapabilityValidationRequest, ExecutionSecurityScope,
    FencingToken, OrganizationRef, ProjectExecutionSnapshotRef, ProjectRef, ProtectedAction,
    ProtectedResource, ProtectedResourceKind, RbacPolicyVersionRef, RunId, SecurityDenialReason,
    TaskLeaseClaim, TaskRunId, TenantAuthorization, WorkerId, WorkerIncarnationId, WorkloadRole,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use subtle::ConstantTimeEq;

use crate::util::SqliteImmediateTransaction;
use crate::worker_repo;

const DENIAL_EVENT_TYPE: &str = "execution.security_denied";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkloadIdentityBindingCreate {
    pub certificate_sha256: String,
    pub spiffe_uri: String,
    pub role: WorkloadRole,
    pub trust_domain: String,
    pub worker_id: Option<WorkerId>,
    pub worker_incarnation_id: Option<WorkerIncarnationId>,
    pub not_before: i64,
    pub not_after: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkloadIdentityBindingRecord {
    pub binding: WorkloadIdentityBindingCreate,
    pub revoked_at: Option<i64>,
    pub revocation_reason: Option<String>,
}

/// Bind one public certificate fingerprint and SPIFFE URI to a current worker
/// incarnation. Exact retries are idempotent; changed retries fail closed.
pub async fn bind_workload_identity(
    pool: &SqlitePool,
    request: WorkloadIdentityBindingCreate,
) -> Result<WorkloadIdentityBindingRecord, SecurityError> {
    validate_identity_binding(pool, &request).await?;
    if let Some(existing) = get_workload_identity_binding(pool, &request.certificate_sha256).await?
    {
        if existing.binding == request && existing.revoked_at.is_none() {
            return Ok(existing);
        }
        return Err(SecurityError::Invalid(
            "certificate fingerprint was reused with a changed or revoked binding".to_string(),
        ));
    }
    sqlx::query(
        "INSERT INTO workload_identity_bindings \
         (certificate_sha256, spiffe_uri, role, trust_domain, worker_id, \
          worker_incarnation_id, not_before, not_after, revoked_at, \
          revocation_reason, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?)",
    )
    .bind(&request.certificate_sha256)
    .bind(&request.spiffe_uri)
    .bind(workload_role_text(request.role))
    .bind(&request.trust_domain)
    .bind(request.worker_id.as_ref().map(WorkerId::as_str))
    .bind(
        request
            .worker_incarnation_id
            .as_ref()
            .map(WorkerIncarnationId::as_str),
    )
    .bind(request.not_before)
    .bind(request.not_after)
    .bind(request.created_at)
    .execute(pool)
    .await
    .map_err(storage_unavailable)?;
    get_workload_identity_binding(pool, &request.certificate_sha256)
        .await?
        .ok_or_else(|| {
            SecurityError::Unavailable("identity binding disappeared after insert".to_string())
        })
}

pub async fn get_workload_identity_binding(
    pool: &SqlitePool,
    certificate_sha256: &str,
) -> Result<Option<WorkloadIdentityBindingRecord>, SecurityError> {
    let row = sqlx::query(
        "SELECT certificate_sha256, spiffe_uri, role, trust_domain, worker_id, \
                worker_incarnation_id, not_before, not_after, revoked_at, \
                revocation_reason, created_at \
         FROM workload_identity_bindings WHERE certificate_sha256 = ?",
    )
    .bind(certificate_sha256)
    .fetch_optional(pool)
    .await
    .map_err(storage_unavailable)?;
    row.as_ref().map(identity_binding_from_row).transpose()
}

/// Revoke an identity binding without deleting its audit-relevant metadata.
pub async fn revoke_workload_identity(
    pool: &SqlitePool,
    certificate_sha256: &str,
    revoked_at: i64,
    reason: &str,
) -> Result<WorkloadIdentityBindingRecord, SecurityError> {
    if !is_sha256(certificate_sha256)
        || revoked_at < 0
        || reason != reason.trim()
        || reason.is_empty()
        || reason.len() > 512
        || reason.chars().any(char::is_control)
    {
        return Err(SecurityError::Invalid(
            "identity revocation requires a non-negative time and reason".to_string(),
        ));
    }
    let reason = reason.trim();
    let mut transaction = SqliteImmediateTransaction::begin(pool)
        .await
        .map_err(storage_unavailable)?;
    let row = sqlx::query(
        "SELECT certificate_sha256, spiffe_uri, role, trust_domain, worker_id, \
                worker_incarnation_id, not_before, not_after, revoked_at, \
                revocation_reason, created_at \
         FROM workload_identity_bindings WHERE certificate_sha256 = ?",
    )
    .bind(certificate_sha256)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(storage_unavailable)?;
    let mut record = match row.as_ref().map(identity_binding_from_row).transpose()? {
        Some(record) => record,
        None => {
            transaction.rollback().await.map_err(storage_unavailable)?;
            return Err(SecurityError::Denied(
                SecurityDenialReason::IdentityUntrusted,
            ));
        }
    };
    if revoked_at < record.binding.created_at {
        transaction.rollback().await.map_err(storage_unavailable)?;
        return Err(SecurityError::Invalid(
            "identity revocation cannot predate the binding".to_string(),
        ));
    }
    if record.revoked_at.is_some() {
        if record.revocation_reason.as_deref() == Some(reason) {
            transaction.commit().await.map_err(storage_unavailable)?;
            return Ok(record);
        }
        transaction.rollback().await.map_err(storage_unavailable)?;
        return Err(SecurityError::Invalid(
            "identity revocation was replayed with a different reason".to_string(),
        ));
    }
    let result = sqlx::query(
        "UPDATE workload_identity_bindings \
         SET revoked_at = ?, revocation_reason = ? \
         WHERE certificate_sha256 = ? AND revoked_at IS NULL",
    )
    .bind(revoked_at)
    .bind(reason)
    .bind(certificate_sha256)
    .execute(&mut *transaction)
    .await
    .map_err(storage_unavailable)?;
    if result.rows_affected() != 1 {
        transaction.rollback().await.map_err(storage_unavailable)?;
        return Err(SecurityError::Invalid(
            "identity revocation did not update the active binding".to_string(),
        ));
    }
    if let (Some(worker_id), Some(incarnation_id)) = (
        record.binding.worker_id.as_ref(),
        record.binding.worker_incarnation_id.as_ref(),
    ) {
        sqlx::query(
            "UPDATE enterprise_worker_availability \
             SET worker_status = 'offline', available_slots = 0, \
                 updated_at = MAX(updated_at, ?) \
             WHERE worker_incarnation_id = ?",
        )
        .bind(revoked_at)
        .bind(incarnation_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(storage_unavailable)?;
        sqlx::query(
            "UPDATE workers SET status = 'offline', updated_at = MAX(updated_at, ?), \
                    record_version = record_version + 1 \
             WHERE id = ? AND status <> 'retired' AND EXISTS (\
                 SELECT 1 FROM worker_incarnations \
                 WHERE id = ? AND worker_id = workers.id AND is_current = 1\
             )",
        )
        .bind(revoked_at)
        .bind(worker_id.as_str())
        .bind(incarnation_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(storage_unavailable)?;
    }
    record.revoked_at = Some(revoked_at);
    record.revocation_reason = Some(reason.to_string());
    transaction.commit().await.map_err(storage_unavailable)?;
    Ok(record)
}

#[derive(Debug, Clone)]
pub struct SqliteAttemptCapabilityRepository<L, A> {
    pool: SqlitePool,
    lease_port: L,
    audit_port: A,
}

impl<L, A> SqliteAttemptCapabilityRepository<L, A> {
    #[must_use]
    pub fn new(pool: SqlitePool, lease_port: L, audit_port: A) -> Self {
        Self {
            pool,
            lease_port,
            audit_port,
        }
    }
}

/// Set the current policy revocation epoch for one immutable Specify scope.
///
/// The epoch is monotonic. Repeating the same value is idempotent.
pub async fn set_policy_revocation_epoch(
    pool: &SqlitePool,
    scope: &ExecutionSecurityScope,
    epoch: u64,
    observed_at: i64,
) -> Result<(), SecurityError> {
    let epoch = i64::try_from(epoch)
        .map_err(|_| SecurityError::Invalid("policy epoch exceeds SQLite range".to_string()))?;
    if observed_at < 0 {
        return Err(SecurityError::Invalid(
            "policy epoch observed_at must be non-negative".to_string(),
        ));
    }
    let result = sqlx::query(
        "INSERT INTO execution_security_policy_epochs \
         (authority_key, organization_id, organization_version, project_id, project_version, \
          snapshot_id, snapshot_version, current_epoch, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (authority_key, organization_id, organization_version, project_id, \
                      project_version, snapshot_id, snapshot_version) \
         DO UPDATE SET current_epoch = excluded.current_epoch, updated_at = excluded.updated_at \
         WHERE excluded.current_epoch >= execution_security_policy_epochs.current_epoch",
    )
    .bind(scope.authority_key.as_str())
    .bind(scope.organization_ref.resource_id())
    .bind(scope.organization_ref.resource_version())
    .bind(scope.project_ref.resource_id())
    .bind(scope.project_ref.resource_version())
    .bind(scope.execution_snapshot_ref.resource_id())
    .bind(scope.execution_snapshot_ref.resource_version())
    .bind(epoch)
    .bind(observed_at)
    .execute(pool)
    .await
    .map_err(storage_unavailable)?;
    if result.rows_affected() == 0 {
        return Err(SecurityError::Invalid(
            "policy revocation epoch cannot decrease".to_string(),
        ));
    }
    Ok(())
}

#[async_trait::async_trait]
impl<L, A> AttemptCapabilityPort for SqliteAttemptCapabilityRepository<L, A>
where
    L: TaskLeasePort,
    A: ExecutionAuditPort,
{
    #[allow(clippy::too_many_lines)]
    async fn issue_capability(
        &self,
        request: &CapabilityIssueRequest,
    ) -> Result<(CapabilityToken, CapabilityAdmission), SecurityError> {
        validate_authorization(&request.authorization)?;
        let authorization = &request.authorization;
        let scope = &authorization.scope;
        validate_current_incarnation(&self.pool, &authorization.workload, scope).await?;
        validate_current_epoch(&self.pool, scope).await?;
        let grant = self
            .lease_port
            .validate_claim(&scope.task_lease_claim, authorization.authorized_at)
            .await
            .map_err(map_issue_lease_error)?;
        let expires_at = request
            .requested_expires_at
            .min(authorization.expires_at)
            .min(authorization.workload.not_after)
            .min(scope.valid_until)
            .min(grant.expires_at);
        if expires_at <= authorization.authorized_at {
            return Err(SecurityError::Invalid(
                "capability expiry must be after authorization time".to_string(),
            ));
        }

        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|error| {
            SecurityError::Unavailable(format!("OS capability randomness unavailable: {error}"))
        })?;
        let token = CapabilityToken::new(bytes);
        let digest = token_digest(&token);
        let id = AttemptCapabilityId::new();
        let worker_id = authorization
            .workload
            .worker_id
            .as_ref()
            .ok_or(SecurityError::Denied(
                SecurityDenialReason::IncarnationStale,
            ))?;
        let resource_json = serde_json::to_string(&authorization.resource).map_err(|error| {
            SecurityError::Invalid(format!("invalid protected resource: {error}"))
        })?;
        let epoch = i64::try_from(scope.policy_revocation_epoch)
            .map_err(|_| SecurityError::Invalid("policy epoch exceeds SQLite range".to_string()))?;
        let fencing_token =
            i64::try_from(scope.task_lease_claim.fencing_token.value()).map_err(|_| {
                SecurityError::Invalid("fencing token exceeds SQLite range".to_string())
            })?;

        sqlx::query(
            "INSERT INTO attempt_capabilities \
             (id, token_sha256, spiffe_uri, workload_role, trust_domain, certificate_sha256, \
              certificate_not_before, certificate_not_after, worker_id, worker_incarnation_id, \
              execution_task_id, lease_id, fencing_token, authority_key, organization_id, \
              organization_version, project_id, project_version, snapshot_id, snapshot_version, \
              rbac_policy_id, rbac_policy_version, sandbox_profile_id, egress_profile_id, \
              policy_revocation_epoch, scope_valid_until, action, resource_json, execution_run_id, \
              snapshot_content_sha256, target_repository_id, target_base_commit, issued_at, \
              expires_at, revoked_at, revocation_reason) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
                     ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL)",
        )
        .bind(id.as_str())
        .bind(digest)
        .bind(&authorization.workload.spiffe_uri)
        .bind(workload_role_text(authorization.workload.role))
        .bind(&authorization.workload.trust_domain)
        .bind(&authorization.workload.certificate_sha256)
        .bind(authorization.workload.not_before)
        .bind(authorization.workload.not_after)
        .bind(worker_id.as_str())
        .bind(scope.worker_incarnation_id.as_str())
        .bind(scope.task_lease_claim.execution_task_id.as_str())
        .bind(scope.task_lease_claim.lease_id.as_str())
        .bind(fencing_token)
        .bind(scope.authority_key.as_str())
        .bind(scope.organization_ref.resource_id())
        .bind(scope.organization_ref.resource_version())
        .bind(scope.project_ref.resource_id())
        .bind(scope.project_ref.resource_version())
        .bind(scope.execution_snapshot_ref.resource_id())
        .bind(scope.execution_snapshot_ref.resource_version())
        .bind(scope.rbac_policy_version_ref.resource_id())
        .bind(scope.rbac_policy_version_ref.resource_version())
        .bind(&scope.sandbox_profile_id)
        .bind(&scope.egress_profile_id)
        .bind(epoch)
        .bind(scope.valid_until)
        .bind(authorization.action.as_str())
        .bind(resource_json)
        .bind(scope.audit_context.execution_run_id.as_str())
        .bind(&scope.audit_context.snapshot_content_sha256)
        .bind(&scope.audit_context.target_repository_id)
        .bind(&scope.audit_context.target_base_commit)
        .bind(authorization.authorized_at)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .map_err(storage_unavailable)?;

        let admission = CapabilityAdmission {
            id,
            workload: authorization.workload.clone(),
            scope: scope.clone(),
            action: authorization.action,
            resource: authorization.resource.clone(),
            issued_at: authorization.authorized_at,
            expires_at,
        };
        Ok((token, admission))
    }

    #[allow(clippy::too_many_lines)]
    async fn validate_capability(
        &self,
        request: &CapabilityValidationRequest,
    ) -> Result<CapabilityAdmission, SecurityError> {
        if request.observed_at < 0 {
            return Err(SecurityError::Invalid(
                "capability observed_at must be non-negative".to_string(),
            ));
        }
        let computed_digest = token_digest(&request.token);
        let row = sqlx::query("SELECT * FROM attempt_capabilities WHERE token_sha256 = ?")
            .bind(&computed_digest)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_unavailable)?;
        let Some(row) = row else {
            return self
                .deny(
                    &request.scope,
                    None,
                    request.action,
                    SecurityDenialReason::CapabilityScopeMismatch,
                    request.observed_at,
                )
                .await;
        };
        let stored_digest: String = row.get("token_sha256");
        if stored_digest
            .as_bytes()
            .ct_eq(computed_digest.as_bytes())
            .unwrap_u8()
            != 1
        {
            return self
                .deny(
                    &request.scope,
                    None,
                    request.action,
                    SecurityDenialReason::CapabilityScopeMismatch,
                    request.observed_at,
                )
                .await;
        }
        let stored = row_to_stored(&row)?;
        if stored.revoked_at.is_some() {
            return self
                .deny(
                    &request.scope,
                    Some(&stored.admission.id),
                    request.action,
                    SecurityDenialReason::CapabilityRevoked,
                    request.observed_at,
                )
                .await;
        }
        if request.observed_at >= stored.admission.expires_at {
            return self
                .deny(
                    &request.scope,
                    Some(&stored.admission.id),
                    request.action,
                    SecurityDenialReason::CapabilityExpired,
                    request.observed_at,
                )
                .await;
        }
        if request.scope != stored.admission.scope
            || request.action != stored.admission.action
            || request.resource != stored.admission.resource
        {
            return self
                .deny(
                    &request.scope,
                    Some(&stored.admission.id),
                    request.action,
                    SecurityDenialReason::CapabilityScopeMismatch,
                    request.observed_at,
                )
                .await;
        }
        if let Err(error) = validate_current_epoch(&self.pool, &request.scope).await {
            let reason = match error {
                SecurityError::Denied(reason) => reason,
                other => return Err(other),
            };
            return self
                .deny(
                    &request.scope,
                    Some(&stored.admission.id),
                    request.action,
                    reason,
                    request.observed_at,
                )
                .await;
        }
        if let Err(error) =
            validate_current_incarnation(&self.pool, &stored.admission.workload, &request.scope)
                .await
        {
            let reason = match error {
                SecurityError::Denied(reason) => reason,
                other => return Err(other),
            };
            return self
                .deny(
                    &request.scope,
                    Some(&stored.admission.id),
                    request.action,
                    reason,
                    request.observed_at,
                )
                .await;
        }
        if let Err(error) = self
            .lease_port
            .validate_claim(&request.scope.task_lease_claim, request.observed_at)
            .await
        {
            return match error {
                TaskLeaseError::Unavailable(message) => Err(SecurityError::Unavailable(message)),
                TaskLeaseError::Invalid(message) => Err(SecurityError::Invalid(message)),
                TaskLeaseError::Rejected { .. }
                | TaskLeaseError::NotFound(_)
                | TaskLeaseError::Conflict(_) => {
                    self.deny(
                        &request.scope,
                        Some(&stored.admission.id),
                        request.action,
                        SecurityDenialReason::LeaseRejected,
                        request.observed_at,
                    )
                    .await
                }
            };
        }
        Ok(stored.admission)
    }

    async fn revoke_capability(
        &self,
        id: &AttemptCapabilityId,
        observed_at: i64,
    ) -> Result<(), SecurityError> {
        if observed_at < 0 {
            return Err(SecurityError::Invalid(
                "capability revocation time must be non-negative".to_string(),
            ));
        }
        let result = sqlx::query(
            "UPDATE attempt_capabilities \
             SET revoked_at = COALESCE(revoked_at, ?), \
                 revocation_reason = COALESCE(revocation_reason, 'explicit_revocation') \
             WHERE id = ?",
        )
        .bind(observed_at)
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(storage_unavailable)?;
        if result.rows_affected() != 1 {
            return Err(SecurityError::Denied(
                SecurityDenialReason::CapabilityScopeMismatch,
            ));
        }
        Ok(())
    }
}

impl<L, A> SqliteAttemptCapabilityRepository<L, A>
where
    L: TaskLeasePort,
    A: ExecutionAuditPort,
{
    async fn deny<T>(
        &self,
        scope: &ExecutionSecurityScope,
        capability_id: Option<&AttemptCapabilityId>,
        action: ProtectedAction,
        reason: SecurityDenialReason,
        observed_at: i64,
    ) -> Result<T, SecurityError> {
        let payload = json!({
            "capability_id": capability_id.map(AttemptCapabilityId::as_str),
            "reason": reason.as_str(),
            "action": action.as_str(),
            "execution_task_id": scope.task_lease_claim.execution_task_id,
            "worker_incarnation_id": scope.worker_incarnation_id,
            "lease_id": scope.task_lease_claim.lease_id,
            "fencing_token": scope.task_lease_claim.fencing_token.value(),
            "authority_key": scope.authority_key,
            "organization_id": scope.organization_ref.resource_id(),
            "project_id": scope.project_ref.resource_id(),
            "snapshot_id": scope.execution_snapshot_ref.resource_id(),
        });
        let audit_id = denial_audit_id(capability_id, reason, observed_at);
        let append = ExecutionAuditAppend {
            id: audit_id.clone(),
            idempotency_scope: format!(
                "security-denial:{}",
                scope.task_lease_claim.execution_task_id
            ),
            idempotency_key: audit_id.to_string(),
            event_type: DENIAL_EVENT_TYPE.to_string(),
            actor_kind: AuditActorKind::ControlPlane,
            actor_ref: "agentd-security".to_string(),
            payload_sha256: sha256_json(&payload)?,
            payload,
            links: audit_links(scope),
            execution_artifact_id: None,
            occurred_at: observed_at,
        };
        self.audit_port
            .append_audit(&append)
            .await
            .map_err(|error| {
                SecurityError::Unavailable(format!("required denial audit failed: {error}"))
            })?;
        Err(SecurityError::Denied(reason))
    }
}

struct StoredCapability {
    admission: CapabilityAdmission,
    revoked_at: Option<i64>,
}

fn row_to_stored(row: &sqlx::sqlite::SqliteRow) -> Result<StoredCapability, SecurityError> {
    let authority_key =
        AuthorityKey::new(row.get::<String, _>("authority_key")).map_err(corrupt_row)?;
    let organization_ref = OrganizationRef::new(
        authority_key.clone(),
        row.get::<String, _>("organization_id"),
        row.get::<String, _>("organization_version"),
    )
    .map_err(corrupt_row)?;
    let project_ref = ProjectRef::new(
        authority_key.clone(),
        row.get::<String, _>("project_id"),
        row.get::<String, _>("project_version"),
    )
    .map_err(corrupt_row)?;
    let execution_snapshot_ref = ProjectExecutionSnapshotRef::new(
        authority_key.clone(),
        row.get::<String, _>("snapshot_id"),
        row.get::<String, _>("snapshot_version"),
    )
    .map_err(corrupt_row)?;
    let rbac_policy_version_ref = RbacPolicyVersionRef::new(
        authority_key.clone(),
        row.get::<String, _>("rbac_policy_id"),
        row.get::<String, _>("rbac_policy_version"),
    )
    .map_err(corrupt_row)?;
    let worker_incarnation_id =
        WorkerIncarnationId::from_string(row.get::<String, _>("worker_incarnation_id"));
    let fencing_value: i64 = row.get("fencing_token");
    let fencing_value = u64::try_from(fencing_value).map_err(corrupt_row)?;
    let fencing_token = FencingToken::new(fencing_value).map_err(corrupt_row)?;
    let resource_json: String = row.get("resource_json");
    let resource: ProtectedResource = serde_json::from_str(&resource_json).map_err(corrupt_row)?;
    let action = protected_action(row.get::<String, _>("action").as_str())?;
    let role = workload_role(row.get::<String, _>("workload_role").as_str())?;
    let epoch: i64 = row.get("policy_revocation_epoch");
    let epoch = u64::try_from(epoch).map_err(corrupt_row)?;
    let scope = ExecutionSecurityScope {
        authority_key,
        organization_ref,
        project_ref,
        execution_snapshot_ref,
        rbac_policy_version_ref,
        worker_incarnation_id: worker_incarnation_id.clone(),
        task_lease_claim: TaskLeaseClaim {
            execution_task_id: TaskRunId::from_string(row.get::<String, _>("execution_task_id")),
            worker_incarnation_id: worker_incarnation_id.clone(),
            lease_id: agentd_core::types::LeaseId::from_string(row.get::<String, _>("lease_id")),
            fencing_token,
        },
        sandbox_profile_id: row.get("sandbox_profile_id"),
        egress_profile_id: row.get("egress_profile_id"),
        policy_revocation_epoch: epoch,
        valid_until: row.get("scope_valid_until"),
        audit_context: agentd_core::types::SecurityAuditContext {
            execution_run_id: RunId::from_string(row.get::<String, _>("execution_run_id")),
            snapshot_content_sha256: row.get("snapshot_content_sha256"),
            target_repository_id: row.get("target_repository_id"),
            target_base_commit: row.get("target_base_commit"),
        },
    };
    let workload = AuthenticatedWorkload {
        spiffe_uri: row.get("spiffe_uri"),
        role,
        trust_domain: row.get("trust_domain"),
        certificate_sha256: row.get("certificate_sha256"),
        not_before: row.get("certificate_not_before"),
        not_after: row.get("certificate_not_after"),
        worker_id: Some(WorkerId::from_string(row.get::<String, _>("worker_id"))),
        worker_incarnation_id: Some(worker_incarnation_id),
    };
    Ok(StoredCapability {
        admission: CapabilityAdmission {
            id: AttemptCapabilityId::from_string(row.get::<String, _>("id")),
            workload,
            scope,
            action,
            resource,
            issued_at: row.get("issued_at"),
            expires_at: row.get("expires_at"),
        },
        revoked_at: row.get("revoked_at"),
    })
}

fn validate_authorization(authorization: &TenantAuthorization) -> Result<(), SecurityError> {
    authorization
        .scope
        .authorize_resource(&authorization.resource)
        .map_err(SecurityError::Denied)?;
    if !action_matches_resource(authorization.action, &authorization.resource.kind) {
        return Err(SecurityError::Denied(SecurityDenialReason::ActionDenied));
    }
    if authorization.authorized_at < authorization.workload.not_before
        || authorization.authorized_at >= authorization.workload.not_after
        || authorization.authorized_at >= authorization.expires_at
        || authorization.authorized_at >= authorization.scope.valid_until
    {
        return Err(SecurityError::Denied(SecurityDenialReason::IdentityExpired));
    }
    if authorization.workload.role != WorkloadRole::Worker
        || authorization.workload.worker_incarnation_id.as_ref()
            != Some(&authorization.scope.worker_incarnation_id)
        || authorization.scope.task_lease_claim.worker_incarnation_id
            != authorization.scope.worker_incarnation_id
    {
        return Err(SecurityError::Denied(
            SecurityDenialReason::IncarnationStale,
        ));
    }
    Ok(())
}

fn action_matches_resource(action: ProtectedAction, resource: &ProtectedResourceKind) -> bool {
    matches!(
        (action, resource),
        (
            ProtectedAction::SandboxPrepare | ProtectedAction::SandboxExecute,
            ProtectedResourceKind::Execution
        ) | (
            ProtectedAction::SecretCheckout,
            ProtectedResourceKind::Secret(_)
        ) | (
            ProtectedAction::ArtifactRead | ProtectedAction::ArtifactWrite,
            ProtectedResourceKind::Artifact(_)
        ) | (
            ProtectedAction::ForgeRead | ProtectedAction::ForgeWrite,
            ProtectedResourceKind::Forge(_) | ProtectedResourceKind::Repository(_)
        ) | (
            ProtectedAction::ToolHighRisk,
            ProtectedResourceKind::Tool(_)
        )
    )
}

async fn validate_current_incarnation(
    pool: &SqlitePool,
    workload: &AuthenticatedWorkload,
    scope: &ExecutionSecurityScope,
) -> Result<(), SecurityError> {
    let worker_id = workload.worker_id.as_ref().ok_or(SecurityError::Denied(
        SecurityDenialReason::IncarnationStale,
    ))?;
    let incarnation_id = workload
        .worker_incarnation_id
        .as_ref()
        .ok_or(SecurityError::Denied(
            SecurityDenialReason::IncarnationStale,
        ))?;
    if incarnation_id != &scope.worker_incarnation_id {
        return Err(SecurityError::Denied(
            SecurityDenialReason::IncarnationStale,
        ));
    }
    let record = worker_repo::get_incarnation(pool, incarnation_id)
        .await
        .map_err(storage_unavailable)?
        .ok_or(SecurityError::Denied(
            SecurityDenialReason::IncarnationStale,
        ))?;
    if !record.is_current || &record.worker_id != worker_id {
        return Err(SecurityError::Denied(
            SecurityDenialReason::IncarnationStale,
        ));
    }
    Ok(())
}

async fn validate_current_epoch(
    pool: &SqlitePool,
    scope: &ExecutionSecurityScope,
) -> Result<(), SecurityError> {
    let current: Option<i64> = sqlx::query_scalar(
        "SELECT current_epoch FROM execution_security_policy_epochs \
         WHERE authority_key = ? AND organization_id = ? AND organization_version = ? \
           AND project_id = ? AND project_version = ? AND snapshot_id = ? \
           AND snapshot_version = ?",
    )
    .bind(scope.authority_key.as_str())
    .bind(scope.organization_ref.resource_id())
    .bind(scope.organization_ref.resource_version())
    .bind(scope.project_ref.resource_id())
    .bind(scope.project_ref.resource_version())
    .bind(scope.execution_snapshot_ref.resource_id())
    .bind(scope.execution_snapshot_ref.resource_version())
    .fetch_optional(pool)
    .await
    .map_err(storage_unavailable)?;
    let current = current.ok_or_else(|| {
        SecurityError::Unavailable("security policy epoch is not configured".to_string())
    })?;
    let expected = i64::try_from(scope.policy_revocation_epoch)
        .map_err(|_| SecurityError::Invalid("policy epoch exceeds SQLite range".to_string()))?;
    if current != expected {
        return Err(SecurityError::Denied(
            SecurityDenialReason::CapabilityRevoked,
        ));
    }
    Ok(())
}

fn token_digest(token: &CapabilityToken) -> String {
    hex::encode(Sha256::digest(token.expose_secret()))
}

fn audit_links(scope: &ExecutionSecurityScope) -> ExecutionEvidenceLinks {
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

fn denial_audit_id(
    capability_id: Option<&AttemptCapabilityId>,
    reason: SecurityDenialReason,
    observed_at: i64,
) -> agentd_core::types::AuditEventId {
    let Some(capability_id) = capability_id else {
        return agentd_core::types::AuditEventId::new();
    };
    let digest = Sha256::digest(format!("{capability_id}:{reason}:{observed_at}"));
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    agentd_core::types::AuditEventId::from_string(format!("ae_{}", ulid::Ulid::from_bytes(bytes)))
}

fn sha256_json(value: &Value) -> Result<String, SecurityError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| SecurityError::Invalid(format!("invalid security audit: {error}")))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn map_issue_lease_error(error: TaskLeaseError) -> SecurityError {
    match error {
        TaskLeaseError::Unavailable(message) => SecurityError::Unavailable(message),
        TaskLeaseError::Invalid(message) => SecurityError::Invalid(message),
        TaskLeaseError::Rejected { .. }
        | TaskLeaseError::NotFound(_)
        | TaskLeaseError::Conflict(_) => SecurityError::Denied(SecurityDenialReason::LeaseRejected),
    }
}

fn workload_role_text(role: WorkloadRole) -> &'static str {
    match role {
        WorkloadRole::ControlPlane => "control_plane",
        WorkloadRole::Gateway => "gateway",
        WorkloadRole::Worker => "worker",
    }
}

fn workload_role(value: &str) -> Result<WorkloadRole, SecurityError> {
    match value {
        "control_plane" => Ok(WorkloadRole::ControlPlane),
        "gateway" => Ok(WorkloadRole::Gateway),
        "worker" => Ok(WorkloadRole::Worker),
        _ => Err(SecurityError::Unavailable(
            "stored capability has an invalid workload role".to_string(),
        )),
    }
}

async fn validate_identity_binding(
    pool: &SqlitePool,
    request: &WorkloadIdentityBindingCreate,
) -> Result<(), SecurityError> {
    if !is_sha256(&request.certificate_sha256)
        || request.spiffe_uri.trim().is_empty()
        || !valid_trust_domain(&request.trust_domain)
        || request.not_before < 0
        || request.not_after <= request.not_before
        || request.created_at < request.not_before
        || request.created_at >= request.not_after
    {
        return Err(SecurityError::Invalid(
            "invalid workload identity binding".to_string(),
        ));
    }
    match request.role {
        WorkloadRole::Worker => {
            let worker_id = request.worker_id.as_ref().ok_or(SecurityError::Denied(
                SecurityDenialReason::IncarnationStale,
            ))?;
            let incarnation_id =
                request
                    .worker_incarnation_id
                    .as_ref()
                    .ok_or(SecurityError::Denied(
                        SecurityDenialReason::IncarnationStale,
                    ))?;
            let incarnation = worker_repo::get_incarnation(pool, incarnation_id)
                .await
                .map_err(storage_unavailable)?
                .ok_or(SecurityError::Denied(
                    SecurityDenialReason::IncarnationStale,
                ))?;
            let worker = worker_repo::get_worker(pool, worker_id)
                .await
                .map_err(storage_unavailable)?
                .ok_or(SecurityError::Denied(
                    SecurityDenialReason::IncarnationStale,
                ))?;
            let expected_spiffe =
                format!("spiffe://{}/worker/{incarnation_id}", request.trust_domain);
            if !valid_typed_id(worker_id.as_str(), "wk_")
                || !valid_typed_id(incarnation_id.as_str(), "wi_")
                || !incarnation.is_current
                || &incarnation.worker_id != worker_id
                || worker.trust_domain != request.trust_domain
                || request.spiffe_uri != expected_spiffe
            {
                return Err(SecurityError::Denied(
                    SecurityDenialReason::IncarnationStale,
                ));
            }
        }
        WorkloadRole::ControlPlane | WorkloadRole::Gateway => {
            if request.worker_id.is_some() || request.worker_incarnation_id.is_some() {
                return Err(SecurityError::Invalid(
                    "non-worker identity cannot bind a worker incarnation".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_typed_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|payload| {
        payload.len() == 26
            && payload
                .parse::<ulid::Ulid>()
                .is_ok_and(|parsed| parsed.to_string() == payload)
    })
}

fn valid_trust_domain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn identity_binding_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<WorkloadIdentityBindingRecord, SecurityError> {
    Ok(WorkloadIdentityBindingRecord {
        binding: WorkloadIdentityBindingCreate {
            certificate_sha256: row.get("certificate_sha256"),
            spiffe_uri: row.get("spiffe_uri"),
            role: workload_role(row.get::<String, _>("role").as_str())?,
            trust_domain: row.get("trust_domain"),
            worker_id: row
                .get::<Option<String>, _>("worker_id")
                .map(WorkerId::from_string),
            worker_incarnation_id: row
                .get::<Option<String>, _>("worker_incarnation_id")
                .map(WorkerIncarnationId::from_string),
            not_before: row.get("not_before"),
            not_after: row.get("not_after"),
            created_at: row.get("created_at"),
        },
        revoked_at: row.get("revoked_at"),
        revocation_reason: row.get("revocation_reason"),
    })
}

fn protected_action(value: &str) -> Result<ProtectedAction, SecurityError> {
    match value {
        "sandbox.prepare" => Ok(ProtectedAction::SandboxPrepare),
        "sandbox.execute" => Ok(ProtectedAction::SandboxExecute),
        "secret.checkout" => Ok(ProtectedAction::SecretCheckout),
        "artifact.read" => Ok(ProtectedAction::ArtifactRead),
        "artifact.write" => Ok(ProtectedAction::ArtifactWrite),
        "forge.read" => Ok(ProtectedAction::ForgeRead),
        "forge.write" => Ok(ProtectedAction::ForgeWrite),
        "tool.high_risk" => Ok(ProtectedAction::ToolHighRisk),
        _ => Err(SecurityError::Unavailable(
            "stored capability has an invalid action".to_string(),
        )),
    }
}

fn corrupt_row(error: impl std::fmt::Display) -> SecurityError {
    SecurityError::Unavailable(format!("stored capability is corrupt: {error}"))
}

fn storage_unavailable(error: impl std::fmt::Display) -> SecurityError {
    SecurityError::Unavailable(format!("execution security storage unavailable: {error}"))
}
