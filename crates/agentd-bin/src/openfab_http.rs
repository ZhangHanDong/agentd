//! Authenticated HTTP transport for the versioned OpenFab certification API.

use std::time::Duration;

use agentd_core::ports::{
    CertificationError, CertificationPort, CertificationRequest, SignedCertificationResult,
    SkillHubPort, SkillPackageTrustRecord,
};
use agentd_core::types::{CertificationRequestId, SkillPackageVersionRef};
use reqwest::{Client, StatusCode, Url};
use zeroize::Zeroizing;

const MAX_RESULT_BYTES: usize = 1024 * 1024;

pub struct HttpOpenFabCertificationTransport {
    client: Client,
    base_url: Url,
    bearer_token: Zeroizing<String>,
}

impl std::fmt::Debug for HttpOpenFabCertificationTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpOpenFabCertificationTransport")
            .field("base_url", &self.base_url)
            .field("bearer_token", &"[REDACTED]")
            .finish()
    }
}

impl HttpOpenFabCertificationTransport {
    pub fn new(
        base_url: &str,
        bearer_token: impl Into<String>,
        timeout: Duration,
        allow_loopback_http: bool,
    ) -> Result<Self, CertificationError> {
        let mut base_url =
            Url::parse(base_url).map_err(|error| CertificationError::Invalid(error.to_string()))?;
        let bearer_token = bearer_token.into();
        let loopback = base_url
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
        if base_url.scheme() != "https" && !(allow_loopback_http && loopback) {
            return Err(CertificationError::Denied(
                "OpenFab transport requires HTTPS except explicit loopback development".to_string(),
            ));
        }
        if !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || bearer_token.trim().is_empty()
            || timeout.is_zero()
        {
            return Err(CertificationError::Invalid(
                "invalid OpenFab endpoint, credential, or timeout".to_string(),
            ));
        }
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        let client = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| CertificationError::Unavailable(error.to_string()))?;
        Ok(Self {
            client,
            base_url,
            bearer_token: Zeroizing::new(bearer_token),
        })
    }

    fn requests_url(&self) -> Result<Url, CertificationError> {
        self.base_url
            .join("v1/certifications/requests")
            .map_err(|error| CertificationError::Invalid(error.to_string()))
    }

    fn result_url(&self, request_id: &CertificationRequestId) -> Result<Url, CertificationError> {
        validate_request_id(request_id)?;
        self.base_url
            .join(&format!(
                "v1/certifications/requests/{}/result",
                request_id.as_str()
            ))
            .map_err(|error| CertificationError::Invalid(error.to_string()))
    }

    fn skill_trust_url(
        &self,
        package_ref: &SkillPackageVersionRef,
    ) -> Result<Url, CertificationError> {
        let package = package_ref.as_resource_ref();
        let mut url = self.base_url.clone();
        let mut segments = url.path_segments_mut().map_err(|()| {
            CertificationError::Invalid("OpenFab base URL cannot hold path segments".to_string())
        })?;
        segments.pop_if_empty().extend([
            "v1",
            "skills",
            "packages",
            package.authority_key().as_str(),
            package.resource_id(),
            "versions",
            package.resource_version(),
            "trust",
        ]);
        drop(segments);
        Ok(url)
    }
}

#[async_trait::async_trait]
impl SkillHubPort for HttpOpenFabCertificationTransport {
    async fn resolve_package_trust(
        &self,
        package_ref: &SkillPackageVersionRef,
        observed_at: i64,
    ) -> Result<SkillPackageTrustRecord, CertificationError> {
        if observed_at < 0 {
            return Err(CertificationError::Invalid(
                "invalid Skill Hub observation time".to_string(),
            ));
        }
        let response = self
            .client
            .get(self.skill_trust_url(package_ref)?)
            .bearer_auth(self.bearer_token.as_str())
            .header("X-Agentd-Observed-At", observed_at.to_string())
            .send()
            .await
            .map_err(|error| CertificationError::Unavailable(error.to_string()))?;
        match response.status() {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(CertificationError::Denied(
                "OpenFab rejected Skill Hub authentication".to_string(),
            )),
            StatusCode::NOT_FOUND => Err(CertificationError::NotFound(format!(
                "Skill Hub package {}@{}",
                package_ref.resource_id(),
                package_ref.resource_version()
            ))),
            StatusCode::OK => {
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|error| CertificationError::Unavailable(error.to_string()))?;
                if bytes.len() > MAX_RESULT_BYTES {
                    return Err(CertificationError::Denied(
                        "Skill Hub trust response exceeds the response bound".to_string(),
                    ));
                }
                let record: SkillPackageTrustRecord = serde_json::from_slice(&bytes)
                    .map_err(|error| CertificationError::Invalid(error.to_string()))?;
                if &record.payload.package.package_ref != package_ref {
                    return Err(CertificationError::Denied(
                        "Skill Hub trust response package identity mismatch".to_string(),
                    ));
                }
                Ok(record)
            }
            status if status.is_client_error() => Err(CertificationError::Invalid(format!(
                "OpenFab rejected Skill Hub lookup with {status}"
            ))),
            status => Err(CertificationError::Unavailable(format!(
                "OpenFab Skill Hub lookup failed with {status}"
            ))),
        }
    }
}

