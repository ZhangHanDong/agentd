//! The daemon's HTTP+SSE observability surface (design §7.2). An axum `Router`
//! over the [`RunHost`] seam: `/healthz`, `/runs/:id` (the `query_run`
//! snapshot), and `/runs/:id/events` (a finite SSE replay from a `seq` cursor).
//! Driven in tests by `tower::oneshot`; bound to a listener by the daemon (P0.9).

use std::convert::Infallible;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use serde::Deserialize;
use serde_json::json;

use agentd_core::types::RunId;

use crate::error::SurfaceError;
use crate::host::RunHost;
use crate::tools::query_run::{QueryRunInput, query_run};

/// Shared state for the surface routes: the [`RunHost`] seam the handlers read.
#[derive(Clone)]
pub struct AppState {
    pub host: Arc<dyn RunHost>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `dyn RunHost` is not `Debug`; the seam identity isn't useful to print.
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

/// Build the surface `Router` (design §7.2).
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/runs/:id", get(get_run))
        .route("/runs/:id/events", get(run_events))
        .with_state(state)
}

#[allow(clippy::unused_async)] // axum handlers are async; this one has nothing to await
async fn healthz() -> &'static str {
    "ok"
}

/// `GET /runs/:id` — the `query_run` snapshot as JSON; `not_found` → 404.
async fn get_run(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match query_run(state.host.as_ref(), QueryRunInput { run_id: id }).await {
        Ok(out) => Json(out).into_response(),
        Err(SurfaceError::NotFound) => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.code() })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    // Absent → 0 (replay from the start); a non-integer value fails the
    // extractor with 400.
    #[serde(default)]
    from_seq: i64,
}

/// `GET /runs/:id/events?from_seq=N` — a finite SSE replay of the run's events
/// with `seq > from_seq`, then the stream ends (no live tail in v0).
async fn run_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> Response {
    let run = RunId::from_string(id);
    match state.host.events_from(&run, q.from_seq).await {
        Ok(events) => {
            let frames: Vec<Result<Event, Infallible>> = events
                .into_iter()
                .map(|e| {
                    Ok(Event::default()
                        .id(e.seq.to_string())
                        .event(e.kind)
                        .data(e.payload))
                })
                .collect();
            Sse::new(futures::stream::iter(frames)).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "internal" })),
        )
            .into_response(),
    }
}
