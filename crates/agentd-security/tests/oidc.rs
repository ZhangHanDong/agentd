use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use agentd_core::ports::{Clock, EnterprisePrincipalPort as _};
use agentd_core::types::{
    AuthorityKey, EnterprisePrincipalId, MatrixTrustPolicy, OidcPrincipalResolveRequest,
    OrganizationRef, PrincipalKind, SecurityDenialReason,
};
use agentd_security::oidc::{OidcAuthenticator, OidcJwk, OidcProviderConfig, OidcSigningAlgorithm};
use agentd_store::SqliteStore;
use agentd_store::principal_repo::{
    OidcSubjectBinding, PrincipalUpsert, SqliteEnterprisePrincipalRepository,
};
use base64::Engine as _;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey as _, LineEnding};
use rsa::rand_core::OsRng;
use rsa::traits::PublicKeyParts as _;
use serde::Serialize;

#[derive(Clone, Copy, Serialize)]
#[serde(untagged)]
enum Audience<'a> {
    One(&'a str),
    Many(&'a [&'a str]),
}

#[derive(Clone, Copy, Serialize)]
struct Claims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: Audience<'a>,
    exp: i64,
    nbf: i64,
    iat: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    azp: Option<&'a str>,
}

#[derive(Debug)]
struct TestClock(AtomicI64);

impl TestClock {
    fn new(now: i64) -> Self {
        Self(AtomicI64::new(now))
    }

