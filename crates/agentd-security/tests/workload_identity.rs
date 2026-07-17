use agentd_core::ports::{SecurityError, WorkloadIdentityPort};
use agentd_core::types::{
    SecurityDenialReason, WorkerId, WorkerIncarnationId, WorkloadIdentityRequest, WorkloadRole,
};
use agentd_security::identity::{RustlsWorkloadIdentityAdapter, certificate_sha256};
use agentd_store::SqliteStore;
use agentd_store::security_repo::{
    WorkloadIdentityBindingCreate, bind_workload_identity, revoke_workload_identity,
};
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, Ia5String, IsCa, KeyPair, SanType,
};
use serde_json::json;
use time::OffsetDateTime;

const VALID_AT: i64 = 1_800_000_000;
const NOT_BEFORE: i64 = 1_700_000_000;
const NOT_AFTER: i64 = 1_900_000_000;

struct Fixture {
    store: SqliteStore,
    _dir: tempfile::TempDir,
    worker_id: WorkerId,
    incarnation_id: WorkerIncarnationId,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let worker_id = WorkerId::new();
    worker_repo::create_worker(
        store.pool(),
        WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "agents.example".to_string(),
            labels: json!({"purpose": "identity-test"}),
        },
    )
    .await
    .expect("worker");
    let incarnation_id = WorkerIncarnationId::new();
    register_incarnation(&store, &worker_id, incarnation_id.clone()).await;
    Fixture {
        store,
        _dir: dir,
        worker_id,
        incarnation_id,
    }
}

async fn register_incarnation(store: &SqliteStore, worker_id: &WorkerId, id: WorkerIncarnationId) {
    worker_repo::register_incarnation(
        store.pool(),
        worker_id,
        WorkerRegistration {
            id,
            daemon_version: "0.0.0-ad-e1".to_string(),
            host_name: "identity-host".to_string(),
            network_zone: Some("test".to_string()),
            capabilities: json!({"sandbox": "oci"}),
        },
    )
    .await
    .expect("incarnation");
}

struct TestCa {
    certificate: rcgen::Certificate,
    key: KeyPair,
}

fn test_ca(common_name: &str) -> TestCa {
    let mut params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, common_name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.not_before = OffsetDateTime::from_unix_timestamp(NOT_BEFORE).expect("not before");
    params.not_after = OffsetDateTime::from_unix_timestamp(NOT_AFTER).expect("not after");
    let key = KeyPair::generate().expect("CA key");
    let certificate = params.self_signed(&key).expect("CA certificate");
    TestCa { certificate, key }
}

fn worker_certificate(ca: &TestCa, incarnation_id: &WorkerIncarnationId) -> Vec<u8> {
    let spiffe_uri = format!("spiffe://agents.example/worker/{incarnation_id}");
    worker_certificate_for_uri(ca, &spiffe_uri)
}

fn worker_certificate_for_uri(ca: &TestCa, spiffe_uri: &str) -> Vec<u8> {
    let mut params = CertificateParams::new(Vec::<String>::new()).expect("leaf params");
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "agentd worker");
    params.subject_alt_names = vec![SanType::URI(
        Ia5String::try_from(spiffe_uri).expect("SPIFFE URI"),
    )];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    params.not_before = OffsetDateTime::from_unix_timestamp(NOT_BEFORE).expect("not before");
    params.not_after = OffsetDateTime::from_unix_timestamp(NOT_AFTER).expect("not after");
    let key = KeyPair::generate().expect("leaf key");
    params
        .signed_by(&key, &ca.certificate, &ca.key)
        .expect("signed leaf")
        .der()
        .to_vec()
}

async fn bind(fixture: &Fixture, certificate_der: &[u8], incarnation_id: &WorkerIncarnationId) {
    bind_workload_identity(
        fixture.store.pool(),
        WorkloadIdentityBindingCreate {
            certificate_sha256: certificate_sha256(certificate_der),
            spiffe_uri: format!("spiffe://agents.example/worker/{incarnation_id}"),
            role: WorkloadRole::Worker,
            trust_domain: "agents.example".to_string(),
            worker_id: Some(fixture.worker_id.clone()),
            worker_incarnation_id: Some(incarnation_id.clone()),
            not_before: NOT_BEFORE,
            not_after: NOT_AFTER,
            created_at: VALID_AT - 10,
        },
    )
    .await
    .expect("identity binding");
}

