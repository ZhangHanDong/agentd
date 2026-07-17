//! `RunHost` — the agent↔engine/store seam the MCP tools sit on. The production
//! host (constructs an `Engine` with the real ports + the run's graph and reads
//! the store/checkpoint) is the daemon's job, wired in P0.9; tests inject a
//! `FakeRunHost`. The engine is `Engine<'a>` (per-call, borrow-based), so the
//! tools must not hold it directly — they hold `Arc<dyn RunHost>`.

use agentd_core::CoreError;
use agentd_core::ports::{
    RuntimeEvent, RuntimeInputAck, RuntimeKeyInput, RuntimeResizeRequest, RuntimeShutdownReport,
    RuntimeShutdownRequest, RuntimeSnapshot, RuntimeTextInput, RuntimeView, RuntimeWaitRequest,
};
use agentd_core::types::{
    CapabilityAdmission, NodeId, ReviewRunId, RunId, RuntimeSessionId, TaskRunId,
};
use agentd_core::{EngineEvent, RunProgress};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

/// A run's state, for `query_run`.
#[derive(Debug, Clone)]
pub struct RunSnapshot {
    pub status: String,
    pub current_node: Option<String>,
    pub completed_nodes: Vec<String>,
    pub context: Value,
}

/// An open task assignment, for `assign_task` and the `submit_outcome`
/// `task_run_id` resolution.
#[derive(Debug, Clone)]
pub struct TaskAssignment {
    pub task_run_id: TaskRunId,
    pub agent_id: String,
    pub worktree: Option<String>,
    pub spec_path: Option<String>,
    pub plan_path: Option<String>,
    pub context_pack: Option<String>,
}

/// One event of a run's append-only log, for SSE replay. Surface-local (mirrors
/// `RunSnapshot`/`TaskAssignment`); the production host maps `agentd-store`'s
/// `event_repo::EventRow` to this in P0.9 so the surface keeps no store dep.
#[derive(Debug, Clone)]
pub struct EventRecord {
    pub seq: i64,
    pub kind: String,
    pub payload: String,
}

/// A live event for the SSE tail (P1): an [`EventRecord`] tagged with its
/// `run_id`, broadcast on the host's lossy channel. `Clone` because
/// `tokio::sync::broadcast` requires it.
#[derive(Debug, Clone)]
pub struct LiveEvent {
    pub run_id: String,
    pub event: EventRecord,
}

/// A run's headline state for the `GET /runs` overview (P1). `Serialize` for the
/// JSON list response.
#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub run_id: String,
    pub status: String,
    pub current_node: Option<String>,
    pub started_at: i64,
}

/// A durable local agent record, shaped for agent-chat registry parity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub id: String,
    pub name: String,
    pub role: Option<String>,
    pub capability: Option<String>,
    pub runtime: Option<String>,
    pub model: Option<String>,
    pub tmux_target: Option<String>,
    pub home_dir: Option<String>,
    pub workdir: Option<String>,
    pub state_dir: Option<String>,
    pub server: Option<String>,
    pub status: String,
    pub offline_reason: Option<String>,
    pub last_seen_at: Option<i64>,
    pub registered_at: i64,
    pub updated_at: i64,
    pub runtime_profile: Value,
    pub runtime_state: Value,
}

/// Agent registration/upsert input for `POST /api/agents`.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentRegistration {
    pub name: String,
    pub role: Option<String>,
    pub capability: Option<String>,
    pub runtime: Option<String>,
    pub model: Option<String>,
    pub tmux_target: Option<String>,
    pub home_dir: Option<String>,
    pub workdir: Option<String>,
    pub state_dir: Option<String>,
    pub server: Option<String>,
    #[serde(default)]
    pub runtime_profile: Value,
}

/// Operator-managed identity update input for `PATCH /api/agents/:name`.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentIdentityPatch {
    pub identity: String,
}

/// Agent heartbeat input for `POST /api/agents/:name/heartbeat`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentHeartbeat {
    pub server: Option<String>,
    pub tmux_target: Option<String>,
    pub workspace_path: Option<String>,
}

/// Agent offline input for `POST /api/agents/:name/offline`.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentOffline {
    pub reason: Option<String>,
    #[serde(default = "default_clear_tmux")]
    pub clear_tmux: bool,
}

/// Serializable runtime handle returned by agent start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStartHandle {
    pub agent_id: String,
    pub backend: String,
    pub address: String,
    pub pane_id: Option<String>,
    pub pid: Option<u32>,
    pub session_name: String,
}

