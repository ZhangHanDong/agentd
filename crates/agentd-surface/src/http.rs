//! The daemon's HTTP+SSE observability surface (design §7.2). An axum `Router`
//! over the [`RunHost`] seam: `/healthz`, `/runs/:id` (the `query_run`
//! snapshot), and `/runs/:id/events` (a LIVE SSE tail — P1: replay from a `seq`
//! cursor, then stream new events until the run terminates, with a lossy
//! broadcast so a slow dashboard never backpressures the engine).
//! Driven in tests by `tower::oneshot`; bound to a listener by the daemon (P0.9).

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::fs;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_stream::stream;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{get, patch, post};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::Stream;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use agentd_core::types::RunId;
use agentd_core::{CoreError, RunProgress};

use crate::error::SurfaceError;
use crate::host::{
    AgentChatTaskCommentInput, AgentChatTaskCreateInput, AgentChatTaskExecutionInput,
    AgentChatTaskGraphCreateInput, AgentChatTaskGraphNodePatchInput, AgentChatTaskListFilters,
    AgentChatTaskPatchInput, AgentChatTaskTransitionInput, AgentHeartbeat, AgentIdentityPatch,
    AgentOffline, AgentRegistration, AgentRuntimeUpdate, DeliveryEventInput, DirectMessageInput,
    EventRecord, GroupCreateInput, GroupMemberUpdate, LiveEvent, MatrixBridgeRoomInput,
    MatrixInboundMessageInput, RelayServerHeartbeat, RelayStreamEventRecord, RunHost, RunSnapshot,
    SchedulerDispatchInput, SchedulerPoolFilters, SchedulerReleaseInput,
};
use crate::mcp_server::dispatch;
use crate::tools::attachments::{
    ATTACHMENT_MAX_BYTES, infer_attachment_kind, normalize_absolute, normalize_attachment_mime,
    normalize_attachment_name, normalize_http_attachments, resolve_media_path,
};
use crate::tools::check_group::{CheckGroupInput, check_group};
use crate::tools::check_inbox::{CheckInboxInput, check_inbox};
use crate::tools::post::{PostInput, post_with_normalized_attachments};
use crate::tools::query_run::{QueryRunInput, query_run};