fn request(certificate_der: Vec<u8>, observed_at: i64) -> WorkloadIdentityRequest {
    WorkloadIdentityRequest {
        peer_certificates_der: vec![certificate_der],
        observed_at,
    }
}

fn assert_denied(error: &SecurityError, expected: SecurityDenialReason) {
    assert_eq!(error.denial_reason(), Some(expected), "got {error:?}");
}

#[tokio::test]
async fn workload_identity_accepts_verified_current_worker_certificate() {
    let fixture = fixture().await;
    let ca = test_ca("trusted test CA");
    let leaf = worker_certificate(&ca, &fixture.incarnation_id);
    bind(&fixture, &leaf, &fixture.incarnation_id).await;
    let adapter = RustlsWorkloadIdentityAdapter::new(
        fixture.store.pool().clone(),
        vec![ca.certificate.der().to_vec()],
        "agents.example",
    )
    .expect("identity adapter");

    let workload = adapter
        .authenticate_workload(&request(leaf.clone(), VALID_AT))
        .await
        .expect("authenticated workload");
    assert_eq!(workload.role, WorkloadRole::Worker);
    assert_eq!(workload.trust_domain, "agents.example");
    assert_eq!(workload.certificate_sha256, certificate_sha256(&leaf));
    assert_eq!(workload.not_before, NOT_BEFORE);
    assert_eq!(workload.not_after, NOT_AFTER);
    assert_eq!(workload.worker_id, Some(fixture.worker_id));
    assert_eq!(workload.worker_incarnation_id, Some(fixture.incarnation_id));
}

#[tokio::test]
async fn enrollment_verifier_accepts_trusted_unbound_worker_certificate() {
    let fixture = fixture().await;
    let ca = test_ca("trusted enrollment CA");
    let leaf = worker_certificate(&ca, &fixture.incarnation_id);
    let adapter = RustlsWorkloadIdentityAdapter::new(
        fixture.store.pool().clone(),
        vec![ca.certificate.der().to_vec()],
        "agents.example",
    )
    .expect("identity adapter");

    let verified = adapter
        .verify_enrollment_certificate(&request(leaf.clone(), VALID_AT))
        .expect("verified enrollment certificate");

    assert_eq!(
        verified.spiffe_uri,
        format!("spiffe://agents.example/worker/{}", fixture.incarnation_id)
    );
    assert_eq!(verified.trust_domain, "agents.example");
    assert_eq!(verified.worker_incarnation_id, fixture.incarnation_id);
    assert_eq!(verified.certificate_sha256, certificate_sha256(&leaf));
    assert_eq!(verified.not_before, NOT_BEFORE);
    assert_eq!(verified.not_after, NOT_AFTER);
}

#[tokio::test]
async fn enrollment_verifier_rejects_noncanonical_identity_and_oversized_chain() {
    let fixture = fixture().await;
    let ca = test_ca("trusted enrollment boundary CA");
    let adapter = RustlsWorkloadIdentityAdapter::new(
        fixture.store.pool().clone(),
        vec![ca.certificate.der().to_vec()],
        "agents.example",
    )
    .expect("identity adapter");
    let invalid_id =
        worker_certificate_for_uri(&ca, "spiffe://agents.example/worker/wi_not-a-ulid");
    let error = adapter
        .verify_enrollment_certificate(&request(invalid_id, VALID_AT))
        .expect_err("noncanonical worker identity");
    assert_denied(&error, SecurityDenialReason::IdentityUntrusted);

    let leaf = worker_certificate(&ca, &fixture.incarnation_id);
    let error = adapter
        .verify_enrollment_certificate(&WorkloadIdentityRequest {
            peer_certificates_der: vec![leaf; 9],
            observed_at: VALID_AT,
        })
        .expect_err("oversized certificate chain");
    assert_denied(&error, SecurityDenialReason::IdentityUntrusted);

    assert!(
        RustlsWorkloadIdentityAdapter::new(
            fixture.store.pool().clone(),
            vec![ca.certificate.der().to_vec()],
            "Agents.example",
        )
        .is_err()
    );
}