/// Result of starting a registered agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStartResult {
    pub agent: AgentRecord,
    pub handle: AgentStartHandle,
}

/// Serializable lifecycle report returned by operator stop/recovery actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLifecycleReport {
    pub method: String,
    pub archive_path: Option<String>,
    pub final_capture_sha: Option<String>,
}

/// Result of stopping a registered agent runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDownResult {
    pub agent: AgentRecord,
    pub report: AgentLifecycleReport,
}

/// Result of rebinding a registered agent runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRebindResult {
    pub agent: AgentRecord,
    pub handle: Option<AgentStartHandle>,
    pub rebound: bool,
}

/// Minimal runtime observation update for `POST /api/agents/:name/runtime`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentRuntimeUpdate {
    pub blocked: Option<bool>,
    pub reason: Option<String>,
    #[serde(alias = "activeNow")]
    pub active_now: Option<bool>,
    #[serde(alias = "activeDurationSec")]
    pub active_duration_sec: Option<i64>,
    #[serde(alias = "idleDurationSec")]
    pub idle_duration_sec: Option<i64>,
    #[serde(alias = "lastTmuxActivitySec")]
    pub last_tmux_activity_sec: Option<i64>,
    #[serde(alias = "workspacePath")]
    pub workspace_path: Option<String>,
    #[serde(alias = "mcpPresent")]
    pub mcp_present: Option<bool>,
}

/// Remote relay server heartbeat input for `POST /api/servers/heartbeat`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RelayServerHeartbeat {
    #[serde(default)]
    pub server: String,
    #[serde(default, rename = "instanceId", alias = "instance_id")]
    pub instance_id: Option<String>,
    #[serde(default, rename = "bootTs", alias = "boot_ts")]
    pub boot_ts: Option<i64>,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub sessions: Vec<String>,
}

/// Durable remote relay server state returned by heartbeat routes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayServerRecord {
    pub id: String,
    #[serde(rename = "instanceId")]
    pub instance_id: Option<String>,
    #[serde(rename = "bootTs")]
    pub boot_ts: Option<i64>,
    pub agents: Vec<String>,
    pub sessions: Vec<String>,
    pub agent_count: i64,
    pub online: bool,
    pub maintenance: bool,
    pub last_seen_at: i64,
    pub heartbeat_at: i64,
    pub updated_at: i64,
}

/// Remote relay delivery-event audit input.
#[derive(Debug, Clone, Deserialize)]
pub struct DeliveryEventInput {
    #[serde(rename = "type", alias = "event_type")]
    pub event_type: String,
    #[serde(default, rename = "messageId", alias = "message_id")]
    pub message_id: Option<String>,
    #[serde(default, rename = "queueEntryId", alias = "queue_entry_id")]
    pub queue_entry_id: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub context: Value,
}

/// One durable delivery event for relay audit inspection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryEventRecord {
    pub id: String,
    pub seq: i64,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(rename = "messageId")]
    pub message_id: Option<String>,
    #[serde(rename = "queueEntryId")]
    pub queue_entry_id: Option<String>,
    pub agent: Option<String>,
    pub target: Option<String>,
    pub reason: Option<String>,
    pub source: Option<String>,
    pub context: Value,
    pub created_at: i64,
}

/// One agent-chat-compatible relay wakeup stream event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayStreamEventRecord {
    pub seq: i64,
    pub event: String,
    pub payload: Value,
    pub created_at: i64,
}

fn default_matrix_room_trusted() -> bool {
    true
}

fn default_matrix_trust_reason() -> String {
    "managed".to_string()
}

/// Matrix bridge room mapping/trust input for the external bridge contract.
#[derive(Debug, Clone, Deserialize)]
pub struct MatrixBridgeRoomInput {
    #[serde(rename = "roomId", alias = "room_id")]
    pub room_id: String,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default = "default_matrix_room_trusted")]
    pub trusted: bool,
    #[serde(
        default = "default_matrix_trust_reason",
        rename = "trustReason",
        alias = "trust_reason"
    )]
    pub trust_reason: String,
    #[serde(default, rename = "inviterMxid", alias = "inviter_mxid")]
    pub inviter_mxid: Option<String>,
    #[serde(default)]
    pub members: Vec<String>,
}

