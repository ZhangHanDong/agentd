//! Ed25519 execution-evidence signing and OpenFab signature verification.

use std::sync::Arc;

use agentd_core::ports::{
    CertificationError, EXECUTION_EVIDENCE_ENVELOPE_SCHEMA_VERSION, EvidenceSignerRole,
    EvidenceSigningPort, EvidenceVerificationPort, ExecutionEvidenceEnvelopePayload,
    OPENFAB_CERTIFICATION_SCHEMA_VERSION, SignedCertificationResult,
    SignedExecutionEvidenceEnvelope, SigningKeyTrustPort, SkillPackageEvidenceRef,
    SkillPackageTrustRecord, canonical_json,
};
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier as _, VerifyingKey};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const ED25519_MULTICODEC: [u8; 2] = [0xed, 0x01];
const SIGNATURE_ALGORITHM: &str = "ed25519";

pub struct Ed25519EvidenceSigner {
    key_id: String,
    role: EvidenceSignerRole,
    seed: Zeroizing<[u8; 32]>,
    signer_did: String,
}

impl std::fmt::Debug for Ed25519EvidenceSigner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Ed25519EvidenceSigner")
            .field("key_id", &self.key_id)
            .field("role", &self.role)
            .field("signer_did", &self.signer_did)
            .field("seed", &"[REDACTED]")
            .finish()
    }
}

impl Ed25519EvidenceSigner {
    pub fn from_seed(
        key_id: impl Into<String>,
        role: EvidenceSignerRole,
        seed: [u8; 32],
    ) -> Result<Self, CertificationError> {
        let key_id = key_id.into();
        validate_opaque(&key_id, "signing key id")?;
        if !matches!(
            role,
            EvidenceSignerRole::Builder | EvidenceSignerRole::Worker
        ) {
            return Err(CertificationError::Denied(
                "agentd cannot sign with the OpenFab certification role".to_string(),
            ));
        }
        let signing = SigningKey::from_bytes(&seed);
        let signer_did = encode_did_key(&signing.verifying_key());
        Ok(Self {
            key_id,
            role,
            seed: Zeroizing::new(seed),
            signer_did,
        })
    }

    #[must_use]
    pub fn signer_did(&self) -> &str {
        &self.signer_did
    }

    #[must_use]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}

#[async_trait::async_trait]
impl EvidenceSigningPort for Ed25519EvidenceSigner {
    async fn sign_evidence(
        &self,
        payload: &ExecutionEvidenceEnvelopePayload,
        signed_at: i64,
    ) -> Result<SignedExecutionEvidenceEnvelope, CertificationError> {
        validate_evidence_payload(payload, signed_at)?;
        let canonical = canonical_json(payload)?;
        let payload_sha256 = sha256_bytes(canonical.as_bytes());
        let signing = SigningKey::from_bytes(&self.seed);
        let signature = signing.sign(canonical.as_bytes());
        Ok(SignedExecutionEvidenceEnvelope {
            schema_version: EXECUTION_EVIDENCE_ENVELOPE_SCHEMA_VERSION,
            payload: payload.clone(),
            payload_sha256,
            signer_key_id: self.key_id.clone(),
            signer_did: self.signer_did.clone(),
            signer_role: self.role,
            signature_algorithm: SIGNATURE_ALGORITHM.to_string(),
            signature_b64: base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
            signed_at,
        })
    }
}

#[derive(Clone)]
pub struct Ed25519EvidenceVerifier {
    trust: Arc<dyn SigningKeyTrustPort>,
}

impl std::fmt::Debug for Ed25519EvidenceVerifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Ed25519EvidenceVerifier")
            .field("trust", &"[CONFIGURED]")
            .finish()
    }
}

impl Ed25519EvidenceVerifier {
    #[must_use]
    pub fn new(trust: Arc<dyn SigningKeyTrustPort>) -> Self {
        Self { trust }
    }

