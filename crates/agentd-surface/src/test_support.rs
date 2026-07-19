//! `FakeRunHost` — an in-memory [`RunHost`] for tool tests. Records delivered
//! events and replays scripted `RunProgress`; serves set snapshots / tasks /
//! review counts. Compiled only under `test-support`/`cfg(test)`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Mutex;

use agentd_core::CoreError;
use agentd_core::types::{NodeId, ReviewRunId, RunId};
use agentd_core::{EngineEvent, RunProgress};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::host::{
    AgentChatTaskComment, AgentChatTaskCommentInput, AgentChatTaskCreateInput,
    AgentChatTaskExecutionInput, AgentChatTaskGraphCreateInput, AgentChatTaskGraphMessageResult,
    AgentChatTaskGraphNode, AgentChatTaskGraphNodePatchInput, AgentChatTaskGraphRecord,
    AgentChatTaskListFilters, AgentChatTaskPatchInput, AgentChatTaskRecord,
    AgentChatTaskTransitionInput, AgentDownResult, AgentHeartbeat, AgentLifecycleReport,
    AgentOffline, AgentRebindResult, AgentRecord, AgentRegistration, AgentRuntimeUpdate,
    AgentStartHandle, AgentStartResult, DeliveryEventInput, DeliveryEventRecord,
    DirectMessageInput, EventRecord, GroupCreateInput, GroupMemberUpdate, GroupMessageInput,
    GroupReadAdvance, GroupReadRequest, GroupReadResult, GroupRecord, InboxMessage, LiveEvent,
    MatrixBridgeRoomInput, MatrixBridgeRoomRecord, MatrixInboundMessageInput,
    MatrixInboundMessageResult, MatrixOutboxCursorInput, RelayServerHeartbeat, RelayServerRecord,
    RelayStreamEventRecord, RunHost, RunSnapshot, RunSummary, SchedulerDispatchInput,
    SchedulerDispatchResult, SchedulerPoolAgent, SchedulerPoolFilters, SchedulerPoolSnapshot,
    SchedulerReleaseInput, SchedulerReleaseResult, SchedulerReservation, TaskAssignment,
};

/// Scripted, recording [`RunHost`] for tests.
#[derive(Debug)]
pub struct FakeRunHost {
    snapshots: Mutex<HashMap<String, RunSnapshot>>,
    tasks: Mutex<HashMap<(String, String), TaskAssignment>>,
    delivered: Mutex<Vec<EngineEvent>>,
    progress: Mutex<VecDeque<RunProgress>>,
    review_counts: Mutex<HashMap<String, (usize, usize)>>,
    events: Mutex<HashMap<String, Vec<EventRecord>>>,
    started: Mutex<Vec<(String, String, Value)>>,
    live_tx: broadcast::Sender<LiveEvent>,
    runs: Mutex<Vec<RunSummary>>,
    list_runs_fails: Mutex<bool>,
    agents: Mutex<HashMap<String, AgentRecord>>,
    agent_chat_tasks: Mutex<HashMap<String, AgentChatTaskRecord>>,
    agent_chat_task_order: Mutex<Vec<String>>,
    agent_chat_task_graphs: Mutex<HashMap<String, AgentChatTaskGraphRecord>>,
    agent_chat_task_graph_order: Mutex<Vec<String>>,
    scheduler_reservations: Mutex<Vec<SchedulerReservation>>,
    scheduler_queue: Mutex<Vec<FakeSchedulerTicket>>,
    groups: Mutex<HashMap<String, GroupRecord>>,
    inbox: Mutex<Vec<FakeInboxEntry>>,
    group_mention_reads: Mutex<HashSet<(String, String)>>,
    group_message_reads: Mutex<HashSet<(String, String, String)>>,
    relay_servers: Mutex<HashMap<String, RelayServerRecord>>,
    delivery_events: Mutex<Vec<DeliveryEventRecord>>,
    stream_events: Mutex<Vec<RelayStreamEventRecord>>,
    matrix_rooms: Mutex<HashMap<String, MatrixBridgeRoomRecord>>,
    matrix_events: Mutex<HashMap<String, MatrixInboundMessageResult>>,
    matrix_outbox_cursors: Mutex<HashMap<String, i64>>,
}

#[derive(Debug, Clone)]
struct FakeInboxEntry {
    message: InboxMessage,
    read: bool,
}

#[derive(Debug, Clone)]
struct FakeSchedulerTicket {
    ticket: String,
    role: String,
    tier: String,
    task: Option<Value>,
    room: Option<String>,
    status: String,
}

#[derive(Debug, Clone)]
struct FakeSchedulerReservationInput {
    role: String,
    tier: String,
    agent: Option<String>,
    provisioned_name: Option<String>,
    status: String,
    task: Option<Value>,
    room: Option<String>,
    runtime: Value,
    ticket: Option<String>,
}

impl Default for FakeRunHost {
    fn default() -> Self {
        Self {
            snapshots: Mutex::default(),
            tasks: Mutex::default(),
            delivered: Mutex::default(),
            progress: Mutex::default(),
            review_counts: Mutex::default(),
            events: Mutex::default(),
            started: Mutex::default(),
            live_tx: broadcast::channel(64).0,
            runs: Mutex::default(),
            list_runs_fails: Mutex::default(),
            agents: Mutex::default(),
            agent_chat_tasks: Mutex::default(),
            agent_chat_task_order: Mutex::default(),
            agent_chat_task_graphs: Mutex::default(),
            agent_chat_task_graph_order: Mutex::default(),
            scheduler_reservations: Mutex::default(),
            scheduler_queue: Mutex::default(),
            groups: Mutex::default(),
            inbox: Mutex::default(),
            group_mention_reads: Mutex::default(),
            group_message_reads: Mutex::default(),
            relay_servers: Mutex::default(),
            delivery_events: Mutex::default(),
            stream_events: Mutex::default(),
            matrix_rooms: Mutex::default(),
            matrix_events: Mutex::default(),
            matrix_outbox_cursors: Mutex::default(),
        }
    }
}

impl FakeRunHost {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish a live event to subscribers (test driver for the SSE live tail).
    pub fn publish(&self, event: LiveEvent) {
        let _ = self.live_tx.send(event);
    }

    /// Set the runs returned by `list_runs` (the `GET /runs` overview).
    pub fn set_runs(&self, runs: Vec<RunSummary>) {
        *self.runs.lock().expect("runs lock") = runs;
    }

    /// Make `list_runs` return an error (for the `GET /runs` 500 path).
    pub fn fail_list_runs(&self) {
        *self.list_runs_fails.lock().expect("list_runs_fails lock") = true;
    }

    /// Set the snapshot returned by `run_snapshot` for `run_id`.
    pub fn set_snapshot(&self, run_id: &str, snapshot: RunSnapshot) {
        self.snapshots
            .lock()
            .expect("snapshots lock")
            .insert(run_id.to_string(), snapshot);
    }

    /// Set the open task returned by `open_task` for `(run_id, node_id)`.
    pub fn set_task(&self, run_id: &str, node_id: &str, task: TaskAssignment) {
        self.tasks
            .lock()
            .expect("tasks lock")
            .insert((run_id.to_string(), node_id.to_string()), task);
    }

    /// Queue one scripted `deliver` result (FIFO).
    pub fn push_progress(&self, progress: RunProgress) {
        self.progress
            .lock()
            .expect("progress lock")
            .push_back(progress);
    }

    /// Set `(expected, got)` returned by `review_counts` for `review_run_id`.
    pub fn set_review_counts(&self, review_run_id: &str, counts: (usize, usize)) {
        self.review_counts
            .lock()
            .expect("review_counts lock")
            .insert(review_run_id.to_string(), counts);
    }

    /// Set the event log returned by `events_from` for `run_id`.
    pub fn set_events(&self, run_id: &str, events: Vec<EventRecord>) {
        self.events
            .lock()
            .expect("events lock")
            .insert(run_id.to_string(), events);
    }

    /// Every event delivered so far, in order.
    #[must_use]
    pub fn delivered(&self) -> Vec<EngineEvent> {
        self.delivered.lock().expect("delivered lock").clone()
    }

    /// Every `(flow, run_id, context)` `start_workflow` call so far, in order.
    #[must_use]
    pub fn started(&self) -> Vec<(String, String, Value)> {
        self.started.lock().expect("started lock").clone()
    }

    /// Push one unread direct message into the fake inbox.
    pub fn push_inbox_message(&self, message: InboxMessage) {
        self.inbox.lock().expect("inbox lock").push(FakeInboxEntry {
            message,
            read: false,
        });
    }

    pub fn set_stream_events(&self, events: Vec<Value>) {
        let records = events
            .into_iter()
            .map(|mut value| {
                let seq = value.get("seq").and_then(Value::as_i64).unwrap_or_default();
                let event = value
                    .get("event")
                    .and_then(Value::as_str)
                    .unwrap_or("message")
                    .to_string();
                if let Value::Object(ref mut object) = value {
                    object.remove("event");
                }
                RelayStreamEventRecord {
                    seq,
                    event,
                    payload: value,
                    created_at: seq,
                }
            })
            .collect();
        *self.stream_events.lock().expect("stream events lock") = records;
    }

    fn fake_insert_scheduler_reservation(
        &self,
        input: FakeSchedulerReservationInput,
    ) -> SchedulerReservation {
        let mut reservations = self.scheduler_reservations.lock().expect("scheduler lock");
        let next = reservations.len() + 1;
        let reservation = SchedulerReservation {
            id: format!("sched_res_1_{next}"),
            role: input.role,
            tier: input.tier,
            agent: input.agent,
            provisioned_name: input.provisioned_name,
            status: input.status,
            task: input.task,
            room: input.room,
            runtime: input.runtime,
            ticket: input.ticket,
            created_at: i64::try_from(next).unwrap_or(i64::MAX),
            updated_at: i64::try_from(next).unwrap_or(i64::MAX),
            released_at: None,
        };
        reservations.push(reservation.clone());
        reservation
    }
}