/// Durable Matrix bridge room mapping/trust state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixBridgeRoomRecord {
    #[serde(rename = "roomId")]
    pub room_id: String,
    pub group: Option<String>,
    pub agent: Option<String>,
    pub trusted: bool,
    #[serde(rename = "trustReason")]
    pub trust_reason: String,
    #[serde(rename = "inviterMxid")]
    pub inviter_mxid: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Matrix inbound event accepted from an external bridge process.
#[derive(Debug, Clone, Deserialize)]
pub struct MatrixInboundMessageInput {
    #[serde(rename = "eventId", alias = "event_id")]
    pub event_id: String,
    #[serde(rename = "roomId", alias = "room_id")]
    pub room_id: String,
    #[serde(rename = "senderMxid", alias = "sender_mxid")]
    pub sender_mxid: String,
    #[serde(default)]
    pub from: Option<String>,
    pub body: String,
    #[serde(default)]
    pub mentions: Vec<String>,
    #[serde(default, rename = "replyTo", alias = "reply_to")]
    pub reply_to: Option<String>,
    #[serde(default, rename = "trustLevel", alias = "trust_level")]
    pub trust_level: Option<String>,
}

/// Result for one Matrix inbound event route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixInboundMessageResult {
    pub ok: bool,
    pub duplicate: bool,
    pub ignored: bool,
    pub route: String,
    #[serde(rename = "eventId")]
    pub event_id: String,
    #[serde(rename = "messageId")]
    pub message_id: Option<String>,
    pub message: Option<InboxMessage>,
}

/// Filters for the agent-chat-compatible pool scheduler view.
#[derive(Debug, Clone, Default)]
pub struct SchedulerPoolFilters {
    pub role: Option<String>,
    pub capability: Option<String>,
    pub state: Option<String>,
}

/// One agent row in the scheduler pool view.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchedulerPoolAgent {
    pub name: String,
    pub role: Option<String>,
    pub capability: String,
    pub online: bool,
    pub busy: bool,
}

/// Agent-chat-compatible pool response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchedulerPoolSnapshot {
    pub grid: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, Vec<SchedulerPoolAgent>>,
    >,
    pub counts: std::collections::BTreeMap<String, std::collections::BTreeMap<String, usize>>,
    pub total: usize,
    pub agents: Vec<SchedulerPoolAgent>,
}

/// Operator dispatch input for `POST /api/dispatch`.
#[derive(Debug, Clone, Deserialize)]
pub struct SchedulerDispatchInput {
    pub role: String,
    #[serde(default)]
    pub capability: Option<String>,
    #[serde(default)]
    pub task: Option<Value>,
    #[serde(default)]
    pub room: Option<String>,
}

/// Operator release input for `POST /api/dispatch/release`.
#[derive(Debug, Clone, Deserialize)]
pub struct SchedulerReleaseInput {
    pub agent: String,
}

/// Durable scheduler reservation in the HTTP response shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchedulerReservation {
    pub id: String,
    pub role: String,
    pub tier: String,
    pub agent: Option<String>,
    #[serde(rename = "provisionedName")]
    pub provisioned_name: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(default)]
    pub runtime: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
    #[serde(
        rename = "releasedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub released_at: Option<i64>,
}

/// Result of `POST /api/dispatch`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchedulerDispatchResult {
    pub status: String,
    pub role: String,
    pub tier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservation: Option<SchedulerReservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    #[serde(
        rename = "queueDepth",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub queue_depth: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub runtime: Value,
}

/// Result of `POST /api/dispatch/release`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchedulerReleaseResult {
    pub status: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservation: Option<SchedulerReservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
}

/// One agent-chat-compatible product task comment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentChatTaskComment {
    pub author: String,
    pub text: String,
    pub ts: String,
}

/// A live product task in the agent-chat-compatible `/api/tasks` shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatTaskRecord {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub priority: String,
    pub granularity: String,
    pub assignee: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub heartbeat_at: Option<String>,
    pub waiting_reason: Option<String>,
    pub waiting_until: Option<String>,
    pub parent_id: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    pub health: Option<Value>,
    #[serde(default)]
    pub comments: Vec<AgentChatTaskComment>,
}

/// Operator input for creating a live product task.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentChatTaskCreateInput {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub granularity: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

/// Operator patch input for `/api/tasks/:id`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentChatTaskPatchInput {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub granularity: Option<String>,
    #[serde(default)]
    pub assignee: Option<Option<String>>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    #[serde(default)]
    pub parent_id: Option<Option<String>>,
}

