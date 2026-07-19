//! Worker-side HTTP adapter for the native runtime control-plane port (AD-E5).
//!
//! Remote workers resolve session/attempt state through this client instead of
//! opening the daemon `SQLite` database. Status classification mirrors
//! [`crate::worker_fleet_client`]: only transport unavailability is retryable;
//! authentication and fencing conflicts are terminal.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use agentd_core::ports::{
    NativeRuntimeAttemptStart, NativeRuntimeAttemptState, NativeRuntimeControlError,
    NativeRuntimeControlPort, NativeRuntimeSessionValidate,
};
use agentd_core::types::{RuntimeSessionId, TaskRunId};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, Clone)]
pub struct NativeRuntimeHttpClient {
    authority: String,
    auth_proof: String,
    timeout: Duration,
}

impl NativeRuntimeHttpClient {
    pub fn new(
        base_url: impl Into<String>,
        auth_proof: impl Into<String>,
    ) -> Result<Self, NativeRuntimeControlError> {
        let authority = base_url
            .into()
            .strip_prefix("http://")
            .map(str::to_owned)
            .ok_or_else(|| {
                NativeRuntimeControlError::Invalid(
                    "native runtime client requires http:// URL".into(),
                )
            })?;
        if authority.is_empty() || authority.contains('/') {
            return Err(NativeRuntimeControlError::Invalid(
                "invalid native runtime control-plane URL".into(),
            ));
        }
        Ok(Self {
            authority,
            auth_proof: auth_proof.into(),
            timeout: Duration::from_secs(10),
        })
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Run one request without blocking the async runtime; the raw socket IO
    /// stays on the blocking pool so a slow daemon cannot stall worker tasks.
    async fn post<Request: Serialize, Response: DeserializeOwned + Send + 'static>(
        &self,
        path: &'static str,
        request: &Request,
    ) -> Result<Response, NativeRuntimeControlError> {
        let body = serde_json::to_vec(request)
            .map_err(|error| NativeRuntimeControlError::Invalid(error.to_string()))?;
        let client = self.clone();
        tokio::task::spawn_blocking(move || client.post_blocking(path, &body))
            .await
            .map_err(|error| NativeRuntimeControlError::Unavailable(error.to_string()))?
    }

    fn post_blocking<Response: DeserializeOwned>(
        &self,
        path: &str,
        body: &[u8],
    ) -> Result<Response, NativeRuntimeControlError> {
        let mut stream = TcpStream::connect(&self.authority)
            .map_err(|error| NativeRuntimeControlError::Unavailable(error.to_string()))?;
        stream.set_read_timeout(Some(self.timeout)).ok();
        stream.set_write_timeout(Some(self.timeout)).ok();
        write!(
            stream,
            "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
            self.authority,
            body.len(),
            self.auth_proof
        )
        .and_then(|()| stream.write_all(body))
        .map_err(|error| NativeRuntimeControlError::Unavailable(error.to_string()))?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|error| NativeRuntimeControlError::Unavailable(error.to_string()))?;
        let response = String::from_utf8(response)
            .map_err(|_| NativeRuntimeControlError::Unavailable("non-UTF8 response".into()))?;
        let (head, body) = response.split_once("\r\n\r\n").ok_or_else(|| {
            NativeRuntimeControlError::Unavailable("malformed HTTP response".into())
        })?;
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(0);
        if !(200..300).contains(&status) {
            return Err(classify_http_error(status, body));
        }
        serde_json::from_str(body)
            .map_err(|error| NativeRuntimeControlError::Unavailable(error.to_string()))
    }
}

fn classify_http_error(status: u16, body: &str) -> NativeRuntimeControlError {
    let message = body.to_string();
    match status {
        400 => NativeRuntimeControlError::Invalid(message),
        404 => NativeRuntimeControlError::NotFound(message),
        408 | 425 | 429 | 500..=599 => NativeRuntimeControlError::Unavailable(message),
        401..=499 => NativeRuntimeControlError::Conflict(message),
        _ => NativeRuntimeControlError::Unavailable(message),
    }
}

#[async_trait::async_trait]
impl NativeRuntimeControlPort for NativeRuntimeHttpClient {
    async fn validate_session_task(
        &self,
        session_id: &RuntimeSessionId,
        task_id: &TaskRunId,
    ) -> Result<(), NativeRuntimeControlError> {
        let request = NativeRuntimeSessionValidate {
            session_id: session_id.clone(),
            task_id: task_id.clone(),
        };
        let _: serde_json::Value = self
            .post("/api/runtime/native/session/validate", &request)
            .await?;
        Ok(())
    }

    async fn session_view(
        &self,
        session_id: &RuntimeSessionId,
    ) -> Result<Option<agentd_core::ports::NativeRuntimeSessionView>, NativeRuntimeControlError>
    {
        self.post(
            "/api/runtime/native/session/view",
            &serde_json::json!({ "session_id": session_id }),
        )
        .await
    }

    async fn start_attempt(
        &self,
        request: &NativeRuntimeAttemptStart,
    ) -> Result<NativeRuntimeAttemptState, NativeRuntimeControlError> {
        self.post("/api/runtime/native/attempt/start", request)
            .await
    }

    async fn update_attempt(
        &self,
        state: &NativeRuntimeAttemptState,
    ) -> Result<(), NativeRuntimeControlError> {
        let _: serde_json::Value = self
            .post("/api/runtime/native/attempt/update", state)
            .await?;
        Ok(())
    }

    async fn mark_attempt_terminal(
        &self,
        state: &NativeRuntimeAttemptState,
    ) -> Result<(), NativeRuntimeControlError> {
        let _: serde_json::Value = self
            .post("/api/runtime/native/attempt/terminal", state)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::classify_http_error;
    use agentd_core::ports::NativeRuntimeControlError;

    #[test]
    fn transient_statuses_map_to_unavailable() {
        assert!(matches!(
            classify_http_error(503, "down"),
            NativeRuntimeControlError::Unavailable(_)
        ));
        assert!(matches!(
            classify_http_error(429, "busy"),
            NativeRuntimeControlError::Unavailable(_)
        ));
    }

    #[test]
    fn terminal_statuses_map_to_typed_errors() {
        assert!(matches!(
            classify_http_error(400, "bad"),
            NativeRuntimeControlError::Invalid(_)
        ));
        assert!(matches!(
            classify_http_error(401, "auth"),
            NativeRuntimeControlError::Conflict(_)
        ));
        assert!(matches!(
            classify_http_error(404, "missing"),
            NativeRuntimeControlError::NotFound(_)
        ));
        assert!(matches!(
            classify_http_error(409, "fenced"),
            NativeRuntimeControlError::Conflict(_)
        ));
    }
}