    async fn verify_trusted_signature<T: serde::Serialize + Sync>(
        &self,
        payload: &T,
        expected_sha256: &str,
        key_id: &str,
        signer_did: &str,
        signature_b64: &str,
        role: EvidenceSignerRole,
        signed_at: i64,
        observed_at: i64,
    ) -> Result<(), CertificationError> {
        if signed_at < 0 || signed_at > observed_at {
            return Err(CertificationError::Denied(
                "signature timestamp is outside the trusted observation window".to_string(),
            ));
        }
        validate_sha256(expected_sha256, "signed payload sha256")?;
        validate_opaque(key_id, "signing key id")?;
        let canonical = canonical_json(payload)?;
        if sha256_bytes(canonical.as_bytes()) != expected_sha256 {
            return Err(CertificationError::Denied(
                "signed payload digest mismatch".to_string(),
            ));
        }
        let trusted = self
            .trust
            .resolve_signing_key(key_id, role, signed_at)
            .await?;
        if trusted.key_id != key_id
            || trusted.role != role
            || trusted.signer_did != signer_did
            || signed_at < trusted.not_before
            || signed_at >= trusted.not_after
            || trusted
                .revoked_at
                .is_some_and(|revoked_at| signed_at >= revoked_at)
        {
            return Err(CertificationError::Denied(
                "signature does not match an active trusted key version".to_string(),
            ));
        }
        verify_did_signature(signer_did, canonical.as_bytes(), signature_b64)
    }
}

#[async_trait::async_trait]
impl EvidenceVerificationPort for Ed25519EvidenceVerifier {
    async fn verify_evidence(
        &self,
        envelope: &SignedExecutionEvidenceEnvelope,
        observed_at: i64,
    ) -> Result<(), CertificationError> {
        if envelope.schema_version != EXECUTION_EVIDENCE_ENVELOPE_SCHEMA_VERSION
            || envelope.payload.schema_version != EXECUTION_EVIDENCE_ENVELOPE_SCHEMA_VERSION
            || envelope.signature_algorithm != SIGNATURE_ALGORITHM
            || !matches!(
                envelope.signer_role,
                EvidenceSignerRole::Builder | EvidenceSignerRole::Worker
            )
        {
            return Err(CertificationError::Invalid(
                "unsupported execution evidence envelope".to_string(),
            ));
        }
        validate_evidence_payload(&envelope.payload, envelope.signed_at)?;
        self.verify_trusted_signature(
            &envelope.payload,
            &envelope.payload_sha256,
            &envelope.signer_key_id,
            &envelope.signer_did,
            &envelope.signature_b64,
            envelope.signer_role,
            envelope.signed_at,
            observed_at,
        )
        .await
    }

    async fn verify_certification_result(
        &self,
        result: &SignedCertificationResult,
        observed_at: i64,
    ) -> Result<(), CertificationError> {
        if result.schema_version != OPENFAB_CERTIFICATION_SCHEMA_VERSION
            || result.payload.schema_version != OPENFAB_CERTIFICATION_SCHEMA_VERSION
            || result.signature_algorithm != SIGNATURE_ALGORITHM
        {
            return Err(CertificationError::Invalid(
                "unsupported OpenFab certification result".to_string(),
            ));
        }
        validate_certification_result(result, observed_at)?;
        self.verify_trusted_signature(
            &result.payload,
            &result.payload_sha256,
            &result.signer_key_id,
            &result.signer_did,
            &result.signature_b64,
            EvidenceSignerRole::OpenFab,
            result.payload.published_at,
            observed_at,
        )
        .await
    }

    async fn verify_skill_trust(
        &self,
        record: &SkillPackageTrustRecord,
        observed_at: i64,
    ) -> Result<(), CertificationError> {
        validate_skill_ref(&record.payload.package)?;
        if record.payload.published_at < 0
            || record.payload.status_changed_at < record.payload.published_at
            || record.payload.valid_until <= record.payload.status_changed_at
            || observed_at >= record.payload.valid_until
        {
            return Err(CertificationError::Invalid(
                "invalid or expired Skill Hub trust timestamps".to_string(),
            ));
        }
        self.verify_trusted_signature(
            &record.payload,
            &record.trust_payload_sha256,
            &record.signer_key_id,
            &record.signer_did,
            &record.signature_b64,
            EvidenceSignerRole::OpenFab,
            record.payload.status_changed_at,
            observed_at,
        )
        .await
    }
}

