//! Peer-certificate worker-fleet transport kept separate from bearer mode.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agentd_core::ports::{
    MtlsWorkloadVerifier, TaskLeaseCloseRequest, TaskLeaseRenewRequest, WorkerFleetDrainRequest,
    WorkerFleetHeartbeat, WorkerFleetPort, WorkerFleetPullRequest, WorkerFleetRegisterRequest,
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;

#[derive(Clone)]
pub struct WorkerFleetMtlsHttpState {
    pub fleet: Arc<dyn WorkerFleetPort>,
    pub verifier: Arc<dyn MtlsWorkloadVerifier>,
}

impl std::fmt::Debug for WorkerFleetMtlsHttpState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerFleetMtlsHttpState")
            .finish_non_exhaustive()
    }
}

pub fn worker_fleet_mtls_router(
    fleet: Arc<dyn WorkerFleetPort>,
    verifier: Arc<dyn MtlsWorkloadVerifier>,
) -> Router {
    let state = WorkerFleetMtlsHttpState { fleet, verifier };
    Router::new()
        .route("/api/worker-fleet/mtls/register", post(register))
        .route("/api/worker-fleet/mtls/heartbeat", post(heartbeat))
        .route("/api/worker-fleet/mtls/pull", post(pull))
        .route("/api/worker-fleet/mtls/drain", post(drain))
        .route("/api/worker-fleet/mtls/lease/renew", post(renew))
        .route("/api/worker-fleet/mtls/lease/release", post(release))
        .route("/api/worker-fleet/mtls/lease/cancel", post(cancel))
        .with_state(state)
}

async fn authenticate(
    state: &WorkerFleetMtlsHttpState,
    headers: &HeaderMap,
) -> Result<agentd_core::ports::AuthenticatedWorkload, Response> {
    let encoded = headers
        .get("x-client-certificate-der")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| unauthorized("client certificate required"))?;
    let der = STANDARD
        .decode(encoded)
        .map_err(|_| unauthorized("invalid client certificate encoding"))?;
    let observed_at = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| unauthorized("clock unavailable"))?
            .as_secs(),
    )
    .map_err(|_| unauthorized("clock out of range"))?;
    state
        .verifier
        .verify_peer(&der, observed_at)
        .await
        .map_err(|_| unauthorized("client certificate rejected"))
}

async fn register(
    State(state): State<WorkerFleetMtlsHttpState>,
    headers: HeaderMap,
    Json(request): Json<WorkerFleetRegisterRequest>,
) -> Response {
    let Ok(workload) = authenticate(&state, &headers).await else {
        return unauthorized("client certificate rejected");
    };
    if workload.worker_incarnation_id.as_ref() != Some(&request.incarnation_id) {
        return unauthorized("worker identity mismatch");
    }
    respond(state.fleet.register(&request).await)
}

async fn heartbeat(
    State(state): State<WorkerFleetMtlsHttpState>,
    headers: HeaderMap,
    Json(request): Json<WorkerFleetHeartbeat>,
) -> Response {
    let Ok(workload) = authenticate(&state, &headers).await else {
        return unauthorized("client certificate rejected");
    };
    if workload.worker_incarnation_id.as_ref() != Some(&request.incarnation_id) {
        return unauthorized("worker identity mismatch");
    }
    respond(state.fleet.heartbeat(&request).await)
}

async fn pull(
    State(state): State<WorkerFleetMtlsHttpState>,
    headers: HeaderMap,
    Json(request): Json<WorkerFleetPullRequest>,
) -> Response {
    let Ok(workload) = authenticate(&state, &headers).await else {
        return unauthorized("client certificate rejected");
    };
    if workload.worker_incarnation_id.as_ref() != Some(&request.worker_incarnation_id) {
        return unauthorized("worker identity mismatch");
    }
    respond(state.fleet.pull(&request).await)
}

async fn drain(
    State(state): State<WorkerFleetMtlsHttpState>,
    headers: HeaderMap,
    Json(request): Json<WorkerFleetDrainRequest>,
) -> Response {
    let Ok(workload) = authenticate(&state, &headers).await else {
        return unauthorized("client certificate rejected");
    };
    if workload.worker_incarnation_id.as_ref() != Some(&request.incarnation_id) {
        return unauthorized("worker identity mismatch");
    }
    respond(
        state
            .fleet
            .set_drain(&request)
            .await
            .map(|()| json!({"ok": true})),
    )
}

async fn renew(
    State(state): State<WorkerFleetMtlsHttpState>,
    headers: HeaderMap,
    Json(request): Json<TaskLeaseRenewRequest>,
) -> Response {
    let Ok(workload) = authenticate(&state, &headers).await else {
        return unauthorized("client certificate rejected");
    };
    if workload.worker_incarnation_id.as_ref() != Some(&request.claim.worker_incarnation_id) {
        return unauthorized("worker identity mismatch");
    }
    respond(state.fleet.renew(&request).await)
}

async fn release(
    State(state): State<WorkerFleetMtlsHttpState>,
    headers: HeaderMap,
    Json(request): Json<TaskLeaseCloseRequest>,
) -> Response {
    let Ok(workload) = authenticate(&state, &headers).await else {
        return unauthorized("client certificate rejected");
    };
    if workload.worker_incarnation_id.as_ref() != Some(&request.claim.worker_incarnation_id) {
        return unauthorized("worker identity mismatch");
    }
    respond(state.fleet.release(&request).await)
}

async fn cancel(
    State(state): State<WorkerFleetMtlsHttpState>,
    headers: HeaderMap,
    Json(request): Json<TaskLeaseCloseRequest>,
) -> Response {
    let Ok(workload) = authenticate(&state, &headers).await else {
        return unauthorized("client certificate rejected");
    };
    if workload.worker_incarnation_id.as_ref() != Some(&request.claim.worker_incarnation_id) {
        return unauthorized("worker identity mismatch");
    }
    respond(state.fleet.cancel(&request).await)
}

fn unauthorized(message: &str) -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": message }))).into_response()
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