#[allow(clippy::too_many_lines)]
#[async_trait::async_trait]
impl RunHost for FakeRunHost {
    fn subscribe_events(&self) -> broadcast::Receiver<LiveEvent> {
        self.live_tx.subscribe()
    }

    async fn list_runs(&self) -> Result<Vec<RunSummary>, CoreError> {
        if *self.list_runs_fails.lock().expect("list_runs_fails lock") {
            return Err(CoreError::Store("injected list_runs failure".to_string()));
        }
        Ok(self.runs.lock().expect("runs lock").clone())
    }

    async fn start_workflow(
        &self,
        flow: &str,
        run_id: &RunId,
        context: Value,
    ) -> Result<RunProgress, CoreError> {
        self.started.lock().expect("started lock").push((
            flow.to_string(),
            run_id.as_str().to_string(),
            context,
        ));
        Ok(self
            .progress
            .lock()
            .expect("progress lock")
            .pop_front()
            .unwrap_or_else(|| RunProgress::Finished {
                run_id: run_id.clone(),
            }))
    }

    async fn deliver(&self, event: EngineEvent) -> Result<RunProgress, CoreError> {
        self.delivered.lock().expect("delivered lock").push(event);
        Ok(self
            .progress
            .lock()
            .expect("progress lock")
            .pop_front()
            .unwrap_or_else(|| RunProgress::Ignored {
                reason: "no scripted progress".to_string(),
            }))
    }

    async fn run_snapshot(&self, run_id: &RunId) -> Result<Option<RunSnapshot>, CoreError> {
        Ok(self
            .snapshots
            .lock()
            .expect("snapshots lock")
            .get(run_id.as_str())
            .cloned())
    }

    async fn open_task(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
    ) -> Result<Option<TaskAssignment>, CoreError> {
        Ok(self
            .tasks
            .lock()
            .expect("tasks lock")
            .get(&(run_id.as_str().to_string(), node_id.as_str().to_string()))
            .cloned())
    }

    async fn review_counts(
        &self,
        review_run_id: &ReviewRunId,
    ) -> Result<(usize, usize), CoreError> {
        Ok(self
            .review_counts
            .lock()
            .expect("review_counts lock")
            .get(review_run_id.as_str())
            .copied()
            .unwrap_or((0, 0)))
    }

    async fn events_from(
        &self,
        run_id: &RunId,
        after_seq: i64,
    ) -> Result<Vec<EventRecord>, CoreError> {
        Ok(self
            .events
            .lock()
            .expect("events lock")
            .get(run_id.as_str())
            .into_iter()
            .flatten()
            .filter(|e| e.seq > after_seq)
            .cloned()
            .collect())
    }

    async fn register_agent(&self, input: AgentRegistration) -> Result<AgentRecord, CoreError> {
        let name = normalize_agent_name(&input.name)?;
        let status = if input
            .tmux_target
            .as_deref()
            .is_some_and(|v| !v.trim().is_empty())
        {
            "online"
        } else {
            "offline"
        };
        let record = AgentRecord {
            id: name.clone(),
            name: name.clone(),
            role: input.role,
            capability: input.capability,
            runtime: input.runtime,
            model: input.model,
            tmux_target: input.tmux_target,
            home_dir: input.home_dir,
            workdir: input.workdir,
            state_dir: input.state_dir,
            server: input.server,
            status: status.to_string(),
            offline_reason: (status == "offline").then(|| "offline".to_string()),
            last_seen_at: (status == "online").then_some(1),
            registered_at: 1,
            updated_at: 1,
            runtime_profile: if input.runtime_profile.is_null() {
                serde_json::json!({})
            } else {
                input.runtime_profile
            },
            runtime_state: serde_json::json!({}),
        };
        self.agents
            .lock()
            .expect("agents lock")
            .insert(name, record.clone());
        Ok(record)
    }