/// Agent execution patch input for `/api/tasks/:id/execution`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentChatTaskExecutionInput {
    #[serde(default)]
    pub heartbeat_at: Option<bool>,
    #[serde(default)]
    pub waiting_reason: Option<Option<String>>,
    #[serde(default)]
    pub waiting_until: Option<Option<String>>,
}

/// Agent transition input for `/api/tasks/:id/transition`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentChatTaskTransitionInput {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub waiting_reason: Option<String>,
    #[serde(default)]
    pub waiting_until: Option<String>,
}

/// Operator comment input for `/api/tasks/:id/comments`.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentChatTaskCommentInput {
    #[serde(default)]
    pub author: Option<String>,
    pub text: String,
}

/// Filters for agent-chat-compatible product task listing.
#[derive(Debug, Clone, Default)]
pub struct AgentChatTaskListFilters {
    pub assignee: Option<String>,
    pub statuses: Vec<String>,
    pub priority: Option<String>,
    pub label: Option<String>,
    pub offset: usize,
    pub limit: Option<usize>,
}

/// A live task-graph record in the agent-chat-compatible `/api/task-graphs`
/// shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentChatTaskGraphRecord {
    pub id: String,
    pub owner: String,
    pub label: String,
    pub status: String,
    #[serde(default)]
    pub nodes: std::collections::BTreeMap<String, AgentChatTaskGraphNode>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(
        rename = "completedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub completed_at: Option<String>,
}

/// One node inside a live task graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentChatTaskGraphNode {
    pub id: String,
    pub assignee: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    #[serde(
        default,
        rename = "schedulerReservationId",
        alias = "scheduler_reservation_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub scheduler_reservation_id: Option<String>,
    #[serde(
        default,
        rename = "schedulerTicket",
        alias = "scheduler_ticket",
        skip_serializing_if = "Option::is_none"
    )]
    pub scheduler_ticket: Option<String>,
    #[serde(
        default,
        rename = "schedulerStatus",
        alias = "scheduler_status",
        skip_serializing_if = "Option::is_none"
    )]
    pub scheduler_status: Option<String>,
    #[serde(
        default,
        rename = "provisionedName",
        alias = "provisioned_name",
        skip_serializing_if = "Option::is_none"
    )]
    pub provisioned_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<Value>,
    pub description: String,
    #[serde(default, alias = "dependsOn")]
    pub depends_on: Vec<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Value>,
    #[serde(
        default,
        rename = "message_id",
        alias = "messageId",
        skip_serializing_if = "Option::is_none"
    )]
    pub message_id: Option<String>,
    #[serde(rename = "startedAt", default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(
        rename = "dispatchedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub dispatched_at: Option<String>,
    #[serde(
        rename = "completedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub completed_at: Option<String>,
}

/// Operator input for creating a live task graph.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentChatTaskGraphCreateInput {
    #[serde(default)]
    pub id: Option<String>,
    pub owner: String,
    pub label: String,
    #[serde(default)]
    pub nodes: std::collections::BTreeMap<String, AgentChatTaskGraphNodeInput>,
}

/// Operator input for one task graph node in a create request.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentChatTaskGraphNodeInput {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub capability: Option<String>,
    pub description: String,
    #[serde(default, alias = "dependsOn")]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub condition: Option<Value>,
}

/// Agent patch input for `/api/task-graphs/:graph/nodes/:node`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentChatTaskGraphNodePatchInput {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Result of handling a task-graph result/failed direct message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentChatTaskGraphMessageResult {
    pub handled: bool,
    #[serde(rename = "graphId")]
    pub graph_id: String,
    #[serde(rename = "nodeId")]
    pub node_id: String,
    pub status: String,
    pub graph: AgentChatTaskGraphRecord,
}

/// A direct inbox message in the agent-chat-compatible read shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxMessage {
    pub id: String,
    pub ts: i64,
    pub at: String,
    pub time: String,
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub priority: String,
    pub summary: String,
    pub full: String,
    #[serde(default)]
    pub mentions: Vec<String>,
    #[serde(default)]
    pub attachments: Vec<Value>,
    pub reply_to: Option<String>,
    pub group: Option<String>,
    pub source: String,
    #[serde(rename = "sourceRoom")]
    pub source_room: Option<String>,
    #[serde(rename = "senderMxid")]
    pub sender_mxid: Option<String>,
    #[serde(rename = "trustLevel")]
    pub trust_level: Option<String>,
    #[serde(rename = "fromId")]
    pub from_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
}

