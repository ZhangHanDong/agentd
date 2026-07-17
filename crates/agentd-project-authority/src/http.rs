//! Authenticated, bounded HTTPS transport for Specify project authority.

use std::time::Duration;

use agentd_core::ports::{
    ProjectAuthorityAvailability, ProjectAuthorityError, ProjectAuthorityHealth,
    ProjectSnapshotResolveRequest,
};
use agentd_core::types::{ProjectExecutionSnapshot, ProjectExecutionSnapshotRef};
use reqwest::{Client, Response, StatusCode, Url};
use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

use crate::SpecifyAuthorityTransport;

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

pub struct HttpSpecifyAuthorityTransport {
    client: Client,
    base_url: Url,
    authorization: Zeroizing<String>,
}

impl std::fmt::Debug for HttpSpecifyAuthorityTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpSpecifyAuthorityTransport")
            .field("client", &self.client)
            .field("base_url", &self.base_url)
            .field("authorization", &"[REDACTED]")
            .finish()
    }
}

impl HttpSpecifyAuthorityTransport {
    pub fn new(
        base_url: &str,
        authorization: impl Into<String>,
        timeout: Duration,
        allow_loopback_http: bool,
    ) -> Result<Self, ProjectAuthorityError> {
        let mut base_url = Url::parse(base_url)
            .map_err(|error| ProjectAuthorityError::Invalid(error.to_string()))?;
        let authorization = authorization.into();
        let loopback = base_url
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
        if base_url.scheme() != "https" && !(allow_loopback_http && loopback) {
            return Err(ProjectAuthorityError::Invalid(
                "Specify transport requires HTTPS except explicit loopback development".to_string(),
            ));
        }
        if !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || authorization.trim().is_empty()
            || authorization
                .chars()
                .any(|character| matches!(character, '\r' | '\n'))
            || timeout.is_zero()
            || timeout > Duration::from_secs(30)
        {
            return Err(ProjectAuthorityError::Invalid(
                "invalid Specify endpoint, workload authorization, or timeout".to_string(),
            ));
        }
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        let client = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .https_only(!allow_loopback_http)
            .build()
            .map_err(|error| ProjectAuthorityError::Unavailable(error.to_string()))?;
        Ok(Self {
            client,
            base_url,
            authorization: Zeroizing::new(authorization),
        })
    }

    fn endpoint(&self, suffix: &str) -> Result<Url, ProjectAuthorityError> {
        self.base_url
            .join(suffix)
            .map_err(|error| ProjectAuthorityError::Invalid(error.to_string()))
    }

    fn refresh_url(
        &self,
        snapshot_ref: &ProjectExecutionSnapshotRef,
    ) -> Result<Url, ProjectAuthorityError> {
        let mut url = self.endpoint("v1/project-authority/snapshots/")?;
        let mut segments = url.path_segments_mut().map_err(|()| {
            ProjectAuthorityError::Invalid(
                "Specify base URL cannot contain hierarchical path segments".to_string(),
            )
        })?;
        segments.pop_if_empty().extend([
            snapshot_ref.authority_key().as_str(),
            snapshot_ref.resource_id(),
            snapshot_ref.resource_version(),
        ]);
        drop(segments);
        Ok(url)
    }

    fn authenticated(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header("Authorization", self.authorization.as_str())
            .header("Accept", "application/json")
            .header("User-Agent", concat!("agentd/", env!("CARGO_PKG_VERSION")))
    }
}

#[async_trait::async_trait]
impl SpecifyAuthorityTransport for HttpSpecifyAuthorityTransport {
    async fn resolve(
        &self,
        request: &ProjectSnapshotResolveRequest,
    ) -> Result<ProjectExecutionSnapshot, ProjectAuthorityError> {
        let response = self
            .authenticated(
                self.client
                    .post(self.endpoint("v1/project-authority/resolve")?),
            )
            .header(
                "Idempotency-Key",
                format!(
                    "resolve:{}:{}:{}",
                    request.expected_authority.as_str(),
                    request.project_ref.resource_id(),
                    request
                        .requested_snapshot_ref
                        .as_ref()
                        .map_or("current", ProjectExecutionSnapshotRef::resource_version)
                ),
            )
            .json(request)
            .send()
            .await
            .map_err(transport_error)?;
        decode_authority_response(response, "Specify resolve").await
    }

    async fn refresh(
        &self,
        snapshot_ref: &ProjectExecutionSnapshotRef,
    ) -> Result<ProjectExecutionSnapshot, ProjectAuthorityError> {
        let response = self
            .authenticated(self.client.get(self.refresh_url(snapshot_ref)?))
            .send()
            .await
            .map_err(transport_error)?;
        decode_authority_response(response, "Specify refresh").await
    }

    async fn health(&self) -> Result<ProjectAuthorityHealth, ProjectAuthorityError> {
        let response = self
            .authenticated(
                self.client
                    .get(self.endpoint("v1/project-authority/health")?),
            )
            .send()
            .await
            .map_err(transport_error)?;
        let health: ProjectAuthorityHealth =
            decode_authority_response(response, "Specify health").await?;
        if health.checked_at < 0 || health.availability == ProjectAuthorityAvailability::Unavailable
        {
            return Err(ProjectAuthorityError::Unavailable(
                "Specify health is unavailable".to_string(),
            ));
        }
        Ok(health)
    }
}

async fn decode_authority_response<T: DeserializeOwned>(
    mut response: Response,
    operation: &str,
) -> Result<T, ProjectAuthorityError> {
    match response.status() {
        StatusCode::OK => {}
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            return Err(ProjectAuthorityError::Unavailable(format!(
                "{operation} authentication was rejected"
            )));
        }
        StatusCode::NOT_FOUND => {
            return Err(ProjectAuthorityError::NotFound(operation.to_string()));
        }
        StatusCode::CONFLICT | StatusCode::PRECONDITION_FAILED => {
            return Err(ProjectAuthorityError::Conflict(format!(
                "{operation} rejected an immutable reference"
            )));
        }
        status if status.is_client_error() => {
            return Err(ProjectAuthorityError::Invalid(format!(
                "{operation} was rejected with {status}"
            )));
        }
        status => {
            return Err(ProjectAuthorityError::Unavailable(format!(
                "{operation} failed with {status}"
            )));
        }
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(ProjectAuthorityError::Unverifiable(format!(
            "{operation} response exceeds the response bound"
        )));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(ProjectAuthorityError::Unverifiable(format!(
                "{operation} response exceeds the response bound"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| ProjectAuthorityError::Unverifiable(error.to_string()))
}

#[allow(clippy::needless_pass_by_value)]
fn transport_error(error: reqwest::Error) -> ProjectAuthorityError {
    ProjectAuthorityError::Unavailable(error.to_string())
}