/// Shared state for the surface routes: the [`RunHost`] seam the handlers read.
#[derive(Clone)]
pub struct AppState {
    pub host: Arc<dyn RunHost>,
    pub auth: AuthConfig,
    pub media: MediaConfig,
    pub scheduler: SchedulerConfig,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `dyn RunHost` is not `Debug`; the seam identity isn't useful to print.
        f.debug_struct("AppState")
            .field("media", &self.media)
            .field("scheduler", &self.scheduler)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchedulerConfig {
    pub max_per_cell: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaConfig {
    pub media_dir: PathBuf,
}

impl MediaConfig {
    #[must_use]
    pub fn new(media_dir: impl Into<PathBuf>) -> Self {
        Self {
            media_dir: media_dir.into(),
        }
    }

    #[must_use]
    pub fn default_local() -> Self {
        Self::new(".agentd/media")
    }

    #[must_use]
    pub fn default_for_tests() -> Self {
        Self::new(
            std::env::temp_dir().join(format!("agentd-surface-media-tests-{}", std::process::id())),
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AgentTokenMode {
    Hard,
    #[default]
    Audit,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthConfig {
    pub api_token: Option<String>,
    pub agent_token_mode: AgentTokenMode,
    pub agent_tokens: BTreeMap<String, String>,
}

impl AuthConfig {
    #[must_use]
    pub fn open() -> Self {
        Self::default()
    }

    fn is_configured(&self) -> bool {
        self.api_token
            .as_deref()
            .is_some_and(|v| !v.trim().is_empty())
            || !self.agent_tokens.is_empty()
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
        .route("/api/media/stage", post(stage_media))
        .route("/api/media/fetch", get(fetch_media))
        .route("/api/stream", get(relay_stream))
        .route("/api/matrix/rooms", post(post_matrix_room))
        .route("/api/matrix/rooms/:room_id", get(get_matrix_room))
        .route("/api/matrix/inbound", post(post_matrix_inbound))
        .route("/api/matrix/outbox", get(matrix_outbox))
        .route("/api/messages", post(post_message))
        .route("/api/delivery-events", post(post_delivery_event))
        .route("/api/inbox/:agent", get(get_inbox))
        .route("/api/tasks", post(create_task).get(list_tasks))
        .route(
            "/api/tasks/:id",
            get(get_task).patch(update_task).delete(delete_task),
        )
        .route("/api/tasks/:id/execution", patch(update_task_execution))
        .route("/api/tasks/:id/accept", post(accept_task))
        .route("/api/tasks/:id/transition", post(transition_task))
        .route("/api/tasks/:id/comments", post(add_task_comment))
        .route(
            "/api/task-graphs",
            post(create_task_graph).get(list_task_graphs),
        )
        .route(
            "/api/task-graphs/:id",
            get(get_task_graph).delete(delete_task_graph),
        )
        .route(
            "/api/task-graphs/:id/nodes/:node_id",
            patch(update_task_graph_node),
        )
        .route("/api/pool", get(get_pool))
        .route("/api/dispatch", post(dispatch_scheduler))
        .route("/api/dispatch/release", post(release_scheduler))
        .route("/api/groups", post(create_group).get(list_groups))
        .route("/api/groups/:name", get(get_group).delete(delete_group))
        .route("/api/groups/:name/members", post(update_group_members))
        .route("/api/groups/:name/messages", get(get_group_messages))
        .route("/api/agents", post(register_agent).get(list_agents))
        .route("/api/agents/:name/tasks", get(get_agent_tasks))
        .route(
            "/api/agents/:name",
            get(get_agent_detail).patch(update_agent_identity),
        )
        .route("/api/agents/:name/launch-env", get(agent_launch_env))
        .route("/api/agents/:name/start", post(agent_start))
        .route("/api/agents/:name/down", post(agent_down))
        .route("/api/agents/:name/rebind", post(agent_rebind))
        .route("/api/agents/:name/runtime", post(agent_runtime))
        .route("/api/agents/:name/heartbeat", post(agent_heartbeat))
        .route("/api/agents/:name/offline", post(agent_offline))
        .route(
            "/api/agents/:name/delivery-events",
            get(get_agent_delivery_events),
        )
        .route("/api/servers/heartbeat", post(server_heartbeat))
        .route("/tools/call", post(tool_call))
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

#[derive(Debug, Deserialize)]
struct ToolCallReq {
    name: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct MessagePostReq {
    #[serde(default, alias = "id")]
    message_id: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
    from: String,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    group: Option<String>,
    #[serde(default, rename = "type", alias = "messageType")]
    message_type: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    summary: String,
    #[serde(default)]
    full: String,
    #[serde(default)]
    mentions: Vec<String>,
    #[serde(default)]
    reply_to: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default, rename = "sourceRoom", alias = "source_room")]
    source_room: Option<String>,
    #[serde(default, rename = "senderMxid", alias = "sender_mxid")]
    sender_mxid: Option<String>,
    #[serde(default, rename = "trustLevel", alias = "trust_level")]
    trust_level: Option<String>,
    #[serde(default, rename = "fromId", alias = "from_id")]
    from_id: Option<String>,
    #[serde(default)]
    schema: Option<Value>,
    #[serde(default)]
    attachments: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct DeliveryEventsQuery {
    #[serde(default = "default_delivery_event_limit")]
    limit: usize,
}

fn default_delivery_event_limit() -> usize {
    50
}

#[derive(Debug, Deserialize)]
struct RelayStreamQuery {
    #[serde(default)]
    from_seq: i64,
}

#[derive(Debug, Deserialize)]
struct MediaStageReq {
    from: String,
    #[serde(default)]
    source_path: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    mime: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    content_base64: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MediaFetchQuery {
    path: String,
}

/// `POST /tools/call` — agent-facing tool dispatch through the central daemon.
async fn tool_call(State(state): State<AppState>, Json(req): Json<ToolCallReq>) -> Response {
    match dispatch(state.host.as_ref(), &req.name, req.arguments).await {
        Ok(out) => Json(out).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.code() })),
        )
            .into_response(),
    }
}

async fn stage_media(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<MediaStageReq>,
) -> Response {
    let Some(from) = clean_required_text(&req.from) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "from required" })),
        )
            .into_response();
    };
    if let Err(err) = require_agent_token(&state.auth, &headers, &from) {
        return err.into_response();
    }
    match state.host.get_agent(&from).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("agent not found: {from}") })),
            )
                .into_response();
        }
        Err(e) => return agent_error_response(e),
    }

    let Some(content_base64) = clean_optional_text(req.content_base64) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "content_base64 required" })),
        )
            .into_response();
    };
    let Ok(bytes) = STANDARD.decode(content_base64.as_bytes()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid base64 payload" })),
        )
            .into_response();
    };
    if bytes.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "empty attachment payload" })),
        )
            .into_response();
    }
    if bytes.len() as u64 > ATTACHMENT_MAX_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({ "error": format!("attachment exceeds max bytes ({ATTACHMENT_MAX_BYTES})") })),
        )
            .into_response();
    }

    let media_dir = match prepare_media_dir(&state.media.media_dir) {
        Ok(path) => path,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e })),
            )
                .into_response();
        }
    };
    let source_path = clean_optional_text(req.source_path);
    let fallback_name = source_path
        .as_deref()
        .and_then(|value| {
            FsPath::new(value)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "file.bin".to_string());
    let name = normalize_attachment_name(req.name, &fallback_name);
    let file_path = media_dir.join(format!("{}-{}-{name}", now_nanos(), std::process::id()));
    if let Err(e) = fs::write(&file_path, &bytes) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("failed to write attachment: {e}") })),
        )
            .into_response();
    }

    let mime = normalize_attachment_mime(req.mime);
    let kind = infer_attachment_kind(req.kind, mime.as_deref(), &name);
    let attachment = json!({
        "path": file_path.to_string_lossy().to_string(),
        "name": name,
        "mime": mime,
        "kind": kind,
        "size": bytes.len(),
        "staged": true,
        "source_path": source_path,
    });
    Json(json!({ "ok": true, "attachment": attachment })).into_response()
}

