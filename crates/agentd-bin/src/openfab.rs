//! Fail-closed OpenFab evidence, certification, forge, and Skill Hub orchestration.

use std::fmt;
use std::sync::Arc;

use agentd_core::ports::{
    CertificationError, CertificationPort, CertificationRequest, CertificationStatePort,
    CertificationStateTransition, Clock, EvidenceEnvelopeStorePort, EvidenceSigningPort,
    EvidenceVerificationPort, ExecutionEvidenceEnvelopePayload, ForgeAdmission,
    ForgeAdmissionRequest, PolicyRevocationPort, SignedCertificationResult,
    SignedExecutionEvidenceEnvelope, SkillHubPort, SkillInstallAdmission, SkillInstallRequest,
};
use agentd_core::types::{
    CertificationRequestId, ProjectExecutionSnapshot, SecurityCheckpoint, SecurityEpochRequest,
    SkillInstallationId,
};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenFabProviderKind {
    EvidenceSigner,
    EvidenceVerifier,
    EvidenceStore,
    CertificationState,
    CertificationTransport,
    SkillHub,
    PolicyRevocation,
    TrustedClock,
}

impl OpenFabProviderKind {
    pub const ALL: [Self; 8] = [
        Self::EvidenceSigner,
        Self::EvidenceVerifier,
        Self::EvidenceStore,
        Self::CertificationState,
        Self::CertificationTransport,
        Self::SkillHub,
        Self::PolicyRevocation,
        Self::TrustedClock,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EvidenceSigner => "evidence_signer",
            Self::EvidenceVerifier => "evidence_verifier",
            Self::EvidenceStore => "evidence_store",
            Self::CertificationState => "certification_state",
            Self::CertificationTransport => "certification_transport",
            Self::SkillHub => "skill_hub",
            Self::PolicyRevocation => "policy_revocation",
            Self::TrustedClock => "trusted_clock",
        }
    }
}

impl fmt::Display for OpenFabProviderKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OpenFabStartupError {
    #[error("OpenFab integration startup missing closed provider: {0}")]
    MissingProvider(OpenFabProviderKind),
}

#[derive(Default)]
pub struct OpenFabProviders {
    evidence_signer: Option<Arc<dyn EvidenceSigningPort>>,
    evidence_verifier: Option<Arc<dyn EvidenceVerificationPort>>,
    evidence_store: Option<Arc<dyn EvidenceEnvelopeStorePort>>,
    certification_state: Option<Arc<dyn CertificationStatePort>>,
    certification_transport: Option<Arc<dyn CertificationPort>>,
    skill_hub: Option<Arc<dyn SkillHubPort>>,
    policy_revocation: Option<Arc<dyn PolicyRevocationPort>>,
    trusted_clock: Option<Arc<dyn Clock>>,
}

impl fmt::Debug for OpenFabProviders {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenFabProviders")
            .field(
                "configured",
                &OpenFabProviderKind::ALL
                    .into_iter()
                    .filter(|kind| self.has(*kind))
                    .map(OpenFabProviderKind::as_str)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl OpenFabProviders {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        evidence_signer: Arc<dyn EvidenceSigningPort>,
        evidence_verifier: Arc<dyn EvidenceVerificationPort>,
        evidence_store: Arc<dyn EvidenceEnvelopeStorePort>,
        certification_state: Arc<dyn CertificationStatePort>,
        certification_transport: Arc<dyn CertificationPort>,
        skill_hub: Arc<dyn SkillHubPort>,
        policy_revocation: Arc<dyn PolicyRevocationPort>,
        trusted_clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            evidence_signer: Some(evidence_signer),
            evidence_verifier: Some(evidence_verifier),
            evidence_store: Some(evidence_store),
            certification_state: Some(certification_state),
            certification_transport: Some(certification_transport),
            skill_hub: Some(skill_hub),
            policy_revocation: Some(policy_revocation),
            trusted_clock: Some(trusted_clock),
        }
    }

    #[must_use]
    pub fn without(mut self, provider: OpenFabProviderKind) -> Self {
        match provider {
            OpenFabProviderKind::EvidenceSigner => self.evidence_signer = None,
            OpenFabProviderKind::EvidenceVerifier => self.evidence_verifier = None,
            OpenFabProviderKind::EvidenceStore => self.evidence_store = None,
            OpenFabProviderKind::CertificationState => self.certification_state = None,
            OpenFabProviderKind::CertificationTransport => self.certification_transport = None,
            OpenFabProviderKind::SkillHub => self.skill_hub = None,
            OpenFabProviderKind::PolicyRevocation => self.policy_revocation = None,
            OpenFabProviderKind::TrustedClock => self.trusted_clock = None,
        }
        self
    }

