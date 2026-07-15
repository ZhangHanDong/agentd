//! Verified workload-identity adapter.

use std::sync::Arc;
use std::time::Duration;

use agentd_core::ports::{SecurityError, WorkloadIdentityPort};
use agentd_core::types::{
    AuthenticatedWorkload, SecurityDenialReason, WorkloadIdentityRequest, WorkloadRole,
};
use agentd_store::security_repo::get_workload_identity_binding;
use agentd_store::worker_repo;
use rustls::RootCertStore;
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::ClientCertVerifier;
use rustls_pki_types::{CertificateDer, UnixTime};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use url::Url;
use x509_parser::extensions::GeneralName;
use x509_parser::parse_x509_certificate;

#[derive(Debug)]
pub struct RustlsWorkloadIdentityAdapter {
    pool: SqlitePool,
    verifier: Arc<dyn ClientCertVerifier>,
    trust_domain: String,
}

impl RustlsWorkloadIdentityAdapter {
    pub fn new(
        pool: SqlitePool,
        trust_roots_der: Vec<Vec<u8>>,
        trust_domain: impl Into<String>,
    ) -> Result<Self, SecurityError> {
        let trust_domain = trust_domain.into();
        if trust_roots_der.is_empty()
            || trust_domain.trim().is_empty()
            || trust_domain.contains('/')
        {
            return Err(SecurityError::Invalid(
                "identity adapter requires trust roots and one DNS-like trust domain".to_string(),
            ));
        }
        let mut roots = RootCertStore::empty();
        for root in trust_roots_der {
            roots
                .add(CertificateDer::from(root))
                .map_err(|error| SecurityError::Invalid(format!("invalid trust root: {error}")))?;
        }
        let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|error| {
                SecurityError::Invalid(format!("invalid client certificate verifier: {error}"))
            })?;
        Ok(Self {
            pool,
            verifier,
            trust_domain,
        })
    }
}

#[async_trait::async_trait]
impl WorkloadIdentityPort for RustlsWorkloadIdentityAdapter {
    async fn authenticate_workload(
        &self,
        request: &WorkloadIdentityRequest,
    ) -> Result<AuthenticatedWorkload, SecurityError> {
        if request.observed_at < 0 {
            return Err(SecurityError::Denied(SecurityDenialReason::IdentityExpired));
        }
        let (leaf_bytes, intermediates) =
            request
                .peer_certificates_der
                .split_first()
                .ok_or(SecurityError::Denied(
                    SecurityDenialReason::IdentityUntrusted,
                ))?;
        let parsed = parse_leaf_identity(leaf_bytes)?;
        if request.observed_at < parsed.not_before || request.observed_at >= parsed.not_after {
            return Err(SecurityError::Denied(SecurityDenialReason::IdentityExpired));
        }
        let leaf = CertificateDer::from(leaf_bytes.clone());
        let intermediates = intermediates
            .iter()
            .cloned()
            .map(CertificateDer::from)
            .collect::<Vec<_>>();
        let now = UnixTime::since_unix_epoch(Duration::from_secs(
            u64::try_from(request.observed_at)
                .map_err(|_| SecurityError::Denied(SecurityDenialReason::IdentityExpired))?,
        ));
        self.verifier
            .verify_client_cert(&leaf, &intermediates, now)
            .map_err(|_| SecurityError::Denied(SecurityDenialReason::IdentityUntrusted))?;

        let fingerprint = certificate_sha256(leaf_bytes);
        let record = get_workload_identity_binding(&self.pool, &fingerprint)
            .await?
            .ok_or(SecurityError::Denied(
                SecurityDenialReason::IdentityUntrusted,
            ))?;
        if record.revoked_at.is_some() {
            return Err(SecurityError::Denied(SecurityDenialReason::IdentityRevoked));
        }
        let binding = record.binding;
        if binding.certificate_sha256 != fingerprint
            || binding.spiffe_uri != parsed.spiffe_uri
            || binding.trust_domain != self.trust_domain
            || parsed.trust_domain != self.trust_domain
            || binding.not_before != parsed.not_before
            || binding.not_after != parsed.not_after
            || binding.role != WorkloadRole::Worker
            || binding.worker_incarnation_id.as_ref() != Some(&parsed.worker_incarnation_id)
        {
            return Err(SecurityError::Denied(
                SecurityDenialReason::IdentityUntrusted,
            ));
        }
        let worker_id = binding.worker_id.as_ref().ok_or(SecurityError::Denied(
            SecurityDenialReason::IncarnationStale,
        ))?;
        let incarnation = worker_repo::get_incarnation(&self.pool, &parsed.worker_incarnation_id)
            .await
            .map_err(|error| {
                SecurityError::Unavailable(format!("worker identity lookup failed: {error}"))
            })?
            .ok_or(SecurityError::Denied(
                SecurityDenialReason::IncarnationStale,
            ))?;
        if !incarnation.is_current || &incarnation.worker_id != worker_id {
            return Err(SecurityError::Denied(
                SecurityDenialReason::IncarnationStale,
            ));
        }
        Ok(AuthenticatedWorkload {
            spiffe_uri: binding.spiffe_uri,
            role: binding.role,
            trust_domain: binding.trust_domain,
            certificate_sha256: fingerprint,
            not_before: parsed.not_before,
            not_after: parsed.not_after,
            worker_id: binding.worker_id,
            worker_incarnation_id: binding.worker_incarnation_id,
        })
    }
}

