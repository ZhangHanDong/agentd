use agentd_core::ports::{
    CertificationRequest, CertificationResultPayload, CertificationStatePort, CertificationVerdict,
    EvidenceEnvelopeStorePort, EvidenceSignerRole, ExecutionEvidenceEnvelopePayload,
    SignedCertificationResult, SignedExecutionEvidenceEnvelope, SigningKeyTrustPort,
    TrustedSigningKey, canonical_sha256,
};
use agentd_core::types::{
    AuthorityKey, CertificationGate, CertificationPolicyVersionRef, CertificationRequestId,
    CertificationResultId, EvidenceEnvelopeId, OrganizationRef, ProjectExecutionSnapshotRef,
    ProjectRef, RepositoryRef, RunId,
};
use agentd_store::SqliteStore;
use agentd_store::certification_control_plane::SqliteCertificationControlPlane;

fn digest(character: char) -> String {
    character.to_string().repeat(64)
}

fn refs() -> (
    OrganizationRef,
    ProjectRef,
    ProjectExecutionSnapshotRef,
    RepositoryRef,
    CertificationPolicyVersionRef,
) {
    let authority = AuthorityKey::new("specify:test").expect("authority");
    (
        OrganizationRef::new(authority.clone(), "org", "1").expect("organization"),
        ProjectRef::new(authority.clone(), "project", "1").expect("project"),
        ProjectExecutionSnapshotRef::new(authority.clone(), "snapshot", "1").expect("snapshot"),
        RepositoryRef::new(authority.clone(), "repo", "1").expect("repository"),
        CertificationPolicyVersionRef::new(authority, "certification", "3").expect("policy"),
    )
}

fn envelope(run_id: RunId, key: &TrustedSigningKey) -> SignedExecutionEvidenceEnvelope {
    let (organization_ref, project_ref, snapshot_ref, repository_ref, certification_policy_ref) =
        refs();
    SignedExecutionEvidenceEnvelope {
        schema_version: 1,
        payload: ExecutionEvidenceEnvelopePayload {
            schema_version: 1,
            envelope_id: EvidenceEnvelopeId::new(),
            organization_ref,
            project_ref,
            snapshot_ref,
            snapshot_content_sha256: digest('1'),
            execution_run_id: run_id,
            execution_task_id: None,
            runtime_session_id: None,
            runtime_attempt_id: None,
            worker_incarnation_id: None,
            agent_profile_id: None,
            lease_id: None,
            fencing_token: None,
            runtime_name: "codex".to_string(),
            model: None,
            sandbox_profile_sha256: digest('2'),
            requirements: vec![],
            frozen_spec: agentd_core::ports::ImmutableEvidenceRef {
                authority_key: "specify:test".to_string(),
                resource_kind: "frozen_spec".to_string(),
                resource_id: "spec".to_string(),
                resource_version: "1".to_string(),
                content_sha256: digest('3'),
            },
            prompt_sha256: digest('4'),
            plan_sha256: None,
            target_repository_ref: repository_ref,
            target_base_commit: "5".repeat(40),
            produced_commit: Some("6".repeat(40)),
            produced_diff_sha256: Some(digest('7')),
            artifacts: vec![agentd_core::ports::EvidenceArtifactSubject {
                artifact_id: agentd_core::types::ExecutionArtifactId::new(),
                kind: "commit".to_string(),
                content_sha256: digest('b'),
                size_bytes: 1,
                storage_ref: "sha256:subject".to_string(),
            }],
            review_evidence: vec![],
            human_decisions: vec![],
            policy_refs: vec![agentd_core::ports::ImmutableEvidenceRef {
                authority_key: certification_policy_ref
                    .authority_key()
                    .as_str()
                    .to_string(),
                resource_kind: "certification_policy".to_string(),
                resource_id: certification_policy_ref.resource_id().to_string(),
                resource_version: certification_policy_ref.resource_version().to_string(),
                content_sha256: digest('c'),
            }],
            skill_packages: vec![],
            usage_ledger_sha256: digest('8'),
            recovery_events_sha256: digest('9'),
            issued_at: 10,
            completed_at: 20,
        },
        payload_sha256: digest('a'),
        signer_key_id: key.key_id.clone(),
        signer_did: key.signer_did.clone(),
        signer_role: EvidenceSignerRole::Builder,
        signature_algorithm: "ed25519".to_string(),
        signature_b64: "test-signature".to_string(),
        signed_at: 30,
    }
}