    fn has(&self, provider: OpenFabProviderKind) -> bool {
        match provider {
            OpenFabProviderKind::EvidenceSigner => self.evidence_signer.is_some(),
            OpenFabProviderKind::EvidenceVerifier => self.evidence_verifier.is_some(),
            OpenFabProviderKind::EvidenceStore => self.evidence_store.is_some(),
            OpenFabProviderKind::CertificationState => self.certification_state.is_some(),
            OpenFabProviderKind::CertificationTransport => self.certification_transport.is_some(),
            OpenFabProviderKind::SkillHub => self.skill_hub.is_some(),
            OpenFabProviderKind::PolicyRevocation => self.policy_revocation.is_some(),
            OpenFabProviderKind::TrustedClock => self.trusted_clock.is_some(),
        }
    }
}

pub fn build_openfab_service(
    mut providers: OpenFabProviders,
) -> Result<OpenFabCertificationService, OpenFabStartupError> {
    if let Some(missing) = OpenFabProviderKind::ALL
        .into_iter()
        .find(|provider| !providers.has(*provider))
    {
        return Err(OpenFabStartupError::MissingProvider(missing));
    }
    Ok(OpenFabCertificationService {
        evidence_signer: take(&mut providers.evidence_signer),
        evidence_verifier: take(&mut providers.evidence_verifier),
        evidence_store: take(&mut providers.evidence_store),
        certification_state: take(&mut providers.certification_state),
        certification_transport: take(&mut providers.certification_transport),
        skill_hub: take(&mut providers.skill_hub),
        policy_revocation: take(&mut providers.policy_revocation),
        trusted_clock: take(&mut providers.trusted_clock),
    })
}

fn take<T: ?Sized>(provider: &mut Option<Arc<T>>) -> Arc<T> {
    provider
        .take()
        .expect("OpenFab provider presence checked before composition")
}

pub struct OpenFabCertificationService {
    evidence_signer: Arc<dyn EvidenceSigningPort>,
    evidence_verifier: Arc<dyn EvidenceVerificationPort>,
    evidence_store: Arc<dyn EvidenceEnvelopeStorePort>,
    certification_state: Arc<dyn CertificationStatePort>,
    certification_transport: Arc<dyn CertificationPort>,
    skill_hub: Arc<dyn SkillHubPort>,
    policy_revocation: Arc<dyn PolicyRevocationPort>,
    trusted_clock: Arc<dyn Clock>,
}

impl fmt::Debug for OpenFabCertificationService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenFabCertificationService")
            .field("providers", &"[CONFIGURED]")
            .finish()
    }
}

impl OpenFabCertificationService {
    pub async fn sign_and_store_evidence(
        &self,
        payload: &ExecutionEvidenceEnvelopePayload,
    ) -> Result<SignedExecutionEvidenceEnvelope, CertificationError> {
        let now = self.now()?;
        let envelope = self.evidence_signer.sign_evidence(payload, now).await?;
        self.evidence_verifier
            .verify_evidence(&envelope, now)
            .await?;
        self.evidence_store.store_evidence_envelope(&envelope).await
    }

    pub async fn request_certification(
        &self,
        mut request: CertificationRequest,
    ) -> Result<CertificationRequest, CertificationError> {
        let now = self.now()?;
        request.requested_at = now;
        let envelope = self
            .evidence_store
            .load_evidence_envelope(&request.envelope_id)
            .await?
            .ok_or_else(|| CertificationError::NotFound("execution evidence".to_string()))?;
        self.evidence_verifier
            .verify_evidence(&envelope, now)
            .await?;
        if envelope.payload_sha256 != request.evidence_payload_sha256 {
            return Err(CertificationError::Denied(
                "certification request evidence digest mismatch".to_string(),
            ));
        }
        let recorded = self
            .certification_state
            .record_certification_request(&request)
            .await?;
        self.certification_transport
            .request_certification(&recorded)
            .await?;
        Ok(recorded)
    }

    pub async fn poll_certification_result(
        &self,
        request_id: &CertificationRequestId,
    ) -> Result<Option<SignedCertificationResult>, CertificationError> {
        let Some(result) = self
            .certification_transport
            .certification_result(request_id)
            .await?
        else {
            return Ok(None);
        };
        self.record_certification_result(&result).await.map(Some)
    }

    pub async fn record_certification_result(
        &self,
        result: &SignedCertificationResult,
    ) -> Result<SignedCertificationResult, CertificationError> {
        let now = self.now()?;
        self.evidence_verifier
            .verify_certification_result(result, now)
            .await?;
        self.certification_state
            .record_certification_result(result)
            .await
    }