struct ParsedLeafIdentity {
    spiffe_uri: String,
    trust_domain: String,
    worker_incarnation_id: agentd_core::types::WorkerIncarnationId,
    not_before: i64,
    not_after: i64,
}

fn parse_leaf_identity(certificate_der: &[u8]) -> Result<ParsedLeafIdentity, SecurityError> {
    let (remaining, certificate) = parse_x509_certificate(certificate_der)
        .map_err(|_| SecurityError::Denied(SecurityDenialReason::IdentityUntrusted))?;
    if !remaining.is_empty() {
        return Err(SecurityError::Denied(
            SecurityDenialReason::IdentityUntrusted,
        ));
    }
    let san = certificate
        .subject_alternative_name()
        .map_err(|_| SecurityError::Denied(SecurityDenialReason::IdentityUntrusted))?
        .ok_or(SecurityError::Denied(
            SecurityDenialReason::IdentityUntrusted,
        ))?;
    let uris = san
        .value
        .general_names
        .iter()
        .filter_map(|name| match name {
            GeneralName::URI(uri) => Some(*uri),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [spiffe_uri] = uris.as_slice() else {
        return Err(SecurityError::Denied(
            SecurityDenialReason::IdentityUntrusted,
        ));
    };
    let uri = Url::parse(spiffe_uri)
        .map_err(|_| SecurityError::Denied(SecurityDenialReason::IdentityUntrusted))?;
    if uri.scheme() != "spiffe"
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.port().is_some()
        || uri.query().is_some()
        || uri.fragment().is_some()
    {
        return Err(SecurityError::Denied(
            SecurityDenialReason::IdentityUntrusted,
        ));
    }
    let trust_domain = uri.host_str().ok_or(SecurityError::Denied(
        SecurityDenialReason::IdentityUntrusted,
    ))?;
    let segments = uri
        .path_segments()
        .ok_or(SecurityError::Denied(
            SecurityDenialReason::IdentityUntrusted,
        ))?
        .collect::<Vec<_>>();
    let ["worker", incarnation_id] = segments.as_slice() else {
        return Err(SecurityError::Denied(
            SecurityDenialReason::IdentityUntrusted,
        ));
    };
    if !incarnation_id.starts_with("wi_") {
        return Err(SecurityError::Denied(
            SecurityDenialReason::IdentityUntrusted,
        ));
    }
    Ok(ParsedLeafIdentity {
        spiffe_uri: (*spiffe_uri).to_string(),
        trust_domain: trust_domain.to_string(),
        worker_incarnation_id: agentd_core::types::WorkerIncarnationId::from_string(
            *incarnation_id,
        ),
        not_before: certificate.validity().not_before.timestamp(),
        not_after: certificate.validity().not_after.timestamp(),
    })
}

#[must_use]
pub fn certificate_sha256(certificate_der: &[u8]) -> String {
    hex::encode(Sha256::digest(certificate_der))
}