/// A durable local group record, shaped for agent-chat group parity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupRecord {
    pub name: String,
    pub members: Vec<String>,
    pub created_at: i64,
}

/// Group creation input for `POST /api/groups`.
#[derive(Debug, Clone, Deserialize)]
pub struct GroupCreateInput {
    pub name: String,
    #[serde(default)]
    pub members: Vec<String>,
}

/// Group membership update input for `POST /api/groups/:name/members`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GroupMemberUpdate {
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
}

/// Operator/bridge input for accepting one durable group message.
#[derive(Debug, Clone, Deserialize)]
pub struct GroupMessageInput {
    #[serde(default, alias = "id")]
    pub message_id: Option<String>,
    #[serde(default)]
    pub ts: Option<i64>,
    pub from: String,
    pub group: String,
    #[serde(default, rename = "type", alias = "messageType")]
    pub message_type: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    pub summary: String,
    #[serde(default)]
    pub full: String,
    #[serde(default)]
    pub mentions: Vec<String>,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub schema: Option<Value>,
    #[serde(default)]
    pub attachments: Vec<Value>,
}

/// Group cursor advancement mode.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum GroupReadAdvance {
    #[default]
    None,
    All,
}

impl GroupReadAdvance {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::All => "all",
        }
    }
}

/// Request for reading group message history for one member.
#[derive(Debug, Clone)]
pub struct GroupReadRequest {
    pub group: String,
    pub agent_id: String,
    pub limit: usize,
    pub unread_limit: Option<usize>,
    pub advance: GroupReadAdvance,
}

/// Agent-chat-style split group history response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupReadResult {
    pub group: String,
    pub unread: Vec<InboxMessage>,
    pub read: Vec<InboxMessage>,
    pub unread_total: usize,
    pub unread_returned: usize,
    pub unread_omitted: usize,
    pub advance: String,
}

/// Operator/bridge input for accepting one durable direct message.
#[derive(Debug, Clone, Deserialize)]
pub struct DirectMessageInput {
    #[serde(default, alias = "id")]
    pub message_id: Option<String>,
    #[serde(default)]
    pub ts: Option<i64>,
    pub from: String,
    pub to: String,
    #[serde(default, rename = "type", alias = "messageType")]
    pub message_type: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    pub summary: String,
    #[serde(default)]
    pub full: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default, rename = "sourceRoom", alias = "source_room")]
    pub source_room: Option<String>,
    #[serde(default, rename = "senderMxid", alias = "sender_mxid")]
    pub sender_mxid: Option<String>,
    #[serde(default, rename = "trustLevel", alias = "trust_level")]
    pub trust_level: Option<String>,
    #[serde(default, rename = "fromId", alias = "from_id")]
    pub from_id: Option<String>,
    #[serde(default)]
    pub schema: Option<Value>,
    #[serde(default)]
    pub attachments: Vec<Value>,
}

fn default_clear_tmux() -> bool {
    true
}

/// The seam: deliver engine events and read the bits the tools need.
#[async_trait::async_trait]
pub trait RunHost: Send + Sync {
    /// Subscribe to the host's live event broadcast — the SSE live tail (P1).
    /// LOSSY + bounded: a slow subscriber lags (and is realigned with a snapshot)
    /// rather than backpressuring the engine. The surface filters by `run_id`.
    /// Not `async` — taking a receiver is synchronous.
    fn subscribe_events(&self) -> broadcast::Receiver<LiveEvent>;

    /// List every run with its current status — the `GET /runs` overview (P1).
    ///
    /// # Errors
    /// [`CoreError`] on a store failure.
    async fn list_runs(&self) -> Result<Vec<RunSummary>, CoreError>;

    /// Create and start a run of `flow` (`"draft"`/`"execute"`) as `run_id` with
    /// an initial `context`, executing from the start node to the first park (or
    /// completion). The daemon's `POST /runs` control path; the host records the
    /// run + resolves its graph (store-side, so the surface stays store-free).
    ///
    /// # Errors
    /// [`CoreError`] on an unknown flow, a store/handler/engine failure.
    async fn start_workflow(
        &self,
        flow: &str,
        run_id: &RunId,
        context: Value,
    ) -> Result<RunProgress, CoreError>;