#[tokio::test]
async fn workload_identity_rejects_untrusted_expired_revoked_and_stale_peers() {
    let fixture = fixture().await;
    let trusted_ca = test_ca("trusted test CA");
    let untrusted_ca = test_ca("untrusted test CA");
    let adapter = RustlsWorkloadIdentityAdapter::new(
        fixture.store.pool().clone(),
        vec![trusted_ca.certificate.der().to_vec()],
        "agents.example",
    )
    .expect("identity adapter");

    let untrusted_leaf = worker_certificate(&untrusted_ca, &fixture.incarnation_id);
    let untrusted = adapter
        .authenticate_workload(&request(untrusted_leaf, VALID_AT))
        .await
        .expect_err("untrusted chain");
    assert_denied(&untrusted, SecurityDenialReason::IdentityUntrusted);

    let expired_leaf = worker_certificate(&trusted_ca, &fixture.incarnation_id);
    bind(&fixture, &expired_leaf, &fixture.incarnation_id).await;
    let expired = adapter
        .authenticate_workload(&request(expired_leaf, NOT_AFTER + 1))
        .await
        .expect_err("expired certificate");
    assert_denied(&expired, SecurityDenialReason::IdentityExpired);

    let revoked_leaf = worker_certificate(&trusted_ca, &fixture.incarnation_id);
    bind(&fixture, &revoked_leaf, &fixture.incarnation_id).await;
    revoke_workload_identity(
        fixture.store.pool(),
        &certificate_sha256(&revoked_leaf),
        VALID_AT - 1,
        "test_revocation",
    )
    .await
    .expect("revoke identity");
    let replay = revoke_workload_identity(
        fixture.store.pool(),
        &certificate_sha256(&revoked_leaf),
        VALID_AT,
        "test_revocation",
    )
    .await
    .expect("replay identity revocation");
    assert_eq!(replay.revoked_at, Some(VALID_AT - 1));
    let worker_status: String = sqlx::query_scalar("SELECT status FROM workers WHERE id = ?")
        .bind(fixture.worker_id.as_str())
        .fetch_one(fixture.store.pool())
        .await
        .expect("worker status");
    assert_eq!(worker_status, "offline");
    assert!(
        revoke_workload_identity(
            fixture.store.pool(),
            &certificate_sha256(&revoked_leaf),
            VALID_AT + 1,
            "changed_reason",
        )
        .await
        .is_err()
    );
    let revoked = adapter
        .authenticate_workload(&request(revoked_leaf, VALID_AT))
        .await
        .expect_err("revoked certificate");
    assert_denied(&revoked, SecurityDenialReason::IdentityRevoked);

    let stale_leaf = worker_certificate(&trusted_ca, &fixture.incarnation_id);
    bind(&fixture, &stale_leaf, &fixture.incarnation_id).await;
    register_incarnation(
        &fixture.store,
        &fixture.worker_id,
        WorkerIncarnationId::new(),
    )
    .await;
    let stale = adapter
        .authenticate_workload(&request(stale_leaf, VALID_AT))
        .await
        .expect_err("superseded incarnation");
    assert_denied(&stale, SecurityDenialReason::IncarnationStale);
}

#[tokio::test]
async fn workload_identity_revocation_cannot_predate_its_binding() {
    let fixture = fixture().await;
    let trusted_ca = test_ca("trusted test CA");
    let leaf = worker_certificate(&trusted_ca, &fixture.incarnation_id);
    bind(&fixture, &leaf, &fixture.incarnation_id).await;

    assert!(
        revoke_workload_identity(
            fixture.store.pool(),
            &certificate_sha256(&leaf),
            VALID_AT - 11,
            "invalid_historical_revocation",
        )
        .await
        .is_err()
    );

    let adapter = RustlsWorkloadIdentityAdapter::new(
        fixture.store.pool().clone(),
        vec![trusted_ca.certificate.der().to_vec()],
        "agents.example",
    )
    .expect("identity adapter");
    adapter
        .authenticate_workload(&request(leaf, VALID_AT))
        .await
        .expect("binding remains active");
}