    fn set(&self, now: i64) {
        self.0.store(now, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_unix(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct TestKey {
    encoding: EncodingKey,
    jwk: OidcJwk,
}

fn test_key() -> TestKey {
    let private = RsaPrivateKey::new(&mut OsRng, 2048).expect("test RSA key");
    let pem = private
        .to_pkcs8_pem(LineEnding::LF)
        .expect("private key PEM");
    let encoding = EncodingKey::from_rsa_pem(pem.as_bytes()).expect("encoding key");
    let encoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    TestKey {
        encoding,
        jwk: OidcJwk {
            kid: "key-a".to_string(),
            algorithm: OidcSigningAlgorithm::Rs256,
            modulus_base64url: encoder.encode(private.n().to_bytes_be()),
            exponent_base64url: encoder.encode(private.e().to_bytes_be()),
        },
    }
}

fn token(key: &EncodingKey, claims: &Claims<'_>) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("key-a".to_string());
    jsonwebtoken::encode(&header, claims, key).expect("signed token")
}

async fn fixture() -> (
    tempfile::TempDir,
    Arc<SqliteEnterprisePrincipalRepository>,
    EnterprisePrincipalId,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let repo = Arc::new(
        SqliteEnterprisePrincipalRepository::new(
            store.pool().clone(),
            MatrixTrustPolicy {
                trusted_homeservers: BTreeSet::new(),
                trusted_appservices: BTreeSet::new(),
            },
            300,
        )
        .expect("repository"),
    );
    let id = EnterprisePrincipalId::new();
    let authority = AuthorityKey::new("specify:oidc-test").expect("authority");
    repo.upsert_principal(PrincipalUpsert {
        id: id.clone(),
        organization_ref: OrganizationRef::new(authority, "org-a", "4").expect("organization"),
        kind: PrincipalKind::Human,
        display_name: "Alex".to_string(),
        observed_at: 100,
    })
    .await
    .expect("principal");
    repo.bind_oidc_subject(OidcSubjectBinding {
        issuer: "https://identity.example".to_string(),
        subject: "subject-a".to_string(),
        principal_id: id.clone(),
        bound_at: 101,
    })
    .await
    .expect("OIDC binding");
    (dir, repo, id)
}

fn config(jwk: OidcJwk) -> OidcProviderConfig {
    OidcProviderConfig {
        issuer: "https://identity.example".to_string(),
        audiences: BTreeSet::from(["agentd-control-plane".to_string()]),
        authorized_parties: BTreeSet::from(["agentd-cli".to_string()]),
        keys: vec![jwk],
    }
}

#[tokio::test]
async fn oidc_authenticator_verifies_signature_claims_and_principal_lifecycle() {
    let (_dir, repo, id) = fixture().await;
    let key = test_key();
    let clock = Arc::new(TestClock::new(120));
    let authenticator = OidcAuthenticator::new(
        Arc::clone(&repo),
        Arc::clone(&clock),
        config(key.jwk.clone()),
    )
    .expect("authenticator");
    let valid_claims = Claims {
        iss: "https://identity.example",
        sub: "subject-a",
        aud: Audience::One("agentd-control-plane"),
        exp: 200,
        nbf: 100,
        iat: 100,
        azp: None,
    };
    let valid_token = token(&key.encoding, &valid_claims);
    let identity = authenticator
        .authenticate(&valid_token)
        .await
        .expect("valid identity");
    assert_eq!(identity.principal.id, id);
    assert_eq!(identity.expires_at, 200, "token expiry caps repository ttl");

    let multiple_audiences = ["agentd-control-plane", "agentd-cli"];
    let missing_azp = Claims {
        aud: Audience::Many(&multiple_audiences),
        ..valid_claims
    };
    assert_eq!(
        authenticator
            .authenticate(&token(&key.encoding, &missing_azp))
            .await
            .expect_err("multiple audiences require an authorized party")
            .denial_reason(),
        Some(SecurityDenialReason::IdentityUntrusted)
    );
    let authorized_multi_audience = Claims {
        aud: Audience::Many(&multiple_audiences),
        azp: Some("agentd-cli"),
        ..valid_claims
    };
    authenticator
        .authenticate(&token(&key.encoding, &authorized_multi_audience))
        .await
        .expect("configured authorized party");

    for claims in [
        Claims {
            iss: "https://foreign.example",
            ..valid_claims
        },
        Claims {
            aud: Audience::One("other-service"),
            ..valid_claims
        },
        Claims {
            nbf: 121,
            ..valid_claims
        },
    ] {
        let denied = authenticator
            .authenticate(&token(&key.encoding, &claims))
            .await
            .expect_err("invalid claims must fail closed");
        assert_eq!(
            denied.denial_reason(),
            Some(SecurityDenialReason::IdentityUntrusted)
        );
    }

    let expired = Claims {
        exp: 120,
        ..valid_claims
    };
    assert_eq!(
        authenticator
            .authenticate(&token(&key.encoding, &expired))
            .await
            .expect_err("expired token")
            .denial_reason(),
        Some(SecurityDenialReason::IdentityExpired)
    );

    repo.disable_principal(&id, 130)
        .await
        .expect("disable principal");
    clock.set(131);
    assert_eq!(
        authenticator
            .authenticate(&valid_token)
            .await
            .expect_err("disabled principal")
            .denial_reason(),
        Some(SecurityDenialReason::PrincipalDisabled)
    );
}

#[tokio::test]
async fn oidc_authenticator_rejects_algorithm_confusion_unknown_kid_and_redacts_token() {
    let (_dir, repo, _id) = fixture().await;
    let key = test_key();
    let authenticator = OidcAuthenticator::new(
        Arc::clone(&repo),
        Arc::new(TestClock::new(120)),
        config(key.jwk),
    )
    .expect("authenticator");
    let claims = Claims {
        iss: "https://identity.example",
        sub: "subject-a",
        aud: Audience::One("agentd-control-plane"),
        exp: 200,
        nbf: 100,
        iat: 100,
        azp: None,
    };

    let mut unknown_header = Header::new(Algorithm::RS256);
    unknown_header.kid = Some("unknown".to_string());
    let unknown =
        jsonwebtoken::encode(&unknown_header, &claims, &key.encoding).expect("unknown-kid token");
    let unknown_error = authenticator
        .authenticate(&unknown)
        .await
        .expect_err("unknown kid");
    assert_eq!(
        unknown_error.denial_reason(),
        Some(SecurityDenialReason::IdentityUntrusted)
    );
    assert!(!unknown_error.to_string().contains(&unknown));

    let mut hmac_header = Header::new(Algorithm::HS256);
    hmac_header.kid = Some("key-a".to_string());
    let confused = jsonwebtoken::encode(
        &hmac_header,
        &claims,
        &EncodingKey::from_secret(b"not-an-rsa-key"),
    )
    .expect("HMAC token");
    let confused_error = authenticator
        .authenticate(&confused)
        .await
        .expect_err("algorithm confusion");
    assert_eq!(
        confused_error.denial_reason(),
        Some(SecurityDenialReason::IdentityUntrusted)
    );
    assert!(!confused_error.to_string().contains(&confused));

    let unmapped = authenticator
        .repository()
        .resolve_oidc(&OidcPrincipalResolveRequest {
            issuer: "https://identity.example".to_string(),
            subject: "missing".to_string(),
            observed_at: 120,
        })
        .await
        .expect_err("unmapped subject");
    assert_eq!(
        unmapped.denial_reason(),
        Some(SecurityDenialReason::PrincipalUnmapped)
    );
}
