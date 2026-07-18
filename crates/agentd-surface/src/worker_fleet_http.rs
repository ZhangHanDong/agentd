//! HTTP transport for the authenticated worker-fleet control protocol.

use std::sync::Arc;

use agentd_core::ports::{
    WorkerFleetDrainRequest, WorkerFleetHeartbeat, WorkerFleetPort, WorkerFleetPullRequest,
    WorkerFleetRegisterRequest,
};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::json;

#[derive(Clone)]
pub struct WorkerFleetHttpState {
    pub fleet: Arc<dyn WorkerFleetPort>,
}

impl std::fmt::Debug for WorkerFleetHttpState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerFleetHttpState")
            .finish_non_exhaustive()
    }
}

/// Build the independently mountable worker-fleet HTTP transport.
pub fn worker_fleet_router(fleet: Arc<dyn WorkerFleetPort>) -> Router {
    let state = WorkerFleetHttpState { fleet };
    Router::new()
        .route("/api/worker-fleet/register", post(register))
        .route("/api/worker-fleet/heartbeat", post(heartbeat))
        .route("/api/worker-fleet/pull", post(pull))
        .route("/api/worker-fleet/drain", post(drain))
        .with_state(state)
}

async fn register(
    State(state): State<WorkerFleetHttpState>,
    Json(request): Json<WorkerFleetRegisterRequest>,
) -> Response {
    respond(state.fleet.register(&request).await)
}

async fn heartbeat(
    State(state): State<WorkerFleetHttpState>,
    Json(request): Json<WorkerFleetHeartbeat>,
) -> Response {
    respond(state.fleet.heartbeat(&request).await)
}

async fn pull(
    State(state): State<WorkerFleetHttpState>,
    Json(request): Json<WorkerFleetPullRequest>,
) -> Response {
    respond(state.fleet.pull(&request).await)
}

async fn drain(
    State(state): State<WorkerFleetHttpState>,
    Json(request): Json<WorkerFleetDrainRequest>,
) -> Response {
    respond(
        state
            .fleet
            .set_drain(&request)
            .await
            .map(|()| json!({ "ok": true })),
    )
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