fn encode_did_key(key: &VerifyingKey) -> String {
    let mut bytes = Vec::with_capacity(34);
    bytes.extend_from_slice(&ED25519_MULTICODEC);
    bytes.extend_from_slice(&key.to_bytes());
    format!("did:key:z{}", bs58::encode(bytes).into_string())
}

fn decode_did_key(did: &str) -> Result<VerifyingKey, CertificationError> {
    let encoded = did
        .strip_prefix("did:key:z")
        .ok_or_else(|| CertificationError::Invalid("signer DID is not did:key:z".to_string()))?;
    let bytes = bs58::decode(encoded)
        .into_vec()
        .map_err(|_| CertificationError::Invalid("invalid signer DID base58".to_string()))?;
    if bytes.len() != 34 || bytes[..2] != ED25519_MULTICODEC {
        return Err(CertificationError::Invalid(
            "signer DID is not an Ed25519 did:key".to_string(),
        ));
    }
    let public_key: [u8; 32] = bytes[2..]
        .try_into()
        .map_err(|_| CertificationError::Invalid("invalid Ed25519 public key".to_string()))?;
    VerifyingKey::from_bytes(&public_key)
        .map_err(|_| CertificationError::Invalid("invalid Ed25519 public key".to_string()))
}

fn verify_did_signature(
    did: &str,
    message: &[u8],
    signature_b64: &str,
) -> Result<(), CertificationError> {
    let key = decode_did_key(did)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(signature_b64)
        .map_err(|_| CertificationError::Invalid("invalid signature base64".to_string()))?;
    let signature = Signature::from_slice(&bytes)
        .map_err(|_| CertificationError::Invalid("invalid Ed25519 signature".to_string()))?;
    key.verify(message, &signature)
        .map_err(|_| CertificationError::Denied("signature verification failed".to_string()))
}

fn validate_evidence_payload(
    payload: &ExecutionEvidenceEnvelopePayload,
    signed_at: i64,
) -> Result<(), CertificationError> {
    if payload.schema_version != EXECUTION_EVIDENCE_ENVELOPE_SCHEMA_VERSION
        || payload.issued_at < 0
        || payload.completed_at < payload.issued_at
        || signed_at < payload.completed_at
    {
        return Err(CertificationError::Invalid(
            "invalid execution evidence version or timestamps".to_string(),
        ));
    }
    for (name, digest) in [
        ("snapshot", payload.snapshot_content_sha256.as_str()),
        ("sandbox profile", payload.sandbox_profile_sha256.as_str()),
        ("prompt", payload.prompt_sha256.as_str()),
        ("usage ledger", payload.usage_ledger_sha256.as_str()),
        ("recovery events", payload.recovery_events_sha256.as_str()),
    ] {
        validate_sha256(digest, name)?;
    }
    if let Some(digest) = &payload.plan_sha256 {
        validate_sha256(digest, "plan")?;
    }
    if let Some(digest) = &payload.produced_diff_sha256 {
        validate_sha256(digest, "produced diff")?;
    }
    validate_commit(&payload.target_base_commit, "target base commit")?;
    if let Some(commit) = &payload.produced_commit {
        validate_commit(commit, "produced commit")?;
    }
    validate_opaque(&payload.runtime_name, "runtime name")?;
    validate_evidence_ref(&payload.frozen_spec)?;
    for reference in payload
        .requirements
        .iter()
        .chain(&payload.review_evidence)
        .chain(&payload.human_decisions)
        .chain(&payload.policy_refs)
    {
        validate_evidence_ref(reference)?;
    }
    for artifact in &payload.artifacts {
        validate_sha256(&artifact.content_sha256, "artifact")?;
        validate_opaque(&artifact.kind, "artifact kind")?;
        validate_immutable_locator(&artifact.storage_ref, "artifact storage ref")?;
    }
    for package in &payload.skill_packages {
        validate_skill_ref(package)?;
    }
    Ok(())
}