    async fn list_agents(&self) -> Result<Vec<AgentRecord>, CoreError> {
        let mut agents = self
            .agents
            .lock()
            .expect("agents lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        agents.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(agents)
    }

    async fn get_agent(&self, name: &str) -> Result<Option<AgentRecord>, CoreError> {
        let name = normalize_agent_name(name)?;
        Ok(self.agents.lock().expect("agents lock").get(&name).cloned())
    }

    async fn update_agent_identity(
        &self,
        name: &str,
        identity: &str,
    ) -> Result<Option<AgentRecord>, CoreError> {
        let name = normalize_agent_name(name)?;
        let identity = normalize_identity_text(identity)?;
        let mut agents = self.agents.lock().expect("agents lock");
        let Some(record) = agents.get_mut(&name) else {
            return Ok(None);
        };
        if !record.runtime_profile.is_object() {
            record.runtime_profile = serde_json::json!({});
        }
        record
            .runtime_profile
            .as_object_mut()
            .expect("runtime_profile normalized to object")
            .insert("identity".to_string(), serde_json::json!(identity));
        record.updated_at = 5;
        Ok(Some(record.clone()))
    }

    async fn heartbeat_agent(
        &self,
        name: &str,
        input: AgentHeartbeat,
    ) -> Result<(AgentRecord, bool), CoreError> {
        let name = normalize_agent_name(name)?;
        let mut agents = self.agents.lock().expect("agents lock");
        let created = !agents.contains_key(&name);
        let record = agents.entry(name.clone()).or_insert_with(|| AgentRecord {
            id: name.clone(),
            name: name.clone(),
            role: Some("agent".to_string()),
            capability: None,
            runtime: None,
            model: None,
            tmux_target: None,
            home_dir: None,
            workdir: None,
            state_dir: None,
            server: None,
            status: "offline".to_string(),
            offline_reason: Some("offline".to_string()),
            last_seen_at: None,
            registered_at: 1,
            updated_at: 1,
            runtime_profile: serde_json::json!({}),
            runtime_state: serde_json::json!({}),
        });
        if input.server.is_some() {
            record.server = input.server;
        }
        if input.tmux_target.is_some() {
            record.tmux_target = input.tmux_target;
        }
        if input.workspace_path.is_some() {
            record.workdir = input.workspace_path;
        }
        record.status = "online".to_string();
        record.offline_reason = None;
        record.last_seen_at = Some(2);
        record.updated_at = 2;
        Ok((record.clone(), created))
    }

    async fn mark_agent_offline(
        &self,
        name: &str,
        input: AgentOffline,
    ) -> Result<Option<AgentRecord>, CoreError> {
        let name = normalize_agent_name(name)?;
        let mut agents = self.agents.lock().expect("agents lock");
        let Some(record) = agents.get_mut(&name) else {
            return Ok(None);
        };
        record.status = "offline".to_string();
        record.offline_reason = Some(
            input
                .reason
                .filter(|r| !r.trim().is_empty())
                .unwrap_or_else(|| "manual-offline".to_string()),
        );
        if input.clear_tmux {
            record.tmux_target = None;
        }
        record.last_seen_at = Some(3);
        record.updated_at = 3;
        Ok(Some(record.clone()))
    }

    async fn start_agent(&self, name: &str) -> Result<Option<AgentStartResult>, CoreError> {
        let name = normalize_agent_name(name)?;
        let mut agents = self.agents.lock().expect("agents lock");
        let Some(record) = agents.get_mut(&name) else {
            return Ok(None);
        };
        if record.status == "online" {
            return Err(CoreError::Invariant("agent already online".to_string()));
        }
        if !matches!(
            record.runtime.as_deref(),
            Some("codex" | "claude" | "claude-code" | "claude_code")
        ) {
            return Err(CoreError::Invariant(
                "unsupported agent runtime".to_string(),
            ));
        }
        if record
            .workdir
            .as_deref()
            .is_none_or(|workdir| workdir.trim().is_empty())
        {
            return Err(CoreError::Invariant("agent workdir required".to_string()));
        }
        let address = format!("fake://{name}");
        record.status = "online".to_string();
        record.offline_reason = None;
        record.tmux_target = Some(address.clone());
        record.last_seen_at = Some(4);
        record.updated_at = 4;
        let handle = AgentStartHandle {
            agent_id: name,
            backend: "tmux".to_string(),
            address,
            pane_id: Some("%0".to_string()),
            pid: Some(4242),
            session_name: format!("agentd-{}", record.name),
        };
        Ok(Some(AgentStartResult {
            agent: record.clone(),
            handle,
        }))
    }

    async fn down_agent(&self, name: &str) -> Result<Option<AgentDownResult>, CoreError> {
        let name = normalize_agent_name(name)?;
        let mut agents = self.agents.lock().expect("agents lock");
        let Some(record) = agents.get_mut(&name) else {
            return Ok(None);
        };
        if record
            .tmux_target
            .as_deref()
            .is_none_or(|target| target.trim().is_empty())
        {
            return Err(CoreError::Invariant(
                "agent tmux target required".to_string(),
            ));
        }
        record.status = "offline".to_string();
        record.offline_reason = Some("agent-down-kill".to_string());
        record.tmux_target = None;
        record.runtime_state = serde_json::json!({
            "agent": name,
            "lifecycle": {
                "state": "down",
                "action": "agent-down-kill",
                "method": "kill",
                "finalCaptureSha": "fake-sha",
            },
            "updatedAt": 6,
        });
        record.updated_at = 6;
        Ok(Some(AgentDownResult {
            agent: record.clone(),
            report: AgentLifecycleReport {
                method: "kill".to_string(),
                archive_path: Some(format!("/tmp/{name}-down.log")),
                final_capture_sha: Some("fake-sha".to_string()),
            },
        }))
    }

    async fn rebind_agent(&self, name: &str) -> Result<Option<AgentRebindResult>, CoreError> {
        let name = normalize_agent_name(name)?;
        let mut agents = self.agents.lock().expect("agents lock");
        let Some(record) = agents.get_mut(&name) else {
            return Ok(None);
        };
        let target = record
            .tmux_target
            .as_deref()
            .map(str::trim)
            .filter(|target| !target.is_empty())
            .ok_or_else(|| CoreError::Invariant("agent tmux target required".to_string()))?
            .to_string();
        record.status = "online".to_string();
        record.offline_reason = None;
        record.runtime_state = serde_json::json!({
            "agent": name,
            "lifecycle": {
                "state": "rebound",
                "target": target,
            },
            "updatedAt": 7,
        });
        record.updated_at = 7;
        let handle = AgentStartHandle {
            agent_id: name,
            backend: "tmux".to_string(),
            address: target,
            pane_id: Some("%0".to_string()),
            pid: Some(4242),
            session_name: format!("agentd-{}", record.name),
        };
        Ok(Some(AgentRebindResult {
            agent: record.clone(),
            handle: Some(handle),
            rebound: true,
        }))
    }

    async fn update_agent_runtime(
        &self,
        name: &str,
        input: AgentRuntimeUpdate,
    ) -> Result<Option<Value>, CoreError> {
        let name = normalize_agent_name(name)?;
        let mut agents = self.agents.lock().expect("agents lock");
        let Some(record) = agents.get_mut(&name) else {
            return Ok(None);
        };
        let runtime = serde_json::json!({
            "agent": name,
            "blocked": input.blocked,
            "blockedReason": input.reason,
            "activeNow": input.active_now,
            "activeDurationSec": input.active_duration_sec,
            "idleDurationSec": input.idle_duration_sec,
            "lastTmuxActivitySec": input.last_tmux_activity_sec,
            "workspacePath": input.workspace_path,
            "mcpPresent": input.mcp_present,
            "updatedAt": 5,
        });
        record.runtime_state = runtime.clone();
        record.updated_at = 5;
        Ok(Some(runtime))
    }

    async fn record_relay_server_heartbeat(
        &self,
        input: RelayServerHeartbeat,
    ) -> Result<RelayServerRecord, CoreError> {
        let id = input.server.trim().to_string();
        if id.is_empty() {
            return Err(CoreError::Store("server required".to_string()));
        }
        let record = RelayServerRecord {
            id: id.clone(),
            instance_id: input.instance_id,
            boot_ts: input.boot_ts,
            agent_count: i64::try_from(input.agents.len()).unwrap_or(i64::MAX),
            agents: input.agents,
            sessions: input.sessions,
            online: true,
            maintenance: false,
            last_seen_at: 1,
            heartbeat_at: 1,
            updated_at: 1,
        };
        self.relay_servers
            .lock()
            .expect("relay server lock")
            .insert(id, record.clone());
        Ok(record)
    }

    async fn append_delivery_event(
        &self,
        input: DeliveryEventInput,
    ) -> Result<DeliveryEventRecord, CoreError> {
        let mut events = self.delivery_events.lock().expect("delivery events lock");
        let seq = i64::try_from(events.len() + 1).unwrap_or(i64::MAX);
        let event = DeliveryEventRecord {
            id: format!("del_fake_{seq}"),
            seq,
            event_type: input.event_type,
            message_id: input.message_id,
            queue_entry_id: input.queue_entry_id,
            agent: input.agent,
            target: input.target,
            reason: input.reason,
            source: input.source,
            context: input.context,
            created_at: seq,
        };
        events.push(event.clone());
        Ok(event)
    }

    async fn list_delivery_events(
        &self,
        agent: &str,
        limit: usize,
    ) -> Result<Vec<DeliveryEventRecord>, CoreError> {
        let mut events = self
            .delivery_events
            .lock()
            .expect("delivery events lock")
            .iter()
            .filter(|event| event.agent.as_deref() == Some(agent))
            .cloned()
            .collect::<Vec<_>>();
        events.sort_by_key(|event| std::cmp::Reverse(event.seq));
        events.truncate(limit.max(1));
        Ok(events)
    }

    async fn relay_stream_events(
        &self,
        after_seq: i64,
    ) -> Result<Vec<RelayStreamEventRecord>, CoreError> {
        Ok(self
            .stream_events
            .lock()
            .expect("stream events lock")
            .iter()
            .filter(|event| event.seq > after_seq)
            .cloned()
            .collect())
    }

    async fn upsert_matrix_bridge_room(
        &self,
        input: MatrixBridgeRoomInput,
    ) -> Result<MatrixBridgeRoomRecord, CoreError> {
        let room_id = normalize_required_text(&input.room_id, 256, "matrix room id required")?;
        let group = clean_text(input.group.as_deref(), 128);
        let agent = clean_text(input.agent.as_deref(), 128);
        if group.is_some() == agent.is_some() {
            return Err(CoreError::Invariant(
                "exactly one of group or agent required".to_string(),
            ));
        }
        if let Some(group) = group.as_deref() {
            if self.get_group(group).await?.is_some() {
                let _ = self
                    .update_group_members(
                        group,
                        GroupMemberUpdate {
                            add: input.members.clone(),
                            remove: Vec::new(),
                        },
                    )
                    .await?;
            } else {
                let _ = self
                    .create_group(GroupCreateInput {
                        name: group.to_string(),
                        members: input.members.clone(),
                    })
                    .await?;
            }
        }
        let record = MatrixBridgeRoomRecord {
            room_id: room_id.clone(),
            project_id: input.project_id.clone(),
            group,
            agent,
            trusted: input.trusted,
            trust_reason: clean_text(Some(&input.trust_reason), 128)
                .unwrap_or_else(|| "managed".to_string()),
            inviter_mxid: clean_text(input.inviter_mxid.as_deref(), 256),
            created_at: 1,
            updated_at: 1,
        };
        self.matrix_rooms
            .lock()
            .expect("matrix rooms lock")
            .insert(room_id, record.clone());
        Ok(record)
    }

    async fn get_matrix_bridge_room(
        &self,
        room_id: &str,
    ) -> Result<Option<MatrixBridgeRoomRecord>, CoreError> {
        let room_id = normalize_required_text(room_id, 256, "matrix room id required")?;
        Ok(self
            .matrix_rooms
            .lock()
            .expect("matrix rooms lock")
            .get(&room_id)
            .cloned())
    }

    async fn acknowledge_matrix_outbox_cursor(
        &self,
        input: MatrixOutboxCursorInput,
    ) -> Result<i64, CoreError> {
        let mut cursors = self
            .matrix_outbox_cursors
            .lock()
            .expect("matrix outbox cursors lock");
        let cursor = cursors.entry(input.bridge_id).or_insert(0);
        *cursor = (*cursor).max(input.last_seq);
        Ok(*cursor)
    }

    async fn matrix_outbox_cursor(&self, bridge_id: &str) -> Result<i64, CoreError> {
        Ok(self
            .matrix_outbox_cursors
            .lock()
            .expect("matrix outbox cursors lock")
            .get(bridge_id)
            .copied()
            .unwrap_or(0))
    }

    async fn post_matrix_inbound_message(
        &self,
        input: MatrixInboundMessageInput,
    ) -> Result<MatrixInboundMessageResult, CoreError> {
        let event_id = normalize_required_text(&input.event_id, 256, "matrix event id required")?;
        if let Some(existing) = self
            .matrix_events
            .lock()
            .expect("matrix events lock")
            .get(&event_id)
            .cloned()
        {
            return Ok(MatrixInboundMessageResult {
                duplicate: true,
                message: None,
                ..existing
            });
        }

        let room_id = normalize_required_text(&input.room_id, 256, "matrix room id required")?;
        let sender_mxid =
            normalize_required_text(&input.sender_mxid, 256, "matrix sender mxid required")?;
        let Some(room) = self.get_matrix_bridge_room(&room_id).await? else {
            return Err(CoreError::Invariant("matrix room not trusted".to_string()));
        };
        if !room.trusted {
            return Err(CoreError::Invariant("matrix room not trusted".to_string()));
        }

        if input
            .body
            .trim_start()
            .to_ascii_uppercase()
            .starts_with("[AGENTIGNORE]")
        {
            let result = MatrixInboundMessageResult {
                ok: true,
                duplicate: false,
                ignored: true,
                route: "ignored".to_string(),
                event_id,
                message_id: None,
                message: None,
            };
            self.matrix_events
                .lock()
                .expect("matrix events lock")
                .insert(result.event_id.clone(), result.clone());
            return Ok(result);
        }

        let from = input
            .from
            .and_then(|value| clean_text(Some(&value), 128))
            .unwrap_or_else(|| matrix_sender_name(&sender_mxid));
        let trust_level = input.trust_level.or_else(|| Some("external".to_string()));
        let (route, message) = if let Some(group) = room.group.clone() {
            (
                "group".to_string(),
                self.post_group_message(GroupMessageInput {
                    message_id: None,
                    ts: None,
                    from,
                    group,
                    message_type: Some("human".to_string()),
                    priority: None,
                    summary: input.body.clone(),
                    full: input.body,
                    mentions: input.mentions,
                    reply_to: input.reply_to,
                    source: Some("matrix".to_string()),
                    schema: None,
                    attachments: Vec::new(),
                })
                .await?,
            )
        } else if let Some(agent) = room.agent.clone() {
            (
                "agent".to_string(),
                self.post_direct_message(DirectMessageInput {
                    message_id: None,
                    ts: None,
                    from,
                    to: agent,
                    message_type: Some("human".to_string()),
                    priority: None,
                    summary: input.body.clone(),
                    full: input.body,
                    reply_to: input.reply_to,
                    source: Some("matrix".to_string()),
                    source_room: Some(room_id),
                    sender_mxid: Some(sender_mxid),
                    trust_level,
                    from_id: None,
                    schema: None,
                    attachments: Vec::new(),
                })
                .await?,
            )
        } else {
            return Err(CoreError::Invariant("matrix room not trusted".to_string()));
        };

        let result = MatrixInboundMessageResult {
            ok: true,
            duplicate: false,
            ignored: false,
            route,
            event_id,
            message_id: Some(message.id.clone()),
            message: Some(message),
        };
        self.matrix_events
            .lock()
            .expect("matrix events lock")
            .insert(result.event_id.clone(), result.clone());
        Ok(result)
    }

    async fn scheduler_pool(
        &self,
        filters: SchedulerPoolFilters,
    ) -> Result<SchedulerPoolSnapshot, CoreError> {
        let role = filters.role.and_then(|value| clean_text(Some(&value), 64));
        let capability = filters.capability.and_then(|value| fake_clean_tier(&value));
        let state = filters
            .state
            .and_then(|value| clean_text(Some(&value), 16))
            .unwrap_or_else(|| "any".to_string())
            .to_ascii_lowercase();
        let busy = fake_busy_agents(&self.scheduler_reservations.lock().expect("scheduler lock"));
        let agents = self
            .agents
            .lock()
            .expect("agents lock")
            .values()
            .map(|agent| fake_pool_agent(agent, &busy))
            .filter(|agent| {
                role.as_deref()
                    .is_none_or(|role| agent.role.as_deref() == Some(role))
            })
            .filter(|agent| {
                capability
                    .as_deref()
                    .is_none_or(|tier| agent.capability == tier)
            })
            .filter(|agent| match state.as_str() {
                "idle" => agent.online && !agent.busy,
                "busy" => agent.busy,
                _ => true,
            })
            .collect::<Vec<_>>();
        Ok(fake_pool_snapshot(agents))
    }

    async fn scheduler_dispatch(
        &self,
        input: SchedulerDispatchInput,
        max_per_cell: i64,
    ) -> Result<SchedulerDispatchResult, CoreError> {
        let role = normalize_required_text(&input.role, 64, "role required")?;
        let tier = fake_resolve_tier(&role, input.capability.as_deref());
        let busy = fake_busy_agents(&self.scheduler_reservations.lock().expect("scheduler lock"));
        let agents = self
            .agents
            .lock()
            .expect("agents lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        if let Some(agent) = fake_select_agent(&agents, &busy, &role, &tier) {
            let reservation =
                self.fake_insert_scheduler_reservation(FakeSchedulerReservationInput {
                    role: role.clone(),
                    tier: tier.clone(),
                    agent: Some(agent.name.clone()),
                    provisioned_name: None,
                    status: "routed".to_string(),
                    task: input.task,
                    room: input.room,
                    runtime: serde_json::json!({}),
                    ticket: None,
                });
            return Ok(SchedulerDispatchResult {
                status: "routed".to_string(),
                role,
                tier,
                agent: Some(agent.name),
                reservation: Some(reservation),
                ticket: None,
                queue_depth: None,
                name: None,
                runtime: serde_json::json!({}),
            });
        }

        if max_per_cell > 0 {
            let in_cell = i64::try_from(
                agents
                    .iter()
                    .filter(|agent| {
                        fake_effective_role(agent).as_deref() == Some(role.as_str())
                            && fake_effective_capability(agent) == tier
                    })
                    .count(),
            )
            .unwrap_or(i64::MAX);
            let provisioned = i64::try_from(
                self.scheduler_reservations
                    .lock()
                    .expect("scheduler lock")
                    .iter()
                    .filter(|reservation| {
                        reservation.role == role
                            && reservation.tier == tier
                            && reservation.status == "provision"
                    })
                    .count(),
            )
            .unwrap_or(i64::MAX);
            if in_cell + provisioned < max_per_cell {
                let reservation_count = self
                    .scheduler_reservations
                    .lock()
                    .expect("scheduler lock")
                    .len();
                let name = format!("mx_{role}_{tier}_{reservation_count}");
                let runtime = fake_tier_runtime(&tier);
                let reservation =
                    self.fake_insert_scheduler_reservation(FakeSchedulerReservationInput {
                        role: role.clone(),
                        tier: tier.clone(),
                        agent: None,
                        provisioned_name: Some(name.clone()),
                        status: "provision".to_string(),
                        task: input.task,
                        room: input.room,
                        runtime: runtime.clone(),
                        ticket: None,
                    });
                return Ok(SchedulerDispatchResult {
                    status: "provision".to_string(),
                    role,
                    tier,
                    agent: None,
                    reservation: Some(reservation),
                    ticket: None,
                    queue_depth: None,
                    name: Some(name),
                    runtime,
                });
            }
        }

        let ticket = {
            let queue_len = self.scheduler_queue.lock().expect("queue lock").len();
            format!("disp-1-{}", queue_len + 1)
        };
        let depth = {
            let mut queue = self.scheduler_queue.lock().expect("queue lock");
            queue.push(FakeSchedulerTicket {
                ticket: ticket.clone(),
                role: role.clone(),
                tier: tier.clone(),
                task: input.task,
                room: input.room,
                status: "queued".to_string(),
            });
            queue
                .iter()
                .filter(|entry| {
                    entry.role == role && entry.tier == tier && entry.status == "queued"
                })
                .count()
        };
        Ok(SchedulerDispatchResult {
            status: "queued".to_string(),
            role,
            tier,
            agent: None,
            reservation: None,
            ticket: Some(ticket),
            queue_depth: Some(depth),
            name: None,
            runtime: serde_json::json!({}),
        })
    }

    async fn scheduler_release(
        &self,
        input: SchedulerReleaseInput,
    ) -> Result<SchedulerReleaseResult, CoreError> {
        let agent_name = normalize_required_text(&input.agent, 128, "agent required")?;
        {
            let mut reservations = self.scheduler_reservations.lock().expect("scheduler lock");
            if let Some(reservation) = reservations.iter_mut().find(|reservation| {
                reservation.agent.as_deref() == Some(agent_name.as_str())
                    && matches!(reservation.status.as_str(), "routed" | "drained")
            }) {
                reservation.status = "released".to_string();
                reservation.released_at = Some(9);
                reservation.updated_at = 9;
            }
        }
        let Some(agent) = self
            .agents
            .lock()
            .expect("agents lock")
            .get(&agent_name)
            .cloned()
        else {
            return Ok(fake_released_scheduler(agent_name));
        };
        let Some(role) = fake_effective_role(&agent) else {
            return Ok(fake_released_scheduler(agent_name));
        };
        let tier = fake_effective_capability(&agent);
        let next = {
            let mut queue = self.scheduler_queue.lock().expect("queue lock");
            let Some(entry) = queue
                .iter_mut()
                .find(|entry| entry.role == role && entry.tier == tier && entry.status == "queued")
            else {
                return Ok(fake_released_scheduler(agent_name));
            };
            entry.status = "drained".to_string();
            entry.clone()
        };
        let reservation = self.fake_insert_scheduler_reservation(FakeSchedulerReservationInput {
            role: role.clone(),
            tier: tier.clone(),
            agent: Some(agent_name.clone()),
            provisioned_name: None,
            status: "drained".to_string(),
            task: next.task.clone(),
            room: next.room.clone(),
            runtime: serde_json::json!({}),
            ticket: Some(next.ticket.clone()),
        });
        Ok(SchedulerReleaseResult {
            status: "drained".to_string(),
            agent: agent_name,
            reservation: Some(reservation),
            ticket: Some(next.ticket),
            role: Some(role),
            tier: Some(tier),
            task: next.task,
            room: next.room,
        })
    }

    async fn create_agent_chat_task(
        &self,
        input: AgentChatTaskCreateInput,
    ) -> Result<AgentChatTaskRecord, CoreError> {
        let title = normalize_required_text(&input.title, 255, "title is required")?;
        let priority = clean_text(input.priority.as_deref(), 8).unwrap_or_else(|| "p2".to_string());
        validate_task_member("priority", &priority, &["p0", "p1", "p2", "p3"])?;
        let granularity =
            clean_text(input.granularity.as_deref(), 16).unwrap_or_else(|| "task".to_string());
        validate_task_member("granularity", &granularity, &["epic", "task", "subtask"])?;
        let parent_id = clean_text(input.parent_id.as_deref(), 64);
        if let Some(parent_id) = parent_id.as_deref()
            && !self
                .agent_chat_tasks
                .lock()
                .expect("agent_chat_tasks lock")
                .contains_key(parent_id)
        {
            return Err(CoreError::Invariant(format!(
                "parent task not found: {parent_id}"
            )));
        }
        let next = self
            .agent_chat_task_order
            .lock()
            .expect("agent_chat_task_order lock")
            .len()
            + 1;
        let id = format!("task_1_{next:06x}");
        let task = AgentChatTaskRecord {
            id: id.clone(),
            title,
            description: clean_text(input.description.as_deref(), 4096).unwrap_or_default(),
            status: "created".to_string(),
            priority,
            granularity,
            assignee: clean_text(input.assignee.as_deref(), 128),
            created_by: clean_text(input.created_by.as_deref(), 128),
            created_at: fake_task_now(),
            updated_at: fake_task_now(),
            started_at: None,
            completed_at: None,
            heartbeat_at: None,
            waiting_reason: None,
            waiting_until: None,
            parent_id,
            labels: normalize_task_labels(input.labels),
            health: None,
            comments: Vec::new(),
        };
        self.agent_chat_tasks
            .lock()
            .expect("agent_chat_tasks lock")
            .insert(id.clone(), task.clone());
        self.agent_chat_task_order
            .lock()
            .expect("agent_chat_task_order lock")
            .push(id);
        Ok(task)
    }

    async fn list_agent_chat_tasks(
        &self,
        filters: AgentChatTaskListFilters,
    ) -> Result<Vec<AgentChatTaskRecord>, CoreError> {
        let tasks = self.agent_chat_tasks.lock().expect("agent_chat_tasks lock");
        let order = self
            .agent_chat_task_order
            .lock()
            .expect("agent_chat_task_order lock");
        let statuses = filters
            .statuses
            .into_iter()
            .filter_map(|status| clean_text(Some(&status), 64))
            .collect::<Vec<_>>();
        let mut out = order
            .iter()
            .filter_map(|id| tasks.get(id))
            .filter(|task| {
                filters
                    .assignee
                    .as_deref()
                    .is_none_or(|assignee| task.assignee.as_deref() == Some(assignee))
            })
            .filter(|task| statuses.is_empty() || statuses.iter().any(|s| s == &task.status))
            .filter(|task| {
                filters
                    .priority
                    .as_deref()
                    .is_none_or(|priority| task.priority == priority)
            })
            .filter(|task| {
                filters
                    .label
                    .as_deref()
                    .is_none_or(|label| task.labels.iter().any(|existing| existing == label))
            })
            .skip(filters.offset)
            .cloned()
            .collect::<Vec<_>>();
        if let Some(limit) = filters.limit {
            out.truncate(limit);
        }
        Ok(out)
    }

    async fn get_agent_chat_task(
        &self,
        id: &str,
    ) -> Result<Option<AgentChatTaskRecord>, CoreError> {
        Ok(self
            .agent_chat_tasks
            .lock()
            .expect("agent_chat_tasks lock")
            .get(id)
            .cloned())
    }

    async fn update_agent_chat_task(
        &self,
        id: &str,
        input: AgentChatTaskPatchInput,
    ) -> Result<Option<AgentChatTaskRecord>, CoreError> {
        if let Some(parent_id) = input
            .parent_id
            .as_ref()
            .and_then(|value| value.as_ref())
            .and_then(|value| clean_text(Some(value), 64))
            && !self
                .agent_chat_tasks
                .lock()
                .expect("agent_chat_tasks lock")
                .contains_key(&parent_id)
        {
            return Err(CoreError::Invariant(format!(
                "parent task not found: {parent_id}"
            )));
        }
        let mut tasks = self.agent_chat_tasks.lock().expect("agent_chat_tasks lock");
        let Some(task) = tasks.get_mut(id) else {
            return Ok(None);
        };
        let mut changed = false;
        if let Some(title) = input.title {
            task.title = normalize_required_text(&title, 255, "title is required")?;
            changed = true;
        }
        if let Some(description) = input.description {
            task.description = clean_text(Some(&description), 4096).unwrap_or_default();
            changed = true;
        }
        if let Some(priority) = input.priority {
            let priority = clean_text(Some(&priority), 8)
                .ok_or_else(|| CoreError::Invariant("invalid priority: ".to_string()))?;
            validate_task_member("priority", &priority, &["p0", "p1", "p2", "p3"])?;
            task.priority = priority;
            changed = true;
        }
        if let Some(granularity) = input.granularity {
            let granularity = clean_text(Some(&granularity), 16)
                .ok_or_else(|| CoreError::Invariant("invalid granularity: ".to_string()))?;
            validate_task_member("granularity", &granularity, &["epic", "task", "subtask"])?;
            task.granularity = granularity;
            changed = true;
        }
        if let Some(assignee) = input.assignee {
            task.assignee = assignee.and_then(|value| clean_text(Some(&value), 128));
            changed = true;
        }
        if let Some(labels) = input.labels {
            task.labels = normalize_task_labels(labels);
            changed = true;
        }
        if let Some(parent_id) = input.parent_id {
            task.parent_id = parent_id.and_then(|value| clean_text(Some(&value), 64));
            changed = true;
        }
        if changed {
            task.updated_at = fake_task_now();
        }
        Ok(Some(task.clone()))
    }

    async fn update_agent_chat_task_execution(
        &self,
        id: &str,
        input: AgentChatTaskExecutionInput,
    ) -> Result<Option<AgentChatTaskRecord>, CoreError> {
        let mut tasks = self.agent_chat_tasks.lock().expect("agent_chat_tasks lock");
        let Some(task) = tasks.get_mut(id) else {
            return Ok(None);
        };
        let mut changed = false;
        if input.heartbeat_at.is_some() {
            task.heartbeat_at = Some(fake_task_now());
            changed = true;
        }
        if let Some(waiting_reason) = input.waiting_reason {
            task.waiting_reason = waiting_reason.and_then(|value| clean_text(Some(&value), 1024));
            changed = true;
        }
        if let Some(waiting_until) = input.waiting_until {
            task.waiting_until = waiting_until.and_then(|value| clean_text(Some(&value), 64));
            changed = true;
        }
        if changed {
            task.updated_at = fake_task_now();
        }
        Ok(Some(task.clone()))
    }

    async fn transition_agent_chat_task(
        &self,
        id: &str,
        input: AgentChatTaskTransitionInput,
    ) -> Result<Option<AgentChatTaskRecord>, CoreError> {
        let status = input
            .status
            .as_deref()
            .and_then(|value| clean_text(Some(value), 64))
            .ok_or_else(|| CoreError::Invariant("status is required".to_string()))?;
        validate_task_member(
            "status",
            &status,
            &["created", "accepted", "in_progress", "blocked", "done"],
        )?;
        let mut tasks = self.agent_chat_tasks.lock().expect("agent_chat_tasks lock");
        let Some(task) = tasks.get_mut(id) else {
            return Ok(None);
        };
        if !task_transition_allowed(&task.status, &status) {
            return Err(CoreError::Invariant(format!(
                "cannot transition from '{}' to '{}'",
                task.status, status
            )));
        }
        let waiting_reason = clean_text(input.waiting_reason.as_deref(), 1024);
        let waiting_until = clean_text(input.waiting_until.as_deref(), 64);
        if status == "blocked" {
            if waiting_reason.is_none() {
                return Err(CoreError::Invariant(
                    "waiting_reason is required when transitioning to blocked".to_string(),
                ));
            }
            if waiting_until.is_none() {
                return Err(CoreError::Invariant(
                    "waiting_until is required when transitioning to blocked".to_string(),
                ));
            }
        }
        let now = fake_task_now();
        task.status.clone_from(&status);
        task.updated_at.clone_from(&now);
        if (status == "accepted" || status == "in_progress") && task.started_at.is_none() {
            task.started_at = Some(now.clone());
        }
        if status == "done" {
            task.completed_at = Some(now);
            task.waiting_reason = None;
            task.waiting_until = None;
        } else if status == "blocked" {
            task.waiting_reason = waiting_reason;
            task.waiting_until = waiting_until;
        } else if status == "in_progress" {
            task.waiting_reason = None;
            task.waiting_until = None;
        }
        Ok(Some(task.clone()))
    }

    async fn add_agent_chat_task_comment(
        &self,
        id: &str,
        input: AgentChatTaskCommentInput,
    ) -> Result<Option<AgentChatTaskRecord>, CoreError> {
        let mut tasks = self.agent_chat_tasks.lock().expect("agent_chat_tasks lock");
        let Some(task) = tasks.get_mut(id) else {
            return Ok(None);
        };
        if task.comments.len() >= 100 {
            return Err(CoreError::Invariant(
                "max 100 comments per task".to_string(),
            ));
        }
        let text = normalize_required_text(&input.text, 4096, "comment text is required")?;
        let author =
            clean_text(input.author.as_deref(), 128).unwrap_or_else(|| "anonymous".to_string());
        task.comments.push(AgentChatTaskComment {
            author,
            text,
            ts: fake_task_now(),
        });
        task.updated_at = fake_task_now();
        Ok(Some(task.clone()))
    }

    async fn delete_agent_chat_task(
        &self,
        id: &str,
    ) -> Result<Option<AgentChatTaskRecord>, CoreError> {
        let removed = self
            .agent_chat_tasks
            .lock()
            .expect("agent_chat_tasks lock")
            .remove(id);
        if removed.is_some() {
            self.agent_chat_task_order
                .lock()
                .expect("agent_chat_task_order lock")
                .retain(|existing| existing != id);
        }
        Ok(removed)
    }

    async fn create_agent_chat_task_graph(
        &self,
        input: AgentChatTaskGraphCreateInput,
    ) -> Result<AgentChatTaskGraphRecord, CoreError> {
        let id = input
            .id
            .and_then(|value| clean_text(Some(&value), 128))
            .unwrap_or_else(|| {
                let next = self
                    .agent_chat_task_graph_order
                    .lock()
                    .expect("agent_chat_task_graph_order lock")
                    .len()
                    + 1;
                format!("graph_1_{next:06x}")
            });
        if self
            .agent_chat_task_graphs
            .lock()
            .expect("agent_chat_task_graphs lock")
            .contains_key(&id)
        {
            return Err(CoreError::Invariant(format!(
                "task graph already exists: {id}"
            )));
        }
        let owner = normalize_required_text(&input.owner, 128, "owner required")?;
        let label = normalize_required_text(&input.label, 255, "label required")?;
        let mut nodes = BTreeMap::new();
        for (node_id, node_input) in input.nodes {
            let node_id = normalize_required_text(&node_id, 128, "node id required")?;
            let explicit_id = node_input
                .id
                .and_then(|value| clean_text(Some(&value), 128))
                .unwrap_or_else(|| node_id.clone());
            if explicit_id != node_id {
                return Err(CoreError::Invariant(format!(
                    "node id mismatch: key '{node_id}' vs id '{explicit_id}'"
                )));
            }
            nodes.insert(
                node_id.clone(),
                AgentChatTaskGraphNode {
                    id: node_id,
                    assignee: node_input
                        .assignee
                        .as_deref()
                        .and_then(|value| clean_text(Some(value), 128))
                        .unwrap_or_default(),
                    role: node_input
                        .role
                        .as_deref()
                        .and_then(|value| clean_text(Some(value), 64)),
                    capability: node_input
                        .capability
                        .as_deref()
                        .and_then(|value| clean_text(Some(value), 64)),
                    tier: None,
                    scheduler_reservation_id: None,
                    scheduler_ticket: None,
                    scheduler_status: None,
                    provisioned_name: None,
                    runtime: None,
                    description: normalize_required_text(
                        &node_input.description,
                        4096,
                        "node description required",
                    )?,
                    depends_on: clean_dedup(node_input.depends_on),
                    status: "pending".to_string(),
                    result: None,
                    error: None,
                    condition: node_input.condition,
                    message_id: None,
                    started_at: None,
                    dispatched_at: None,
                    completed_at: None,
                },
            );
        }
        fake_validate_task_graph_nodes(&nodes)?;
        let mut graph = AgentChatTaskGraphRecord {
            id: id.clone(),
            owner,
            label,
            status: "active".to_string(),
            nodes,
            created_at: fake_task_now(),
            updated_at: fake_task_now(),
            completed_at: None,
        };
        {
            let mut inbox = self.inbox.lock().expect("inbox lock");
            fake_advance_task_graph(&mut graph, &mut inbox);
        }
        self.agent_chat_task_graphs
            .lock()
            .expect("agent_chat_task_graphs lock")
            .insert(id.clone(), graph.clone());
        self.agent_chat_task_graph_order
            .lock()
            .expect("agent_chat_task_graph_order lock")
            .push(id);
        Ok(graph)
    }

    async fn list_agent_chat_task_graphs(
        &self,
        status: Option<String>,
    ) -> Result<Vec<AgentChatTaskGraphRecord>, CoreError> {
        let graphs = self
            .agent_chat_task_graphs
            .lock()
            .expect("agent_chat_task_graphs lock");
        let order = self
            .agent_chat_task_graph_order
            .lock()
            .expect("agent_chat_task_graph_order lock");
        let status = status.and_then(|value| clean_text(Some(&value), 64));
        Ok(order
            .iter()
            .filter_map(|id| graphs.get(id))
            .filter(|graph| {
                status
                    .as_deref()
                    .is_none_or(|status| graph.status == status)
            })
            .cloned()
            .collect())
    }

    async fn get_agent_chat_task_graph(
        &self,
        id: &str,
    ) -> Result<Option<AgentChatTaskGraphRecord>, CoreError> {
        Ok(self
            .agent_chat_task_graphs
            .lock()
            .expect("agent_chat_task_graphs lock")
            .get(id)
            .cloned())
    }

    async fn delete_agent_chat_task_graph(
        &self,
        id: &str,
    ) -> Result<Option<AgentChatTaskGraphRecord>, CoreError> {
        let mut graphs = self
            .agent_chat_task_graphs
            .lock()
            .expect("agent_chat_task_graphs lock");
        let Some(graph) = graphs.get_mut(id) else {
            return Ok(None);
        };
        graph.status = "cancelled".to_string();
        graph.updated_at = fake_task_now();
        graph.completed_at = Some(fake_task_now());
        for node in graph.nodes.values_mut() {
            if !fake_task_graph_node_terminal(&node.status) {
                node.status = "cancelled".to_string();
                node.completed_at = Some(fake_task_now());
            }
        }
        Ok(Some(graph.clone()))
    }

    async fn update_agent_chat_task_graph_node(
        &self,
        graph_id: &str,
        node_id: &str,
        input: AgentChatTaskGraphNodePatchInput,
    ) -> Result<Option<(AgentChatTaskGraphRecord, AgentChatTaskGraphNode)>, CoreError> {
        let mut graphs = self
            .agent_chat_task_graphs
            .lock()
            .expect("agent_chat_task_graphs lock");
        let Some(graph) = graphs.get_mut(graph_id) else {
            return Ok(None);
        };
        let Some(node) = graph.nodes.get_mut(node_id) else {
            return Ok(None);
        };
        fake_apply_task_graph_node_patch(node, input)?;
        {
            let mut inbox = self.inbox.lock().expect("inbox lock");
            fake_advance_task_graph(graph, &mut inbox);
        }
        let Some(node) = graph.nodes.get(node_id).cloned() else {
            return Ok(None);
        };
        Ok(Some((graph.clone(), node)))
    }

    async fn handle_agent_chat_task_graph_message(
        &self,
        from: &str,
        reply_to: Option<String>,
        schema: Option<Value>,
    ) -> Result<Option<AgentChatTaskGraphMessageResult>, CoreError> {
        let Some(reply_to) = reply_to.and_then(|value| clean_text(Some(&value), 128)) else {
            return Ok(None);
        };
        let Some(schema) = schema else {
            return Ok(None);
        };
        let kind = schema
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(kind, "task_graph_result" | "task_graph_failed") {
            return Ok(None);
        }
        let Some(payload) = schema.get("payload").and_then(Value::as_object) else {
            return Ok(None);
        };
        let Some(graph_id) = payload
            .get("graphId")
            .and_then(Value::as_str)
            .and_then(|value| clean_text(Some(value), 128))
        else {
            return Ok(None);
        };
        let Some(node_id) = payload
            .get("nodeId")
            .and_then(Value::as_str)
            .and_then(|value| clean_text(Some(value), 128))
        else {
            return Ok(None);
        };
        let mut graphs = self
            .agent_chat_task_graphs
            .lock()
            .expect("agent_chat_task_graphs lock");
        let Some(graph) = graphs.get_mut(&graph_id) else {
            return Ok(None);
        };
        let Some(node) = graph.nodes.get(&node_id) else {
            return Ok(None);
        };
        if node.assignee != from || node.message_id.as_deref() != Some(reply_to.as_str()) {
            return Ok(None);
        }
        let patch = if kind == "task_graph_result" {
            AgentChatTaskGraphNodePatchInput {
                status: Some("complete".to_string()),
                result: payload.get("result").cloned(),
                error: None,
            }
        } else {
            AgentChatTaskGraphNodePatchInput {
                status: Some("failed".to_string()),
                result: payload.get("result").cloned(),
                error: payload
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| Some("task graph node failed".to_string())),
            }
        };
        if let Some(node) = graph.nodes.get_mut(&node_id) {
            fake_apply_task_graph_node_patch(node, patch)?;
        }
        {
            let mut inbox = self.inbox.lock().expect("inbox lock");
            fake_advance_task_graph(graph, &mut inbox);
        }
        let status = graph
            .nodes
            .get(&node_id)
            .map(|node| node.status.clone())
            .unwrap_or_default();
        Ok(Some(AgentChatTaskGraphMessageResult {
            handled: true,
            graph_id,
            node_id,
            status,
            graph: graph.clone(),
        }))
    }

