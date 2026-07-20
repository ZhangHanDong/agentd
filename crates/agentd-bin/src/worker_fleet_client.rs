//! Worker-side client for the authenticated daemon worker-fleet protocol.

use std::future::Future;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use agentd_core::ports::{
    TaskLeaseCloseRequest, TaskLeaseDispatchRequest, TaskLeaseError, TaskLeasePort,
    TaskLeaseRenewRequest, WorkerFleetDrainRequest, WorkerFleetError, WorkerFleetHeartbeat,
    WorkerFleetHeartbeatResult, WorkerFleetPort, WorkerFleetPullRequest,
    WorkerFleetRegisterRequest, WorkerFleetRegistration,
};
use agentd_core::types::{TaskLeaseClaim, TaskLeaseGrant, WorkerIncarnationId};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, Clone)]
pub struct WorkerFleetHttpClient {
    authority: String,
    auth_proof: String,
    client_certificate_der: Option<Vec<u8>>,
    timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerFleetRetryPolicy {
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for WorkerFleetRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_secs(2),
        }
    }
}

impl WorkerFleetHttpClient {
    pub fn new(
        base_url: impl Into<String>,
        auth_proof: impl Into<String>,
    ) -> Result<Self, WorkerFleetError> {
        let authority = base_url
            .into()
            .strip_prefix("http://")
            .map(str::to_owned)
            .ok_or_else(|| {
                WorkerFleetError::Invalid("worker fleet client requires http:// URL".into())
            })?;
        if authority.is_empty() || authority.contains('/') {
            return Err(WorkerFleetError::Invalid("invalid worker fleet URL".into()));
        }
        Ok(Self {
            authority,
            auth_proof: auth_proof.into(),
            client_certificate_der: None,
            timeout: Duration::from_secs(10),
        })
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_client_certificate_der(mut self, der: Vec<u8>) -> Self {
        self.client_certificate_der = Some(der);
        self
    }

    fn endpoint(&self, bearer: &'static str, mtls: &'static str) -> &'static str {
        if self.client_certificate_der.is_some() {
            mtls
        } else {
            bearer
        }
    }

    /// Pull with bounded reconnect backoff. Only transport unavailability is
    /// retried; authentication, stale-worker, and lease conflicts are final.
    pub async fn pull_with_retry(
        &self,
        request: &WorkerFleetPullRequest,
        policy: WorkerFleetRetryPolicy,
    ) -> Result<Option<TaskLeaseGrant>, WorkerFleetError> {
        let attempts = policy.max_attempts.max(1);
        let mut backoff = policy.initial_backoff.min(policy.max_backoff);
        for attempt in 0..attempts {
            match self.pull(request).await {
                Ok(grant) => return Ok(grant),
                Err(error)
                    if matches!(error, WorkerFleetError::Unavailable(_))
                        && attempt + 1 < attempts =>
                {
                    tokio::time::sleep(backoff).await;
                    backoff = backoff.saturating_mul(2).min(policy.max_backoff);
                }
                Err(error) => return Err(error),
            }
        }
        Err(WorkerFleetError::Unavailable(
            "worker pull retry exhausted".into(),
        ))
    }

    /// Pull a native grant only when the control plane supplied the security
    /// scope required to construct a secured runtime binding.
    pub async fn pull_native_with_scope(
        &self,
        request: &WorkerFleetPullRequest,
        policy: WorkerFleetRetryPolicy,
    ) -> Result<Option<TaskLeaseGrant>, WorkerFleetError> {
        let grant = self.pull_with_retry(request, policy).await?;
        if let Some(grant) = &grant {
            if grant.execution_spec.is_some() && grant.security_scope.is_none() {
                return Err(WorkerFleetError::Invalid(
                    "native lease grant is missing execution security scope".into(),
                ));
            }
        }
        Ok(grant)
    }

