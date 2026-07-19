//! HTTP transport for the authenticated worker-fleet control protocol.

use std::sync::Arc;

use crate::http::AuthConfig;
use agentd_core::ports::{
    TaskLeaseCloseRequest, TaskLeaseRenewRequest, WorkerFleetDrainRequest, WorkerFleetHeartbeat,
    WorkerFleetPort, WorkerFleetPullRequest, WorkerFleetRegisterRequest,
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
pub struct WorkerFleetHttpState {
    pub fleet: Arc<dyn WorkerFleetPort>,
    pub auth: AuthConfig,
}

impl std::fmt::Debug for WorkerFleetHttpState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerFleetHttpState")
            .finish_non_exhaustive()
    }
}

/// Build the independently mountable worker-fleet HTTP transport.
pub fn worker_fleet_router(fleet: Arc<dyn WorkerFleetPort>, auth: AuthConfig) -> Router {
    let state = WorkerFleetHttpState { fleet, auth };
    Router::new()
        .route("/api/worker-fleet/register", post(register))
        .route("/api/worker-fleet/heartbeat", post(heartbeat))
        .route("/api/worker-fleet/pull", post(pull))
        .route("/api/worker-fleet/drain", post(drain))
        .route("/api/worker-fleet/lease/renew", post(renew))
        .route("/api/worker-fleet/lease/release", post(release))
        .route("/api/worker-fleet/lease/cancel", post(cancel))
        .with_state(state)
}

async fn register(
    State(state): State<WorkerFleetHttpState>,
    headers: HeaderMap,
    Json(request): Json<WorkerFleetRegisterRequest>,
) -> Response {
    if let Err(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(state.fleet.register(&request).await)
}

async fn heartbeat(
    State(state): State<WorkerFleetHttpState>,
    headers: HeaderMap,
    Json(request): Json<WorkerFleetHeartbeat>,
) -> Response {
    if let Err(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(state.fleet.heartbeat(&request).await)
}

async fn pull(
    State(state): State<WorkerFleetHttpState>,
    headers: HeaderMap,
    Json(request): Json<WorkerFleetPullRequest>,
) -> Response {
    if let Err(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(state.fleet.pull(&request).await)
}

async fn drain(
    State(state): State<WorkerFleetHttpState>,
    headers: HeaderMap,
    Json(request): Json<WorkerFleetDrainRequest>,
) -> Response {
    if let Err(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(
        state
            .fleet
            .set_drain(&request)
            .await
            .map(|()| json!({ "ok": true })),
    )
}

async fn renew(
    State(state): State<WorkerFleetHttpState>,
    headers: HeaderMap,
    Json(request): Json<TaskLeaseRenewRequest>,
) -> Response {
    if let Err(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(state.fleet.renew(&request).await)
}

async fn release(
    State(state): State<WorkerFleetHttpState>,
    headers: HeaderMap,
    Json(request): Json<TaskLeaseCloseRequest>,
) -> Response {
    if let Err(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(state.fleet.release(&request).await)
}

async fn cancel(
    State(state): State<WorkerFleetHttpState>,
    headers: HeaderMap,
    Json(request): Json<TaskLeaseCloseRequest>,
) -> Response {
    if let Err(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(state.fleet.cancel(&request).await)
}

fn respond<T: serde::Serialize, E: std::fmt::Display>(result: Result<T, E>) -> Response {
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

fn authenticate(auth: &AuthConfig, headers: &HeaderMap) -> Result<(), Response> {
    let Some(expected) = auth
        .api_token
        .as_deref()
        .filter(|token| !token.trim().is_empty())
    else {
        return Ok(());
    };
    let valid = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token == expected);
    if valid {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "operator bearer token required"})),
        )
            .into_response())
    }
}