    async fn create_group(&self, input: GroupCreateInput) -> Result<GroupRecord, CoreError> {
        let name = normalize_agent_name(&input.name)?;
        let record = GroupRecord {
            name: name.clone(),
            members: clean_dedup(input.members),
            created_at: 1,
        };
        self.groups
            .lock()
            .expect("groups lock")
            .insert(name, record.clone());
        Ok(record)
    }

    async fn list_groups(&self) -> Result<Vec<GroupRecord>, CoreError> {
        let mut groups = self
            .groups
            .lock()
            .expect("groups lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        groups.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(groups)
    }

    async fn get_group(&self, name: &str) -> Result<Option<GroupRecord>, CoreError> {
        let name = normalize_agent_name(name)?;
        Ok(self.groups.lock().expect("groups lock").get(&name).cloned())
    }

    async fn update_group_members(
        &self,
        name: &str,
        input: GroupMemberUpdate,
    ) -> Result<Option<GroupRecord>, CoreError> {
        let name = normalize_agent_name(name)?;
        let mut groups = self.groups.lock().expect("groups lock");
        let Some(group) = groups.get_mut(&name) else {
            return Ok(None);
        };
        for member in clean_dedup(input.add) {
            if !group
                .members
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&member))
            {
                group.members.push(member);
            }
        }
        let remove = clean_dedup(input.remove);
        group.members.retain(|member| {
            !remove
                .iter()
                .any(|removed| removed.eq_ignore_ascii_case(member))
        });
        Ok(Some(group.clone()))
    }

    async fn delete_group(&self, name: &str) -> Result<Option<GroupRecord>, CoreError> {
        let name = normalize_agent_name(name)?;
        Ok(self.groups.lock().expect("groups lock").remove(&name))
    }

    async fn post_direct_message(
        &self,
        input: DirectMessageInput,
    ) -> Result<InboxMessage, CoreError> {
        let id = input.message_id.unwrap_or_else(|| {
            let next = self.inbox.lock().expect("inbox lock").len() + 1;
            format!("msg_fake_{next}")
        });
        let message = InboxMessage {
            id,
            ts: input.ts.unwrap_or(1),
            at: "1970-01-01T00:00:00.001Z".to_string(),
            time: "0s ago".to_string(),
            from: input.from,
            to: input.to,
            message_type: input.message_type.unwrap_or_else(|| "human".to_string()),
            priority: input.priority.unwrap_or_else(|| "normal".to_string()),
            summary: input.summary,
            full: input.full,
            mentions: Vec::new(),
            attachments: input.attachments,
            reply_to: input.reply_to,
            group: None,
            source: input.source.unwrap_or_else(|| "api".to_string()),
            source_room: input.source_room,
            sender_mxid: input.sender_mxid,
            trust_level: input.trust_level,
            from_id: input.from_id,
            schema: input.schema,
        };
        self.push_inbox_message(message.clone());
        Ok(message)
    }

    async fn post_group_message(
        &self,
        input: GroupMessageInput,
    ) -> Result<InboxMessage, CoreError> {
        let group = normalize_agent_name(&input.group)?;
        if !self
            .groups
            .lock()
            .expect("groups lock")
            .contains_key(&group)
        {
            return Err(CoreError::Invariant("group not found".to_string()));
        }
        let id = input.message_id.unwrap_or_else(|| {
            let next = self.inbox.lock().expect("inbox lock").len() + 1;
            format!("msg_group_fake_{next}")
        });
        let message = InboxMessage {
            id,
            ts: input.ts.unwrap_or(1),
            at: "1970-01-01T00:00:00.001Z".to_string(),
            time: "0s ago".to_string(),
            from: input.from,
            to: String::new(),
            message_type: input.message_type.unwrap_or_else(|| "inform".to_string()),
            priority: input.priority.unwrap_or_else(|| "normal".to_string()),
            summary: input.summary,
            full: input.full,
            mentions: clean_dedup(input.mentions),
            attachments: input.attachments,
            reply_to: input.reply_to,
            group: Some(group),
            source: input.source.unwrap_or_else(|| "api".to_string()),
            source_room: None,
            sender_mxid: None,
            trust_level: None,
            from_id: None,
            schema: input.schema,
        };
        self.push_inbox_message(message.clone());
        Ok(message)
    }

    async fn check_inbox(
        &self,
        agent_id: &str,
        drain: bool,
    ) -> Result<Vec<InboxMessage>, CoreError> {
        let agent_id = normalize_agent_name(agent_id)?;
        let mut inbox = self.inbox.lock().expect("inbox lock");
        let mut mention_reads = self
            .group_mention_reads
            .lock()
            .expect("group_mention_reads lock");
        let messages = inbox
            .iter()
            .filter(|entry| {
                if entry.message.group.is_some() {
                    entry
                        .message
                        .mentions
                        .iter()
                        .any(|mention| mention.eq_ignore_ascii_case(&agent_id))
                        && !mention_reads.contains(&(agent_id.clone(), entry.message.id.clone()))
                } else {
                    !entry.read && entry.message.to == agent_id
                }
            })
            .map(|entry| entry.message.clone())
            .collect::<Vec<_>>();
        if drain {
            for entry in inbox.iter_mut() {
                if entry.message.group.is_some()
                    && messages
                        .iter()
                        .any(|message| message.id == entry.message.id)
                {
                    mention_reads.insert((agent_id.clone(), entry.message.id.clone()));
                } else if messages
                    .iter()
                    .any(|message| message.id == entry.message.id)
                {
                    entry.read = true;
                }
            }
        }
        Ok(messages)
    }

    async fn read_group_messages(
        &self,
        input: GroupReadRequest,
    ) -> Result<GroupReadResult, CoreError> {
        let group = normalize_agent_name(&input.group)?;
        let agent_id = normalize_agent_name(&input.agent_id)?;
        let inbox = self.inbox.lock().expect("inbox lock");
        let mut reads = self
            .group_message_reads
            .lock()
            .expect("group_message_reads lock");
        let mut unread_all = Vec::new();
        let mut read_all = Vec::new();
        for entry in inbox.iter().filter(|entry| {
            entry
                .message
                .group
                .as_deref()
                .is_some_and(|entry_group| entry_group.eq_ignore_ascii_case(&group))
        }) {
            let key = (agent_id.clone(), group.clone(), entry.message.id.clone());
            if reads.contains(&key) {
                read_all.push(entry.message.clone());
            } else {
                unread_all.push(entry.message.clone());
            }
        }
        if input.advance == GroupReadAdvance::All {
            for message in &unread_all {
                reads.insert((agent_id.clone(), group.clone(), message.id.clone()));
            }
        }
        let unread_total = unread_all.len();
        let unread_cap = input
            .unread_limit
            .unwrap_or(if input.advance == GroupReadAdvance::All {
                usize::MAX
            } else {
                input.limit
            })
            .max(1);
        let unread = unread_all
            .iter()
            .take(unread_cap)
            .cloned()
            .collect::<Vec<_>>();
        let unread_returned = unread.len();
        let unread_omitted = unread_total.saturating_sub(unread_returned);
        let read = read_all
            .into_iter()
            .rev()
            .take(input.limit.max(1))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();
        Ok(GroupReadResult {
            group,
            unread,
            read,
            unread_total,
            unread_returned,
            unread_omitted,
            advance: input.advance.as_str().to_string(),
        })
    }
}