    /// Deliver an event to the engine, advancing the run.
    ///
    /// # Errors
    /// [`CoreError`] on a store/handler/engine failure.
    async fn deliver(&self, event: EngineEvent) -> Result<RunProgress, CoreError>;

    /// Snapshot a run's state, or `None` if unknown.
    ///
    /// # Errors
    /// [`CoreError`] on a store failure.
    async fn run_snapshot(&self, run_id: &RunId) -> Result<Option<RunSnapshot>, CoreError>;

    /// The open task for `(run, node)`, or `None` if there is none.
    ///
    /// # Errors
    /// [`CoreError`] on a store failure.
    async fn open_task(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
    ) -> Result<Option<TaskAssignment>, CoreError>;

    /// `(expected, got)` verdict counts for a review run.
    ///
    /// # Errors
    /// [`CoreError`] on a store failure.
    async fn review_counts(&self, review_run_id: &ReviewRunId)
    -> Result<(usize, usize), CoreError>;

    /// A run's events with `seq > after_seq`, in `seq` order — the SSE replay
    /// cursor. The production host reads `event_repo::read_from`.
    ///
    /// # Errors
    /// [`CoreError`] on a store failure.
    async fn events_from(
        &self,
        run_id: &RunId,
        after_seq: i64,
    ) -> Result<Vec<EventRecord>, CoreError>;

    /// Register or update a local agent record.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn register_agent(&self, input: AgentRegistration) -> Result<AgentRecord, CoreError>;

    /// List local agent records.
    ///
    /// # Errors
    /// [`CoreError`] on store failure.
    async fn list_agents(&self) -> Result<Vec<AgentRecord>, CoreError>;

    /// Inspect one local agent by name, or `None` if unknown.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn get_agent(&self, name: &str) -> Result<Option<AgentRecord>, CoreError>;