async fn fetch_media(
    State(state): State<AppState>,
    Query(query): Query<MediaFetchQuery>,
) -> Response {
    let media_dir = match normalize_absolute(&state.media.media_dir) {
        Ok(path) => path,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e })),
            )
                .into_response();
        }
    };
    let file_path = match resolve_media_path(&query.path, &media_dir) {
        Ok(path) => path,
        Err(e) if e == "path not allowed" => {
            return (StatusCode::FORBIDDEN, Json(json!({ "error": e }))).into_response();
        }
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    };
    let Ok(meta) = fs::metadata(&file_path) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "file not found" })),
        )
            .into_response();
    };
    if !meta.is_file() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "path is not a file" })),
        )
            .into_response();
    }
    if meta.len() == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "file is empty" })),
        )
            .into_response();
    }
    if meta.len() > ATTACHMENT_MAX_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({ "error": format!("file exceeds max bytes ({ATTACHMENT_MAX_BYTES})") })),
        )
            .into_response();
    }
    let bytes = match fs::read(&file_path) {
        Ok(bytes) => bytes,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("failed to read file: {e}") })),
            )
                .into_response();
        }
    };
    let file_name = normalize_attachment_name(
        file_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_string),
        "file.bin",
    );
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(guess_mime_from_path(&file_path)),
    );
    if let Ok(value) = HeaderValue::from_str(&meta.len().to_string()) {
        response.headers_mut().insert(header::CONTENT_LENGTH, value);
    }
    if let Ok(value) = HeaderValue::from_str(&format!("inline; filename=\"{file_name}\"")) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

async fn relay_stream(
    State(state): State<AppState>,
    Query(q): Query<RelayStreamQuery>,
) -> Response {
    let Ok(events) = state.host.relay_stream_events(q.from_seq).await else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "internal" })),
        )
            .into_response();
    };
    let frames = events
        .into_iter()
        .map(|event| Ok::<Event, Infallible>(relay_stream_event_frame(&event)));
    Sse::new(futures::stream::iter(frames)).into_response()
}

async fn post_matrix_room(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<MatrixBridgeRoomInput>,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.upsert_matrix_bridge_room(req).await {
        Ok(room) => Json(json!({ "ok": true, "room": room })).into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn get_matrix_room(
    State(state): State<AppState>,
    AxumPath(room_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.get_matrix_bridge_room(&room_id).await {
        Ok(Some(room)) => Json(json!({ "room": room })).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "matrix room not found" })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn post_matrix_inbound(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<MatrixInboundMessageInput>,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.post_matrix_inbound_message(req).await {
        Ok(result) if result.duplicate || result.ignored => Json(result).into_response(),
        Ok(result) => (StatusCode::CREATED, Json(result)).into_response(),
        Err(CoreError::Invariant(message)) if message == "matrix room not trusted" => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "matrix room not trusted" })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn matrix_outbox(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<RelayStreamQuery>,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.relay_stream_events(q.from_seq).await {
        Ok(events) => {
            let events = events
                .into_iter()
                .filter(|event| {
                    event.event == "message"
                        && event.payload.get("source").and_then(Value::as_str) != Some("matrix")
                })
                .collect::<Vec<_>>();
            Json(json!({ "events": events })).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "internal" })),
        )
            .into_response(),
    }
}