fn normalize_agent_name(name: &str) -> Result<String, CoreError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CoreError::Invariant("agent name required".to_string()));
    }
    Ok(trimmed.to_string())
}

fn normalize_identity_text(identity: &str) -> Result<String, CoreError> {
    let trimmed = identity.trim();
    if trimmed.is_empty() {
        return Err(CoreError::Invariant("identity text required".to_string()));
    }
    Ok(trimmed.to_string())
}

fn clean_dedup(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !out
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(trimmed))
        {
            out.push(trimmed.to_string());
        }
    }
    out
}

fn fake_task_now() -> String {
    "1970-01-01T00:00:00.001Z".to_string()
}

fn normalize_required_text(
    value: &str,
    max_len: usize,
    message: &str,
) -> Result<String, CoreError> {
    clean_text(Some(value), max_len).ok_or_else(|| CoreError::Invariant(message.to_string()))
}

fn clean_text(value: Option<&str>, max_len: usize) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(max_len).collect())
}

fn matrix_sender_name(sender_mxid: &str) -> String {
    let trimmed = sender_mxid.trim();
    let localpart = trimmed
        .strip_prefix('@')
        .and_then(|value| value.split(':').next())
        .unwrap_or(trimmed)
        .trim();
    if localpart.is_empty() {
        "matrix".to_string()
    } else {
        localpart.to_string()
    }
}

