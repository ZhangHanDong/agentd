//! The daemon's HTTP+SSE observability surface (design §7.2). An axum `Router`
//! over the [`RunHost`] seam: `/healthz`, `/runs/:id` (the `query_run`
//! snapshot), and `/runs/:id/events` (a LIVE SSE tail — P1: replay from a `seq`
//! cursor, then stream new events until the run terminates, with a lossy
//! broadcast so a slow dashboard never backpressures the engine).
//! Driven in tests by `tower::oneshot`; bound to a listener by the daemon (P0.9).

use std::convert::Infallible;
use std::sync::Arc;

use async_stream::stream;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{get, post};
use futures::Stream;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use agentd_core::RunProgress;
use agentd_core::types::RunId;

use crate::error::SurfaceError;
use crate::host::{EventRecord, LiveEvent, RunHost, RunSnapshot};
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
        .route("/dashboard", get(dashboard))
        .route("/dashboard/", get(dashboard))
        .route("/runs", post(start_run).get(get_runs))
        .route("/runs/:id", get(get_run))
        .route("/runs/:id/events", get(run_events))
        .with_state(state)
}

#[allow(clippy::unused_async)] // axum handlers are async; the shell is embedded.
async fn dashboard() -> Html<&'static str> {
    Html(include_str!("dashboard.html"))
}

/// `GET /runs` — the at-a-glance overview: every run's current status (P1).
async fn get_runs(State(state): State<AppState>) -> Response {
    match state.host.list_runs().await {
        Ok(runs) => Json(runs).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "internal" })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct StartRunReq {
    /// `"draft"` or `"execute"`.
    flow: String,
    run_id: String,
    #[serde(default)]
    context: Value,
}

/// `POST /runs` — create + start a workflow run; returns `{run_id, status}`.
async fn start_run(State(state): State<AppState>, Json(req): Json<StartRunReq>) -> Response {
    let run = RunId::from_string(req.run_id.clone());
    match state
        .host
        .start_workflow(&req.flow, &run, req.context)
        .await
    {
        Ok(progress) => (
            StatusCode::CREATED,
            Json(json!({ "run_id": req.run_id, "status": progress_kind(&progress) })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// The wire status string for a `RunProgress`.
fn progress_kind(progress: &RunProgress) -> &'static str {
    match progress {
        RunProgress::Parked { .. } => "parked",
        RunProgress::Finished { .. } => "finished",
        RunProgress::Failed { .. } => "failed",
        RunProgress::Ignored { .. } => "ignored",
    }
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

/// `GET /runs/:id/events?from_seq=N` — the LIVE SSE tail (P1): replay the run's
/// events with `seq > from_seq`, then stream new events until the run terminates.
async fn run_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> Response {
    let run = RunId::from_string(id);
    // Subscribe BEFORE replaying so no event emitted during the replay read is
    // missed (the seq overlap is deduped in the stream).
    let rx = state.host.subscribe_events();
    let Ok(replay) = state.host.events_from(&run, q.from_seq).await else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "internal" })),
        )
            .into_response();
    };
    Sse::new(live_event_stream(replay, rx, state.host.clone(), run))
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// The live SSE stream (P1, herdr/mosh): yield the replayed frames, then tail
/// `rx` for new live events of `run` (deduping the seq overlap with the replay),
/// closing on a terminal event (`run_finished`/`run_failed`). A LAGGING receiver
/// gets ONE `state_resync` snapshot frame — realign to latest, not backfill —
/// rather than an error. Pub so it can be tested deterministically over a
/// pre-loaded receiver.
pub fn live_event_stream(
    replay: Vec<EventRecord>,
    mut rx: broadcast::Receiver<LiveEvent>,
    host: Arc<dyn RunHost>,
    run: RunId,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let run_id = run.as_str().to_string();
    stream! {
        let mut max_seq = i64::MIN;
        for rec in replay {
            max_seq = max_seq.max(rec.seq);
            let terminal = is_terminal_kind(&rec.kind);
            yield Ok(event_frame(&rec));
            if terminal {
                return;
            }
        }
        loop {
            match rx.recv().await {
                Ok(live) => {
                    if live.run_id != run_id || live.event.seq <= max_seq {
                        continue; // not this run, or a deduped replay-overlap event
                    }
                    max_seq = live.event.seq;
                    let terminal = is_terminal_kind(&live.event.kind);
                    yield Ok(event_frame(&live.event));
                    if terminal {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Slow subscriber: resync to the authoritative latest state
                    // (mosh) instead of backfilling every dropped event.
                    if let Ok(Some(snap)) = host.run_snapshot(&run).await {
                        let terminal = is_terminal_status(&snap.status);
                        yield Ok(resync_frame(&snap));
                        if terminal {
                            break;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

fn event_frame(rec: &EventRecord) -> Event {
    Event::default()
        .id(rec.seq.to_string())
        .event(sanitize_sse_event_name(&rec.kind))
        .data(sanitize_sse_data(&rec.payload))
}

fn resync_frame(snap: &RunSnapshot) -> Event {
    let data = json!({
        "status": snap.status,
        "current_node": snap.current_node,
        "completed_nodes": snap.completed_nodes,
        "context": snap.context,
    });
    Event::default()
        .event(sanitize_sse_event_name("state_resync"))
        .data(sanitize_sse_data(&data.to_string()))
}

fn sanitize_sse_event_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\r' | '\n' => '_',
            _ => ch,
        })
        .collect()
}

fn sanitize_sse_data(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

fn is_terminal_kind(kind: &str) -> bool {
    kind == "run_finished" || kind == "run_failed"
}

fn is_terminal_status(status: &str) -> bool {
    status == "finished" || status == "failed"
}