async fn post_delivery_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<DeliveryEventInput>,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.append_delivery_event(req).await {
        Ok(event) => (
            StatusCode::CREATED,
            Json(json!({ "ok": true, "event": event })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn get_agent_delivery_events(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Query(query): Query<DeliveryEventsQuery>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_agent_token(&state.auth, &headers, &name) {
        return err.into_response();
    }
    match state.host.list_delivery_events(&name, query.limit).await {
        Ok(events) => Json(json!({ "events": events })).into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn server_heartbeat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RelayServerHeartbeat>,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    if req.server.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "server required" })),
        )
            .into_response();
    }
    match state.host.record_relay_server_heartbeat(req).await {
        Ok(server) => Json(json!({ "ok": true, "server": server })).into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn post_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<MessagePostReq>,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    let to = req.to.clone();
    let group = req.group.clone();
    match (to, group) {
        (Some(_), Some(_)) | (None, None) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "exactly one of to or group required" })),
        )
            .into_response(),
        (Some(to), None) => {
            let attachments =
                match normalize_http_attachments(req.attachments, &state.media.media_dir) {
                    Ok(attachments) => attachments,
                    Err(e) => return surface_error_response(e),
                };
            let from = req.from.clone();
            let reply_to = req.reply_to.clone();
            let schema = req.schema.clone();
            match state
                .host
                .post_direct_message(DirectMessageInput {
                    message_id: req.message_id,
                    ts: req.ts,
                    from: req.from,
                    to,
                    message_type: req.message_type,
                    priority: req.priority,
                    summary: req.summary,
                    full: req.full,
                    reply_to: req.reply_to,
                    source: req.source,
                    source_room: req.source_room,
                    sender_mxid: req.sender_mxid,
                    trust_level: req.trust_level,
                    from_id: req.from_id,
                    schema,
                    attachments,
                })
                .await
            {
                Ok(message) => match state
                    .host
                    .handle_agent_chat_task_graph_message(&from, reply_to, req.schema)
                    .await
                {
                    Ok(task_graph) => (
                        StatusCode::CREATED,
                        Json(json!({ "ok": true, "message": message, "taskGraph": task_graph })),
                    )
                        .into_response(),
                    Err(e) => task_error_response(e),
                },
                Err(e) => agent_error_response(e),
            }
        }
        (None, Some(group)) => {
            let attachments =
                match normalize_http_attachments(req.attachments, &state.media.media_dir) {
                    Ok(attachments) => attachments,
                    Err(e) => return surface_error_response(e),
                };
            match post_with_normalized_attachments(
                state.host.as_ref(),
                PostInput {
                    from_agent: Some(req.from),
                    group,
                    summary: req.summary,
                    full: req.full,
                    message_type: req.message_type,
                    priority: req.priority,
                    mentions: req.mentions,
                    reply_to: req.reply_to,
                    schema: req.schema,
                    attachments: Vec::new(),
                },
                attachments,
            )
            .await
            {
                Ok(out) => Json(out).into_response(),
                Err(e) => surface_error_response(e),
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct InboxQuery {
    #[serde(default)]
    drain: bool,
}

async fn get_inbox(
    State(state): State<AppState>,
    AxumPath(agent): AxumPath<String>,
    Query(query): Query<InboxQuery>,
) -> Response {
    match check_inbox(
        state.host.as_ref(),
        CheckInboxInput {
            agent_id: agent,
            drain: query.drain,
        },
    )
    .await
    {
        Ok(out) => Json(out).into_response(),
        Err(e) => surface_error_response(e),
    }
}

#[derive(Debug, Deserialize)]
struct TaskListQuery {
    assignee: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    label: Option<String>,
    offset: Option<String>,
    limit: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TaskGraphListQuery {
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PoolQuery {
    role: Option<String>,
    capability: Option<String>,
    state: Option<String>,
}

async fn create_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AgentChatTaskCreateInput>,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.create_agent_chat_task(req).await {
        Ok(task) => Json(json!({ "ok": true, "task": task })).into_response(),
        Err(e) => task_error_response(e),
    }
}

async fn list_tasks(State(state): State<AppState>, Query(query): Query<TaskListQuery>) -> Response {
    match state
        .host
        .list_agent_chat_tasks(task_filters_from_query(query))
        .await
    {
        Ok(tasks) => Json(tasks).into_response(),
        Err(e) => task_error_response(e),
    }
}

async fn get_task(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    match state.host.get_agent_chat_task(&id).await {
        Ok(Some(task)) => Json(task).into_response(),
        Ok(None) => task_not_found_response(),
        Err(e) => task_error_response(e),
    }
}

async fn update_task(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    Json(req): Json<AgentChatTaskPatchInput>,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.update_agent_chat_task(&id, req).await {
        Ok(Some(task)) => Json(json!({ "ok": true, "task": task })).into_response(),
        Ok(None) => task_not_found_response(),
        Err(e) => task_error_response(e),
    }
}

async fn update_task_execution(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    Json(req): Json<AgentChatTaskExecutionInput>,
) -> Response {
    if let Err(response) = require_task_assignee_token(&state, &headers, &id).await {
        return response;
    }
    match state.host.update_agent_chat_task_execution(&id, req).await {
        Ok(Some(task)) => Json(json!({ "ok": true, "task": task })).into_response(),
        Ok(None) => task_not_found_response(),
        Err(e) => task_error_response(e),
    }
}

async fn delete_task(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.delete_agent_chat_task(&id).await {
        Ok(Some(task)) => Json(json!({ "ok": true, "task": task })).into_response(),
        Ok(None) => task_not_found_response(),
        Err(e) => task_error_response(e),
    }
}

async fn accept_task(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_task_assignee_token(&state, &headers, &id).await {
        return response;
    }
    match state
        .host
        .transition_agent_chat_task(
            &id,
            AgentChatTaskTransitionInput {
                status: Some("accepted".to_string()),
                waiting_reason: None,
                waiting_until: None,
            },
        )
        .await
    {
        Ok(Some(task)) => Json(json!({ "ok": true, "task": task })).into_response(),
        Ok(None) => task_not_found_response(),
        Err(e) => task_error_response(e),
    }
}

async fn transition_task(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    Json(req): Json<AgentChatTaskTransitionInput>,
) -> Response {
    if let Err(response) = require_task_assignee_token(&state, &headers, &id).await {
        return response;
    }
    match state.host.transition_agent_chat_task(&id, req).await {
        Ok(Some(task)) => Json(json!({ "ok": true, "task": task })).into_response(),
        Ok(None) => task_not_found_response(),
        Err(e) => task_error_response(e),
    }
}

async fn add_task_comment(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    Json(req): Json<AgentChatTaskCommentInput>,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.add_agent_chat_task_comment(&id, req).await {
        Ok(Some(task)) => Json(json!({ "ok": true, "task": task })).into_response(),
        Ok(None) => task_not_found_response(),
        Err(e) => task_error_response(e),
    }
}

async fn create_task_graph(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AgentChatTaskGraphCreateInput>,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.create_agent_chat_task_graph(req).await {
        Ok(graph) => Json(json!({ "ok": true, "graph": graph })).into_response(),
        Err(e) => task_error_response(e),
    }
}

async fn list_task_graphs(
    State(state): State<AppState>,
    Query(query): Query<TaskGraphListQuery>,
) -> Response {
    match state.host.list_agent_chat_task_graphs(query.status).await {
        Ok(graphs) => Json(graphs).into_response(),
        Err(e) => task_error_response(e),
    }
}

async fn get_task_graph(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    match state.host.get_agent_chat_task_graph(&id).await {
        Ok(Some(graph)) => Json(graph).into_response(),
        Ok(None) => task_graph_not_found_response(),
        Err(e) => task_error_response(e),
    }
}

async fn delete_task_graph(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.delete_agent_chat_task_graph(&id).await {
        Ok(Some(graph)) => Json(json!({ "ok": true, "graph": graph })).into_response(),
        Ok(None) => task_graph_not_found_response(),
        Err(e) => task_error_response(e),
    }
}

async fn update_task_graph_node(
    State(state): State<AppState>,
    AxumPath((id, node_id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(req): Json<AgentChatTaskGraphNodePatchInput>,
) -> Response {
    if let Err(response) =
        require_task_graph_node_assignee_token(&state, &headers, &id, &node_id).await
    {
        return response;
    }
    match state
        .host
        .update_agent_chat_task_graph_node(&id, &node_id, req)
        .await
    {
        Ok(Some((graph, node))) => {
            Json(json!({ "ok": true, "graph": graph, "node": node })).into_response()
        }
        Ok(None) => task_graph_not_found_response(),
        Err(e) => task_error_response(e),
    }
}

async fn get_pool(State(state): State<AppState>, Query(query): Query<PoolQuery>) -> Response {
    match state
        .host
        .scheduler_pool(SchedulerPoolFilters {
            role: query.role,
            capability: query.capability,
            state: query.state,
        })
        .await
    {
        Ok(pool) => Json(pool).into_response(),
        Err(e) => scheduler_error_response(e),
    }
}

async fn dispatch_scheduler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SchedulerDispatchInput>,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state
        .host
        .scheduler_dispatch(req, state.scheduler.max_per_cell)
        .await
    {
        Ok(result) => Json(result).into_response(),
        Err(e) => scheduler_error_response(e),
    }
}

async fn release_scheduler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SchedulerReleaseInput>,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.scheduler_release(req).await {
        Ok(result) => Json(result).into_response(),
        Err(e) => scheduler_error_response(e),
    }
}

async fn get_agent_tasks(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    let Some(name) = clean_required_text(&name) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid agent name" })),
        )
            .into_response();
    };
    match state
        .host
        .list_agent_chat_tasks(AgentChatTaskListFilters {
            assignee: Some(name),
            ..AgentChatTaskListFilters::default()
        })
        .await
    {
        Ok(tasks) => Json(tasks).into_response(),
        Err(e) => task_error_response(e),
    }
}

async fn create_group(
    State(state): State<AppState>,
    Json(req): Json<GroupCreateInput>,
) -> Response {
    match state.host.create_group(req).await {
        Ok(group) => (
            StatusCode::CREATED,
            Json(json!({ "ok": true, "group": group })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn list_groups(State(state): State<AppState>) -> Response {
    match state.host.list_groups().await {
        Ok(groups) => Json(groups).into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn get_group(State(state): State<AppState>, AxumPath(name): AxumPath<String>) -> Response {
    match state.host.get_group(&name).await {
        Ok(Some(group)) => Json(group).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "group_not_found" })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn update_group_members(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Json(req): Json<GroupMemberUpdate>,
) -> Response {
    match state.host.update_group_members(&name, req).await {
        Ok(Some(group)) => Json(json!({ "ok": true, "group": group })).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "group_not_found" })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn delete_group(State(state): State<AppState>, AxumPath(name): AxumPath<String>) -> Response {
    match state.host.delete_group(&name).await {
        Ok(Some(group)) => Json(json!({ "ok": true, "group": group })).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "group_not_found" })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

#[derive(Debug, Deserialize)]
struct GroupMessagesQuery {
    agent: Option<String>,
    limit: Option<usize>,
    unread_limit: Option<usize>,
    advance: Option<String>,
}

async fn get_group_messages(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Query(query): Query<GroupMessagesQuery>,
) -> Response {
    let Some(agent_id) = query.agent else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "agent required" })),
        )
            .into_response();
    };
    let read_all = matches!(query.advance.as_deref(), Some("all"));
    match check_group(
        state.host.as_ref(),
        CheckGroupInput {
            group: name,
            agent_id: Some(agent_id),
            limit: query.limit,
            unread_limit: query.unread_limit,
            read_all: Some(read_all),
        },
    )
    .await
    {
        Ok(out) => Json(out).into_response(),
        Err(e) => surface_error_response(e),
    }
}

async fn register_agent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AgentRegistration>,
) -> Response {
    if let Err(err) = require_agent_token(&state.auth, &headers, &req.name) {
        return err.into_response();
    }
    match state.host.register_agent(req).await {
        Ok(agent) => Json(json!({ "ok": true, "agent": agent })).into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn list_agents(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.list_agents().await {
        Ok(agents) => Json(agents).into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn get_agent_detail(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.get_agent(&name).await {
        Ok(Some(agent)) => Json(agent).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "agent_not_found" })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn update_agent_identity(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
    Json(req): Json<AgentIdentityPatch>,
) -> Response {
    if let Err(err) = require_local_operator(&state.auth, &headers) {
        return err.into_response();
    }
    let identity = req.identity.trim();
    if identity.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "identity text required" })),
        )
            .into_response();
    }
    match state.host.update_agent_identity(&name, identity).await {
        Ok(Some(agent)) => Json(json!({ "ok": true, "agent": agent })).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "agent_not_found" })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn agent_launch_env(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_local_operator(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.get_agent(&name).await {
        Ok(Some(agent)) => Json(json!({ "runtimeProfile": agent.runtime_profile })).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "agent_not_found" })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn agent_start(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_local_operator(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.start_agent(&name).await {
        Ok(Some(result)) => Json(json!({
            "ok": true,
            "agent": result.agent,
            "handle": result.handle,
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "agent_not_found" })),
        )
            .into_response(),
        Err(e) => agent_start_error_response(e),
    }
}

async fn agent_down(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_local_operator(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.down_agent(&name).await {
        Ok(Some(result)) => Json(json!({
            "ok": true,
            "action": "agent-down-kill",
            "agent": result.agent,
            "report": result.report,
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "agent_not_found" })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn agent_rebind(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_local_operator(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.rebind_agent(&name).await {
        Ok(Some(result)) => Json(json!({
            "ok": true,
            "rebound": result.rebound,
            "agent": result.agent,
            "handle": result.handle,
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "agent_not_found" })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn agent_runtime(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
    Json(req): Json<AgentRuntimeUpdate>,
) -> Response {
    if let Err(err) = require_agent_token(&state.auth, &headers, &name) {
        return err.into_response();
    }
    match state.host.update_agent_runtime(&name, req).await {
        Ok(Some(runtime)) => Json(json!({ "ok": true, "runtime": runtime })).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "agent_not_found" })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn agent_heartbeat(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
    Json(req): Json<AgentHeartbeat>,
) -> Response {
    if let Err(err) = require_agent_token(&state.auth, &headers, &name) {
        return err.into_response();
    }
    match state.host.heartbeat_agent(&name, req).await {
        Ok((agent, created)) => {
            let runtime = json!({
                "agent": agent.name.clone(),
                "last_seen": agent.last_seen_at,
                "updated_at": agent.updated_at,
                "workspace_path": agent.workdir.clone(),
            });
            Json(json!({
                "ok": true,
                "created": created,
                "agent": agent,
                "runtime": runtime,
            }))
            .into_response()
        }
        Err(e) => agent_error_response(e),
    }
}

async fn agent_offline(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
    Json(req): Json<AgentOffline>,
) -> Response {
    if let Err(err) = require_agent_token(&state.auth, &headers, &name) {
        return err.into_response();
    }
    match state.host.mark_agent_offline(&name, req).await {
        Ok(Some(agent)) => Json(json!({ "ok": true, "agent": agent })).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "agent_not_found" })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

fn task_filters_from_query(query: TaskListQuery) -> AgentChatTaskListFilters {
    AgentChatTaskListFilters {
        assignee: query.assignee.and_then(|value| clean_required_text(&value)),
        statuses: query
            .status
            .unwrap_or_default()
            .split(',')
            .filter_map(clean_required_text)
            .collect(),
        priority: query.priority.and_then(|value| clean_required_text(&value)),
        label: query.label.and_then(|value| clean_required_text(&value)),
        offset: parse_task_page_int(query.offset.as_deref(), 0),
        limit: parse_task_page_limit(query.limit.as_deref()),
    }
}

fn parse_task_page_int(value: Option<&str>, fallback: usize) -> usize {
    value
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(fallback)
}

fn parse_task_page_limit(value: Option<&str>) -> Option<usize> {
    value
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.min(500))
}

async fn require_task_assignee_token(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
) -> Result<(), Response> {
    match state.host.get_agent_chat_task(id).await {
        Ok(Some(task)) => {
            if let Some(assignee) = task.assignee.as_deref().filter(|value| !value.is_empty()) {
                require_agent_token(&state.auth, headers, assignee)
                    .map_err(AuthRejection::into_response)?;
            }
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(e) => Err(task_error_response(e)),
    }
}

async fn require_task_graph_node_assignee_token(
    state: &AppState,
    headers: &HeaderMap,
    graph_id: &str,
    node_id: &str,
) -> Result<(), Response> {
    match state.host.get_agent_chat_task_graph(graph_id).await {
        Ok(Some(graph)) => {
            if let Some(node) = graph.nodes.get(node_id) {
                require_agent_token(&state.auth, headers, &node.assignee)
                    .map_err(AuthRejection::into_response)?;
            }
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(e) => Err(task_error_response(e)),
    }
}

fn task_not_found_response() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "task not found" })),
    )
        .into_response()
}

fn task_graph_not_found_response() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "task graph not found" })),
    )
        .into_response()
}