    pub async fn transition_certification_state(
        &self,
        mut transition: CertificationStateTransition,
    ) -> Result<CertificationStateTransition, CertificationError> {
        transition.observed_at = self.now()?;
        self.certification_state
            .transition_certification_state(&transition)
            .await
    }

    pub async fn admit_forge_operation(
        &self,
        mut request: ForgeAdmissionRequest,
    ) -> Result<ForgeAdmission, CertificationError> {
        let now = self.now()?;
        request.observed_at = now;
        self.check_epoch(&request.snapshot, SecurityCheckpoint::Release, now)
            .await?;
        if let Some(result) = &request.certification_result {
            self.evidence_verifier
                .verify_certification_result(result, now)
                .await?;
        }
        self.certification_state
            .admit_forge_operation(&request)
            .await
    }

    pub async fn admit_skill_install(
        &self,
        request: SkillInstallRequest,
    ) -> Result<SkillInstallAdmission, CertificationError> {
        let now = self.now()?;
        request
            .snapshot
            .validate()
            .map_err(|error| CertificationError::Invalid(error.to_string()))?;
        if now < request.snapshot.issued_at || now >= request.snapshot.valid_until {
            return Err(CertificationError::Denied(
                "skill installation snapshot is not current".to_string(),
            ));
        }
        self.check_epoch(
            &request.snapshot,
            SecurityCheckpoint::ArtifactAcceptance,
            now,
        )
        .await?;
        let pinned = request
            .snapshot
            .skill_packages
            .iter()
            .find(|package| package.package_ref == request.package.package_ref)
            .ok_or_else(|| {
                CertificationError::Denied(
                    "skill package is not pinned by the execution snapshot".to_string(),
                )
            })?;
        if pinned != &request.package {
            return Err(CertificationError::Denied(
                "skill package hashes differ from the execution snapshot".to_string(),
            ));
        }
        let trust = self
            .skill_hub
            .resolve_package_trust(&request.package.package_ref, now)
            .await?;
        self.evidence_verifier
            .verify_skill_trust(&trust, now)
            .await?;
        if !trust.payload.status.permits_new_install() {
            return Err(CertificationError::Denied(
                "Skill Hub package status does not permit new installation".to_string(),
            ));
        }
        let admission = SkillInstallAdmission {
            id: SkillInstallationId::new(),
            snapshot_ref: request.snapshot.snapshot_ref.clone(),
            snapshot_content_sha256: request.snapshot.content_sha256.clone(),
            package_ref: request.package.package_ref.clone(),
            archive_sha256: request.package.archive_sha256.clone(),
            manifest_sha256: request.package.manifest_sha256.clone(),
            dependency_lock_sha256: request.package.dependency_lock_sha256.clone(),
            permissions_sha256: request.package.permissions_sha256.clone(),
            install_root_ref: request.install_root_ref,
            trust_status_at_install: trust.payload.status,
            trust_record: trust,
            admitted_at: now,
        };
        self.certification_state
            .record_skill_installation(&admission)
            .await
    }

    async fn check_epoch(
        &self,
        snapshot: &ProjectExecutionSnapshot,
        checkpoint: SecurityCheckpoint,
        observed_at: i64,
    ) -> Result<(), CertificationError> {
        let request = SecurityEpochRequest {
            checkpoint,
            organization_ref: snapshot.organization_ref.clone(),
            project_ref: snapshot.project_ref.clone(),
            execution_snapshot_ref: snapshot.snapshot_ref.clone(),
            pinned_epoch: snapshot.policy_revocation_epoch,
            observed_at,
        };
        let status = self
            .policy_revocation
            .check_security_epoch(&request)
            .await
            .map_err(|error| CertificationError::Unavailable(error.to_string()))?;
        if status.observed_at > observed_at || observed_at.saturating_sub(status.observed_at) > 60 {
            return Err(CertificationError::Unavailable(
                "policy revocation status is stale or from the future".to_string(),
            ));
        }
        status
            .validate_request(&request)
            .and_then(|()| status.validate_pinned_epoch(request.pinned_epoch))
            .map_err(|error| CertificationError::Denied(error.to_string()))
    }

    fn now(&self) -> Result<i64, CertificationError> {
        let now = self.trusted_clock.now_unix();
        if now < 0 {
            return Err(CertificationError::Unavailable(
                "trusted OpenFab integration clock returned invalid time".to_string(),
            ));
        }
        Ok(now)
    }
}
