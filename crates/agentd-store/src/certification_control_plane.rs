//! Durable AD-E4 execution-evidence and `OpenFab` certification control plane.

use agentd_core::ports::{
    CertificationError, CertificationRequest, CertificationStatePort, CertificationStateTransition,
    CertificationVerdict, DeliveryCertificationState, EvidenceEnvelopeStorePort,
    EvidenceSignerRole, ForgeAdmission, ForgeAdmissionRequest, SignedCertificationResult,
    SignedExecutionEvidenceEnvelope, SigningKeyTrustPort, SkillInstallAdmission, TrustedSigningKey,
    canonical_sha256,
};
use agentd_core::types::{
    CertificationGate, CertificationPolicyVersionRef, CertificationRequestId,
    CertificationResultId, EvidenceEnvelopeId, ForgeAdmissionId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenFabProtocolEvent {
    pub sequence: i64,
    pub event_key: String,
    pub event_kind: OpenFabProtocolEventKind,
    pub aggregate_id: String,
    pub payload_sha256: String,
    pub payload_json: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenFabProtocolEventKind {
    CertificationRequest,
    CertificationResult,
}

impl OpenFabProtocolEventKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CertificationRequest => "certification_request",
            Self::CertificationResult => "certification_result",
        }
    }

    fn parse(value: &str) -> Result<Self, CertificationError> {
        match value {
            "certification_request" => Ok(Self::CertificationRequest),
            "certification_result" => Ok(Self::CertificationResult),
            _ => Err(CertificationError::Unavailable(
                "stored OpenFab protocol event kind is invalid".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SqliteCertificationControlPlane {
    pool: SqlitePool,
}

impl SqliteCertificationControlPlane {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub const fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn register_signing_key(
        &self,
        key: &TrustedSigningKey,
        registered_at: i64,
    ) -> Result<TrustedSigningKey, CertificationError> {
        validate_key(key, registered_at)?;
        if let Some(existing) = load_key(&self.pool, &key.key_id).await? {
            return if existing == *key {
                Ok(existing)
            } else {
                Err(CertificationError::Conflict(
                    "signing key id already has different trust material".to_string(),
                ))
            };
        }
        sqlx::query(
            "INSERT INTO trusted_evidence_signing_keys (key_id, signer_did, signer_role, \
             not_before, not_after, revoked_at, superseded_by, registered_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&key.key_id)
        .bind(&key.signer_did)
        .bind(key.role.as_str())
        .bind(key.not_before)
        .bind(key.not_after)
        .bind(key.revoked_at)
        .bind(&key.superseded_by)
        .bind(registered_at)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(key.clone())
    }

    pub async fn revoke_signing_key(
        &self,
        key_id: &str,
        revoked_at: i64,
        superseded_by: Option<&str>,
    ) -> Result<TrustedSigningKey, CertificationError> {
        let current = load_key(&self.pool, key_id)
            .await?
            .ok_or_else(|| CertificationError::NotFound(format!("signing key {key_id}")))?;
        if revoked_at < current.not_before || revoked_at > current.not_after {
            return Err(CertificationError::Invalid(
                "signing key revocation is outside its validity window".to_string(),
            ));
        }
        if let Some(replacement) = superseded_by {
            if replacement == key_id || load_key(&self.pool, replacement).await?.is_none() {
                return Err(CertificationError::Invalid(
                    "signing key replacement must name another registered key".to_string(),
                ));
            }
        }
        if current.revoked_at.is_some() || current.superseded_by.is_some() {
            if current.revoked_at == Some(revoked_at)
                && current.superseded_by.as_deref() == superseded_by
            {
                return Ok(current);
            }
            return Err(CertificationError::Conflict(
                "signing key lifecycle is already terminal".to_string(),
            ));
        }
        sqlx::query(
            "UPDATE trusted_evidence_signing_keys SET revoked_at = ?, superseded_by = ? \
             WHERE key_id = ? AND revoked_at IS NULL AND superseded_by IS NULL",
        )
        .bind(revoked_at)
        .bind(superseded_by)
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        load_key(&self.pool, key_id).await?.ok_or_else(|| {
            CertificationError::Unavailable("revoked signing key disappeared".to_string())
        })
    }

    pub async fn pending_protocol_events(
        &self,
        limit: u32,
    ) -> Result<Vec<OpenFabProtocolEvent>, CertificationError> {
        if limit == 0 || limit > 1_000 {
            return Err(CertificationError::Invalid(
                "OpenFab protocol event limit must be between 1 and 1000".to_string(),
            ));
        }
        let rows = sqlx::query(
            "SELECT sequence, event_key, event_kind, aggregate_id, payload_sha256, \
             payload_json, created_at FROM openfab_protocol_outbox \
             WHERE delivered_at IS NULL ORDER BY sequence LIMIT ?",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        rows.iter().map(protocol_event_from_row).collect()
    }

    pub async fn acknowledge_protocol_event(
        &self,
        event_key: &str,
        delivered_at: i64,
    ) -> Result<(), CertificationError> {
        if event_key.trim().is_empty() || delivered_at < 0 {
            return Err(CertificationError::Invalid(
                "invalid OpenFab event acknowledgement".to_string(),
            ));
        }
        let result = sqlx::query(
            "UPDATE openfab_protocol_outbox SET delivered_at = ? \
             WHERE event_key = ? AND delivered_at IS NULL",
        )
        .bind(delivered_at)
        .bind(event_key)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        if result.rows_affected() == 0 {
            let existing: Option<i64> = sqlx::query_scalar(
                "SELECT delivered_at FROM openfab_protocol_outbox WHERE event_key = ?",
            )
            .bind(event_key)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .flatten();
            if existing != Some(delivered_at) {
                return Err(CertificationError::Conflict(
                    "OpenFab event acknowledgement is missing or differs".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SigningKeyTrustPort for SqliteCertificationControlPlane {
    async fn resolve_signing_key(
        &self,
        key_id: &str,
        role: EvidenceSignerRole,
        signed_at: i64,
    ) -> Result<TrustedSigningKey, CertificationError> {
        let key = load_key(&self.pool, key_id)
            .await?
            .ok_or_else(|| CertificationError::NotFound(format!("signing key {key_id}")))?;
        if key.role != role
            || signed_at < key.not_before
            || signed_at >= key.not_after
            || key.revoked_at.is_some_and(|at| signed_at >= at)
        {
            return Err(CertificationError::Denied(
                "signing key is not trusted for this role and timestamp".to_string(),
            ));
        }
        Ok(key)
    }
}

#[async_trait::async_trait]
impl EvidenceEnvelopeStorePort for SqliteCertificationControlPlane {
    async fn store_evidence_envelope(
        &self,
        envelope: &SignedExecutionEvidenceEnvelope,
    ) -> Result<SignedExecutionEvidenceEnvelope, CertificationError> {
        let envelope_json = json(envelope)?;
        let envelope_sha256 = sha256(envelope_json.as_bytes());
        if let Some(existing) = load_evidence_by_identity(
            &self.pool,
            envelope.payload.envelope_id.as_str(),
            &envelope.payload_sha256,
        )
        .await?
        {
            return if existing == *envelope {
                Ok(existing)
            } else {
                Err(CertificationError::Conflict(
                    "evidence envelope identity already has different bytes".to_string(),
                ))
            };
        }
        let snapshot = envelope.payload.snapshot_ref.as_resource_ref();
        sqlx::query(
            "INSERT INTO signed_execution_evidence (envelope_id, schema_version, \
             payload_sha256, envelope_sha256, envelope_json, execution_run_id, \
             execution_task_id, snapshot_authority_key, snapshot_resource_id, \
             snapshot_resource_version, snapshot_content_sha256, signer_key_id, signer_did, \
             signer_role, signed_at, stored_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(envelope.payload.envelope_id.as_str())
        .bind(i64::from(envelope.schema_version))
        .bind(&envelope.payload_sha256)
        .bind(envelope_sha256)
        .bind(envelope_json)
        .bind(envelope.payload.execution_run_id.as_str())
        .bind(
            envelope
                .payload
                .execution_task_id
                .as_ref()
                .map(agentd_core::types::TaskRunId::as_str),
        )
        .bind(snapshot.authority_key().as_str())
        .bind(snapshot.resource_id())
        .bind(snapshot.resource_version())
        .bind(&envelope.payload.snapshot_content_sha256)
        .bind(&envelope.signer_key_id)
        .bind(&envelope.signer_did)
        .bind(envelope.signer_role.as_str())
        .bind(envelope.signed_at)
        .bind(envelope.signed_at)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(envelope.clone())
    }

    async fn load_evidence_envelope(
        &self,
        envelope_id: &EvidenceEnvelopeId,
    ) -> Result<Option<SignedExecutionEvidenceEnvelope>, CertificationError> {
        load_evidence_by_identity(&self.pool, envelope_id.as_str(), "").await
    }
}

#[allow(clippy::too_many_lines)]
#[async_trait::async_trait]
impl CertificationStatePort for SqliteCertificationControlPlane {
    async fn record_certification_request(
        &self,
        request: &CertificationRequest,
    ) -> Result<CertificationRequest, CertificationError> {
        validate_certification_request(request)?;
        let request_json = json(request)?;
        let request_sha256 = sha256(request_json.as_bytes());
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        if let Some(existing) = existing_request(&mut transaction, request).await? {
            transaction.commit().await.map_err(storage_error)?;
            return if existing == *request {
                Ok(existing)
            } else {
                Err(CertificationError::Conflict(
                    "certification request replay differs".to_string(),
                ))
            };
        }
        let evidence = sqlx::query(
            "SELECT payload_sha256, envelope_json, snapshot_authority_key, snapshot_resource_id, \
             snapshot_resource_version, snapshot_content_sha256 FROM signed_execution_evidence \
             WHERE envelope_id = ?",
        )
        .bind(request.envelope_id.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| CertificationError::NotFound("execution evidence envelope".to_string()))?;
        let snapshot = request.snapshot_ref.as_resource_ref();
        let evidence_envelope: SignedExecutionEvidenceEnvelope =
            from_json(evidence.get("envelope_json"))?;
        let certification_policy = request.certification_policy_ref.as_resource_ref();
        let evidence_has_policy = evidence_envelope
            .payload
            .policy_refs
            .iter()
            .any(|reference| {
                reference.authority_key == certification_policy.authority_key().as_str()
                    && reference.resource_kind == "certification_policy"
                    && reference.resource_id == certification_policy.resource_id()
                    && reference.resource_version == certification_policy.resource_version()
                    && reference.content_sha256 == request.certification_policy_sha256
            });
        if evidence.get::<String, _>("payload_sha256") != request.evidence_payload_sha256
            || evidence.get::<String, _>("snapshot_authority_key")
                != snapshot.authority_key().as_str()
            || evidence.get::<String, _>("snapshot_resource_id") != snapshot.resource_id()
            || evidence.get::<String, _>("snapshot_resource_version") != snapshot.resource_version()
            || evidence.get::<String, _>("snapshot_content_sha256")
                != request.snapshot_content_sha256
            || evidence_envelope.payload.skill_packages != request.skill_packages
            || canonical_sha256(&request.skill_packages)? != request.skill_packages_sha256
            || evidence_envelope.payload.produced_commit.as_deref()
                != Some(request.source_commit.as_str())
            || evidence_envelope.payload.frozen_spec.content_sha256 != request.spec_sha256
            || !evidence_envelope
                .payload
                .artifacts
                .iter()
                .any(|artifact| artifact.content_sha256 == request.subject_sha256)
            || !evidence_has_policy
        {
            return Err(CertificationError::Denied(
                "certification request does not bind the stored evidence".to_string(),
            ));
        }
        let policy = certification_policy;
        sqlx::query(
            "INSERT INTO openfab_certification_requests (request_id, idempotency_key, \
             request_sha256, request_json, openfab_authority_key, envelope_id, \
             evidence_payload_sha256, snapshot_authority_key, snapshot_resource_id, \
             snapshot_resource_version, snapshot_content_sha256, source_commit, subject_sha256, \
             spec_sha256, policy_authority_key, policy_resource_id, policy_resource_version, policy_sha256, \
             gate_json, skill_packages_sha256, requested_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(request.request_id.as_str())
        .bind(request.idempotency_key.trim())
        .bind(&request_sha256)
        .bind(&request_json)
        .bind(request.openfab_authority_key.trim())
        .bind(request.envelope_id.as_str())
        .bind(&request.evidence_payload_sha256)
        .bind(snapshot.authority_key().as_str())
        .bind(snapshot.resource_id())
        .bind(snapshot.resource_version())
        .bind(&request.snapshot_content_sha256)
        .bind(&request.source_commit)
        .bind(&request.subject_sha256)
        .bind(&request.spec_sha256)
        .bind(policy.authority_key().as_str())
        .bind(policy.resource_id())
        .bind(policy.resource_version())
        .bind(&request.certification_policy_sha256)
        .bind(json(&request.gate)?)
        .bind(&request.skill_packages_sha256)
        .bind(request.requested_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        insert_protocol_event(
            &mut transaction,
            &format!("request:{}", request.request_id),
            OpenFabProtocolEventKind::CertificationRequest,
            request.request_id.as_str(),
            &request_sha256,
            &request_json,
            request.requested_at,
        )
        .await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(request.clone())
    }

    async fn record_certification_result(
        &self,
        result: &SignedCertificationResult,
    ) -> Result<SignedCertificationResult, CertificationError> {
        validate_result_shape(result)?;
        let result_json = json(result)?;
        let envelope_sha256 = sha256(result_json.as_bytes());
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        if let Some(existing) = existing_result(&mut transaction, result).await? {
            transaction.commit().await.map_err(storage_error)?;
            return if existing == *result {
                Ok(existing)
            } else {
                Err(CertificationError::Conflict(
                    "OpenFab result replay differs".to_string(),
                ))
            };
        }
        let request = load_request_row(&mut transaction, &result.payload.request_id).await?;
        ensure_result_matches_request(result, &request)?;
        let trusted = load_key_tx(&mut transaction, &result.signer_key_id)
            .await?
            .ok_or_else(|| CertificationError::NotFound("OpenFab signing key".to_string()))?;
        if trusted.role != EvidenceSignerRole::OpenFab
            || trusted.signer_did != result.signer_did
            || result.payload.published_at < trusted.not_before
            || result.payload.published_at >= trusted.not_after
            || trusted
                .revoked_at
                .is_some_and(|at| result.payload.published_at >= at)
        {
            return Err(CertificationError::Denied(
                "OpenFab result signer is not trusted at publication time".to_string(),
            ));
        }
        let policy = result.payload.certification_policy_ref.as_resource_ref();
        sqlx::query(
            "INSERT INTO openfab_certification_results (result_id, request_id, \
             result_payload_sha256, result_envelope_sha256, result_json, openfab_authority_key, \
             evidence_payload_sha256, snapshot_content_sha256, source_commit, subject_sha256, \
             spec_sha256, policy_authority_key, policy_resource_id, policy_resource_version, policy_sha256, \
             skill_packages_sha256, \
             verdict, machine_attested, required_human_signoffs, eligible_human_signoffs, \
             accepted_human_signoffs, signer_key_id, signer_did, published_at, revoked_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(result.payload.result_id.as_str())
        .bind(result.payload.request_id.as_str())
        .bind(&result.payload_sha256)
        .bind(envelope_sha256)
        .bind(&result_json)
        .bind(&result.payload.openfab_authority_key)
        .bind(&result.payload.evidence_payload_sha256)
        .bind(&result.payload.snapshot_content_sha256)
        .bind(&result.payload.source_commit)
        .bind(&result.payload.subject_sha256)
        .bind(&result.payload.spec_sha256)
        .bind(policy.authority_key().as_str())
        .bind(policy.resource_id())
        .bind(policy.resource_version())
        .bind(&result.payload.certification_policy_sha256)
        .bind(&result.payload.skill_packages_sha256)
        .bind(result.payload.verdict.as_str())
        .bind(result.payload.machine_attested)
        .bind(i64::from(result.payload.required_human_signoffs))
        .bind(i64::from(result.payload.eligible_human_signoffs))
        .bind(i64::from(result.payload.accepted_human_signoffs))
        .bind(&result.signer_key_id)
        .bind(&result.signer_did)
        .bind(result.payload.published_at)
        .bind(result.payload.revoked_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        insert_protocol_event(
            &mut transaction,
            &format!("result:{}", result.payload.result_id),
            OpenFabProtocolEventKind::CertificationResult,
            result.payload.result_id.as_str(),
            &result.payload_sha256,
            &result_json,
            result.payload.published_at,
        )
        .await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(result.clone())
    }

    async fn transition_certification_state(
        &self,
        transition: &CertificationStateTransition,
    ) -> Result<CertificationStateTransition, CertificationError> {
        validate_transition(transition)?;
        let transition_json = json(transition)?;
        let transition_sha256 = sha256(transition_json.as_bytes());
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        if let Some(existing) = sqlx::query(
            "SELECT transition_json FROM artifact_certification_state_events \
             WHERE idempotency_key = ?",
        )
        .bind(transition.idempotency_key.trim())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?
        {
            let stored: CertificationStateTransition = from_json(existing.get("transition_json"))?;
            transaction.commit().await.map_err(storage_error)?;
            return if stored == *transition {
                Ok(stored)
            } else {
                Err(CertificationError::Conflict(
                    "certification state transition replay differs".to_string(),
                ))
            };
        }
        let current: Option<String> = sqlx::query_scalar(
            "SELECT next_state FROM artifact_certification_state_events \
             WHERE execution_artifact_id = ? ORDER BY sequence DESC LIMIT 1",
        )
        .bind(transition.execution_artifact_id.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let current = current.as_deref().map(parse_state).transpose()?;
        if current != transition.previous_state
            || !legal_transition(transition.previous_state, transition.next_state)
        {
            return Err(CertificationError::Denied(
                "illegal or stale certification state transition".to_string(),
            ));
        }
        sqlx::query(
            "INSERT INTO artifact_certification_state_events (idempotency_key, \
             transition_sha256, transition_json, execution_artifact_id, previous_state, \
             next_state, certification_result_id, reason_code, observed_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(transition.idempotency_key.trim())
        .bind(transition_sha256)
        .bind(transition_json)
        .bind(transition.execution_artifact_id.as_str())
        .bind(
            transition
                .previous_state
                .map(DeliveryCertificationState::as_str),
        )
        .bind(transition.next_state.as_str())
        .bind(
            transition
                .certification_result_id
                .as_ref()
                .map(CertificationResultId::as_str),
        )
        .bind(transition.reason_code.trim())
        .bind(transition.observed_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(transition.clone())
    }

    async fn admit_forge_operation(
        &self,
        request: &ForgeAdmissionRequest,
    ) -> Result<ForgeAdmission, CertificationError> {
        validate_forge_request(request)?;
        let request_json = json(request)?;
        let admission_sha256 = sha256(request_json.as_bytes());
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        if let Some(existing) = sqlx::query(
            "SELECT admission_sha256, admission_id, certification_result_id, \
             result_payload_sha256, admitted_at FROM forge_admissions WHERE idempotency_key = ?",
        )
        .bind(request.idempotency_key.trim())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?
        {
            if existing.get::<String, _>("admission_sha256") != admission_sha256 {
                return Err(CertificationError::Conflict(
                    "forge admission replay differs".to_string(),
                ));
            }
            let admission = forge_admission_from_request(
                ForgeAdmissionId::from_string(existing.get::<String, _>("admission_id")),
                request,
                existing.get::<i64, _>("admitted_at"),
            );
            transaction.commit().await.map_err(storage_error)?;
            return Ok(admission);
        }
        authorize_forge(&mut transaction, request).await?;
        let admission =
            forge_admission_from_request(ForgeAdmissionId::new(), request, request.observed_at);
        let snapshot = request.snapshot.snapshot_ref.as_resource_ref();
        let policy = request
            .snapshot
            .certification_policy_version_ref
            .as_ref()
            .map(CertificationPolicyVersionRef::as_resource_ref);
        sqlx::query(
            "INSERT INTO forge_admissions (admission_id, idempotency_key, admission_sha256, \
             operation, snapshot_authority_key, snapshot_resource_id, snapshot_resource_version, \
             snapshot_content_sha256, execution_artifact_id, source_commit, subject_sha256, \
             policy_authority_key, policy_resource_id, policy_resource_version, policy_sha256, \
             certification_result_id, result_payload_sha256, admitted_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(admission.id.as_str())
        .bind(request.idempotency_key.trim())
        .bind(admission_sha256)
        .bind(request.operation.as_str())
        .bind(snapshot.authority_key().as_str())
        .bind(snapshot.resource_id())
        .bind(snapshot.resource_version())
        .bind(&request.snapshot.content_sha256)
        .bind(request.execution_artifact_id.as_str())
        .bind(&request.source_commit)
        .bind(&request.subject_sha256)
        .bind(policy.map(|value| value.authority_key().as_str()))
        .bind(policy.map(agentd_core::types::AuthorityResourceRef::resource_id))
        .bind(policy.map(agentd_core::types::AuthorityResourceRef::resource_version))
        .bind(request.certification_policy_sha256.as_deref())
        .bind(
            request
                .certification_result
                .as_ref()
                .map(|result| result.payload.result_id.as_str()),
        )
        .bind(
            request
                .certification_result
                .as_ref()
                .map(|result| result.payload_sha256.as_str()),
        )
        .bind(request.observed_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(admission)
    }

    async fn record_skill_installation(
        &self,
        admission: &SkillInstallAdmission,
    ) -> Result<SkillInstallAdmission, CertificationError> {
        validate_skill_admission(admission)?;
        let admission_json = json(admission)?;
        let admission_sha256 = sha256(admission_json.as_bytes());
        let trust_json = json(&admission.trust_record)?;
        let snapshot = admission.snapshot_ref.as_resource_ref();
        let package = admission.package_ref.as_resource_ref();
        let trust_package = admission
            .trust_record
            .payload
            .package
            .package_ref
            .as_resource_ref();
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        if let Some(existing) = sqlx::query(
            "SELECT admission_json FROM skill_installations WHERE installation_id = ? \
             OR (snapshot_authority_key = ? AND snapshot_resource_id = ? \
             AND snapshot_resource_version = ? AND package_authority_key = ? \
             AND package_resource_id = ? AND package_resource_version = ? AND install_root_ref = ?)",
        )
        .bind(admission.id.as_str())
        .bind(snapshot.authority_key().as_str())
        .bind(snapshot.resource_id())
        .bind(snapshot.resource_version())
        .bind(package.authority_key().as_str())
        .bind(package.resource_id())
        .bind(package.resource_version())
        .bind(&admission.install_root_ref)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?
        {
            let stored: SkillInstallAdmission = from_json(existing.get("admission_json"))?;
            transaction.commit().await.map_err(storage_error)?;
            return if stored == *admission {
                Ok(stored)
            } else {
                Err(CertificationError::Conflict(
                    "skill installation identity already differs".to_string(),
                ))
            };
        }
        sqlx::query(
            "INSERT INTO skill_package_trust_observations (trust_payload_sha256, \
             package_authority_key, package_resource_id, package_resource_version, archive_sha256, \
             trust_status, trust_record_json, signer_key_id, signer_did, status_changed_at, \
             valid_until, observed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(trust_payload_sha256) DO NOTHING",
        )
        .bind(&admission.trust_record.trust_payload_sha256)
        .bind(trust_package.authority_key().as_str())
        .bind(trust_package.resource_id())
        .bind(trust_package.resource_version())
        .bind(&admission.archive_sha256)
        .bind(admission.trust_status_at_install.as_str())
        .bind(&trust_json)
        .bind(&admission.trust_record.signer_key_id)
        .bind(&admission.trust_record.signer_did)
        .bind(admission.trust_record.payload.status_changed_at)
        .bind(admission.trust_record.payload.valid_until)
        .bind(admission.admitted_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let exact_trust: String = sqlx::query_scalar(
            "SELECT trust_record_json FROM skill_package_trust_observations \
             WHERE trust_payload_sha256 = ?",
        )
        .bind(&admission.trust_record.trust_payload_sha256)
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if exact_trust != trust_json {
            return Err(CertificationError::Conflict(
                "Skill Hub trust digest already binds different bytes".to_string(),
            ));
        }
        sqlx::query(
            "INSERT INTO skill_installations (installation_id, admission_sha256, admission_json, \
             snapshot_authority_key, snapshot_resource_id, snapshot_resource_version, \
             snapshot_content_sha256, package_authority_key, package_resource_id, \
             package_resource_version, archive_sha256, manifest_sha256, dependency_lock_sha256, \
             permissions_sha256, install_root_ref, trust_status_at_install, trust_payload_sha256, \
             admitted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(admission.id.as_str())
        .bind(admission_sha256)
        .bind(admission_json)
        .bind(snapshot.authority_key().as_str())
        .bind(snapshot.resource_id())
        .bind(snapshot.resource_version())
        .bind(&admission.snapshot_content_sha256)
        .bind(package.authority_key().as_str())
        .bind(package.resource_id())
        .bind(package.resource_version())
        .bind(&admission.archive_sha256)
        .bind(&admission.manifest_sha256)
        .bind(&admission.dependency_lock_sha256)
        .bind(&admission.permissions_sha256)
        .bind(&admission.install_root_ref)
        .bind(admission.trust_status_at_install.as_str())
        .bind(&admission.trust_record.trust_payload_sha256)
        .bind(admission.admitted_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(admission.clone())
    }
}

async fn authorize_forge(
    transaction: &mut Transaction<'_, Sqlite>,
    request: &ForgeAdmissionRequest,
) -> Result<(), CertificationError> {
    let artifact_sha256: String =
        sqlx::query_scalar("SELECT content_sha256 FROM execution_artifacts WHERE id = ?")
            .bind(request.execution_artifact_id.as_str())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(storage_error)?
            .ok_or_else(|| CertificationError::NotFound("forge subject artifact".to_string()))?;
    if artifact_sha256 != request.subject_sha256 {
        return Err(CertificationError::Denied(
            "forge subject digest does not match the immutable artifact".to_string(),
        ));
    }
    match request.snapshot.certification_gate {
        CertificationGate::None => {
            if let Some(result) = &request.certification_result {
                ensure_forge_result_exact(transaction, request, result).await?;
            }
        }
        CertificationGate::Machine => {
            let result = request.certification_result.as_ref().ok_or_else(|| {
                CertificationError::Denied("machine certification is required".to_string())
            })?;
            ensure_forge_result_exact(transaction, request, result).await?;
            if result.payload.verdict != CertificationVerdict::Pass
                || !result.payload.machine_attested
            {
                return Err(CertificationError::Denied(
                    "machine certification did not pass".to_string(),
                ));
            }
        }
        CertificationGate::Human { required, eligible } => {
            let result = request.certification_result.as_ref().ok_or_else(|| {
                CertificationError::Denied("human certification is required".to_string())
            })?;
            ensure_forge_result_exact(transaction, request, result).await?;
            if result.payload.verdict != CertificationVerdict::Pass
                || !result.payload.machine_attested
                || result.payload.required_human_signoffs != required
                || result.payload.eligible_human_signoffs != eligible
                || result.payload.accepted_human_signoffs < required
            {
                return Err(CertificationError::Denied(
                    "human certification threshold did not pass".to_string(),
                ));
            }
        }
    }
    Ok(())
}

async fn ensure_forge_result_exact(
    transaction: &mut Transaction<'_, Sqlite>,
    request: &ForgeAdmissionRequest,
    result: &SignedCertificationResult,
) -> Result<(), CertificationError> {
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT result_json FROM openfab_certification_results \
         WHERE result_id = ? AND result_payload_sha256 = ?",
    )
    .bind(result.payload.result_id.as_str())
    .bind(&result.payload_sha256)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?;
    if stored.as_deref() != Some(json(result)?.as_str()) {
        return Err(CertificationError::Denied(
            "forge admission requires the exact stored OpenFab result".to_string(),
        ));
    }
    let snapshot_ref = request.snapshot.snapshot_ref.as_resource_ref();
    let result_snapshot = result.payload.snapshot_ref.as_resource_ref();
    let policy = request
        .snapshot
        .certification_policy_version_ref
        .as_ref()
        .ok_or_else(|| {
            CertificationError::Denied("snapshot has no certification policy".to_string())
        })?;
    if result_snapshot != snapshot_ref
        || result.payload.snapshot_content_sha256 != request.snapshot.content_sha256
        || result.payload.source_commit != request.source_commit
        || result.payload.subject_sha256 != request.subject_sha256
        || &result.payload.certification_policy_ref != policy
        || Some(result.payload.certification_policy_sha256.as_str())
            != request.certification_policy_sha256.as_deref()
        || result.payload.verdict == CertificationVerdict::Revoked
        || result
            .payload
            .revoked_at
            .is_some_and(|revoked_at| revoked_at <= request.observed_at)
    {
        return Err(CertificationError::Denied(
            "OpenFab result does not bind the current forge subject and policy".to_string(),
        ));
    }
    Ok(())
}

fn forge_admission_from_request(
    id: ForgeAdmissionId,
    request: &ForgeAdmissionRequest,
    admitted_at: i64,
) -> ForgeAdmission {
    ForgeAdmission {
        id,
        operation: request.operation,
        snapshot_ref: request.snapshot.snapshot_ref.clone(),
        snapshot_content_sha256: request.snapshot.content_sha256.clone(),
        execution_artifact_id: request.execution_artifact_id.clone(),
        source_commit: request.source_commit.clone(),
        subject_sha256: request.subject_sha256.clone(),
        certification_policy_ref: request.snapshot.certification_policy_version_ref.clone(),
        certification_policy_sha256: request.certification_policy_sha256.clone(),
        certification_result_id: request
            .certification_result
            .as_ref()
            .map(|result| result.payload.result_id.clone()),
        certification_result_payload_sha256: request
            .certification_result
            .as_ref()
            .map(|result| result.payload_sha256.clone()),
        admitted_at,
    }
}

fn validate_forge_request(request: &ForgeAdmissionRequest) -> Result<(), CertificationError> {
    request
        .snapshot
        .validate()
        .map_err(|error| CertificationError::Invalid(error.to_string()))?;
    if request.idempotency_key.trim().is_empty()
        || request.observed_at < request.snapshot.issued_at
        || request.observed_at >= request.snapshot.valid_until
    {
        return Err(CertificationError::Denied(
            "forge request uses an invalid identity or expired snapshot".to_string(),
        ));
    }
    validate_sha256(&request.subject_sha256, "forge subject")?;
    validate_commit(&request.source_commit)?;
    match (
        request.snapshot.certification_gate,
        request.snapshot.certification_policy_version_ref.as_ref(),
        request.certification_policy_sha256.as_deref(),
    ) {
        (CertificationGate::None, None, None) => {}
        (_, Some(_), Some(digest)) => validate_sha256(digest, "certification policy")?,
        _ => {
            return Err(CertificationError::Denied(
                "forge request certification policy is incomplete".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_skill_admission(admission: &SkillInstallAdmission) -> Result<(), CertificationError> {
    if !admission.trust_status_at_install.permits_new_install()
        || admission.trust_record.payload.status != admission.trust_status_at_install
        || admission.trust_record.payload.package.package_ref != admission.package_ref
        || admission.trust_record.payload.package.archive_sha256 != admission.archive_sha256
        || admission.trust_record.payload.package.manifest_sha256 != admission.manifest_sha256
        || admission
            .trust_record
            .payload
            .package
            .dependency_lock_sha256
            != admission.dependency_lock_sha256
        || admission.trust_record.payload.package.permissions_sha256 != admission.permissions_sha256
        || admission.install_root_ref.trim().is_empty()
        || admission
            .install_root_ref
            .to_ascii_lowercase()
            .contains("latest")
        || admission.admitted_at < admission.trust_record.payload.status_changed_at
        || admission.admitted_at >= admission.trust_record.payload.valid_until
    {
        return Err(CertificationError::Denied(
            "skill installation does not bind an approved current package".to_string(),
        ));
    }
    for (name, value) in [
        ("snapshot", admission.snapshot_content_sha256.as_str()),
        ("skill archive", admission.archive_sha256.as_str()),
        ("skill manifest", admission.manifest_sha256.as_str()),
        (
            "skill dependency lock",
            admission.dependency_lock_sha256.as_str(),
        ),
        ("skill permissions", admission.permissions_sha256.as_str()),
        (
            "Skill Hub trust payload",
            admission.trust_record.trust_payload_sha256.as_str(),
        ),
    ] {
        validate_sha256(value, name)?;
    }
    Ok(())
}

fn validate_transition(
    transition: &CertificationStateTransition,
) -> Result<(), CertificationError> {
    if transition.idempotency_key.trim().is_empty()
        || transition.reason_code.trim().is_empty()
        || transition.observed_at < 0
    {
        return Err(CertificationError::Invalid(
            "invalid certification state transition".to_string(),
        ));
    }
    Ok(())
}

const fn legal_transition(
    previous: Option<DeliveryCertificationState>,
    next: DeliveryCertificationState,
) -> bool {
    use DeliveryCertificationState as State;
    matches!(
        (previous, next),
        (None, State::Produced)
            | (Some(State::Produced), State::Delivered | State::Revoked)
            | (
                Some(State::Delivered),
                State::MachineAttested | State::Released | State::Revoked
            )
            | (
                Some(State::MachineAttested),
                State::HumanCertified | State::Released | State::Revoked
            )
            | (
                Some(State::HumanCertified),
                State::Released | State::Revoked
            )
            | (Some(State::Released), State::Revoked)
    )
}

fn parse_state(value: &str) -> Result<DeliveryCertificationState, CertificationError> {
    match value {
        "produced" => Ok(DeliveryCertificationState::Produced),
        "delivered" => Ok(DeliveryCertificationState::Delivered),
        "machine_attested" => Ok(DeliveryCertificationState::MachineAttested),
        "human_certified" => Ok(DeliveryCertificationState::HumanCertified),
        "released" => Ok(DeliveryCertificationState::Released),
        "revoked" => Ok(DeliveryCertificationState::Revoked),
        _ => Err(CertificationError::Unavailable(
            "stored certification state is invalid".to_string(),
        )),
    }
}

fn validate_certification_request(
    request: &CertificationRequest,
) -> Result<(), CertificationError> {
    if request.schema_version != 1
        || request.idempotency_key.trim().is_empty()
        || request.openfab_authority_key.trim().is_empty()
        || request.evidence_storage_ref.trim().is_empty()
        || request.requested_at < 0
    {
        return Err(CertificationError::Invalid(
            "invalid certification request identity".to_string(),
        ));
    }
    for (name, digest) in [
        ("evidence payload", request.evidence_payload_sha256.as_str()),
        ("snapshot", request.snapshot_content_sha256.as_str()),
        ("subject", request.subject_sha256.as_str()),
        ("spec", request.spec_sha256.as_str()),
        (
            "certification policy",
            request.certification_policy_sha256.as_str(),
        ),
        ("skill packages", request.skill_packages_sha256.as_str()),
    ] {
        validate_sha256(digest, name)?;
    }
    validate_commit(&request.source_commit)
}

fn validate_result_shape(result: &SignedCertificationResult) -> Result<(), CertificationError> {
    if result.schema_version != 1
        || result.payload.schema_version != 1
        || result.signature_algorithm != "ed25519"
        || result.payload.published_at < 0
    {
        return Err(CertificationError::Invalid(
            "invalid OpenFab result envelope".to_string(),
        ));
    }
    validate_sha256(&result.payload_sha256, "OpenFab result payload")
}

#[derive(Debug)]
struct StoredRequestBinding {
    openfab_authority_key: String,
    evidence_payload_sha256: String,
    snapshot_authority_key: String,
    snapshot_resource_id: String,
    snapshot_resource_version: String,
    snapshot_content_sha256: String,
    source_commit: String,
    subject_sha256: String,
    spec_sha256: String,
    policy_authority_key: String,
    policy_resource_id: String,
    policy_resource_version: String,
    policy_sha256: String,
    skill_packages_sha256: String,
}

async fn load_request_row(
    transaction: &mut Transaction<'_, Sqlite>,
    request_id: &CertificationRequestId,
) -> Result<StoredRequestBinding, CertificationError> {
    let row = sqlx::query(
        "SELECT openfab_authority_key, evidence_payload_sha256, snapshot_authority_key, \
         snapshot_resource_id, snapshot_resource_version, snapshot_content_sha256, source_commit, \
         subject_sha256, spec_sha256, policy_authority_key, policy_resource_id, policy_resource_version, \
         policy_sha256, skill_packages_sha256 \
         FROM openfab_certification_requests WHERE request_id = ?",
    )
    .bind(request_id.as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?
    .ok_or_else(|| CertificationError::NotFound("OpenFab certification request".to_string()))?;
    Ok(StoredRequestBinding {
        openfab_authority_key: row.get("openfab_authority_key"),
        evidence_payload_sha256: row.get("evidence_payload_sha256"),
        snapshot_authority_key: row.get("snapshot_authority_key"),
        snapshot_resource_id: row.get("snapshot_resource_id"),
        snapshot_resource_version: row.get("snapshot_resource_version"),
        snapshot_content_sha256: row.get("snapshot_content_sha256"),
        source_commit: row.get("source_commit"),
        subject_sha256: row.get("subject_sha256"),
        spec_sha256: row.get("spec_sha256"),
        policy_authority_key: row.get("policy_authority_key"),
        policy_resource_id: row.get("policy_resource_id"),
        policy_resource_version: row.get("policy_resource_version"),
        policy_sha256: row.get("policy_sha256"),
        skill_packages_sha256: row.get("skill_packages_sha256"),
    })
}

fn ensure_result_matches_request(
    result: &SignedCertificationResult,
    request: &StoredRequestBinding,
) -> Result<(), CertificationError> {
    let snapshot = result.payload.snapshot_ref.as_resource_ref();
    let policy = result.payload.certification_policy_ref.as_resource_ref();
    if result.payload.openfab_authority_key != request.openfab_authority_key
        || result.payload.evidence_payload_sha256 != request.evidence_payload_sha256
        || snapshot.authority_key().as_str() != request.snapshot_authority_key
        || snapshot.resource_id() != request.snapshot_resource_id
        || snapshot.resource_version() != request.snapshot_resource_version
        || result.payload.snapshot_content_sha256 != request.snapshot_content_sha256
        || result.payload.source_commit != request.source_commit
        || result.payload.subject_sha256 != request.subject_sha256
        || result.payload.spec_sha256 != request.spec_sha256
        || policy.authority_key().as_str() != request.policy_authority_key
        || policy.resource_id() != request.policy_resource_id
        || policy.resource_version() != request.policy_resource_version
        || result.payload.certification_policy_sha256 != request.policy_sha256
        || result.payload.skill_packages_sha256 != request.skill_packages_sha256
    {
        return Err(CertificationError::Denied(
            "OpenFab result does not bind its exact request".to_string(),
        ));
    }
    Ok(())
}

async fn existing_request(
    transaction: &mut Transaction<'_, Sqlite>,
    request: &CertificationRequest,
) -> Result<Option<CertificationRequest>, CertificationError> {
    let row = sqlx::query(
        "SELECT request_json FROM openfab_certification_requests \
         WHERE request_id = ? OR idempotency_key = ?",
    )
    .bind(request.request_id.as_str())
    .bind(request.idempotency_key.trim())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?;
    row.map(|row| from_json(row.get("request_json")))
        .transpose()
}

async fn existing_result(
    transaction: &mut Transaction<'_, Sqlite>,
    result: &SignedCertificationResult,
) -> Result<Option<SignedCertificationResult>, CertificationError> {
    let row = sqlx::query(
        "SELECT result_json FROM openfab_certification_results \
         WHERE result_id = ? OR request_id = ?",
    )
    .bind(result.payload.result_id.as_str())
    .bind(result.payload.request_id.as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?;
    row.map(|row| from_json(row.get("result_json"))).transpose()
}

async fn insert_protocol_event(
    transaction: &mut Transaction<'_, Sqlite>,
    event_key: &str,
    event_kind: OpenFabProtocolEventKind,
    aggregate_id: &str,
    payload_sha256: &str,
    payload_json: &str,
    created_at: i64,
) -> Result<(), CertificationError> {
    sqlx::query(
        "INSERT INTO openfab_protocol_outbox (event_key, event_kind, aggregate_id, \
         payload_sha256, payload_json, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(event_key)
    .bind(event_kind.as_str())
    .bind(aggregate_id)
    .bind(payload_sha256)
    .bind(payload_json)
    .bind(created_at)
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(())
}

async fn load_evidence_by_identity(
    pool: &SqlitePool,
    envelope_id: &str,
    payload_sha256: &str,
) -> Result<Option<SignedExecutionEvidenceEnvelope>, CertificationError> {
    let row = if payload_sha256.is_empty() {
        sqlx::query("SELECT envelope_json FROM signed_execution_evidence WHERE envelope_id = ?")
            .bind(envelope_id)
            .fetch_optional(pool)
            .await
            .map_err(storage_error)?
    } else {
        sqlx::query(
            "SELECT envelope_json FROM signed_execution_evidence \
             WHERE envelope_id = ? OR payload_sha256 = ?",
        )
        .bind(envelope_id)
        .bind(payload_sha256)
        .fetch_optional(pool)
        .await
        .map_err(storage_error)?
    };
    row.map(|row| from_json(row.get("envelope_json")))
        .transpose()
}

async fn load_key(
    pool: &SqlitePool,
    key_id: &str,
) -> Result<Option<TrustedSigningKey>, CertificationError> {
    let row = sqlx::query(
        "SELECT key_id, signer_did, signer_role, not_before, not_after, revoked_at, \
         superseded_by FROM trusted_evidence_signing_keys WHERE key_id = ?",
    )
    .bind(key_id)
    .fetch_optional(pool)
    .await
    .map_err(storage_error)?;
    row.as_ref().map(key_from_row).transpose()
}

async fn load_key_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    key_id: &str,
) -> Result<Option<TrustedSigningKey>, CertificationError> {
    let row = sqlx::query(
        "SELECT key_id, signer_did, signer_role, not_before, not_after, revoked_at, \
         superseded_by FROM trusted_evidence_signing_keys WHERE key_id = ?",
    )
    .bind(key_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?;
    row.as_ref().map(key_from_row).transpose()
}

fn key_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<TrustedSigningKey, CertificationError> {
    let role = match row.get::<String, _>("signer_role").as_str() {
        "builder" => EvidenceSignerRole::Builder,
        "worker" => EvidenceSignerRole::Worker,
        "openfab" => EvidenceSignerRole::OpenFab,
        _ => {
            return Err(CertificationError::Unavailable(
                "stored signing key role is invalid".to_string(),
            ));
        }
    };
    Ok(TrustedSigningKey {
        key_id: row.get("key_id"),
        signer_did: row.get("signer_did"),
        role,
        not_before: row.get("not_before"),
        not_after: row.get("not_after"),
        revoked_at: row.get("revoked_at"),
        superseded_by: row.get("superseded_by"),
    })
}

fn validate_key(key: &TrustedSigningKey, registered_at: i64) -> Result<(), CertificationError> {
    if key.key_id.trim().is_empty()
        || !key.signer_did.starts_with("did:key:z")
        || key.not_before >= key.not_after
        || registered_at < 0
        || key.revoked_at.is_some_and(|at| at < key.not_before)
        || key.superseded_by.as_deref() == Some(key.key_id.as_str())
    {
        return Err(CertificationError::Invalid(
            "invalid trusted signing key lifecycle".to_string(),
        ));
    }
    Ok(())
}

fn protocol_event_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<OpenFabProtocolEvent, CertificationError> {
    Ok(OpenFabProtocolEvent {
        sequence: row.get("sequence"),
        event_key: row.get("event_key"),
        event_kind: OpenFabProtocolEventKind::parse(&row.get::<String, _>("event_kind"))?,
        aggregate_id: row.get("aggregate_id"),
        payload_sha256: row.get("payload_sha256"),
        payload_json: row.get("payload_json"),
        created_at: row.get("created_at"),
    })
}

fn validate_sha256(value: &str, field: &str) -> Result<(), CertificationError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(CertificationError::Invalid(format!(
            "{field} must be lowercase SHA-256"
        )));
    }
    Ok(())
}

fn validate_commit(value: &str) -> Result<(), CertificationError> {
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CertificationError::Invalid(
            "source commit must be hexadecimal".to_string(),
        ));
    }
    Ok(())
}

fn json<T: Serialize>(value: &T) -> Result<String, CertificationError> {
    serde_json::to_string(value).map_err(|error| CertificationError::Invalid(error.to_string()))
}

#[allow(clippy::needless_pass_by_value)]
fn from_json<T: for<'de> Deserialize<'de>>(value: String) -> Result<T, CertificationError> {
    serde_json::from_str(&value).map_err(|error| CertificationError::Unavailable(error.to_string()))
}

fn sha256(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

#[allow(clippy::needless_pass_by_value)]
fn storage_error(error: sqlx::Error) -> CertificationError {
    CertificationError::Unavailable(error.to_string())
}