fn validate_certification_result(
    result: &SignedCertificationResult,
    observed_at: i64,
) -> Result<(), CertificationError> {
    let payload = &result.payload;
    if payload.published_at < 0 || payload.published_at > observed_at {
        return Err(CertificationError::Invalid(
            "invalid certification publication timestamp".to_string(),
        ));
    }
    for (name, digest) in [
        ("evidence payload", payload.evidence_payload_sha256.as_str()),
        ("snapshot", payload.snapshot_content_sha256.as_str()),
        ("subject", payload.subject_sha256.as_str()),
        ("spec", payload.spec_sha256.as_str()),
        (
            "certification policy",
            payload.certification_policy_sha256.as_str(),
        ),
        ("skill packages", payload.skill_packages_sha256.as_str()),
    ] {
        validate_sha256(digest, name)?;
    }
    validate_commit(&payload.source_commit, "source commit")?;
    validate_immutable_locator(&payload.signed_ref, "OpenFab signed ref")?;
    if payload.required_human_signoffs > payload.eligible_human_signoffs
        || payload.accepted_human_signoffs > payload.eligible_human_signoffs
    {
        return Err(CertificationError::Invalid(
            "invalid certification human signoff counts".to_string(),
        ));
    }
    if payload
        .revoked_at
        .is_some_and(|revoked_at| revoked_at < payload.published_at)
    {
        return Err(CertificationError::Invalid(
            "certification revocation predates publication".to_string(),
        ));
    }
    Ok(())
}

fn validate_evidence_ref(
    reference: &agentd_core::ports::ImmutableEvidenceRef,
) -> Result<(), CertificationError> {
    validate_opaque(&reference.authority_key, "evidence authority")?;
    validate_opaque(&reference.resource_kind, "evidence resource kind")?;
    validate_opaque(&reference.resource_id, "evidence resource id")?;
    validate_opaque(&reference.resource_version, "evidence resource version")?;
    if reference.resource_id.eq_ignore_ascii_case("latest")
        || reference.resource_version.eq_ignore_ascii_case("latest")
    {
        return Err(CertificationError::Denied(
            "mutable evidence reference is forbidden".to_string(),
        ));
    }
    validate_sha256(&reference.content_sha256, "evidence reference")
}

fn validate_skill_ref(package: &SkillPackageEvidenceRef) -> Result<(), CertificationError> {
    for (name, digest) in [
        ("skill archive", package.archive_sha256.as_str()),
        ("skill manifest", package.manifest_sha256.as_str()),
        (
            "skill dependency lock",
            package.dependency_lock_sha256.as_str(),
        ),
        ("skill permissions", package.permissions_sha256.as_str()),
    ] {
        validate_sha256(digest, name)?;
    }
    Ok(())
}

fn validate_immutable_locator(value: &str, field: &str) -> Result<(), CertificationError> {
    validate_opaque(value, field)?;
    let lowercase = value.to_ascii_lowercase();
    if lowercase.contains("/latest")
        || lowercase.contains("ref=latest")
        || lowercase.contains("version=latest")
    {
        return Err(CertificationError::Denied(format!(
            "{field} must be immutable"
        )));
    }
    Ok(())
}

fn validate_commit(value: &str, field: &str) -> Result<(), CertificationError> {
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CertificationError::Invalid(format!(
            "{field} must be a hexadecimal commit id"
        )));
    }
    Ok(())
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

fn validate_opaque(value: &str, field: &str) -> Result<(), CertificationError> {
    if value.trim().is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(CertificationError::Invalid(format!("invalid {field}")));
    }
    Ok(())
}

fn sha256_bytes(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}
