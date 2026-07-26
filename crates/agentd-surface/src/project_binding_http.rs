//! Operator HTTP transport for the durable project ↔ room ↔ repository
//! binding. Mounted independently, like the worker-fleet transport.

use std::sync::Arc;

use agentd_core::ports::{ProjectBindingPort, ProjectRoomRepoBindingRequest};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use serde::Deserialize;
use serde_json::json;

use crate::control_plane_status::ControlPlaneErrorStatus;
use crate::http::AuthConfig;

#[derive(Clone)]
pub struct ProjectBindingHttpState {
    pub bindings: Arc<dyn ProjectBindingPort>,
    pub auth: AuthConfig,
}

impl std::fmt::Debug for ProjectBindingHttpState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectBindingHttpState")
            .finish_non_exhaustive()
    }
}

/// Body of `PUT /api/projects/:project_id/binding`. The project id comes from
/// the path, so the body never contradicts the URL.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectBindingBody {
    pub room_id: String,
    pub repository_id: String,
    pub repository_url: String,
    pub default_branch: String,
}

/// Build the independently mountable project-binding transport.
pub fn project_binding_router(bindings: Arc<dyn ProjectBindingPort>, auth: AuthConfig) -> Router {
    let state = ProjectBindingHttpState { bindings, auth };
    Router::new()
        .route(
            "/api/projects/:project_id/binding",
            put(put_binding).get(get_project_binding),
        )
        .route("/api/rooms/:room_id/binding", get(get_room_binding))
        .with_state(state)
}

async fn put_binding(
    State(state): State<ProjectBindingHttpState>,
    AxumPath(project_id): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    let body: ProjectBindingBody = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid binding body: {error}") })),
            )
                .into_response();
        }
    };
    let request = ProjectRoomRepoBindingRequest {
        project_id,
        room_id: body.room_id,
        repository_id: body.repository_id,
        repository_url: body.repository_url,
        default_branch: body.default_branch,
    };
    respond(state.bindings.put_binding(&request).await)
}

async fn get_project_binding(
    State(state): State<ProjectBindingHttpState>,
    AxumPath(project_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(state.bindings.get_binding_for_project(&project_id).await)
}

async fn get_room_binding(
    State(state): State<ProjectBindingHttpState>,
    AxumPath(room_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(state.bindings.get_binding_for_room(&room_id).await)
}

fn respond<T: serde::Serialize, E: std::fmt::Display + ControlPlaneErrorStatus>(
    result: Result<T, E>,
) -> Response {
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => (
            error.http_status(),
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

/// Returns the rejection response when the bearer token is missing or wrong.
/// Delegates to the shared operator check so this transport cannot drift from
/// `/api/*` on scheme casing or token trimming.
fn authenticate(auth: &AuthConfig, headers: &HeaderMap) -> Option<Response> {
    crate::http::require_operator_bearer(auth, headers)
        .err()
        .map(crate::http::AuthRejection::into_response)
}