    /// Heartbeat with bounded reconnect backoff. Authentication and stale
    /// incarnation responses remain terminal; only transport availability is
    /// retried so a disconnected worker can rejoin without a restart.
    pub async fn heartbeat_with_retry(
        &self,
        request: &WorkerFleetHeartbeat,
        policy: WorkerFleetRetryPolicy,
    ) -> Result<WorkerFleetHeartbeatResult, WorkerFleetError> {
        let attempts = policy.max_attempts.max(1);
        let mut backoff = policy.initial_backoff.min(policy.max_backoff);
        for attempt in 0..attempts {
            match self.heartbeat(request).await {
                Ok(result) => return Ok(result),
                Err(error)
                    if matches!(error, WorkerFleetError::Unavailable(_))
                        && attempt + 1 < attempts =>
                {
                    tokio::time::sleep(backoff).await;
                    backoff = backoff.saturating_mul(2).min(policy.max_backoff);
                }
                Err(error) => return Err(error),
            }
        }
        Err(WorkerFleetError::Unavailable(
            "worker heartbeat retry exhausted".into(),
        ))
    }

    /// Register with bounded reconnect backoff so a worker can start while
    /// the daemon is still binding or recovering its database.
    pub async fn register_with_retry(
        &self,
        request: &WorkerFleetRegisterRequest,
        policy: WorkerFleetRetryPolicy,
    ) -> Result<WorkerFleetRegistration, WorkerFleetError> {
        let attempts = policy.max_attempts.max(1);
        let mut backoff = policy.initial_backoff.min(policy.max_backoff);
        for attempt in 0..attempts {
            match self.register(request).await {
                Ok(result) => return Ok(result),
                Err(error)
                    if matches!(error, WorkerFleetError::Unavailable(_))
                        && attempt + 1 < attempts =>
                {
                    tokio::time::sleep(backoff).await;
                    backoff = backoff.saturating_mul(2).min(policy.max_backoff);
                }
                Err(error) => return Err(error),
            }
        }
        Err(WorkerFleetError::Unavailable(
            "worker registration retry exhausted".into(),
        ))
    }