fn normalize_task_labels(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        if out.len() >= 20 {
            break;
        }
        let Some(label) = clean_text(Some(&value), 64) else {
            continue;
        };
        if !out.iter().any(|existing| existing == &label) {
            out.push(label);
        }
    }
    out
}

fn validate_task_member(field: &str, value: &str, allowed: &[&str]) -> Result<(), CoreError> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(CoreError::Invariant(format!("invalid {field}: {value}")))
}

fn task_transition_allowed(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("created", "accepted")
            | ("accepted" | "blocked", "in_progress")
            | ("in_progress", "blocked" | "done")
    )
}

fn fake_busy_agents(reservations: &[SchedulerReservation]) -> BTreeSet<String> {
    reservations
        .iter()
        .filter(|reservation| matches!(reservation.status.as_str(), "routed" | "drained"))
        .filter_map(|reservation| reservation.agent.clone())
        .collect()
}

fn fake_pool_agent(agent: &AgentRecord, busy: &BTreeSet<String>) -> SchedulerPoolAgent {
    SchedulerPoolAgent {
        name: agent.name.clone(),
        role: fake_effective_role(agent),
        capability: fake_effective_capability(agent),
        online: agent.status == "online",
        busy: busy.contains(&agent.name),
    }
}

fn fake_pool_snapshot(mut agents: Vec<SchedulerPoolAgent>) -> SchedulerPoolSnapshot {
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    let mut grid: BTreeMap<String, BTreeMap<String, Vec<SchedulerPoolAgent>>> = BTreeMap::new();
    for agent in &agents {
        if let Some(role) = agent.role.as_deref() {
            grid.entry(role.to_string())
                .or_default()
                .entry(agent.capability.clone())
                .or_default()
                .push(agent.clone());
        }
    }
    let counts = grid
        .iter()
        .map(|(role, by_tier)| {
            (
                role.clone(),
                by_tier
                    .iter()
                    .map(|(tier, agents)| (tier.clone(), agents.len()))
                    .collect::<BTreeMap<_, _>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    SchedulerPoolSnapshot {
        total: agents.len(),
        grid,
        counts,
        agents,
    }
}

fn fake_select_agent(
    agents: &[AgentRecord],
    busy: &BTreeSet<String>,
    role: &str,
    tier: &str,
) -> Option<AgentRecord> {
    let mut eligible = agents
        .iter()
        .filter(|agent| agent.status == "online")
        .filter(|agent| !busy.contains(&agent.name))
        .filter(|agent| fake_effective_role(agent).as_deref() == Some(role))
        .filter(|agent| fake_tier_rank(&fake_effective_capability(agent)) >= fake_tier_rank(tier))
        .cloned()
        .collect::<Vec<_>>();
    eligible.sort_by(|a, b| {
        fake_tier_rank(&fake_effective_capability(a))
            .cmp(&fake_tier_rank(&fake_effective_capability(b)))
            .then_with(|| a.name.cmp(&b.name))
    });
    eligible.into_iter().next()
}

fn fake_effective_role(agent: &AgentRecord) -> Option<String> {
    agent
        .role
        .as_deref()
        .and_then(|role| clean_text(Some(role), 64))
        .filter(|role| role != "agent")
        .or_else(|| fake_canonical_role(&agent.name))
}

fn fake_effective_capability(agent: &AgentRecord) -> String {
    agent
        .capability
        .as_deref()
        .and_then(fake_clean_tier)
        .or_else(|| fake_effective_role(agent).map(|role| fake_default_tier(&role).to_string()))
        .unwrap_or_else(|| "medium".to_string())
}

fn fake_canonical_role(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    if lower.contains("architect") || lower.contains("coordinator") {
        return Some("architect".to_string());
    }
    if lower.contains("final_reviewer") || lower.contains("final-reviewer") {
        return Some("review".to_string());
    }
    if lower.contains("review") {
        return Some("review".to_string());
    }
    if lower.contains("test") || lower.contains("qa") {
        return Some("testing".to_string());
    }
    if lower.contains("integrat") {
        return Some("integration".to_string());
    }
    if lower.contains("doc") {
        return Some("documentation".to_string());
    }
    if lower.contains("implement") || lower.contains("coder") || lower.contains("coding") {
        return Some("coding".to_string());
    }
    None
}

fn fake_resolve_tier(role: &str, requested: Option<&str>) -> String {
    requested
        .and_then(fake_clean_tier)
        .unwrap_or_else(|| fake_default_tier(role).to_string())
}

fn fake_default_tier(role: &str) -> &'static str {
    match role {
        "architect" | "review" => "strong",
        "documentation" => "lightweight",
        _ => "medium",
    }
}

fn fake_tier_rank(tier: &str) -> i32 {
    match tier {
        "strong" => 2,
        "lightweight" => 0,
        _ => 1,
    }
}

fn fake_clean_tier(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "strong" => Some("strong".to_string()),
        "medium" => Some("medium".to_string()),
        "lightweight" => Some("lightweight".to_string()),
        _ => None,
    }
}

fn fake_tier_runtime(tier: &str) -> Value {
    match tier {
        "strong" => serde_json::json!({"runtime": "claude", "model": "opus"}),
        "lightweight" => serde_json::json!({"runtime": "claude", "model": "haiku"}),
        _ => serde_json::json!({"runtime": "claude", "model": "sonnet"}),
    }
}

fn fake_released_scheduler(agent: String) -> SchedulerReleaseResult {
    SchedulerReleaseResult {
        status: "released".to_string(),
        agent,
        reservation: None,
        ticket: None,
        role: None,
        tier: None,
        task: None,
        room: None,
    }
}

fn fake_validate_task_graph_nodes(
    nodes: &BTreeMap<String, AgentChatTaskGraphNode>,
) -> Result<(), CoreError> {
    if nodes.is_empty() {
        return Err(CoreError::Invariant("nodes required".to_string()));
    }
    let ids = nodes.keys().cloned().collect::<BTreeSet<_>>();
    for (id, node) in nodes {
        if node.id != *id {
            return Err(CoreError::Invariant(format!(
                "node id mismatch: key '{id}' vs id '{}'",
                node.id
            )));
        }
        if clean_text(Some(&node.assignee), 128).is_none()
            && node
                .role
                .as_deref()
                .and_then(|role| clean_text(Some(role), 64))
                .is_none()
        {
            return Err(CoreError::Invariant(format!(
                "node '{id}' assignee or role required"
            )));
        }
        for dep in &node.depends_on {
            if dep == id {
                return Err(CoreError::Invariant(format!(
                    "node '{id}' cannot depend on itself"
                )));
            }
            if !ids.contains(dep) {
                return Err(CoreError::Invariant(format!(
                    "node '{id}' depends on missing node '{dep}'"
                )));
            }
        }
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for id in nodes.keys() {
        fake_visit_task_graph_node(id, nodes, &mut visiting, &mut visited)?;
    }
    Ok(())
}

fn fake_visit_task_graph_node(
    id: &str,
    nodes: &BTreeMap<String, AgentChatTaskGraphNode>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
) -> Result<(), CoreError> {
    if visited.contains(id) {
        return Ok(());
    }
    if !visiting.insert(id.to_string()) {
        return Err(CoreError::Invariant(
            "dependency cycle detected".to_string(),
        ));
    }
    let Some(node) = nodes.get(id) else {
        return Ok(());
    };
    for dep in &node.depends_on {
        fake_visit_task_graph_node(dep, nodes, visiting, visited)?;
    }
    visiting.remove(id);
    visited.insert(id.to_string());
    Ok(())
}

fn fake_advance_task_graph(graph: &mut AgentChatTaskGraphRecord, inbox: &mut Vec<FakeInboxEntry>) {
    if graph.status != "active" {
        return;
    }
    let mut changed = false;
    loop {
        let mut changed_this_pass = false;
        let node_ids = graph.nodes.keys().cloned().collect::<Vec<_>>();
        for node_id in node_ids {
            let Some(snapshot) = graph.nodes.get(&node_id).cloned() else {
                continue;
            };
            if snapshot.status != "pending" {
                continue;
            }
            if snapshot.depends_on.iter().any(|dep| {
                graph
                    .nodes
                    .get(dep)
                    .is_some_and(|node| matches!(node.status.as_str(), "failed" | "cancelled"))
            }) {
                if let Some(node) = graph.nodes.get_mut(&node_id) {
                    node.status = "failed".to_string();
                    node.error = Some("dependency failed".to_string());
                    node.completed_at = Some(fake_task_now());
                }
                changed = true;
                changed_this_pass = true;
                continue;
            }
            let deps_ready = snapshot.depends_on.iter().all(|dep| {
                graph
                    .nodes
                    .get(dep)
                    .is_some_and(|node| matches!(node.status.as_str(), "complete" | "skipped"))
            });
            if !deps_ready {
                continue;
            }
            if !fake_task_graph_condition_matches(graph, &snapshot) {
                if let Some(node) = graph.nodes.get_mut(&node_id) {
                    node.status = "skipped".to_string();
                    node.completed_at = Some(fake_task_now());
                }
                changed = true;
                changed_this_pass = true;
                continue;
            }
            let message = fake_task_graph_dispatch_message(graph, &snapshot);
            if let Some(node) = graph.nodes.get_mut(&node_id) {
                node.status = "dispatched".to_string();
                node.message_id = Some(message.id.clone());
                node.dispatched_at.get_or_insert_with(fake_task_now);
            }
            if !inbox.iter().any(|entry| entry.message.id == message.id) {
                inbox.push(FakeInboxEntry {
                    message,
                    read: false,
                });
            }
            changed = true;
            changed_this_pass = true;
        }
        if !changed_this_pass {
            break;
        }
    }
    if graph
        .nodes
        .values()
        .all(|node| fake_task_graph_node_terminal(&node.status))
    {
        graph.status = if graph.nodes.values().any(|node| node.status == "failed") {
            "failed".to_string()
        } else {
            "complete".to_string()
        };
        graph.completed_at = Some(fake_task_now());
        changed = true;
    }
    if changed {
        graph.updated_at = fake_task_now();
    }
}

fn fake_task_graph_dispatch_message(
    graph: &AgentChatTaskGraphRecord,
    node: &AgentChatTaskGraphNode,
) -> InboxMessage {
    let id = node
        .message_id
        .clone()
        .unwrap_or_else(|| fake_task_graph_dispatch_id(&graph.id, &node.id));
    let dependency_results = node
        .depends_on
        .iter()
        .filter_map(|dep| {
            graph.nodes.get(dep).map(|dep_node| {
                serde_json::json!({
                    "nodeId": dep,
                    "status": dep_node.status.clone(),
                    "result": dep_node.result.clone(),
                    "error": dep_node.error.clone(),
                })
            })
        })
        .collect::<Vec<_>>();
    InboxMessage {
        id,
        ts: 1,
        at: "1970-01-01T00:00:00.001Z".to_string(),
        time: "0s ago".to_string(),
        from: graph.owner.clone(),
        to: node.assignee.clone(),
        message_type: "request".to_string(),
        priority: "high".to_string(),
        summary: format!(
            "Task graph {} / {}: {}",
            graph.id, node.id, node.description
        ),
        full: node.description.clone(),
        mentions: Vec::new(),
        attachments: Vec::new(),
        reply_to: None,
        group: None,
        source: "task_graph".to_string(),
        source_room: None,
        sender_mxid: None,
        trust_level: Some("system".to_string()),
        from_id: Some(graph.owner.clone()),
        schema: Some(serde_json::json!({
            "kind": "task_graph_dispatch",
            "version": 1,
            "payload": {
                "dispatchKey": format!("{}:{}", graph.id, node.id),
                "graphId": graph.id.clone(),
                "nodeId": node.id.clone(),
                "description": node.description.clone(),
                "dependencyResults": dependency_results,
            }
        })),
    }
}

fn fake_apply_task_graph_node_patch(
    node: &mut AgentChatTaskGraphNode,
    input: AgentChatTaskGraphNodePatchInput,
) -> Result<(), CoreError> {
    if let Some(status) = input.status {
        let status = normalize_required_text(&status, 64, "status required")?;
        validate_task_member(
            "node status",
            &status,
            &[
                "pending",
                "dispatched",
                "active",
                "complete",
                "failed",
                "skipped",
                "cancelled",
            ],
        )?;
        node.status.clone_from(&status);
        if status == "active" && node.started_at.is_none() {
            node.started_at = Some(fake_task_now());
        }
        if fake_task_graph_node_terminal(&status) && node.completed_at.is_none() {
            node.completed_at = Some(fake_task_now());
        }
    }
    if let Some(result) = input.result {
        node.result = Some(result);
    }
    if let Some(error) = input.error {
        node.error = clean_text(Some(&error), 1024);
    }
    Ok(())
}

fn fake_task_graph_condition_matches(
    graph: &AgentChatTaskGraphRecord,
    node: &AgentChatTaskGraphNode,
) -> bool {
    let Some(condition) = node.condition.as_ref() else {
        return true;
    };
    let Some(object) = condition.as_object() else {
        return true;
    };
    let dep_id = object
        .get("dep")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(dep) = graph.nodes.get(dep_id) else {
        return false;
    };
    let mut actual = dep.result.as_ref().unwrap_or(&Value::Null);
    if let Some(path) = object.get("path").and_then(Value::as_str) {
        for segment in path.split('.').filter(|segment| !segment.is_empty()) {
            if matches!(segment, "__proto__" | "constructor" | "prototype") {
                return false;
            }
            actual = actual.get(segment).unwrap_or(&Value::Null);
        }
    }
    if let Some(expected) = object.get("eq") {
        return actual == expected;
    }
    if let Some(expected) = object.get("neq") {
        return actual != expected;
    }
    if let Some(values) = object.get("in").and_then(Value::as_array) {
        return values.iter().any(|value| value == actual);
    }
    true
}

fn fake_task_graph_node_terminal(status: &str) -> bool {
    matches!(status, "complete" | "failed" | "skipped" | "cancelled")
}

fn fake_task_graph_dispatch_id(graph_id: &str, node_id: &str) -> String {
    format!(
        "msg_task_graph_dispatch_{}_{}",
        fake_sanitize_id(graph_id),
        fake_sanitize_id(node_id)
    )
}

fn fake_sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}
