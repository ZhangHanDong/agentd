use std::sync::Arc;
use std::time::Duration;

use agentd_bin::openfab::{
    OpenFabProviderKind, OpenFabProviders, OpenFabStartupError, build_openfab_service,
};
use agentd_bin::openfab_http::HttpOpenFabCertificationTransport;
use agentd_core::ports::{
    CertificationError, CertificationPort, CertificationRequest, CertificationStatePort,
    CertificationStateTransition, Clock, EvidenceEnvelopeStorePort, EvidenceSignerRole,
    EvidenceSigningPort, EvidenceVerificationPort, ExecutionEvidenceEnvelopePayload,
    ForgeAdmission, ForgeAdmissionRequest, PolicyRevocationPort, SignedCertificationResult,
    SignedExecutionEvidenceEnvelope, SigningKeyTrustPort, SkillHubPort, SkillInstallAdmission,
    SkillPackageTrustRecord, TrustedSigningKey,
};
use agentd_core::types::{
    CertificationRequestId, EvidenceEnvelopeId, SecurityEpochRequest, SecurityEpochStatus,
    SkillPackageVersionRef,
};

#[derive(Debug)]
struct UnusedProvider;

impl Clock for UnusedProvider {
    fn now_unix(&self) -> i64 {
        100
    }
}

#[async_trait::async_trait]
impl SigningKeyTrustPort for UnusedProvider {
    async fn resolve_signing_key(
        &self,
        _key_id: &str,
        _role: EvidenceSignerRole,
        _signed_at: i64,
    ) -> Result<TrustedSigningKey, CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }
}

#[async_trait::async_trait]
impl EvidenceSigningPort for UnusedProvider {
    async fn sign_evidence(
        &self,
        _payload: &ExecutionEvidenceEnvelopePayload,
        _signed_at: i64,
    ) -> Result<SignedExecutionEvidenceEnvelope, CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }
}

#[async_trait::async_trait]
impl EvidenceVerificationPort for UnusedProvider {
    async fn verify_evidence(
        &self,
        _envelope: &SignedExecutionEvidenceEnvelope,
        _observed_at: i64,
    ) -> Result<(), CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }

    async fn verify_certification_result(
        &self,
        _result: &SignedCertificationResult,
        _observed_at: i64,
    ) -> Result<(), CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }

    async fn verify_skill_trust(
        &self,
        _record: &SkillPackageTrustRecord,
        _observed_at: i64,
    ) -> Result<(), CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }
}

#[async_trait::async_trait]
impl EvidenceEnvelopeStorePort for UnusedProvider {
    async fn store_evidence_envelope(
        &self,
        _envelope: &SignedExecutionEvidenceEnvelope,
    ) -> Result<SignedExecutionEvidenceEnvelope, CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }

    async fn load_evidence_envelope(
        &self,
        _envelope_id: &EvidenceEnvelopeId,
    ) -> Result<Option<SignedExecutionEvidenceEnvelope>, CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }
}

#[async_trait::async_trait]
impl CertificationStatePort for UnusedProvider {
    async fn record_certification_request(
        &self,
        _request: &CertificationRequest,
    ) -> Result<CertificationRequest, CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }

    async fn record_certification_result(
        &self,
        _result: &SignedCertificationResult,
    ) -> Result<SignedCertificationResult, CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }

    async fn transition_certification_state(
        &self,
        _transition: &CertificationStateTransition,
    ) -> Result<CertificationStateTransition, CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }

    async fn admit_forge_operation(
        &self,
        _request: &ForgeAdmissionRequest,
    ) -> Result<ForgeAdmission, CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }

    async fn record_skill_installation(
        &self,
        _admission: &SkillInstallAdmission,
    ) -> Result<SkillInstallAdmission, CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }
}

#[async_trait::async_trait]
impl CertificationPort for UnusedProvider {
    async fn request_certification(
        &self,
        _request: &CertificationRequest,
    ) -> Result<(), CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }

    async fn certification_result(
        &self,
        _request_id: &CertificationRequestId,
    ) -> Result<Option<SignedCertificationResult>, CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }
}

#[async_trait::async_trait]
impl SkillHubPort for UnusedProvider {
    async fn resolve_package_trust(
        &self,
        _package_ref: &SkillPackageVersionRef,
        _observed_at: i64,
    ) -> Result<SkillPackageTrustRecord, CertificationError> {
        Err(CertificationError::Unavailable("unused".to_string()))
    }
}

#[async_trait::async_trait]
impl PolicyRevocationPort for UnusedProvider {
    async fn check_security_epoch(
        &self,
        _request: &SecurityEpochRequest,
    ) -> Result<SecurityEpochStatus, agentd_core::ports::SecurityError> {
        Err(agentd_core::ports::SecurityError::Unavailable(
            "unused".to_string(),
        ))
    }
}

fn providers() -> OpenFabProviders {
    OpenFabProviders::new(
        Arc::new(UnusedProvider),
        Arc::new(UnusedProvider),
        Arc::new(UnusedProvider),
        Arc::new(UnusedProvider),
        Arc::new(UnusedProvider),
        Arc::new(UnusedProvider),
        Arc::new(UnusedProvider),
        Arc::new(UnusedProvider),
    )
}

#[test]
fn every_openfab_provider_is_fail_closed_at_startup() {
    for provider in OpenFabProviderKind::ALL {
        assert_eq!(
            build_openfab_service(providers().without(provider))
                .expect_err("missing provider must fail closed"),
            OpenFabStartupError::MissingProvider(provider)
        );
    }
}

#[test]
fn http_transport_requires_tls_and_redacts_credentials() {
    assert!(
        HttpOpenFabCertificationTransport::new(
            "http://openfab.example/",
            "secret-token",
            Duration::from_secs(5),
            false,
        )
        .is_err()
    );
    let transport = HttpOpenFabCertificationTransport::new(
        "http://127.0.0.1:9999/openfab/",
        "secret-token",
        Duration::from_secs(5),
        true,
    )
    .expect("explicit loopback development transport");
    let debug = format!("{transport:?}");
    assert!(!debug.contains("secret-token"));
    assert!(debug.contains("[REDACTED]"));
}