fn request(envelope: &SignedExecutionEvidenceEnvelope) -> CertificationRequest {
    let (_, _, _, _, certification_policy_ref) = refs();
    CertificationRequest {
        schema_version: 1,
        request_id: CertificationRequestId::new(),
        idempotency_key: "request-once".to_string(),
        openfab_authority_key: "openfab:test".to_string(),
        envelope_id: envelope.payload.envelope_id.clone(),
        evidence_payload_sha256: envelope.payload_sha256.clone(),
        evidence_storage_ref: "sha256:evidence".to_string(),
        snapshot_ref: envelope.payload.snapshot_ref.clone(),
        snapshot_content_sha256: envelope.payload.snapshot_content_sha256.clone(),
        source_commit: "6".repeat(40),
        subject_sha256: digest('b'),
        spec_sha256: envelope.payload.frozen_spec.content_sha256.clone(),
        certification_policy_ref,
        certification_policy_sha256: digest('c'),
        gate: CertificationGate::Machine,
        skill_packages: vec![],
        skill_packages_sha256: canonical_sha256(&Vec::<serde_json::Value>::new())
            .expect("skill digest"),
        requested_at: 40,
    }
}

fn result(
    request: &CertificationRequest,
    openfab_key: &TrustedSigningKey,
) -> SignedCertificationResult {
    SignedCertificationResult {
        schema_version: 1,
        payload: CertificationResultPayload {
            schema_version: 1,
            result_id: CertificationResultId::new(),
            request_id: request.request_id.clone(),
            openfab_authority_key: request.openfab_authority_key.clone(),
            evidence_payload_sha256: request.evidence_payload_sha256.clone(),
            snapshot_ref: request.snapshot_ref.clone(),
            snapshot_content_sha256: request.snapshot_content_sha256.clone(),
            source_commit: request.source_commit.clone(),
            subject_sha256: request.subject_sha256.clone(),
            spec_sha256: request.spec_sha256.clone(),
            certification_policy_ref: request.certification_policy_ref.clone(),
            certification_policy_sha256: request.certification_policy_sha256.clone(),
            skill_packages_sha256: request.skill_packages_sha256.clone(),
            verdict: CertificationVerdict::Pass,
            machine_attested: true,
            required_human_signoffs: 0,
            eligible_human_signoffs: 0,
            accepted_human_signoffs: 0,
            reason_code: None,
            signed_ref: "sha256:openfab-result".to_string(),
            published_at: 50,
            revoked_at: None,
        },
        payload_sha256: digest('d'),
        signer_key_id: openfab_key.key_id.clone(),
        signer_did: openfab_key.signer_did.clone(),
        signature_algorithm: "ed25519".to_string(),
        signature_b64: "openfab-signature".to_string(),
    }
}

#[tokio::test]
async fn durable_request_and_result_are_exact_independent_events() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let store = SqliteStore::connect(&temporary.path().join("agentd.db"))
        .await
        .expect("store");
    let control = SqliteCertificationControlPlane::new(store.pool().clone());
    let builder_key = TrustedSigningKey {
        key_id: "builder-1".to_string(),
        signer_did: "did:key:zBuilderTest".to_string(),
        role: EvidenceSignerRole::Builder,
        not_before: 1,
        not_after: 100,
        revoked_at: None,
        superseded_by: None,
    };
    let openfab_key = TrustedSigningKey {
        key_id: "openfab-1".to_string(),
        signer_did: "did:key:zOpenFabTest".to_string(),
        role: EvidenceSignerRole::OpenFab,
        not_before: 1,
        not_after: 100,
        revoked_at: None,
        superseded_by: None,
    };
    control
        .register_signing_key(&builder_key, 1)
        .await
        .expect("builder key");
    control
        .register_signing_key(&openfab_key, 1)
        .await
        .expect("OpenFab key");
    assert_eq!(
        control
            .resolve_signing_key("builder-1", EvidenceSignerRole::Builder, 30)
            .await
            .expect("resolved key"),
        builder_key
    );

    let run_id = RunId::new();
    sqlx::query(
        "INSERT INTO runs (id, workflow_sha, status, started_at, last_heartbeat) \
         VALUES (?, ?, 'running', 1, 1)",
    )
    .bind(run_id.as_str())
    .bind(digest('e'))
    .execute(store.pool())
    .await
    .expect("run");
    let evidence = envelope(run_id, &builder_key);
    control
        .store_evidence_envelope(&evidence)
        .await
        .expect("evidence");
    let request = request(&evidence);
    assert_eq!(
        control
            .record_certification_request(&request)
            .await
            .expect("request"),
        request
    );
    let result = result(&request, &openfab_key);
    assert_eq!(
        control
            .record_certification_result(&result)
            .await
            .expect("result"),
        result
    );
    assert_eq!(
        control
            .pending_protocol_events(10)
            .await
            .expect("events")
            .len(),
        2
    );

    control
        .record_certification_request(&request)
        .await
        .expect("exact request replay");
    control
        .record_certification_result(&result)
        .await
        .expect("exact result replay");
    let mut mismatched = result;
    mismatched.payload.subject_sha256 = digest('f');
    assert!(
        control
            .record_certification_result(&mismatched)
            .await
            .is_err()
    );
}