    /// Register once and maintain liveness until the caller signals shutdown.
    /// The loop is deliberately worker-owned; daemon startup never implicitly
    /// creates a worker identity or execution scope.
    pub async fn run_heartbeat_loop(
        &self,
        registration: &WorkerFleetRegisterRequest,
        heartbeat: &WorkerFleetHeartbeat,
        interval: Duration,
        policy: WorkerFleetRetryPolicy,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), WorkerFleetError> {
        self.register_with_retry(registration, policy).await?;
        let mut ticker = tokio::time::interval(interval.max(Duration::from_millis(10)));
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    match self.heartbeat_with_retry(heartbeat, policy).await {
                        Ok(_) | Err(WorkerFleetError::Unavailable(_)) => {
                            // Success or a transient outage: keep the worker
                            // alive; the next tick retries.
                        }
                        Err(error) => return Err(error),
                    }
                }
                changed = shutdown.changed() => {
                    match changed {
                        Ok(()) if *shutdown.borrow() => return Ok(()),
                        Ok(()) => {}
                        Err(_) => return Ok(()),
                    }
                }
            }
        }
    }

    /// Run the worker-owned pull lifecycle. A transient daemon outage causes
    /// registration to be replayed before pulling resumes; terminal identity
    /// or lease errors are returned to the process supervisor.
    pub async fn run_pull_loop<F, Fut>(
        &self,
        registration: &WorkerFleetRegisterRequest,
        incarnation_id: WorkerIncarnationId,
        poll_interval: Duration,
        lease_ttl: Duration,
        policy: WorkerFleetRetryPolicy,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
        mut execute: F,
    ) -> Result<(), WorkerFleetError>
    where
        F: FnMut(TaskLeaseGrant) -> Fut,
        Fut: Future<Output = Result<(), WorkerFleetError>>,
    {
        self.register_with_retry(registration, policy).await?;
        let mut ticker = tokio::time::interval(poll_interval.max(Duration::from_millis(10)));
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let observed_at = unix_now();
                    let request = self.incarnation_request(
                        incarnation_id.clone(),
                        observed_at,
                        observed_at.saturating_add(i64::try_from(lease_ttl.as_secs().max(1)).unwrap_or(i64::MAX)),
                    );
                    match self.pull_native_with_scope(&request, policy).await {
                        Ok(Some(grant)) => execute(grant).await?,
                        Ok(None) => {}
                        Err(WorkerFleetError::Unavailable(_)) => {
                            // Registration is replayed on the next poll tick;
                            // an outage must not terminate the worker process.
                            match self.register_with_retry(registration, policy).await {
                                Ok(_) | Err(WorkerFleetError::Unavailable(_)) => {}
                                Err(error) => return Err(error),
                            }
                        }
                        Err(error) => return Err(error),
                    }
                }
                changed = shutdown.changed() => {
                    match changed {
                        Ok(()) if *shutdown.borrow() => return Ok(()),
                        Ok(()) => {}
                        Err(_) => return Ok(()),
                    }
                }
            }
        }
    }

    fn post<Request: Serialize, Response: DeserializeOwned>(
        &self,
        path: &str,
        request: &Request,
    ) -> Result<Response, WorkerFleetError> {
        let body = serde_json::to_vec(request)
            .map_err(|error| WorkerFleetError::Invalid(error.to_string()))?;
        let mut stream = TcpStream::connect(&self.authority)
            .map_err(|error| WorkerFleetError::Unavailable(error.to_string()))?;
        stream.set_read_timeout(Some(self.timeout)).ok();
        stream.set_write_timeout(Some(self.timeout)).ok();
        let certificate_header = self
            .client_certificate_der
            .as_ref()
            .map(|der| format!("X-Client-Certificate-DER: {}\r\n", STANDARD.encode(der)))
            .unwrap_or_default();
        let authorization = if self.client_certificate_der.is_none() {
            format!("Authorization: Bearer {}\r\n", self.auth_proof)
        } else {
            String::new()
        };
        write!(stream, "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{authorization}{certificate_header}Connection: close\r\n\r\n", self.authority, body.len())
            .and_then(|()| stream.write_all(&body))
            .map_err(|error| WorkerFleetError::Unavailable(error.to_string()))?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|error| WorkerFleetError::Unavailable(error.to_string()))?;
        let response = String::from_utf8(response)
            .map_err(|_| WorkerFleetError::Unavailable("non-UTF8 response".into()))?;
        let (head, body) = response
            .split_once("\r\n\r\n")
            .ok_or_else(|| WorkerFleetError::Unavailable("malformed HTTP response".into()))?;
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(0);
        if !(200..300).contains(&status) {
            return Err(classify_http_error(status, body));
        }
        serde_json::from_str(body).map_err(|error| WorkerFleetError::Unavailable(error.to_string()))
    }

    fn close(
        &self,
        path: &str,
        request: &TaskLeaseCloseRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        self.post(path, request)
            .map_err(|error| TaskLeaseError::Unavailable(error.to_string()))
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn classify_http_error(status: u16, body: &str) -> WorkerFleetError {
    let message = body.to_string();
    match status {
        404 => WorkerFleetError::NotFound(message),
        408 | 425 | 429 | 500..=599 => WorkerFleetError::Unavailable(message),
        400..=499 => WorkerFleetError::Conflict(message),
        _ => WorkerFleetError::Unavailable(message),
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::classify_http_error;
    use agentd_core::ports::WorkerFleetError;

    #[test]
    fn transient_http_statuses_trigger_reconnect() {
        assert!(matches!(
            classify_http_error(503, "down"),
            WorkerFleetError::Unavailable(_)
        ));
        assert!(matches!(
            classify_http_error(429, "busy"),
            WorkerFleetError::Unavailable(_)
        ));
        assert!(matches!(
            classify_http_error(409, "stale"),
            WorkerFleetError::Conflict(_)
        ));
        assert!(matches!(
            classify_http_error(404, "missing"),
            WorkerFleetError::NotFound(_)
        ));
    }
}

#[async_trait::async_trait]
impl WorkerFleetPort for WorkerFleetHttpClient {
    async fn register(
        &self,
        request: &WorkerFleetRegisterRequest,
    ) -> Result<WorkerFleetRegistration, WorkerFleetError> {
        self.post(
            self.endpoint(
                "/api/worker-fleet/register",
                "/api/worker-fleet/mtls/register",
            ),
            request,
        )
    }

    async fn heartbeat(
        &self,
        request: &WorkerFleetHeartbeat,
    ) -> Result<WorkerFleetHeartbeatResult, WorkerFleetError> {
        self.post(
            self.endpoint(
                "/api/worker-fleet/heartbeat",
                "/api/worker-fleet/mtls/heartbeat",
            ),
            request,
        )
    }

    async fn set_drain(&self, request: &WorkerFleetDrainRequest) -> Result<(), WorkerFleetError> {
        let _: serde_json::Value = self.post(
            self.endpoint("/api/worker-fleet/drain", "/api/worker-fleet/mtls/drain"),
            request,
        )?;
        Ok(())
    }

    async fn recover_offline(&self, _heartbeat_cutoff: i64) -> Result<u64, WorkerFleetError> {
        Err(WorkerFleetError::Unavailable(
            "offline recovery is daemon-owned".into(),
        ))
    }

    async fn pull(
        &self,
        request: &WorkerFleetPullRequest,
    ) -> Result<Option<TaskLeaseGrant>, WorkerFleetError> {
        self.post(
            self.endpoint("/api/worker-fleet/pull", "/api/worker-fleet/mtls/pull"),
            request,
        )
    }
}

#[async_trait::async_trait]
impl TaskLeasePort for WorkerFleetHttpClient {
    async fn dispatch(
        &self,
        _request: &TaskLeaseDispatchRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        Err(TaskLeaseError::Unavailable(
            "dispatch is daemon-owned; use pull".into(),
        ))
    }

    async fn renew(
        &self,
        request: &TaskLeaseRenewRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        self.post(
            self.endpoint(
                "/api/worker-fleet/lease/renew",
                "/api/worker-fleet/mtls/lease/renew",
            ),
            request,
        )
        .map_err(|error| TaskLeaseError::Unavailable(error.to_string()))
    }

    async fn release(
        &self,
        request: &TaskLeaseCloseRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        self.close(
            self.endpoint(
                "/api/worker-fleet/lease/release",
                "/api/worker-fleet/mtls/lease/release",
            ),
            request,
        )
    }

    async fn cancel(
        &self,
        request: &TaskLeaseCloseRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        self.close(
            self.endpoint(
                "/api/worker-fleet/lease/cancel",
                "/api/worker-fleet/mtls/lease/cancel",
            ),
            request,
        )
    }

    async fn validate_claim(
        &self,
        _claim: &TaskLeaseClaim,
        _observed_at: i64,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        Err(TaskLeaseError::Unavailable(
            "claim validation is daemon-owned".into(),
        ))
    }

    async fn expire_due(&self, _observed_at: i64) -> Result<u64, TaskLeaseError> {
        Err(TaskLeaseError::Unavailable(
            "lease expiry is daemon-owned".into(),
        ))
    }
}

impl WorkerFleetHttpClient {
    #[must_use]
    pub fn auth_proof(&self) -> &str {
        &self.auth_proof
    }
    #[must_use]
    pub fn incarnation_request(
        &self,
        incarnation_id: WorkerIncarnationId,
        observed_at: i64,
        expires_at: i64,
    ) -> WorkerFleetPullRequest {
        WorkerFleetPullRequest {
            auth_proof: self.auth_proof.clone(),
            worker_incarnation_id: incarnation_id,
            observed_at,
            expires_at,
        }
    }
}