fn task_error_response(e: CoreError) -> Response {
    match e {
        CoreError::Invariant(message) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
        }
        CoreError::Store(message) if message.starts_with("invariant violated: ") => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": message.trim_start_matches("invariant violated: ") })),
        )
            .into_response(),
        other => agent_error_response(other),
    }
}

fn scheduler_error_response(e: CoreError) -> Response {
    task_error_response(e)
}

enum AuthRejection {
    BearerRequired,
    LocalOnly,
    AgentTokenRequired,
}

impl AuthRejection {
    fn into_response(self) -> Response {
        match self {
            Self::BearerRequired => (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "bearer token required" })),
            )
                .into_response(),
            Self::LocalOnly => (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "local-only endpoint" })),
            )
                .into_response(),
            Self::AgentTokenRequired => (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "agent token required" })),
            )
                .into_response(),
        }
    }
}

fn require_operator_bearer(auth: &AuthConfig, headers: &HeaderMap) -> Result<(), AuthRejection> {
    let Some(expected) = auth.api_token.as_deref().filter(|v| !v.trim().is_empty()) else {
        return Ok(());
    };
    if bearer_token(headers).as_deref() == Some(expected.trim()) {
        return Ok(());
    }
    Err(AuthRejection::BearerRequired)
}

fn require_local_operator(auth: &AuthConfig, headers: &HeaderMap) -> Result<(), AuthRejection> {
    require_operator_bearer(auth, headers)?;
    if auth.is_configured() && !is_local_request(headers) {
        return Err(AuthRejection::LocalOnly);
    }
    Ok(())
}