#[async_trait::async_trait]
impl CertificationPort for HttpOpenFabCertificationTransport {
    async fn request_certification(
        &self,
        request: &CertificationRequest,
    ) -> Result<(), CertificationError> {
        validate_request_id(&request.request_id)?;
        let response = self
            .client
            .post(self.requests_url()?)
            .bearer_auth(self.bearer_token.as_str())
            .header("Idempotency-Key", request.idempotency_key.trim())
            .json(request)
            .send()
            .await
            .map_err(|error| CertificationError::Unavailable(error.to_string()))?;
        match response.status() {
            StatusCode::OK
            | StatusCode::CREATED
            | StatusCode::ACCEPTED
            | StatusCode::NO_CONTENT => Ok(()),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(CertificationError::Denied(
                "OpenFab rejected transport authentication".to_string(),
            )),
            StatusCode::CONFLICT => Err(CertificationError::Conflict(
                "OpenFab rejected a mismatched idempotent request".to_string(),
            )),
            status if status.is_client_error() => Err(CertificationError::Invalid(format!(
                "OpenFab rejected certification request with {status}"
            ))),
            status => Err(CertificationError::Unavailable(format!(
                "OpenFab certification request failed with {status}"
            ))),
        }
    }

    async fn certification_result(
        &self,
        request_id: &CertificationRequestId,
    ) -> Result<Option<SignedCertificationResult>, CertificationError> {
        let response = self
            .client
            .get(self.result_url(request_id)?)
            .bearer_auth(self.bearer_token.as_str())
            .send()
            .await
            .map_err(|error| CertificationError::Unavailable(error.to_string()))?;
        match response.status() {
            StatusCode::NOT_FOUND | StatusCode::NO_CONTENT => Ok(None),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(CertificationError::Denied(
                "OpenFab rejected transport authentication".to_string(),
            )),
            StatusCode::OK => {
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|error| CertificationError::Unavailable(error.to_string()))?;
                if bytes.len() > MAX_RESULT_BYTES {
                    return Err(CertificationError::Denied(
                        "OpenFab certification result exceeds the response bound".to_string(),
                    ));
                }
                let result: SignedCertificationResult = serde_json::from_slice(&bytes)
                    .map_err(|error| CertificationError::Invalid(error.to_string()))?;
                if &result.payload.request_id != request_id {
                    return Err(CertificationError::Denied(
                        "OpenFab result request identity mismatch".to_string(),
                    ));
                }
                Ok(Some(result))
            }
            status if status.is_client_error() => Err(CertificationError::Invalid(format!(
                "OpenFab rejected certification result lookup with {status}"
            ))),
            status => Err(CertificationError::Unavailable(format!(
                "OpenFab certification lookup failed with {status}"
            ))),
        }
    }
}

fn validate_request_id(request_id: &CertificationRequestId) -> Result<(), CertificationError> {
    let value = request_id.as_str();
    if value.len() != 29
        || !value.starts_with("cr_")
        || !value[3..]
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'A'..=b'H' | b'J'..=b'K' | b'M'..=b'N' | b'P'..=b'T' | b'V'..=b'Z'))
    {
        return Err(CertificationError::Invalid(
            "invalid certification request id".to_string(),
        ));
    }
    Ok(())
}
