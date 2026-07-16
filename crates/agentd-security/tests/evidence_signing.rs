use std::sync::Arc;

use agentd_core::ports::{
    CertificationError, EvidenceArtifactSubject, EvidenceSignerRole, EvidenceSigningPort,
    EvidenceVerificationPort, ExecutionEvidenceEnvelopePayload, ImmutableEvidenceRef,
    SigningKeyTrustPort, TrustedSigningKey, canonical_json,
};
use agentd_core::types::{
    AuthorityKey, EvidenceEnvelopeId, ExecutionArtifactId, OrganizationRef,
    ProjectExecutionSnapshotRef, ProjectRef, RepositoryRef, RunId,
};
use agentd_security::evidence_signing::{Ed25519EvidenceSigner, Ed25519EvidenceVerifier};

#[derive(Debug, Clone)]
struct FixedTrust {
    key: TrustedSigningKey,
}

#[async_trait::async_trait]
impl SigningKeyTrustPort for FixedTrust {
    async fn resolve_signing_key(
        &self,
        key_id: &str,
        role: EvidenceSignerRole,
        _signed_at: i64,
    ) -> Result<TrustedSigningKey, CertificationError> {
        if self.key.key_id != key_id || self.key.role != role {
            return Err(CertificationError::NotFound("signing key".to_string()));
        }
        Ok(self.key.clone())
    }
}

fn evidence() -> ExecutionEvidenceEnvelopePayload {
    let authority = AuthorityKey::new("specify:test").expect("authority");
    ExecutionEvidenceEnvelopePayload {
        schema_version: 1,
        envelope_id: EvidenceEnvelopeId::new(),
        organization_ref: OrganizationRef::new(authority.clone(), "org", "1")
            .expect("organization"),
        project_ref: ProjectRef::new(authority.clone(), "project", "1").expect("project"),
        snapshot_ref: ProjectExecutionSnapshotRef::new(authority.clone(), "snapshot", "1")
            .expect("snapshot"),
        snapshot_content_sha256: "1".repeat(64),
        execution_run_id: RunId::new(),
        execution_task_id: None,
        runtime_session_id: None,
        runtime_attempt_id: None,
        worker_incarnation_id: None,
        agent_profile_id: None,
        lease_id: None,
        fencing_token: None,
        runtime_name: "codex".to_string(),
        model: Some("gpt-5-codex".to_string()),
        sandbox_profile_sha256: "2".repeat(64),
        requirements: vec![immutable_ref("requirement", "req", "1", '3')],
        frozen_spec: immutable_ref("frozen_spec", "spec", "1", '4'),
        prompt_sha256: "5".repeat(64),
        plan_sha256: Some("6".repeat(64)),
        target_repository_ref: RepositoryRef::new(authority, "repo", "1").expect("repository"),
        target_base_commit: "7".repeat(40),
        produced_commit: Some("8".repeat(40)),
        produced_diff_sha256: Some("9".repeat(64)),
        artifacts: vec![EvidenceArtifactSubject {
            artifact_id: ExecutionArtifactId::new(),
            kind: "commit".to_string(),
            content_sha256: "a".repeat(64),
            size_bytes: 42,
            storage_ref: "sha256:a".to_string(),
        }],
        review_evidence: vec![],
        human_decisions: vec![],
        policy_refs: vec![immutable_ref("rbac_policy", "rbac", "3", 'b')],
        skill_packages: vec![],
        usage_ledger_sha256: "c".repeat(64),
        recovery_events_sha256: "d".repeat(64),
        issued_at: 10,
        completed_at: 20,
    }
}

fn immutable_ref(
    resource_kind: &str,
    resource_id: &str,
    resource_version: &str,
    digest: char,
) -> ImmutableEvidenceRef {
    ImmutableEvidenceRef {
        authority_key: "specify:test".to_string(),
        resource_kind: resource_kind.to_string(),
        resource_id: resource_id.to_string(),
        resource_version: resource_version.to_string(),
        content_sha256: digest.to_string().repeat(64),
    }
}

#[test]
fn canonical_json_recursively_sorts_keys() {
    let value = serde_json::json!({"z": 1, "a": {"d": 2, "b": 3}});
    assert_eq!(
        canonical_json(&value).expect("canonical JSON"),
        r#"{"a":{"b":3,"d":2},"z":1}"#
    );
}

#[tokio::test]
async fn signed_evidence_verifies_and_tampering_is_denied() {
    let signer =
        Ed25519EvidenceSigner::from_seed("builder-2026-07", EvidenceSignerRole::Builder, [7; 32])
            .expect("signer");
    let trust = TrustedSigningKey {
        key_id: signer.key_id().to_string(),
        signer_did: signer.signer_did().to_string(),
        role: EvidenceSignerRole::Builder,
        not_before: 1,
        not_after: 100,
        revoked_at: None,
        superseded_by: None,
    };
    let verifier = Ed25519EvidenceVerifier::new(Arc::new(FixedTrust { key: trust }));
    let envelope = signer
        .sign_evidence(&evidence(), 30)
        .await
        .expect("signed evidence");
    verifier
        .verify_evidence(&envelope, 40)
        .await
        .expect("verified evidence");

    let mut tampered = envelope;
    tampered.payload.prompt_sha256 = "e".repeat(64);
    assert!(verifier.verify_evidence(&tampered, 40).await.is_err());
}

#[tokio::test]
async fn revocation_preserves_old_evidence_and_rejects_new_signatures() {
    let signer =
        Ed25519EvidenceSigner::from_seed("worker-2026-07", EvidenceSignerRole::Worker, [9; 32])
            .expect("signer");
    let verifier = Ed25519EvidenceVerifier::new(Arc::new(FixedTrust {
        key: TrustedSigningKey {
            key_id: signer.key_id().to_string(),
            signer_did: signer.signer_did().to_string(),
            role: EvidenceSignerRole::Worker,
            not_before: 1,
            not_after: 100,
            revoked_at: Some(31),
            superseded_by: Some("worker-2026-08".to_string()),
        },
    }));
    let historical = signer
        .sign_evidence(&evidence(), 30)
        .await
        .expect("historical evidence");
    verifier
        .verify_evidence(&historical, 50)
        .await
        .expect("historical signature remains valid");

    let after_revocation = signer
        .sign_evidence(&evidence(), 32)
        .await
        .expect("cryptographic signature can still be presented");
    assert!(
        verifier
            .verify_evidence(&after_revocation, 50)
            .await
            .is_err()
    );
}

#[test]
fn agentd_signer_cannot_assume_openfab_role() {
    assert!(
        Ed25519EvidenceSigner::from_seed(
            "forbidden-openfab-key",
            EvidenceSignerRole::OpenFab,
            [3; 32],
        )
        .is_err()
    );
}