fn require_agent_token(
    auth: &AuthConfig,
    headers: &HeaderMap,
    agent_name: &str,
) -> Result<(), AuthRejection> {
    let Some(expected) = auth.agent_tokens.get(agent_name).map(String::as_str) else {
        return Ok(());
    };
    let provided = headers
        .get("x-agent-token")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty());
    if provided == Some(expected) {
        return Ok(());
    }
    if auth.agent_token_mode == AgentTokenMode::Audit {
        return Ok(());
    }
    Err(AuthRejection::AgentTokenRequired)
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("authorization")?.to_str().ok()?.trim();
    let (scheme, token) = raw.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn is_local_request(headers: &HeaderMap) -> bool {
    let Some(forwarded) = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    else {
        return true;
    };
    let first = forwarded.split(',').next().unwrap_or("").trim();
    matches!(first, "127.0.0.1" | "::1" | "localhost")
}

fn clean_required_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn clean_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn prepare_media_dir(path: &FsPath) -> Result<PathBuf, String> {
    let media_dir = normalize_absolute(path)?;
    if let Err(e) = fs::create_dir_all(&media_dir) {
        return Err(format!("failed to create media dir: {e}"));
    }
    Ok(media_dir)
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn guess_mime_from_path(path: &FsPath) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" | "md" | "log" => "text/plain; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn agent_error_response(e: CoreError) -> Response {
    match e {
        CoreError::Invariant(message) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
        }
        other => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": other.to_string() })),
        )
            .into_response(),
    }
}