    /// Update the operator-facing identity text for one local agent. `None`
    /// means unknown agent.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn update_agent_identity(
        &self,
        name: &str,
        identity: &str,
    ) -> Result<Option<AgentRecord>, CoreError>;

    /// Mark a local agent online and update heartbeat metadata; returns
    /// `(record, created)`.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn heartbeat_agent(
        &self,
        name: &str,
        input: AgentHeartbeat,
    ) -> Result<(AgentRecord, bool), CoreError>;

    /// Mark a local agent offline. `None` means unknown agent.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn mark_agent_offline(
        &self,
        name: &str,
        input: AgentOffline,
    ) -> Result<Option<AgentRecord>, CoreError>;

    /// Start a registered local agent through the configured backend. `None`
    /// means unknown agent.
    ///
    /// # Errors
    /// [`CoreError`] on validation, backend, or store failure.
    async fn start_agent(&self, name: &str) -> Result<Option<AgentStartResult>, CoreError>;

    /// Stop a registered local agent runtime and mark it offline. `None` means
    /// unknown agent.
    ///
    /// # Errors
    /// [`CoreError`] on validation, backend, or store failure.
    async fn down_agent(&self, name: &str) -> Result<Option<AgentDownResult>, CoreError>;

    /// Rebind a registered local agent runtime from its stored target. `None`
    /// means unknown agent.
    ///
    /// # Errors
    /// [`CoreError`] on validation, backend, or store failure.
    async fn rebind_agent(&self, name: &str) -> Result<Option<AgentRebindResult>, CoreError>;

    /// Record a runtime observation for an existing agent. `None` means unknown
    /// agent.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn update_agent_runtime(
        &self,
        name: &str,
        input: AgentRuntimeUpdate,
    ) -> Result<Option<Value>, CoreError>;

    /// Record a remote relay server heartbeat.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn record_relay_server_heartbeat(
        &self,
        input: RelayServerHeartbeat,
    ) -> Result<RelayServerRecord, CoreError>;

    /// Append one delivery-event audit row.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn append_delivery_event(
        &self,
        input: DeliveryEventInput,
    ) -> Result<DeliveryEventRecord, CoreError>;

    /// List delivery-event audit rows for one agent.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn list_delivery_events(
        &self,
        agent: &str,
        limit: usize,
    ) -> Result<Vec<DeliveryEventRecord>, CoreError>;

    /// Relay wakeup stream events with `seq > after_seq`.
    ///
    /// # Errors
    /// [`CoreError`] on store failure.
    async fn relay_stream_events(
        &self,
        after_seq: i64,
    ) -> Result<Vec<RelayStreamEventRecord>, CoreError>;

    /// Register or update one trusted Matrix room mapping for the external
    /// bridge contract.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn upsert_matrix_bridge_room(
        &self,
        input: MatrixBridgeRoomInput,
    ) -> Result<MatrixBridgeRoomRecord, CoreError>;

    /// Inspect one Matrix room mapping.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn get_matrix_bridge_room(
        &self,
        room_id: &str,
    ) -> Result<Option<MatrixBridgeRoomRecord>, CoreError>;

    /// Accept one inbound Matrix event from an external bridge process.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn post_matrix_inbound_message(
        &self,
        input: MatrixInboundMessageInput,
    ) -> Result<MatrixInboundMessageResult, CoreError>;

    /// Read the agent-chat-compatible pool scheduler view.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn scheduler_pool(
        &self,
        filters: SchedulerPoolFilters,
    ) -> Result<SchedulerPoolSnapshot, CoreError>;

    /// Reserve, queue, or plan provision for one scheduler dispatch.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn scheduler_dispatch(
        &self,
        input: SchedulerDispatchInput,
        max_per_cell: i64,
    ) -> Result<SchedulerDispatchResult, CoreError>;

    /// Release one reserved scheduler agent and drain a queued ticket if present.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn scheduler_release(
        &self,
        input: SchedulerReleaseInput,
    ) -> Result<SchedulerReleaseResult, CoreError>;

    /// Create a live agent-chat-compatible product task.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn create_agent_chat_task(
        &self,
        input: AgentChatTaskCreateInput,
    ) -> Result<AgentChatTaskRecord, CoreError>;

    /// List live product tasks.
    ///
    /// # Errors
    /// [`CoreError`] on store failure.
    async fn list_agent_chat_tasks(
        &self,
        filters: AgentChatTaskListFilters,
    ) -> Result<Vec<AgentChatTaskRecord>, CoreError>;

    /// Inspect one live product task.
    ///
    /// # Errors
    /// [`CoreError`] on store failure.
    async fn get_agent_chat_task(&self, id: &str)
    -> Result<Option<AgentChatTaskRecord>, CoreError>;

    /// Patch one live product task. `None` means unknown task.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn update_agent_chat_task(
        &self,
        id: &str,
        input: AgentChatTaskPatchInput,
    ) -> Result<Option<AgentChatTaskRecord>, CoreError>;

    /// Patch one live task's execution metadata. `None` means unknown task.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn update_agent_chat_task_execution(
        &self,
        id: &str,
        input: AgentChatTaskExecutionInput,
    ) -> Result<Option<AgentChatTaskRecord>, CoreError>;

    /// Transition one live task. `None` means unknown task.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn transition_agent_chat_task(
        &self,
        id: &str,
        input: AgentChatTaskTransitionInput,
    ) -> Result<Option<AgentChatTaskRecord>, CoreError>;

    /// Add a task comment. `None` means unknown task.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn add_agent_chat_task_comment(
        &self,
        id: &str,
        input: AgentChatTaskCommentInput,
    ) -> Result<Option<AgentChatTaskRecord>, CoreError>;

    /// Delete one live product task. `None` means unknown task.
    ///
    /// # Errors
    /// [`CoreError`] on store failure.
    async fn delete_agent_chat_task(
        &self,
        id: &str,
    ) -> Result<Option<AgentChatTaskRecord>, CoreError>;

    /// Create and immediately advance a live agent-chat-compatible task graph.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn create_agent_chat_task_graph(
        &self,
        input: AgentChatTaskGraphCreateInput,
    ) -> Result<AgentChatTaskGraphRecord, CoreError>;

    /// List live task graphs.
    ///
    /// # Errors
    /// [`CoreError`] on store failure.
    async fn list_agent_chat_task_graphs(
        &self,
        status: Option<String>,
    ) -> Result<Vec<AgentChatTaskGraphRecord>, CoreError>;

    /// Inspect one live task graph.
    ///
    /// # Errors
    /// [`CoreError`] on store failure.
    async fn get_agent_chat_task_graph(
        &self,
        id: &str,
    ) -> Result<Option<AgentChatTaskGraphRecord>, CoreError>;

    /// Cancel one live task graph. `None` means unknown graph.
    ///
    /// # Errors
    /// [`CoreError`] on store failure.
    async fn delete_agent_chat_task_graph(
        &self,
        id: &str,
    ) -> Result<Option<AgentChatTaskGraphRecord>, CoreError>;

    /// Patch one task-graph node and advance the graph. `None` means unknown
    /// graph or node.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn update_agent_chat_task_graph_node(
        &self,
        graph_id: &str,
        node_id: &str,
        input: AgentChatTaskGraphNodePatchInput,
    ) -> Result<Option<(AgentChatTaskGraphRecord, AgentChatTaskGraphNode)>, CoreError>;

    /// Handle an inbound direct task-graph result/failed schema message.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn handle_agent_chat_task_graph_message(
        &self,
        from: &str,
        reply_to: Option<String>,
        schema: Option<Value>,
    ) -> Result<Option<AgentChatTaskGraphMessageResult>, CoreError>;

    /// Create a durable group.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn create_group(&self, input: GroupCreateInput) -> Result<GroupRecord, CoreError>;

    /// List durable groups.
    ///
    /// # Errors
    /// [`CoreError`] on store failure.
    async fn list_groups(&self) -> Result<Vec<GroupRecord>, CoreError>;

    /// Inspect one group by name.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn get_group(&self, name: &str) -> Result<Option<GroupRecord>, CoreError>;

    /// Update group members. `None` means unknown group.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn update_group_members(
        &self,
        name: &str,
        input: GroupMemberUpdate,
    ) -> Result<Option<GroupRecord>, CoreError>;

    /// Delete one group. `None` means unknown group.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn delete_group(&self, name: &str) -> Result<Option<GroupRecord>, CoreError>;

    /// Accept one durable direct message for a target agent.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn post_direct_message(
        &self,
        input: DirectMessageInput,
    ) -> Result<InboxMessage, CoreError>;

    /// Accept one durable group message.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn post_group_message(&self, input: GroupMessageInput)
    -> Result<InboxMessage, CoreError>;

    /// Read unread direct messages for `agent_id`; `drain=true` marks returned
    /// direct and group mention rows read.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn check_inbox(
        &self,
        agent_id: &str,
        drain: bool,
    ) -> Result<Vec<InboxMessage>, CoreError>;

    /// Read one group's message history for a member.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn read_group_messages(
        &self,
        input: GroupReadRequest,
    ) -> Result<GroupReadResult, CoreError>;

    async fn native_runtime_snapshot(
        &self,
        _session_id: &RuntimeSessionId,
    ) -> Result<Option<RuntimeView>, CoreError> {
        Err(CoreError::Backend(
            "native runtime is not configured".to_string(),
        ))
    }

    async fn native_runtime_events(
        &self,
        _session_id: &RuntimeSessionId,
        _after_event_index: u64,
        _limit: u32,
    ) -> Result<Vec<RuntimeEvent>, CoreError> {
        Err(CoreError::Backend(
            "native runtime is not configured".to_string(),
        ))
    }

    async fn native_runtime_wait(
        &self,
        _request: RuntimeWaitRequest,
    ) -> Result<RuntimeView, CoreError> {
        Err(CoreError::Backend(
            "native runtime is not configured".to_string(),
        ))
    }

    async fn native_runtime_send_text(
        &self,
        _admission: CapabilityAdmission,
        _request: RuntimeTextInput,
    ) -> Result<RuntimeInputAck, CoreError> {
        Err(CoreError::Backend(
            "native runtime is not configured".to_string(),
        ))
    }

    async fn native_runtime_send_key(
        &self,
        _admission: CapabilityAdmission,
        _request: RuntimeKeyInput,
    ) -> Result<RuntimeInputAck, CoreError> {
        Err(CoreError::Backend(
            "native runtime is not configured".to_string(),
        ))
    }

    async fn native_runtime_resize(
        &self,
        _admission: CapabilityAdmission,
        _request: RuntimeResizeRequest,
    ) -> Result<RuntimeSnapshot, CoreError> {
        Err(CoreError::Backend(
            "native runtime is not configured".to_string(),
        ))
    }

    async fn native_runtime_interrupt(
        &self,
        _admission: CapabilityAdmission,
        _request: RuntimeKeyInput,
    ) -> Result<RuntimeInputAck, CoreError> {
        Err(CoreError::Backend(
            "native runtime is not configured".to_string(),
        ))
    }

    async fn native_runtime_shutdown(
        &self,
        _admission: CapabilityAdmission,
        _request: RuntimeShutdownRequest,
    ) -> Result<RuntimeShutdownReport, CoreError> {
        Err(CoreError::Backend(
            "native runtime is not configured".to_string(),
        ))
    }
}
