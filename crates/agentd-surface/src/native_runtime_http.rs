//! HTTP transport for the native runtime control-plane port (AD-E5).
//!
//! Remote workers must not open the daemon `SQLite` database; every runtime
//! session/attempt mutation crosses this authenticated boundary instead.

use std::sync::Arc;

use crate::http::AuthConfig;
use agentd_core::ports::{
    NativeRuntimeAttemptStart, NativeRuntimeAttemptState, NativeRuntimeControlError,
    NativeRuntimeControlPort, NativeRuntimeSessionValidate,
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::json;

#[derive(Clone)]
pub struct NativeRuntimeHttpState {
    pub control: Arc<dyn NativeRuntimeControlPort>,
    pub auth: AuthConfig,
}

impl std::fmt::Debug for NativeRuntimeHttpState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeRuntimeHttpState")
            .finish_non_exhaustive()
    }
}

/// Build the independently mountable native runtime control-plane transport.
pub fn native_runtime_router(
    control: Arc<dyn NativeRuntimeControlPort>,
    auth: AuthConfig,
) -> Router {
    let state = NativeRuntimeHttpState { control, auth };
    Router::new()
        .route("/api/runtime/native/session/validate", post(validate))
        .route("/api/runtime/native/session/view", post(session_view))
        .route("/api/runtime/native/attempt/start", post(start_attempt))
        .route("/api/runtime/native/attempt/update", post(update_attempt))
        .route("/api/runtime/native/attempt/terminal", post(mark_terminal))
        .with_state(state)
}

async fn validate(
    State(state): State<NativeRuntimeHttpState>,
    headers: HeaderMap,
    Json(request): Json<NativeRuntimeSessionValidate>,
) -> Response {
    if let Some(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(
        state
            .control
            .validate_session_task(&request.session_id, &request.task_id)
            .await
            .map(|()| json!({ "ok": true })),
    )
}

/// Session lookup request body: only the session id is required.
#[derive(Debug, serde::Deserialize)]
struct SessionViewRequest {
    session_id: agentd_core::types::RuntimeSessionId,
}

async fn session_view(
    State(state): State<NativeRuntimeHttpState>,
    headers: HeaderMap,
    Json(request): Json<SessionViewRequest>,
) -> Response {
    if let Some(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(state.control.session_view(&request.session_id).await)
}

async fn start_attempt(
    State(state): State<NativeRuntimeHttpState>,
    headers: HeaderMap,
    Json(request): Json<NativeRuntimeAttemptStart>,
) -> Response {
    if let Some(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(state.control.start_attempt(&request).await)
}

async fn update_attempt(
    State(state): State<NativeRuntimeHttpState>,
    headers: HeaderMap,
    Json(request): Json<NativeRuntimeAttemptState>,
) -> Response {
    if let Some(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(
        state
            .control
            .update_attempt(&request)
            .await
            .map(|()| json!({ "ok": true })),
    )
}

async fn mark_terminal(
    State(state): State<NativeRuntimeHttpState>,
    headers: HeaderMap,
    Json(request): Json<NativeRuntimeAttemptState>,
) -> Response {
    if let Some(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(
        state
            .control
            .mark_attempt_terminal(&request)
            .await
            .map(|()| json!({ "ok": true })),
    )
}

/// Map control-plane errors onto statuses the worker client classifies for
/// retry: only `Unavailable` becomes a retryable 5xx.
fn respond<T: serde::Serialize>(result: Result<T, NativeRuntimeControlError>) -> Response {
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => {
            let status = match &error {
                NativeRuntimeControlError::Invalid(_) => StatusCode::BAD_REQUEST,
                NativeRuntimeControlError::NotFound(_) => StatusCode::NOT_FOUND,
                NativeRuntimeControlError::Conflict(_) => StatusCode::CONFLICT,
                NativeRuntimeControlError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            };
            (status, Json(json!({ "error": error.to_string() }))).into_response()
        }
    }
}

/// When authentication is configured, reject a missing or wrong bearer token.
/// An empty token preserves `AuthConfig`'s local-development open mode.
fn authenticate(auth: &AuthConfig, headers: &HeaderMap) -> Option<Response> {
    let expected = auth
        .api_token
        .as_deref()
        .filter(|token| !token.trim().is_empty())?;
    let valid = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token == expected);
    if valid {
        None
    } else {
        Some(
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "worker bearer token required"})),
            )
                .into_response(),
        )
    }
}