fn surface_error_response(e: SurfaceError) -> Response {
    match e {
        SurfaceError::BadRequest(message) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
        }
        SurfaceError::NotFound => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response()
        }
        SurfaceError::Forbidden => {
            (StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))).into_response()
        }
        other => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": other.code() })),
        )
            .into_response(),
    }
}

fn agent_start_error_response(e: CoreError) -> Response {
    match e {
        CoreError::Invariant(message) if message.contains("already online") => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "agent_already_online" })),
        )
            .into_response(),
        other => agent_error_response(other),
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
async fn get_run(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
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
    AxumPath(id): AxumPath<String>,
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
        .event(rec.kind.clone())
        .data(rec.payload.clone())
}

fn relay_stream_event_frame(rec: &RelayStreamEventRecord) -> Event {
    let mut data = match rec.payload.clone() {
        Value::Object(object) => object,
        other => serde_json::Map::from_iter([("payload".to_string(), other)]),
    };
    data.insert("seq".to_string(), json!(rec.seq));
    data.insert("event".to_string(), json!(rec.event));
    Event::default()
        .id(rec.seq.to_string())
        .event(rec.event.clone())
        .data(Value::Object(data).to_string())
}

fn resync_frame(snap: &RunSnapshot) -> Event {
    let data = json!({
        "status": snap.status,
        "current_node": snap.current_node,
        "completed_nodes": snap.completed_nodes,
        "context": snap.context,
    });
    Event::default()
        .event("state_resync")
        .data(data.to_string())
}

fn is_terminal_kind(kind: &str) -> bool {
    kind == "run_finished" || kind == "run_failed"
}

fn is_terminal_status(status: &str) -> bool {
    status == "finished" || status == "failed"
}
