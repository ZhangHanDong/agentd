//! `ProductionRunHost` — the real [`RunHost`] (P0.9): the 5 agent-facing methods
//! over a real `SqliteStore` + a per-call `Engine`. It lives in the daemon crate
//! (the composition root) — NOT in `agentd-surface`, which stays store-free
//! (P0.7 D2). agentd-core stays frozen (D1): `deliver` just constructs `Engine`
//! and calls `deliver_event` (which loads the checkpoint and resumes internally).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use agentd_core::CoreError;
use agentd_core::dot::parser;
use agentd_core::engine::{Checkpoint, Engine, EngineEvent, ParkReason, RunProgress};
use agentd_core::graph::NodeGraph;
use agentd_core::handler::{HandlerRegistry, Ports};
use agentd_core::handler::{agent_allocation_json, append_agent_allocation_prompt_context};
use agentd_core::ports::{
    AgentAllocation, AgentAllocationRequest, AgentAllocationStatus, AgentAllocator, AgentBackend,
    Clock, CommandRunner, DirectAgentAllocator, MempalClient, RunStatus, Store, WorktreeAllocator,
};
use agentd_core::types::{
    AgentHandle, AgentId, BackendKind, CliKind, LaunchStrategy, NodeId, ReviewRunId, RunContext,
    RunId, SpawnRequest, TaskRunId,
};
use agentd_specify::{AgentdEventRef, OfflineSpecify, SpecifyClient, report_agentd_event};
use agentd_store::{
    SqliteStore, StoreError, agent_chat_task_graph_repo, agent_chat_task_repo, agent_repo,
    agent_scheduler_repo, event_repo, matrix_bridge_repo, message_repo, relay_repo, review_repo,
    run_repo, task_repo,
};
use agentd_surface::host::{
    AgentChatTaskComment as SurfaceAgentChatTaskComment,
    AgentChatTaskCommentInput as SurfaceAgentChatTaskCommentInput,
    AgentChatTaskCreateInput as SurfaceAgentChatTaskCreateInput,
    AgentChatTaskExecutionInput as SurfaceAgentChatTaskExecutionInput,
    AgentChatTaskGraphCreateInput as SurfaceAgentChatTaskGraphCreateInput,
    AgentChatTaskGraphMessageResult as SurfaceAgentChatTaskGraphMessageResult,
    AgentChatTaskGraphNode as SurfaceAgentChatTaskGraphNode,
    AgentChatTaskGraphNodePatchInput as SurfaceAgentChatTaskGraphNodePatchInput,
    AgentChatTaskGraphRecord as SurfaceAgentChatTaskGraphRecord,
    AgentChatTaskListFilters as SurfaceAgentChatTaskListFilters,
    AgentChatTaskPatchInput as SurfaceAgentChatTaskPatchInput,
    AgentChatTaskRecord as SurfaceAgentChatTaskRecord,
    AgentChatTaskTransitionInput as SurfaceAgentChatTaskTransitionInput,
    AgentDownResult as SurfaceAgentDownResult, AgentHeartbeat as SurfaceAgentHeartbeat,
    AgentLifecycleReport as SurfaceAgentLifecycleReport, AgentOffline as SurfaceAgentOffline,
    AgentRebindResult as SurfaceAgentRebindResult, AgentRecord as SurfaceAgentRecord,
    AgentRegistration as SurfaceAgentRegistration, AgentRuntimeUpdate as SurfaceAgentRuntimeUpdate,
    AgentStartHandle as SurfaceAgentStartHandle, AgentStartResult as SurfaceAgentStartResult,
    DeliveryEventInput as SurfaceDeliveryEventInput,
    DeliveryEventRecord as SurfaceDeliveryEventRecord,
    DirectMessageInput as SurfaceDirectMessageInput, EventRecord,
    GroupCreateInput as SurfaceGroupCreateInput, GroupMemberUpdate as SurfaceGroupMemberUpdate,
    GroupMessageInput as SurfaceGroupMessageInput, GroupReadAdvance as SurfaceGroupReadAdvance,
    GroupReadRequest as SurfaceGroupReadRequest, GroupReadResult as SurfaceGroupReadResult,
    GroupRecord as SurfaceGroupRecord, InboxMessage as SurfaceInboxMessage, LiveEvent,
    MatrixBridgeRoomInput as SurfaceMatrixBridgeRoomInput,
    MatrixBridgeRoomRecord as SurfaceMatrixBridgeRoomRecord,
    MatrixInboundMessageInput as SurfaceMatrixInboundMessageInput,
    MatrixInboundMessageResult as SurfaceMatrixInboundMessageResult,
    MatrixOutboxCursorInput as SurfaceMatrixOutboxCursorInput,
    RelayServerHeartbeat as SurfaceRelayServerHeartbeat,
    RelayServerRecord as SurfaceRelayServerRecord,
    RelayStreamEventRecord as SurfaceRelayStreamEventRecord, RunHost, RunSnapshot, RunSummary,
    SchedulerDispatchInput as SurfaceSchedulerDispatchInput,
    SchedulerDispatchResult as SurfaceSchedulerDispatchResult,
    SchedulerPoolAgent as SurfaceSchedulerPoolAgent,
    SchedulerPoolFilters as SurfaceSchedulerPoolFilters,
    SchedulerPoolSnapshot as SurfaceSchedulerPoolSnapshot,
    SchedulerReleaseInput as SurfaceSchedulerReleaseInput,
    SchedulerReleaseResult as SurfaceSchedulerReleaseResult,
    SchedulerReservation as SurfaceSchedulerReservation, TaskAssignment,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

/// Capacity of the live-event broadcast: a subscriber more than this many events
/// behind lags and is realigned with a snapshot (P1).
const LIVE_BROADCAST_CAPACITY: usize = 256;

/// Per-run lock registry (P2 Foundation A): hands out one `Arc<Mutex>` per run id
/// so the daemon can serialize a run's advancing operations (`deliver`/`start_run`)
/// while different runs proceed concurrently. The std mutex guards only the
/// map's get-or-insert and is never held across an `.await`. No eviction — one
/// entry per distinct run id, bounded by run volume (see p98 Out of Scope).
#[derive(Default)]
struct RunLockRegistry {
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl RunLockRegistry {
    /// The lock for `run_id` — the SAME `Arc` for a given run, a different one
    /// for a different run.
    fn lock_for(&self, run_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.locks.lock().expect("run-lock map poisoned");
        map.entry(run_id.to_string()).or_default().clone()
    }
}

#[derive(Debug)]
struct SchedulerWorkflowAllocator {
    store: SqliteStore,
    config: agent_scheduler_repo::SchedulerConfig,
}

impl SchedulerWorkflowAllocator {
    fn new(store: SqliteStore, max_per_cell: i64) -> Self {
        Self {
            store,
            config: agent_scheduler_repo::SchedulerConfig { max_per_cell },
        }
    }
}

#[async_trait::async_trait]
impl AgentAllocator for SchedulerWorkflowAllocator {
    async fn allocate(&self, req: AgentAllocationRequest) -> Result<AgentAllocation, CoreError> {
        let requested_role = req.role.clone();
        let result = agent_scheduler_repo::dispatch(
            self.store.pool(),
            agent_scheduler_repo::DispatchRequest {
                role: req.role,
                capability: req.capability,
                task: Some(req.task),
                room: Some(req.run_id.as_str().to_string()),
            },
            self.config,
        )
        .await?;
        let allocation = allocation_from_dispatch(requested_role, result)?;
        enrich_scheduler_allocation(&self.store, allocation).await
    }

    async fn release(&self, agent_id: &AgentId) -> Result<Option<AgentAllocation>, CoreError> {
        let result = agent_scheduler_repo::release(
            self.store.pool(),
            agent_scheduler_repo::ReleaseRequest {
                agent: agent_id.as_str().to_string(),
            },
        )
        .await?;
        let Some(allocation) = allocation_from_release(result) else {
            return Ok(None);
        };
        enrich_scheduler_allocation(&self.store, allocation)
            .await
            .map(Some)
    }
}

fn allocation_from_dispatch(
    requested_role: String,
    result: agent_scheduler_repo::DispatchResult,
) -> Result<AgentAllocation, CoreError> {
    let status = allocation_status(&result.status)?;
    let agent = if status == AgentAllocationStatus::Queued {
        requested_role.clone()
    } else {
        result
            .agent
            .as_deref()
            .or(result.name.as_deref())
            .or_else(|| {
                result
                    .reservation
                    .as_ref()
                    .and_then(|reservation| reservation.provisioned_name.as_deref())
            })
            .ok_or_else(|| {
                CoreError::Invariant(format!(
                    "workflow scheduler result '{}' did not include an agent or provisioned name",
                    result.status
                ))
            })?
            .to_string()
    };
    Ok(AgentAllocation {
        requested_role,
        agent_id: AgentId::parsed(&agent),
        status,
        tier: Some(result.tier),
        reservation_id: result
            .reservation
            .as_ref()
            .map(|reservation| reservation.id.clone()),
        ticket: result.ticket,
        provisioned_name: result.name,
        runtime: result.runtime,
    })
}

async fn enrich_scheduler_allocation(
    store: &SqliteStore,
    mut allocation: AgentAllocation,
) -> Result<AgentAllocation, CoreError> {
    if !matches!(
        allocation.status,
        AgentAllocationStatus::Routed | AgentAllocationStatus::Drained
    ) {
        return Ok(allocation);
    }
    let Some(agent) = agent_repo::get_agent(store.pool(), allocation.agent_id.as_str()).await?
    else {
        return Ok(allocation);
    };
    let mut runtime = allocation
        .runtime
        .as_object()
        .cloned()
        .unwrap_or_else(serde_json::Map::new);
    insert_runtime_string(&mut runtime, "runtime", agent.runtime.as_deref());
    insert_runtime_string(&mut runtime, "model", agent.model.as_deref());
    if let Some(tmux_target) = agent.tmux_target.as_deref() {
        insert_runtime_string(&mut runtime, "tmuxTarget", Some(tmux_target));
        insert_runtime_string(&mut runtime, "tmux_target", Some(tmux_target));
    }
    insert_runtime_string(&mut runtime, "homeDir", agent.home_dir.as_deref());
    insert_runtime_string(&mut runtime, "home_dir", agent.home_dir.as_deref());
    insert_runtime_string(&mut runtime, "workdir", agent.workdir.as_deref());
    insert_runtime_string(&mut runtime, "stateDir", agent.state_dir.as_deref());
    insert_runtime_string(&mut runtime, "state_dir", agent.state_dir.as_deref());
    insert_runtime_string(&mut runtime, "server", agent.server.as_deref());
    if meaningful_json(&agent.runtime_profile) {
        runtime.insert("runtimeProfile".to_string(), agent.runtime_profile.clone());
        runtime.insert("runtime_profile".to_string(), agent.runtime_profile);
    }
    if meaningful_json(&agent.runtime_state) {
        runtime.insert("runtimeState".to_string(), agent.runtime_state.clone());
        runtime.insert("runtime_state".to_string(), agent.runtime_state);
    }
    allocation.runtime = Value::Object(runtime);
    Ok(allocation)
}

fn insert_runtime_string(
    runtime: &mut serde_json::Map<String, Value>,
    key: &str,
    value: Option<&str>,
) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    runtime.insert(key.to_string(), Value::String(value.to_string()));
}

fn meaningful_json(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Object(object) => !object.is_empty(),
        _ => true,
    }
}

fn allocation_from_release(
    mut result: agent_scheduler_repo::ReleaseResult,
) -> Option<AgentAllocation> {
    let mut reservation = result.reservation.take()?;
    let status = allocation_status(&result.status).ok()?;
    let mut runtime = reservation.runtime.as_object().cloned().unwrap_or_default();
    if let Some(task) = result.task.take() {
        runtime.insert("schedulerTask".to_string(), task);
    }
    if let Some(room) = result.room.take() {
        runtime.insert("schedulerRoom".to_string(), Value::String(room));
    }
    reservation.runtime = Value::Object(runtime);
    Some(AgentAllocation {
        requested_role: result.role.unwrap_or_else(|| reservation.role.clone()),
        agent_id: AgentId::parsed(&result.agent),
        status,
        tier: result.tier.or(Some(reservation.tier)),
        reservation_id: Some(reservation.id),
        ticket: result.ticket,
        provisioned_name: reservation.provisioned_name,
        runtime: reservation.runtime,
    })
}

fn allocation_status(status: &str) -> Result<AgentAllocationStatus, CoreError> {
    match status {
        "routed" => Ok(AgentAllocationStatus::Routed),
        "queued" => Ok(AgentAllocationStatus::Queued),
        "provision" => Ok(AgentAllocationStatus::Provision),
        "drained" => Ok(AgentAllocationStatus::Drained),
        "direct" => Ok(AgentAllocationStatus::Direct),
        other => Err(CoreError::Invariant(format!(
            "unsupported workflow scheduler status '{other}'"
        ))),
    }
}

/// Shutdown options for the production agent lifecycle port.
#[derive(Debug, Clone)]
pub struct AgentLifecycleShutdown {
    pub archive_to: PathBuf,
}

/// Shutdown result returned by the production agent lifecycle port.
#[derive(Debug, Clone)]
pub struct AgentLifecycleShutdownReport {
    pub method: String,
    pub final_capture_sha: String,
}

/// Stop/rebind operations for registered local runtimes. This is separate from
/// [`AgentBackend`] so workflow dispatch stays focused on prompt delivery.
#[async_trait::async_trait]
pub trait AgentLifecycle: Send + Sync {
    async fn shutdown(
        &self,
        handle: &AgentHandle,
        opts: AgentLifecycleShutdown,
    ) -> Result<AgentLifecycleShutdownReport, CoreError>;

    async fn rebind(&self, target: &str) -> Result<Option<AgentHandle>, CoreError>;
}

#[derive(Debug)]
struct UnconfiguredAgentLifecycle;

#[async_trait::async_trait]
impl AgentLifecycle for UnconfiguredAgentLifecycle {
    async fn shutdown(
        &self,
        _handle: &AgentHandle,
        _opts: AgentLifecycleShutdown,
    ) -> Result<AgentLifecycleShutdownReport, CoreError> {
        Err(CoreError::Backend(
            "agent lifecycle is not configured".to_string(),
        ))
    }

    async fn rebind(&self, _target: &str) -> Result<Option<AgentHandle>, CoreError> {
        Err(CoreError::Backend(
            "agent lifecycle is not configured".to_string(),
        ))
    }
}

/// The daemon's production `RunHost`. Holds the real store + the swappable ports
/// as trait objects (the daemon supplies `TmuxBackend`/`SystemClock`/…; tests
/// supply the in-memory fakes), and re-resolves each run's graph from
/// `runs.workflow_path` under `workflows_dir`.
pub struct ProductionRunHost {
    store: SqliteStore,
    backend: Box<dyn AgentBackend>,
    agent_lifecycle: Box<dyn AgentLifecycle>,
    runner: Box<dyn CommandRunner>,
    mempal: Box<dyn MempalClient>,
    clock: Box<dyn Clock>,
    agent_allocator: Box<dyn AgentAllocator>,
    worktree_allocator: Option<Box<dyn WorktreeAllocator>>,
    specify: Arc<dyn SpecifyClient>,
    accept_workflow_change: bool,
    registry: HandlerRegistry,
    workflows_dir: PathBuf,
    /// The live-event broadcast (P1): the emit point publishes here for the SSE
    /// tail. Lossy/bounded — `send` never blocks the engine on a slow subscriber.
    live_tx: broadcast::Sender<LiveEvent>,
    /// Per-run delivery serialization (P2 Foundation A): one lock per run id, so
    /// concurrent events for one run can't double-advance it.
    run_locks: RunLockRegistry,
}

struct NoopCommandRunner;

#[async_trait::async_trait]
impl CommandRunner for NoopCommandRunner {
    async fn run(
        &self,
        program: &str,
        _args: &[String],
        _opts: agentd_core::ports::RunOpts,
    ) -> Result<agentd_core::ports::CommandOutput, agentd_core::ports::CommandError> {
        Err(agentd_core::ports::CommandError {
            message: format!("command runner placeholder invoked for {program}"),
            stderr: String::new(),
            status: None,
        })
    }
}

struct StableCwdCommandRunner {
    inner: Box<dyn CommandRunner>,
    cwd: PathBuf,
}

impl StableCwdCommandRunner {
    fn new(inner: Box<dyn CommandRunner>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            inner,
            cwd: cwd.into(),
        }
    }
}

#[async_trait::async_trait]
impl CommandRunner for StableCwdCommandRunner {
    async fn run(
        &self,
        program: &str,
        args: &[String],
        mut opts: agentd_core::ports::RunOpts,
    ) -> Result<agentd_core::ports::CommandOutput, agentd_core::ports::CommandError> {
        if opts.cwd.is_none() {
            opts.cwd = Some(self.cwd.clone());
        }
        self.inner.run(program, args, opts).await
    }
}

impl std::fmt::Debug for ProductionRunHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProductionRunHost")
            .field("workflows_dir", &self.workflows_dir)
            .finish_non_exhaustive()
    }
}

impl ProductionRunHost {
    /// Assemble the production host from the real store + the chosen ports.
    pub fn new(
        store: SqliteStore,
        backend: Box<dyn AgentBackend>,
        runner: Box<dyn CommandRunner>,
        mempal: Box<dyn MempalClient>,
        clock: Box<dyn Clock>,
        workflows_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            store,
            backend,
            agent_lifecycle: Box::new(UnconfiguredAgentLifecycle),
            runner,
            mempal,
            clock,
            agent_allocator: Box::new(DirectAgentAllocator),
            worktree_allocator: None,
            specify: Arc::new(OfflineSpecify::new()),
            accept_workflow_change: false,
            registry: HandlerRegistry::with_builtins(),
            workflows_dir: workflows_dir.into(),
            live_tx: broadcast::channel(LIVE_BROADCAST_CAPACITY).0,
            run_locks: RunLockRegistry::default(),
        }
    }

    /// The underlying store (for the daemon's run-start + recovery paths).
    #[must_use]
    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    /// Inject the optional per-`task_run` worktree allocator used by codergen.
    #[must_use]
    pub fn with_worktree_allocator(
        mut self,
        allocator: Option<Box<dyn WorktreeAllocator>>,
    ) -> Self {
        self.worktree_allocator = allocator;
        self
    }

    /// Use the durable agent-chat scheduler for workflow `codergen`/`fan_out`
    /// allocation. Kept opt-in so older direct-spawn workflows and tests keep
    /// their established behavior until a cutover config selects scheduler mode.
    #[must_use]
    pub fn with_scheduler_allocator(mut self, max_per_cell: i64) -> Self {
        self.agent_allocator = Box::new(SchedulerWorkflowAllocator::new(
            self.store.clone(),
            max_per_cell,
        ));
        self
    }

    /// Inject the lifecycle adapter used by operator down/rebind actions.
    #[must_use]
    pub fn with_agent_lifecycle(mut self, agent_lifecycle: Box<dyn AgentLifecycle>) -> Self {
        self.agent_lifecycle = agent_lifecycle;
        self
    }

    /// Set the stable cwd for tool commands that do not request one explicitly.
    /// This keeps event-driven `mcp-stdio` continuations from inheriting a
    /// transient agent or reviewer worktree cwd.
    #[must_use]
    pub fn with_tool_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        let runner = std::mem::replace(&mut self.runner, Box::new(NoopCommandRunner));
        self.runner = Box::new(StableCwdCommandRunner::new(runner, cwd));
        self
    }

    /// Inject the optional Specify semantic-event reporter.
    #[must_use]
    pub fn with_specify_client(mut self, specify: Arc<dyn SpecifyClient>) -> Self {
        self.specify = specify;
        self
    }

    /// Operator policy for resuming a parked run after its workflow file content
    /// sha changed. Default false; daemon config exposes the explicit opt-in.
    #[must_use]
    pub fn with_accept_workflow_change(mut self, accept: bool) -> Self {
        self.accept_workflow_change = accept;
        self
    }

    /// Re-read + build the run's graph from `runs.workflow_path`, returning it
    /// with the current content sha. Errors if the run has no recorded path.
    pub async fn resolve_graph(&self, run_id: &RunId) -> Result<(NodeGraph, String), CoreError> {
        let path = run_repo::workflow_path(self.store.pool(), run_id)
            .await?
            .ok_or_else(|| {
                CoreError::Invariant(format!("run '{}' has no workflow_path", run_id.as_str()))
            })?;
        let full = if Path::new(&path).is_absolute() {
            PathBuf::from(&path)
        } else {
            self.workflows_dir.join(&path)
        };
        let src = std::fs::read_to_string(&full)?;
        let sha = sha256_hex(src.as_bytes());
        let ast = parser::parse(&src).map_err(|e| CoreError::DotParse(format!("{e:?}")))?;
        let graph = NodeGraph::from_ast(&ast)?;
        Ok((graph, sha))
    }

    /// Build a per-call `Engine` over the real ports for `graph`/`sha`.
    fn engine<'a>(&'a self, graph: &'a NodeGraph, sha: &str) -> Engine<'a> {
        Engine::new(
            graph,
            &self.registry,
            Ports {
                backend: &*self.backend,
                runner: &*self.runner,
                store: &self.store,
                mempal: &*self.mempal,
                clock: &*self.clock,
                agent_allocator: &*self.agent_allocator,
            },
            sha.to_string(),
        )
        .with_accept_workflow_change(self.accept_workflow_change)
        .with_worktree_allocator(self.worktree_allocator.as_deref())
    }

    /// Start a run: resolve its graph and execute from the start node to the
    /// first park (or completion), emitting the resulting state-change event.
    /// The daemon's run-start path (POST /runs) and the contract tests call this.
    ///
    /// # Errors
    /// [`CoreError`] on a store/handler/engine failure or an unresolved graph.
    pub async fn start_run(&self, run_id: &RunId) -> Result<RunProgress, CoreError> {
        self.start_run_with_context(run_id, RunContext::new()).await
    }

    /// Start a run with an explicit initial context.
    ///
    /// # Errors
    /// [`CoreError`] on a store/handler/engine failure or an unresolved graph.
    async fn start_run_with_context(
        &self,
        run_id: &RunId,
        initial_context: RunContext,
    ) -> Result<RunProgress, CoreError> {
        // Serialize per run (P2 Foundation A): every run-advancing op on one run
        // is mutually exclusive, so it can't race a concurrent deliver.
        let lock = self.run_locks.lock_for(run_id.as_str());
        let _guard = lock.lock().await;
        let (graph, sha) = self.resolve_graph(run_id).await?;
        let progress = match self
            .engine(&graph, &sha)
            .execute_with_context(run_id, initial_context)
            .await
        {
            Ok(progress) => progress,
            Err(err) => {
                let reason = err.to_string();
                run_repo::update_run_status(self.store.pool(), run_id, RunStatus::Failed).await?;
                RunProgress::Failed {
                    run_id: run_id.clone(),
                    reason,
                }
            }
        };
        self.emit(run_id, &progress).await?;
        Ok(progress)
    }

    /// Emit ONE event row per STATE-CHANGING `RunProgress` (P0.7-deferred emit
    /// point, D6): `Parked`→`run_parked`, `Finished`→`run_finished`,
    /// `Failed`→`run_failed`. `Ignored` emits nothing. The payload is COMPACT
    /// JSON (no newlines — avoids the P0.7 D9 SSE CR/LF hazard).
    async fn emit(&self, run_id: &RunId, progress: &RunProgress) -> Result<(), CoreError> {
        let (kind, mut payload) = match progress {
            RunProgress::Parked {
                node_id,
                reason: ParkReason::ReviewVerdicts { round, .. },
                ..
            } => (
                "run_parked",
                serde_json::json!({ "node": node_id.as_str(), "round": round }),
            ),
            RunProgress::Parked { node_id, .. } => (
                "run_parked",
                serde_json::json!({ "node": node_id.as_str() }),
            ),
            RunProgress::Finished { .. } => ("run_finished", serde_json::json!({})),
            RunProgress::Failed { reason, .. } => {
                ("run_failed", serde_json::json!({ "reason": reason }))
            }
            RunProgress::Ignored { .. } => return Ok(()),
        };
        if kind == "run_parked" {
            if let Some(scheduler) = self.scheduler_event_allocations(run_id).await? {
                if let Some(object) = payload.as_object_mut() {
                    object.insert("scheduler".to_string(), scheduler);
                }
            }
        }
        let payload = payload.to_string();
        // Dedup consecutive same-node re-parks (P1 re-park-noise gap): a fan-out
        // review re-parks at the SAME node per non-final verdict. If the most
        // recent event is already a `run_parked` for this same node, emit nothing
        // — no durable row AND no broadcast. Distinct-node parks and terminals
        // (kind != "run_parked") always fall through.
        if kind == "run_parked" {
            if let Some((last_kind, last_payload)) =
                event_repo::last(self.store.pool(), run_id).await?
            {
                if last_kind == "run_parked" && last_payload == payload {
                    return Ok(());
                }
            }
        }
        // DUAL-WRITE: persist (durable/audit) then broadcast (the live SSE tail).
        let seq = event_repo::append(self.store.pool(), run_id, kind, &payload).await?;
        let payload_for_report = payload.clone();
        // Lossy + non-blocking: an absent/slow subscriber never blocks the engine.
        let _ = self.live_tx.send(LiveEvent {
            run_id: run_id.as_str().to_string(),
            event: EventRecord {
                seq,
                kind: kind.to_string(),
                payload,
            },
        });
        if let Err(err) = report_agentd_event(
            self.specify.as_ref(),
            run_id.as_str(),
            AgentdEventRef {
                run_id: run_id.as_str(),
                seq,
                kind,
                payload: &payload_for_report,
            },
        )
        .await
        {
            tracing::debug!(
                error = %err,
                run_id = run_id.as_str(),
                seq,
                kind,
                "optional Specify event reporting failed"
            );
        }
        Ok(())
    }

    async fn scheduler_event_allocations(
        &self,
        run_id: &RunId,
    ) -> Result<Option<Value>, CoreError> {
        let Some(checkpoint) = self.store.load_checkpoint(run_id).await? else {
            return Ok(None);
        };
        Ok(checkpoint
            .context_snapshot
            .get("agentd_scheduler_allocations")
            .and_then(non_direct_scheduler_allocations))
    }

    async fn wake_drained_workflow_tickets(&self, run_id: &RunId) -> Result<(), CoreError> {
        let Some(checkpoint) = self.store.load_checkpoint(run_id).await? else {
            return Ok(());
        };
        for allocation in drained_workflow_allocations(&checkpoint.context_snapshot) {
            let kind = allocation
                .runtime
                .get("schedulerTask")
                .and_then(|task| task.get("kind"))
                .and_then(Value::as_str);
            match kind {
                Some("workflow_codergen") => {
                    self.dispatch_drained_codergen_allocation(allocation)
                        .await?;
                }
                Some("workflow_fan_out_reviewer") => {
                    self.dispatch_drained_fan_out_allocation(allocation).await?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn dispatch_drained_codergen_allocation(
        &self,
        allocation: AgentAllocation,
    ) -> Result<(), CoreError> {
        let Some(task) = allocation.runtime.get("schedulerTask") else {
            return Ok(());
        };
        if task.get("kind").and_then(Value::as_str) != Some("workflow_codergen") {
            return Ok(());
        }
        let Some(run_id) = task
            .get("runId")
            .and_then(Value::as_str)
            .map(RunId::from_string)
        else {
            return Ok(());
        };
        let Some(node_id) = task
            .get("nodeId")
            .and_then(Value::as_str)
            .map(NodeId::parsed)
        else {
            return Ok(());
        };
        let Some(task_run_id) = task
            .get("taskRunId")
            .and_then(Value::as_str)
            .map(TaskRunId::from_string)
        else {
            return Ok(());
        };
        let Some((open_run_id, open_node_id)) =
            self.store.lookup_park_by_task_run(&task_run_id).await?
        else {
            return Ok(());
        };
        if open_run_id != run_id || open_node_id != node_id {
            return Ok(());
        }
        let Some(mut checkpoint) = self.store.load_checkpoint(&run_id).await? else {
            return Ok(());
        };
        if checkpoint.current_node != node_id {
            return Ok(());
        }
        let Some(queued) =
            take_queued_codergen_dispatch(&mut checkpoint.context_snapshot, &node_id, &task_run_id)
        else {
            return Ok(());
        };

        push_scheduler_allocation(&mut checkpoint.context_snapshot, &node_id, &allocation);
        self.store
            .set_task_run_agent(&task_run_id, &allocation.agent_id)
            .await?;
        self.store.write_checkpoint(&checkpoint).await?;

        let mut prompt = queued.base_prompt;
        append_agent_allocation_prompt_context(&mut prompt, &allocation);
        append_codergen_outcome_submission_context(
            &mut prompt,
            &run_id,
            &node_id,
            allocation.agent_id.as_str(),
            &task_run_id,
        );
        let request = SpawnRequest {
            agent_id: allocation.agent_id.clone(),
            mxid: None,
            cli: cli_kind_for_agent(allocation.agent_id.as_str()),
            worktree: queued.worktree,
            initial_prompt: Some(prompt),
            env_overrides: HashMap::new(),
            launch_strategy: LaunchStrategy::Direct,
        };
        self.backend
            .dispatch_allocated(request, &allocation)
            .await?;
        self.emit(
            &run_id,
            &RunProgress::Parked {
                run_id: run_id.clone(),
                node_id,
                reason: ParkReason::AgentOutcome { task_run_id },
            },
        )
        .await?;
        Ok(())
    }

    async fn dispatch_drained_fan_out_allocation(
        &self,
        allocation: AgentAllocation,
    ) -> Result<(), CoreError> {
        let Some(task) = drained_fan_out_task(&allocation) else {
            return Ok(());
        };
        let Some((open_run_id, open_node_id)) = self
            .store
            .lookup_park_by_review_run(&task.review_run_id)
            .await?
        else {
            return Ok(());
        };
        if open_run_id != task.run_id || open_node_id != task.node_id {
            return Ok(());
        }
        let Some(mut checkpoint) = self.store.load_checkpoint(&task.run_id).await? else {
            return Ok(());
        };
        if checkpoint.current_node != task.node_id {
            return Ok(());
        }
        let Some(queued) = take_queued_fan_out_dispatch(
            &mut checkpoint.context_snapshot,
            &task.node_id,
            &task.review_run_id,
            &task.requested_role,
        ) else {
            return Ok(());
        };

        let review_worktree = self
            .drained_reviewer_worktree(&task.review_run_id, &allocation, &queued)
            .await?;
        self.store
            .set_review_worktree(&task.review_run_id, &allocation.agent_id, &review_worktree)
            .await?;
        push_scheduler_allocation(&mut checkpoint.context_snapshot, &task.node_id, &allocation);
        self.store.write_checkpoint(&checkpoint).await?;

        let mut prompt = queued.base_prompt;
        append_fan_out_review_runtime_context(
            &mut prompt,
            &checkpoint.context_snapshot,
            &queued.source_worktree,
            &review_worktree,
        );
        append_agent_allocation_prompt_context(&mut prompt, &allocation);
        append_fan_out_review_submission_context(
            &mut prompt,
            &task.run_id,
            &task.node_id,
            allocation.agent_id.as_str(),
            &task.review_run_id,
        );
        let request = SpawnRequest {
            agent_id: allocation.agent_id.clone(),
            mxid: None,
            cli: cli_kind_for_agent(allocation.agent_id.as_str()),
            worktree: review_worktree,
            initial_prompt: Some(prompt),
            env_overrides: HashMap::new(),
            launch_strategy: LaunchStrategy::Direct,
        };
        self.backend
            .dispatch_allocated(request, &allocation)
            .await?;
        let expected = self
            .store
            .review_expected(&task.review_run_id)
            .await?
            .ok_or_else(|| {
                CoreError::Invariant(format!(
                    "review run {} vanished before drained wakeup",
                    task.review_run_id.as_str()
                ))
            })?;
        let round = self
            .store
            .review_round(&task.review_run_id)
            .await?
            .ok_or_else(|| {
                CoreError::Invariant(format!(
                    "review run {} vanished before drained wakeup",
                    task.review_run_id.as_str()
                ))
            })?;
        self.emit(
            &task.run_id,
            &RunProgress::Parked {
                run_id: task.run_id.clone(),
                node_id: task.node_id,
                reason: ParkReason::ReviewVerdicts {
                    review_run_id: task.review_run_id,
                    expected,
                    round,
                },
            },
        )
        .await?;
        Ok(())
    }

    async fn drained_reviewer_worktree(
        &self,
        review_run_id: &ReviewRunId,
        allocation: &AgentAllocation,
        queued: &QueuedFanOutDispatch,
    ) -> Result<PathBuf, CoreError> {
        let Some(allocator) = self.worktree_allocator.as_deref() else {
            return Ok(queued.source_worktree.clone());
        };
        let key = reviewer_worktree_key(review_run_id, &allocation.agent_id);
        allocator
            .allocate_snapshot(&key, queued.source_worktree.as_path())
            .await
    }

    /// Resolve which run an inbound event belongs to via the store's park
    /// lookups; `None` if no open park matches (a replayed / already-resolved
    /// event — the replay-safe path).
    async fn run_for_event(&self, event: &EngineEvent) -> Result<Option<RunId>, CoreError> {
        let park = match event {
            EngineEvent::HumanAnswered { wait_id, .. } => {
                self.store.lookup_park_by_wait_id(wait_id).await?
            }
            EngineEvent::ReviewVerdictSubmitted { review_run_id, .. } => {
                self.store.lookup_park_by_review_run(review_run_id).await?
            }
            EngineEvent::AgentOutcomeSubmitted { task_run_id, .. } => {
                self.store.lookup_park_by_task_run(task_run_id).await?
            }
        };
        Ok(park.map(|(run_id, _node)| run_id))
    }
}

fn non_direct_scheduler_allocations(value: &Value) -> Option<Value> {
    let allocations = value.as_object()?;
    let mut filtered = serde_json::Map::new();
    for (node, entries) in allocations {
        let Some(entries) = entries.as_array() else {
            continue;
        };
        let kept = entries
            .iter()
            .filter(|entry| {
                entry
                    .get("schedulerStatus")
                    .and_then(Value::as_str)
                    .is_some_and(|status| status != "direct")
            })
            .cloned()
            .collect::<Vec<_>>();
        if !kept.is_empty() {
            filtered.insert(node.clone(), Value::Array(kept));
        }
    }
    (!filtered.is_empty()).then_some(Value::Object(filtered))
}

#[derive(Debug)]
struct QueuedCodergenDispatch {
    base_prompt: String,
    worktree: PathBuf,
}

#[derive(Debug)]
struct QueuedFanOutDispatch {
    base_prompt: String,
    source_worktree: PathBuf,
}

#[derive(Debug)]
struct DrainedFanOutTask {
    run_id: RunId,
    node_id: NodeId,
    review_run_id: ReviewRunId,
    requested_role: String,
}

fn drained_fan_out_task(allocation: &AgentAllocation) -> Option<DrainedFanOutTask> {
    let task = allocation.runtime.get("schedulerTask")?;
    if task.get("kind").and_then(Value::as_str) != Some("workflow_fan_out_reviewer") {
        return None;
    }
    Some(DrainedFanOutTask {
        run_id: task
            .get("runId")
            .and_then(Value::as_str)
            .map(RunId::from_string)?,
        node_id: task
            .get("nodeId")
            .and_then(Value::as_str)
            .map(NodeId::parsed)?,
        review_run_id: task
            .get("reviewRunId")
            .and_then(Value::as_str)
            .map(ReviewRunId::from_string)?,
        requested_role: task
            .get("requestedRole")
            .and_then(Value::as_str)?
            .to_string(),
    })
}

fn drained_workflow_allocations(context: &RunContext) -> Vec<AgentAllocation> {
    context
        .get("agentd_scheduler_allocations")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|by_node| by_node.values())
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(allocation_from_context_value)
        .filter(|allocation| allocation.status == AgentAllocationStatus::Drained)
        .filter(|allocation| {
            matches!(
                allocation
                    .runtime
                    .get("schedulerTask")
                    .and_then(|task| task.get("kind"))
                    .and_then(Value::as_str),
                Some("workflow_codergen" | "workflow_fan_out_reviewer")
            )
        })
        .collect()
}

fn allocation_from_context_value(value: &Value) -> Option<AgentAllocation> {
    let status = value
        .get("schedulerStatus")
        .and_then(Value::as_str)
        .and_then(|status| allocation_status(status).ok())?;
    Some(AgentAllocation {
        requested_role: value
            .get("requestedRole")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        agent_id: AgentId::parsed(value.get("agentId").and_then(Value::as_str)?),
        status,
        tier: value
            .get("tier")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        reservation_id: value
            .get("schedulerReservationId")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        ticket: value
            .get("schedulerTicket")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        provisioned_name: value
            .get("provisionedName")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        runtime: value.get("runtime").cloned().unwrap_or(Value::Null),
    })
}

fn take_queued_codergen_dispatch(
    context: &mut RunContext,
    node_id: &NodeId,
    task_run_id: &TaskRunId,
) -> Option<QueuedCodergenDispatch> {
    let node_key = node_id.as_str();
    let entry = context
        .get("agentd_queued_workflow_dispatches")
        .and_then(Value::as_object)?
        .get(node_key)?
        .clone();
    if entry.get("handler").and_then(Value::as_str) != Some("codergen") {
        return None;
    }
    if entry.get("taskRunId").and_then(Value::as_str) != Some(task_run_id.as_str()) {
        return None;
    }
    let base_prompt = entry.get("basePrompt").and_then(Value::as_str)?.to_string();
    let worktree = PathBuf::from(entry.get("worktree").and_then(Value::as_str).unwrap_or("."));
    let root_is_empty = if let Some(root) = context
        .0
        .get_mut("agentd_queued_workflow_dispatches")
        .and_then(Value::as_object_mut)
    {
        root.remove(node_key);
        root.is_empty()
    } else {
        false
    };
    if root_is_empty {
        context.0.remove("agentd_queued_workflow_dispatches");
    }
    Some(QueuedCodergenDispatch {
        base_prompt,
        worktree,
    })
}

fn take_queued_fan_out_dispatch(
    context: &mut RunContext,
    node_id: &NodeId,
    review_run_id: &ReviewRunId,
    requested_role: &str,
) -> Option<QueuedFanOutDispatch> {
    let node_key = node_id.as_str();
    let entry = context
        .get("agentd_queued_workflow_dispatches")
        .and_then(Value::as_object)?
        .get(node_key)?
        .clone();
    if entry.get("handler").and_then(Value::as_str) != Some("parallel.fan_out") {
        return None;
    }
    let reviewer = entry
        .get("reviewers")
        .and_then(Value::as_object)?
        .get(requested_role)?
        .clone();
    if reviewer.get("handler").and_then(Value::as_str) != Some("parallel.fan_out") {
        return None;
    }
    if reviewer.get("reviewRunId").and_then(Value::as_str) != Some(review_run_id.as_str()) {
        return None;
    }
    if reviewer.get("requestedRole").and_then(Value::as_str) != Some(requested_role) {
        return None;
    }
    let base_prompt = reviewer
        .get("basePrompt")
        .and_then(Value::as_str)?
        .to_string();
    let source_worktree = PathBuf::from(
        reviewer
            .get("sourceWorktree")
            .and_then(Value::as_str)
            .unwrap_or("."),
    );

    let root_is_empty = if let Some(root) = context
        .0
        .get_mut("agentd_queued_workflow_dispatches")
        .and_then(Value::as_object_mut)
    {
        let remove_node =
            if let Some(node_entry) = root.get_mut(node_key).and_then(Value::as_object_mut) {
                if let Some(reviewers) = node_entry
                    .get_mut("reviewers")
                    .and_then(Value::as_object_mut)
                {
                    reviewers.remove(requested_role);
                    reviewers.is_empty()
                } else {
                    false
                }
            } else {
                false
            };
        if remove_node {
            root.remove(node_key);
        }
        root.is_empty()
    } else {
        false
    };
    if root_is_empty {
        context.0.remove("agentd_queued_workflow_dispatches");
    }
    Some(QueuedFanOutDispatch {
        base_prompt,
        source_worktree,
    })
}

fn push_scheduler_allocation(
    context: &mut RunContext,
    node_id: &NodeId,
    allocation: &AgentAllocation,
) {
    let mut root = context
        .get("agentd_scheduler_allocations")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut values = root
        .remove(node_id.as_str())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    values.push(agent_allocation_json(allocation));
    root.insert(node_id.as_str().to_string(), Value::Array(values));
    context.0.insert(
        "agentd_scheduler_allocations".to_string(),
        Value::Object(root),
    );
}

fn append_codergen_outcome_submission_context(
    prompt: &mut String,
    run_id: &RunId,
    node_id: &NodeId,
    agent_id: &str,
    task_run_id: &TaskRunId,
) {
    use std::fmt::Write as _;

    if !prompt.is_empty() && !prompt.ends_with('\n') {
        prompt.push('\n');
    }
    let _ = writeln!(prompt, "agentd_run_id: {}", run_id.as_str());
    let _ = writeln!(prompt, "agentd_node_id: {}", node_id.as_str());
    let _ = writeln!(prompt, "agentd_agent_id: {agent_id}");
    let _ = writeln!(prompt, "agentd_task_run_id: {}", task_run_id.as_str());
    let _ = writeln!(
        prompt,
        "agentd_submit_outcome: use JSON-RPC tools/call name=submit_outcome arguments={{run_id:\"{}\",node_id:\"{}\",attempt:1,status:\"success|fail|retry|partial_success\",context_updates:{{}}}}",
        run_id.as_str(),
        node_id.as_str()
    );
}

fn append_fan_out_review_runtime_context(
    prompt: &mut String,
    context: &RunContext,
    source_worktree: &Path,
    review_worktree: &Path,
) {
    use std::fmt::Write as _;

    if !prompt.is_empty() && !prompt.ends_with('\n') {
        prompt.push('\n');
    }
    let cwd = std::env::current_dir().map_or_else(
        |_| "<unknown>".to_string(),
        |path| path.display().to_string(),
    );
    let _ = writeln!(prompt, "agentd_daemon_cwd: {cwd}");
    let _ = writeln!(
        prompt,
        "agentd_runtime_path_rule: relative paths in this prompt resolve from agentd_daemon_cwd; review code in review_worktree."
    );
    for key in ["spec_path", "plan_path"] {
        if let Some(value) = context.get(key).and_then(Value::as_str) {
            let _ = writeln!(prompt, "{key}: {value}");
        }
    }
    if let Some(worktree) = context.get("worktree").and_then(Value::as_str) {
        let _ = writeln!(prompt, "implementation_worktree: {worktree}");
    } else {
        let _ = writeln!(
            prompt,
            "implementation_worktree: {}",
            source_worktree.to_string_lossy()
        );
    }
    let _ = writeln!(
        prompt,
        "review_worktree: {}",
        review_worktree.to_string_lossy()
    );
    let _ = writeln!(
        prompt,
        "agentd_review_task: review the current worktree against the listed spec and plan, then submit pass|concern|blocker with findings."
    );
}

fn append_fan_out_review_submission_context(
    prompt: &mut String,
    run_id: &RunId,
    node_id: &NodeId,
    reviewer_id: &str,
    review_run_id: &ReviewRunId,
) {
    use std::fmt::Write as _;

    if !prompt.is_empty() && !prompt.ends_with('\n') {
        prompt.push('\n');
    }
    let _ = writeln!(prompt, "agentd_run_id: {}", run_id.as_str());
    let _ = writeln!(prompt, "agentd_node_id: {}", node_id.as_str());
    let _ = writeln!(prompt, "agentd_reviewer_id: {reviewer_id}");
    let _ = writeln!(prompt, "agentd_review_run_id: {}", review_run_id.as_str());
    let _ = writeln!(
        prompt,
        "agentd_submit_review: use JSON-RPC tools/call name=submit_review arguments={{review_run_id:\"{}\",reviewer_id:\"{}\",verdict:\"pass|concern|blocker\",findings:[]}}",
        review_run_id.as_str(),
        reviewer_id
    );
}

fn reviewer_worktree_key(review_run_id: &ReviewRunId, reviewer_id: &AgentId) -> String {
    format!("review-{}-{}", review_run_id.as_str(), reviewer_id.as_str())
}

fn cli_kind_for_agent(agent_id: &str) -> CliKind {
    if agent_id.starts_with("codex-") {
        CliKind::Codex
    } else {
        CliKind::ClaudeCode
    }
}

#[allow(clippy::too_many_lines)]
#[async_trait::async_trait]
impl RunHost for ProductionRunHost {
    fn subscribe_events(&self) -> broadcast::Receiver<LiveEvent> {
        self.live_tx.subscribe()
    }

    async fn list_runs(&self) -> Result<Vec<RunSummary>, CoreError> {
        let rows = run_repo::list_runs(self.store.pool()).await?;
        Ok(rows
            .into_iter()
            .map(|(run_id, status, current_node, started_at)| RunSummary {
                run_id,
                status,
                current_node,
                started_at,
            })
            .collect())
    }

    async fn operational_doctor(&self) -> Result<Value, CoreError> {
        let report = agentd_store::doctor::OperationalDoctor::new(self.store.pool().clone())
            .check()
            .await
            .map_err(|error| CoreError::Store(error.to_string()))?;
        serde_json::to_value(report).map_err(|error| CoreError::Store(error.to_string()))
    }

    async fn start_workflow(
        &self,
        flow: &str,
        run_id: &RunId,
        context: Value,
    ) -> Result<RunProgress, CoreError> {
        let file = flow_to_file(flow)
            .ok_or_else(|| CoreError::Invariant(format!("unknown flow '{flow}'")))?;
        let initial_context = run_context_from_value(context)?;
        let src = std::fs::read_to_string(self.workflows_dir.join(file))?;
        let sha = sha256_hex(src.as_bytes());
        run_repo::record_run(self.store.pool(), run_id, file, &sha).await?;
        self.start_run_with_context(run_id, initial_context).await
    }

    async fn deliver(&self, event: EngineEvent) -> Result<RunProgress, CoreError> {
        // Resolve the run from the event; an unmatched event is replay-safe Ignored.
        // (Read-only resolve — safe unlocked; concurrent callers for one run all
        // resolve the same run id while the park is open, then serialize below.)
        let Some(run_id) = self.run_for_event(&event).await? else {
            return Ok(RunProgress::Ignored {
                reason: "no open park matches this event (unknown or already resolved)".to_string(),
            });
        };
        // Serialize per run (P2 Foundation A): the mutation runs INSIDE the lock,
        // so a serialized later caller's `deliver_event` re-resolves the gate and
        // sees the prior insert — exactly one caller advances, the rest re-park.
        let lock = self.run_locks.lock_for(run_id.as_str());
        let _guard = lock.lock().await;
        let (graph, sha) = self.resolve_graph(&run_id).await?;
        let progress = self.engine(&graph, &sha).deliver_event(event).await?;
        self.emit(&run_id, &progress).await?;
        self.wake_drained_workflow_tickets(&run_id).await?;
        Ok(progress)
    }

    async fn run_snapshot(&self, run_id: &RunId) -> Result<Option<RunSnapshot>, CoreError> {
        let Some((status, current_from_run)) =
            run_repo::read_status(self.store.pool(), run_id).await?
        else {
            return Ok(None);
        };
        let checkpoint: Option<Checkpoint> = self.store.load_checkpoint(run_id).await?;
        let (current_node, completed_nodes, context) = match checkpoint {
            Some(cp) => (
                Some(cp.current_node.as_str().to_string()),
                cp.completed_nodes
                    .iter()
                    .map(|n| n.as_str().to_string())
                    .collect(),
                Value::Object(cp.context_snapshot.0),
            ),
            None => (
                current_from_run,
                Vec::new(),
                Value::Object(serde_json::Map::new()),
            ),
        };
        Ok(Some(RunSnapshot {
            status,
            current_node,
            completed_nodes,
            context,
        }))
    }

    async fn open_task(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
    ) -> Result<Option<TaskAssignment>, CoreError> {
        let found = task_repo::find_open_task_run(self.store.pool(), run_id, node_id).await?;
        let Some((task_run_id, worktree, agent_id)) = found else {
            return Ok(None);
        };
        let checkpoint = self.store.load_checkpoint(run_id).await?;
        Ok(Some(TaskAssignment {
            task_run_id,
            agent_id: agent_id.unwrap_or_default(),
            worktree,
            spec_path: checkpoint_context_string(checkpoint.as_ref(), "spec_path"),
            plan_path: checkpoint_context_string(checkpoint.as_ref(), "plan_path"),
            context_pack: checkpoint_context_string(checkpoint.as_ref(), "context_pack"),
        }))
    }

    async fn review_counts(
        &self,
        review_run_id: &ReviewRunId,
    ) -> Result<(usize, usize), CoreError> {
        let expected = review_repo::review_expected(self.store.pool(), review_run_id)
            .await?
            .unwrap_or(0);
        let got = review_repo::count_verdicts(self.store.pool(), review_run_id).await?;
        Ok((expected, got))
    }

    async fn events_from(
        &self,
        run_id: &RunId,
        after_seq: i64,
    ) -> Result<Vec<EventRecord>, CoreError> {
        let rows = event_repo::read_from(self.store.pool(), run_id, after_seq).await?;
        Ok(rows
            .into_iter()
            .map(|r| EventRecord {
                seq: r.seq,
                kind: r.kind,
                payload: r.payload,
            })
            .collect())
    }

    async fn register_agent(
        &self,
        input: SurfaceAgentRegistration,
    ) -> Result<SurfaceAgentRecord, CoreError> {
        let record = agent_repo::register_agent(
            self.store.pool(),
            agent_repo::RegisterAgent {
                name: input.name,
                role: input.role,
                capability: input.capability,
                runtime: input.runtime,
                model: input.model,
                tmux_target: input.tmux_target,
                home_dir: input.home_dir,
                workdir: input.workdir,
                state_dir: input.state_dir,
                server: input.server,
                runtime_profile: input.runtime_profile,
            },
        )
        .await?;
        Ok(surface_agent_record(record))
    }

    async fn list_agents(&self) -> Result<Vec<SurfaceAgentRecord>, CoreError> {
        let records = agent_repo::list_agents(self.store.pool()).await?;
        Ok(records.into_iter().map(surface_agent_record).collect())
    }

    async fn get_agent(&self, name: &str) -> Result<Option<SurfaceAgentRecord>, CoreError> {
        Ok(agent_repo::get_agent(self.store.pool(), name)
            .await?
            .map(surface_agent_record))
    }

    async fn update_agent_identity(
        &self,
        name: &str,
        identity: &str,
    ) -> Result<Option<SurfaceAgentRecord>, CoreError> {
        Ok(
            agent_repo::update_agent_identity(self.store.pool(), name, identity)
                .await?
                .map(surface_agent_record),
        )
    }

    async fn heartbeat_agent(
        &self,
        name: &str,
        input: SurfaceAgentHeartbeat,
    ) -> Result<(SurfaceAgentRecord, bool), CoreError> {
        let (record, created) = agent_repo::heartbeat_agent(
            self.store.pool(),
            name,
            agent_repo::HeartbeatAgent {
                server: input.server,
                tmux_target: input.tmux_target,
                workspace_path: input.workspace_path,
            },
        )
        .await?;
        Ok((surface_agent_record(record), created))
    }

    async fn mark_agent_offline(
        &self,
        name: &str,
        input: SurfaceAgentOffline,
    ) -> Result<Option<SurfaceAgentRecord>, CoreError> {
        Ok(agent_repo::mark_agent_offline(
            self.store.pool(),
            name,
            agent_repo::OfflineAgent {
                reason: input.reason,
                clear_tmux: input.clear_tmux,
            },
        )
        .await?
        .map(surface_agent_record))
    }

    async fn start_agent(&self, name: &str) -> Result<Option<SurfaceAgentStartResult>, CoreError> {
        let Some(agent) = agent_repo::get_agent(self.store.pool(), name).await? else {
            return Ok(None);
        };
        if agent.status == "online" {
            return Err(CoreError::Invariant("agent already online".to_string()));
        }
        let cli = runtime_cli_kind(&agent)?;
        let worktree = clean_required(agent.workdir.as_deref(), "agent workdir required")?;
        let req = SpawnRequest {
            agent_id: AgentId::parsed(agent.name.clone()),
            mxid: None,
            cli,
            worktree: PathBuf::from(worktree),
            initial_prompt: Some(format!(
                "agentd_start: registered agent '{}' is now online.",
                agent.name
            )),
            env_overrides: launch_env_overrides(&agent.runtime_profile),
            launch_strategy: LaunchStrategy::Direct,
        };
        let handle = self.backend.spawn(req).await?;
        let started = agent_repo::mark_agent_started(
            self.store.pool(),
            &agent.name,
            agent_repo::StartedAgent {
                tmux_target: handle.address.clone(),
            },
        )
        .await?
        .ok_or_else(|| {
            CoreError::Invariant(format!("started agent '{}' is missing", agent.name))
        })?;
        Ok(Some(SurfaceAgentStartResult {
            agent: surface_agent_record(started),
            handle: surface_agent_handle(handle),
        }))
    }

    async fn down_agent(&self, name: &str) -> Result<Option<SurfaceAgentDownResult>, CoreError> {
        let Some(agent) = agent_repo::get_agent(self.store.pool(), name).await? else {
            return Ok(None);
        };
        let target = clean_required(agent.tmux_target.as_deref(), "agent tmux target required")?;
        let archive_to = agent_down_archive_path(&agent, self.clock.now_unix())?;
        let handle = agent_handle_from_record(&agent, &target);
        let report = self
            .agent_lifecycle
            .shutdown(
                &handle,
                AgentLifecycleShutdown {
                    archive_to: archive_to.clone(),
                },
            )
            .await?;
        let report_method = report.method.clone();
        let report_sha = report.final_capture_sha.clone();
        agent_repo::merge_agent_runtime_state(
            self.store.pool(),
            &agent.name,
            serde_json::json!({
                "lifecycle": {
                    "state": "down",
                    "action": "agent-down-kill",
                    "target": target,
                    "method": report_method,
                    "archivePath": archive_to.to_string_lossy(),
                    "finalCaptureSha": report_sha,
                    "updatedAt": self.clock.now_unix(),
                }
            }),
        )
        .await?;
        let offline = agent_repo::mark_agent_offline(
            self.store.pool(),
            &agent.name,
            agent_repo::OfflineAgent {
                reason: Some("agent-down-kill".to_string()),
                clear_tmux: true,
            },
        )
        .await?
        .ok_or_else(|| CoreError::Invariant(format!("down agent '{}' is missing", agent.name)))?;
        Ok(Some(SurfaceAgentDownResult {
            agent: surface_agent_record(offline),
            report: SurfaceAgentLifecycleReport {
                method: report.method,
                archive_path: Some(archive_to.to_string_lossy().to_string()),
                final_capture_sha: Some(report.final_capture_sha),
            },
        }))
    }

    async fn rebind_agent(
        &self,
        name: &str,
    ) -> Result<Option<SurfaceAgentRebindResult>, CoreError> {
        let Some(agent) = agent_repo::get_agent(self.store.pool(), name).await? else {
            return Ok(None);
        };
        let target = clean_required(agent.tmux_target.as_deref(), "agent tmux target required")?;
        let Some(handle) = self.agent_lifecycle.rebind(&target).await? else {
            agent_repo::merge_agent_runtime_state(
                self.store.pool(),
                &agent.name,
                serde_json::json!({
                    "lifecycle": {
                        "state": "missing",
                        "action": "rebind",
                        "target": target,
                        "updatedAt": self.clock.now_unix(),
                    }
                }),
            )
            .await?;
            let offline = agent_repo::mark_agent_offline(
                self.store.pool(),
                &agent.name,
                agent_repo::OfflineAgent {
                    reason: Some("rebind-missing-session".to_string()),
                    clear_tmux: true,
                },
            )
            .await?
            .ok_or_else(|| {
                CoreError::Invariant(format!("rebind agent '{}' is missing", agent.name))
            })?;
            return Ok(Some(SurfaceAgentRebindResult {
                agent: surface_agent_record(offline),
                handle: None,
                rebound: false,
            }));
        };
        let rebound_target = handle.address.clone();
        let rebound_session = handle.session_name.clone();

        agent_repo::mark_agent_started(
            self.store.pool(),
            &agent.name,
            agent_repo::StartedAgent {
                tmux_target: rebound_target.clone(),
            },
        )
        .await?
        .ok_or_else(|| {
            CoreError::Invariant(format!("rebound agent '{}' is missing", agent.name))
        })?;
        agent_repo::merge_agent_runtime_state(
            self.store.pool(),
            &agent.name,
            serde_json::json!({
                "lifecycle": {
                    "state": "rebound",
                    "action": "rebind",
                    "target": rebound_target,
                    "sessionName": rebound_session,
                    "updatedAt": self.clock.now_unix(),
                }
            }),
        )
        .await?;
        let updated = agent_repo::get_agent(self.store.pool(), &agent.name)
            .await?
            .ok_or_else(|| {
                CoreError::Invariant(format!("rebound agent '{}' is missing", agent.name))
            })?;
        Ok(Some(SurfaceAgentRebindResult {
            agent: surface_agent_record(updated),
            handle: Some(surface_agent_handle(handle)),
            rebound: true,
        }))
    }

    async fn update_agent_runtime(
        &self,
        name: &str,
        input: SurfaceAgentRuntimeUpdate,
    ) -> Result<Option<Value>, CoreError> {
        agent_repo::update_agent_runtime(
            self.store.pool(),
            name,
            agent_repo::RuntimeUpdate {
                blocked: input.blocked,
                blocked_reason: input.reason,
                active_now: input.active_now,
                active_duration_sec: input.active_duration_sec,
                idle_duration_sec: input.idle_duration_sec,
                last_tmux_activity_sec: input.last_tmux_activity_sec,
                workspace_path: input.workspace_path,
                mcp_present: input.mcp_present,
            },
        )
        .await
        .map_err(Into::into)
    }

    async fn record_relay_server_heartbeat(
        &self,
        input: SurfaceRelayServerHeartbeat,
    ) -> Result<SurfaceRelayServerRecord, CoreError> {
        let server_id = clean_required(Some(&input.server), "server required")?;
        let advertised_agents = clean_string_vec(input.agents.clone());
        let advertised_sessions = clean_string_vec(input.sessions.clone());
        let server = relay_repo::record_server_heartbeat(
            self.store.pool(),
            relay_repo::ServerHeartbeatInput {
                server: server_id.clone(),
                instance_id: input.instance_id,
                boot_ts: input.boot_ts,
                agents: advertised_agents.clone(),
                sessions: advertised_sessions.clone(),
            },
        )
        .await?;

        let advertised = advertised_agents
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        for agent in &advertised_agents {
            let target = advertised_sessions
                .iter()
                .find(|session| session_agent_name(session).as_deref() == Some(agent.as_str()))
                .cloned();
            if agent_repo::get_agent(self.store.pool(), agent)
                .await?
                .is_none()
            {
                agent_repo::register_agent(
                    self.store.pool(),
                    agent_repo::RegisterAgent {
                        name: agent.clone(),
                        role: Some("agent".to_string()),
                        capability: None,
                        runtime: Some("codex".to_string()),
                        model: None,
                        tmux_target: target.clone(),
                        home_dir: None,
                        workdir: None,
                        state_dir: None,
                        server: Some(server_id.clone()),
                        runtime_profile: serde_json::json!({
                            "primary": { "framework": "codex" }
                        }),
                    },
                )
                .await?;
            }
            agent_repo::heartbeat_agent(
                self.store.pool(),
                agent,
                agent_repo::HeartbeatAgent {
                    server: Some(server_id.clone()),
                    tmux_target: target,
                    workspace_path: None,
                },
            )
            .await?;
        }

        let existing = agent_repo::list_agents(self.store.pool()).await?;
        for agent in existing {
            if agent.server.as_deref() == Some(server_id.as_str())
                && agent.status == "online"
                && !advertised.contains(&agent.name)
            {
                agent_repo::mark_agent_offline(
                    self.store.pool(),
                    &agent.name,
                    agent_repo::OfflineAgent {
                        reason: Some(format!("heartbeat-missing:{server_id}")),
                        clear_tmux: true,
                    },
                )
                .await?;
            }
        }

        Ok(surface_relay_server(server))
    }

    async fn append_delivery_event(
        &self,
        input: SurfaceDeliveryEventInput,
    ) -> Result<SurfaceDeliveryEventRecord, CoreError> {
        let event = relay_repo::append_delivery_event(
            self.store.pool(),
            relay_repo::DeliveryEventInput {
                event_type: input.event_type,
                message_id: input.message_id,
                queue_entry_id: input.queue_entry_id,
                agent: input.agent,
                target: input.target,
                reason: input.reason,
                source: input.source,
                context: input.context,
            },
        )
        .await?;
        Ok(surface_delivery_event(event))
    }

    async fn list_delivery_events(
        &self,
        agent: &str,
        limit: usize,
    ) -> Result<Vec<SurfaceDeliveryEventRecord>, CoreError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let events =
            relay_repo::list_delivery_events_for_agent(self.store.pool(), agent, limit).await?;
        Ok(events.into_iter().map(surface_delivery_event).collect())
    }

    async fn relay_stream_events(
        &self,
        after_seq: i64,
    ) -> Result<Vec<SurfaceRelayStreamEventRecord>, CoreError> {
        let events = relay_repo::list_relay_stream_events(self.store.pool(), after_seq).await?;
        Ok(events.into_iter().map(surface_relay_stream_event).collect())
    }

    async fn acknowledge_matrix_outbox_cursor(
        &self,
        input: SurfaceMatrixOutboxCursorInput,
    ) -> Result<i64, CoreError> {
        Ok(matrix_bridge_repo::acknowledge_outbox_cursor(
            self.store.pool(),
            &input.bridge_id,
            input.last_seq,
        )
        .await?)
    }

    async fn matrix_outbox_cursor(&self, bridge_id: &str) -> Result<i64, CoreError> {
        Ok(matrix_bridge_repo::get_outbox_cursor(self.store.pool(), bridge_id).await?)
    }

    async fn upsert_matrix_bridge_room(
        &self,
        input: SurfaceMatrixBridgeRoomInput,
    ) -> Result<SurfaceMatrixBridgeRoomRecord, CoreError> {
        let room_id = clean_required(Some(&input.room_id), "matrix room id required")?;
        let group = clean_optional_string(input.group);
        let agent = clean_optional_string(input.agent);
        if group.is_some() == agent.is_some() {
            return Err(CoreError::Invariant(
                "exactly one of group or agent required".to_string(),
            ));
        }

        if let Some(group) = group.as_deref() {
            let members = clean_string_vec(input.members);
            if message_repo::get_group(self.store.pool(), group)
                .await?
                .is_some()
            {
                if !members.is_empty() {
                    let empty = Vec::new();
                    let _ = message_repo::update_group_members(
                        self.store.pool(),
                        group,
                        &members,
                        &empty,
                    )
                    .await?;
                }
            } else {
                let _ = message_repo::create_group(
                    self.store.pool(),
                    message_repo::GroupCreateInput {
                        name: group.to_string(),
                        members,
                    },
                )
                .await?;
            }
        }

        if let Some(project_id) = input.project_id.as_deref() {
            if agentd_store::cutover_repo::get(self.store.pool(), project_id)
                .await?
                .is_none()
            {
                return Err(CoreError::Invariant(
                    "matrix room project binding requires durable cutover state".into(),
                ));
            }
        }

        let room = matrix_bridge_repo::upsert_room(
            self.store.pool(),
            matrix_bridge_repo::MatrixBridgeRoomInput {
                room_id,
                project_id: input.project_id,
                group_name: group,
                agent_name: agent,
                trusted: input.trusted,
                trust_reason: input.trust_reason,
                inviter_mxid: input.inviter_mxid,
            },
        )
        .await?;
        Ok(surface_matrix_bridge_room(room))
    }

    async fn get_matrix_bridge_room(
        &self,
        room_id: &str,
    ) -> Result<Option<SurfaceMatrixBridgeRoomRecord>, CoreError> {
        let room = matrix_bridge_repo::get_room(self.store.pool(), room_id).await?;
        Ok(room.map(surface_matrix_bridge_room))
    }

    async fn post_matrix_inbound_message(
        &self,
        input: SurfaceMatrixInboundMessageInput,
    ) -> Result<SurfaceMatrixInboundMessageResult, CoreError> {
        let room = matrix_bridge_repo::get_room(self.store.pool(), &input.room_id)
            .await?
            .ok_or_else(|| CoreError::Invariant("matrix room not trusted".into()))?;
        if let Some(project_id) = room.project_id.as_deref() {
            let state = agentd_store::cutover_repo::get(self.store.pool(), project_id)
                .await?
                .ok_or_else(|| {
                    CoreError::Invariant("matrix project cutover state not found".into())
                })?;
            let phase_allows_ingress = matches!(
                state.phase,
                agentd_store::cutover_repo::CutoverPhase::Observe
                    | agentd_store::cutover_repo::CutoverPhase::Shadow
                    | agentd_store::cutover_repo::CutoverPhase::Canary
                    | agentd_store::cutover_repo::CutoverPhase::Cutover
            );
            if !phase_allows_ingress {
                return Err(CoreError::Invariant(
                    "matrix project is not accepting ingress".into(),
                ));
            }
            if input.project_id.as_deref() != Some(project_id)
                || input.authority_revision.as_deref() != Some(state.authority_revision.as_str())
                || input.lease_epoch != Some(state.lease_epoch)
            {
                return Err(CoreError::Invariant(
                    "matrix project authority or lease fence mismatch".into(),
                ));
            }
        }
        let event_id = clean_required(Some(&input.event_id), "matrix event id required")?;
        if let Some(existing) = matrix_bridge_repo::get_event(self.store.pool(), &event_id).await? {
            return Ok(SurfaceMatrixInboundMessageResult {
                ok: true,
                duplicate: true,
                ignored: existing.ignored,
                route: existing.route,
                event_id: existing.event_id,
                message_id: existing.message_id,
                message: None,
            });
        }

        let room_id = clean_required(Some(&input.room_id), "matrix room id required")?;
        let sender_mxid = clean_required(Some(&input.sender_mxid), "matrix sender mxid required")?;
        let Some(room) = matrix_bridge_repo::get_room(self.store.pool(), &room_id).await? else {
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
            let event = matrix_bridge_repo::record_event(
                self.store.pool(),
                matrix_bridge_repo::MatrixBridgeEventInput {
                    event_id,
                    room_id,
                    sender_mxid,
                    message_id: None,
                    route: "ignored".to_string(),
                    ignored: true,
                },
            )
            .await?;
            return Ok(SurfaceMatrixInboundMessageResult {
                ok: true,
                duplicate: false,
                ignored: true,
                route: event.route,
                event_id: event.event_id,
                message_id: None,
                message: None,
            });
        }

        let from =
            clean_optional_string(input.from).unwrap_or_else(|| matrix_sender_name(&sender_mxid));
        let trust_level =
            clean_optional_string(input.trust_level).or_else(|| Some("external".to_string()));
        let (route, message) = if let Some(group) = room.group_name.clone() {
            (
                "group".to_string(),
                self.post_group_message(SurfaceGroupMessageInput {
                    message_id: None,
                    ts: None,
                    from,
                    group,
                    message_type: Some("human".to_string()),
                    priority: None,
                    summary: input.body.clone(),
                    full: input.body,
                    mentions: clean_string_vec(input.mentions),
                    reply_to: input.reply_to,
                    source: Some("matrix".to_string()),
                    schema: None,
                    attachments: Vec::new(),
                })
                .await?,
            )
        } else if let Some(agent) = room.agent_name.clone() {
            (
                "agent".to_string(),
                self.post_direct_message(SurfaceDirectMessageInput {
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
                    source_room: Some(room_id.clone()),
                    sender_mxid: Some(sender_mxid.clone()),
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

        let event = matrix_bridge_repo::record_event(
            self.store.pool(),
            matrix_bridge_repo::MatrixBridgeEventInput {
                event_id,
                room_id,
                sender_mxid,
                message_id: Some(message.id.clone()),
                route: route.clone(),
                ignored: false,
            },
        )
        .await?;
        Ok(SurfaceMatrixInboundMessageResult {
            ok: true,
            duplicate: false,
            ignored: false,
            route: event.route,
            event_id: event.event_id,
            message_id: event.message_id,
            message: Some(message),
        })
    }

    async fn scheduler_pool(
        &self,
        filters: SurfaceSchedulerPoolFilters,
    ) -> Result<SurfaceSchedulerPoolSnapshot, CoreError> {
        let snapshot = agent_scheduler_repo::pool_snapshot(
            self.store.pool(),
            agent_scheduler_repo::PoolFilters {
                role: filters.role,
                capability: filters.capability,
                state: filters.state,
            },
        )
        .await?;
        Ok(surface_scheduler_pool(snapshot))
    }

    async fn scheduler_dispatch(
        &self,
        input: SurfaceSchedulerDispatchInput,
        max_per_cell: i64,
    ) -> Result<SurfaceSchedulerDispatchResult, CoreError> {
        let result = agent_scheduler_repo::dispatch(
            self.store.pool(),
            agent_scheduler_repo::DispatchRequest {
                role: input.role,
                capability: input.capability,
                task: input.task,
                room: input.room,
            },
            agent_scheduler_repo::SchedulerConfig { max_per_cell },
        )
        .await?;
        Ok(surface_scheduler_dispatch(result))
    }

    async fn scheduler_release(
        &self,
        input: SurfaceSchedulerReleaseInput,
    ) -> Result<SurfaceSchedulerReleaseResult, CoreError> {
        let result = agent_scheduler_repo::release(
            self.store.pool(),
            agent_scheduler_repo::ReleaseRequest { agent: input.agent },
        )
        .await?;
        Ok(surface_scheduler_release(result))
    }

    async fn create_agent_chat_task(
        &self,
        input: SurfaceAgentChatTaskCreateInput,
    ) -> Result<SurfaceAgentChatTaskRecord, CoreError> {
        let record = agent_chat_task_repo::create_task(
            self.store.pool(),
            agent_chat_task_repo::CreateAgentChatTask {
                title: input.title,
                description: input.description,
                priority: input.priority,
                granularity: input.granularity,
                assignee: input.assignee,
                created_by: input.created_by,
                parent_id: input.parent_id,
                labels: input.labels,
            },
        )
        .await
        .map_err(core_from_store_error)?;
        Ok(surface_agent_chat_task(record))
    }

    async fn list_agent_chat_tasks(
        &self,
        filters: SurfaceAgentChatTaskListFilters,
    ) -> Result<Vec<SurfaceAgentChatTaskRecord>, CoreError> {
        let records = agent_chat_task_repo::list_tasks(
            self.store.pool(),
            agent_chat_task_repo::AgentChatTaskFilters {
                assignee: filters.assignee,
                statuses: filters.statuses,
                priority: filters.priority,
                label: filters.label,
                offset: filters.offset,
                limit: filters.limit,
            },
        )
        .await
        .map_err(core_from_store_error)?;
        Ok(records.into_iter().map(surface_agent_chat_task).collect())
    }

    async fn get_agent_chat_task(
        &self,
        id: &str,
    ) -> Result<Option<SurfaceAgentChatTaskRecord>, CoreError> {
        agent_chat_task_repo::get_task(self.store.pool(), id)
            .await
            .map(|record| record.map(surface_agent_chat_task))
            .map_err(core_from_store_error)
    }

    async fn update_agent_chat_task(
        &self,
        id: &str,
        input: SurfaceAgentChatTaskPatchInput,
    ) -> Result<Option<SurfaceAgentChatTaskRecord>, CoreError> {
        agent_chat_task_repo::update_task(
            self.store.pool(),
            id,
            agent_chat_task_repo::UpdateAgentChatTask {
                title: input.title,
                description: input.description,
                priority: input.priority,
                granularity: input.granularity,
                assignee: input.assignee,
                labels: input.labels,
                parent_id: input.parent_id,
            },
        )
        .await
        .map(|record| record.map(surface_agent_chat_task))
        .map_err(core_from_store_error)
    }

    async fn update_agent_chat_task_execution(
        &self,
        id: &str,
        input: SurfaceAgentChatTaskExecutionInput,
    ) -> Result<Option<SurfaceAgentChatTaskRecord>, CoreError> {
        agent_chat_task_repo::update_task_execution(
            self.store.pool(),
            id,
            agent_chat_task_repo::UpdateAgentChatTaskExecution {
                heartbeat_at: input.heartbeat_at,
                waiting_reason: input.waiting_reason,
                waiting_until: input.waiting_until,
            },
        )
        .await
        .map(|record| record.map(surface_agent_chat_task))
        .map_err(core_from_store_error)
    }

    async fn transition_agent_chat_task(
        &self,
        id: &str,
        input: SurfaceAgentChatTaskTransitionInput,
    ) -> Result<Option<SurfaceAgentChatTaskRecord>, CoreError> {
        let status = input
            .status
            .ok_or_else(|| CoreError::Invariant("status is required".to_string()))?;
        agent_chat_task_repo::transition_task(
            self.store.pool(),
            id,
            agent_chat_task_repo::TransitionAgentChatTask {
                status,
                waiting_reason: input.waiting_reason,
                waiting_until: input.waiting_until,
            },
        )
        .await
        .map(|record| record.map(surface_agent_chat_task))
        .map_err(core_from_store_error)
    }

    async fn add_agent_chat_task_comment(
        &self,
        id: &str,
        input: SurfaceAgentChatTaskCommentInput,
    ) -> Result<Option<SurfaceAgentChatTaskRecord>, CoreError> {
        agent_chat_task_repo::add_comment(
            self.store.pool(),
            id,
            agent_chat_task_repo::AddAgentChatTaskComment {
                author: input.author,
                text: input.text,
            },
        )
        .await
        .map(|record| record.map(surface_agent_chat_task))
        .map_err(core_from_store_error)
    }

    async fn delete_agent_chat_task(
        &self,
        id: &str,
    ) -> Result<Option<SurfaceAgentChatTaskRecord>, CoreError> {
        agent_chat_task_repo::delete_task(self.store.pool(), id)
            .await
            .map(|record| record.map(surface_agent_chat_task))
            .map_err(core_from_store_error)
    }

    async fn create_agent_chat_task_graph(
        &self,
        input: SurfaceAgentChatTaskGraphCreateInput,
    ) -> Result<SurfaceAgentChatTaskGraphRecord, CoreError> {
        let graph = agent_chat_task_graph_repo::create_graph(
            self.store.pool(),
            agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
                id: input.id,
                owner: input.owner,
                label: input.label,
                nodes: input
                    .nodes
                    .into_iter()
                    .map(|(id, node)| {
                        (
                            id,
                            agent_chat_task_graph_repo::AgentChatTaskGraphNodeInput {
                                id: node.id,
                                assignee: node.assignee,
                                role: node.role,
                                capability: node.capability,
                                description: node.description,
                                depends_on: node.depends_on,
                                condition: node.condition,
                            },
                        )
                    })
                    .collect(),
            },
        )
        .await
        .map_err(core_from_store_error)?;
        let graph = agent_chat_task_graph_repo::advance_graph(self.store.pool(), &graph.id)
            .await
            .map_err(core_from_store_error)?
            .ok_or_else(|| CoreError::Invariant("created task graph is missing".to_string()))?;
        Ok(surface_agent_chat_task_graph(graph))
    }

    async fn list_agent_chat_task_graphs(
        &self,
        status: Option<String>,
    ) -> Result<Vec<SurfaceAgentChatTaskGraphRecord>, CoreError> {
        let records = agent_chat_task_graph_repo::list_graphs(self.store.pool(), status.as_deref())
            .await
            .map_err(core_from_store_error)?;
        Ok(records
            .into_iter()
            .map(surface_agent_chat_task_graph)
            .collect())
    }

    async fn get_agent_chat_task_graph(
        &self,
        id: &str,
    ) -> Result<Option<SurfaceAgentChatTaskGraphRecord>, CoreError> {
        agent_chat_task_graph_repo::get_graph(self.store.pool(), id)
            .await
            .map(|record| record.map(surface_agent_chat_task_graph))
            .map_err(core_from_store_error)
    }

    async fn delete_agent_chat_task_graph(
        &self,
        id: &str,
    ) -> Result<Option<SurfaceAgentChatTaskGraphRecord>, CoreError> {
        agent_chat_task_graph_repo::delete_graph(self.store.pool(), id)
            .await
            .map(|record| record.map(surface_agent_chat_task_graph))
            .map_err(core_from_store_error)
    }

    async fn update_agent_chat_task_graph_node(
        &self,
        graph_id: &str,
        node_id: &str,
        input: SurfaceAgentChatTaskGraphNodePatchInput,
    ) -> Result<
        Option<(
            SurfaceAgentChatTaskGraphRecord,
            SurfaceAgentChatTaskGraphNode,
        )>,
        CoreError,
    > {
        agent_chat_task_graph_repo::update_node_and_advance(
            self.store.pool(),
            graph_id,
            node_id,
            agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
                status: input.status,
                result: input.result,
                error: input.error,
            },
        )
        .await
        .map(|record| {
            record.map(|(graph, node)| {
                (
                    surface_agent_chat_task_graph(graph),
                    surface_agent_chat_task_graph_node(node),
                )
            })
        })
        .map_err(core_from_store_error)
    }

    async fn handle_agent_chat_task_graph_message(
        &self,
        from: &str,
        reply_to: Option<String>,
        schema: Option<Value>,
    ) -> Result<Option<SurfaceAgentChatTaskGraphMessageResult>, CoreError> {
        agent_chat_task_graph_repo::handle_result_message(
            self.store.pool(),
            from,
            reply_to.as_deref(),
            schema.as_ref(),
        )
        .await
        .map(|result| {
            result.map(|result| SurfaceAgentChatTaskGraphMessageResult {
                handled: true,
                graph_id: result.graph_id,
                node_id: result.node_id,
                status: result.status,
                graph: surface_agent_chat_task_graph(result.graph),
            })
        })
        .map_err(core_from_store_error)
    }

    async fn create_group(
        &self,
        input: SurfaceGroupCreateInput,
    ) -> Result<SurfaceGroupRecord, CoreError> {
        let record = message_repo::create_group(
            self.store.pool(),
            message_repo::GroupCreateInput {
                name: input.name,
                members: input.members,
            },
        )
        .await?;
        Ok(surface_group_record(record))
    }

    async fn list_groups(&self) -> Result<Vec<SurfaceGroupRecord>, CoreError> {
        let records = message_repo::list_groups(self.store.pool()).await?;
        Ok(records.into_iter().map(surface_group_record).collect())
    }

    async fn get_group(&self, name: &str) -> Result<Option<SurfaceGroupRecord>, CoreError> {
        message_repo::get_group(self.store.pool(), name)
            .await
            .map(|record| record.map(surface_group_record))
            .map_err(Into::into)
    }

    async fn update_group_members(
        &self,
        name: &str,
        input: SurfaceGroupMemberUpdate,
    ) -> Result<Option<SurfaceGroupRecord>, CoreError> {
        message_repo::update_group_members(self.store.pool(), name, &input.add, &input.remove)
            .await
            .map(|record| record.map(surface_group_record))
            .map_err(Into::into)
    }

    async fn delete_group(&self, name: &str) -> Result<Option<SurfaceGroupRecord>, CoreError> {
        message_repo::delete_group(self.store.pool(), name)
            .await
            .map(|record| record.map(surface_group_record))
            .map_err(Into::into)
    }

    async fn post_direct_message(
        &self,
        input: SurfaceDirectMessageInput,
    ) -> Result<SurfaceInboxMessage, CoreError> {
        let record = message_repo::insert_direct_message(
            self.store.pool(),
            message_repo::DirectMessageInput {
                message_id: input.message_id,
                ts: input.ts,
                from: input.from,
                to: input.to,
                message_type: input.message_type,
                priority: input.priority,
                summary: input.summary,
                full: input.full,
                reply_to: input.reply_to,
                source: input.source,
                source_room: input.source_room,
                sender_mxid: input.sender_mxid,
                trust_level: input.trust_level,
                from_id: input.from_id,
                schema: input.schema,
                attachments: input.attachments,
            },
        )
        .await?;
        relay_repo::append_relay_stream_event(
            self.store.pool(),
            "message",
            serde_json::json!({
                "messageId": &record.id,
                "agent": &record.to,
                "target": &record.to,
                "from": &record.from,
                "summary": &record.summary,
                "priority": &record.priority,
                "source": &record.source,
                "kind": "direct",
            }),
        )
        .await?;
        Ok(surface_inbox_message(record))
    }

    async fn post_group_message(
        &self,
        input: SurfaceGroupMessageInput,
    ) -> Result<SurfaceInboxMessage, CoreError> {
        let record = message_repo::insert_group_message(
            self.store.pool(),
            message_repo::GroupMessageInput {
                message_id: input.message_id,
                ts: input.ts,
                from: input.from,
                group: input.group,
                message_type: input.message_type,
                priority: input.priority,
                summary: input.summary,
                full: input.full,
                mentions: input.mentions,
                reply_to: input.reply_to,
                source: input.source,
                schema: input.schema,
                attachments: input.attachments,
            },
        )
        .await?;
        relay_repo::append_relay_stream_event(
            self.store.pool(),
            "message",
            serde_json::json!({
                "messageId": &record.id,
                "agent": &record.group,
                "target": &record.group,
                "from": &record.from,
                "summary": &record.summary,
                "priority": &record.priority,
                "source": &record.source,
                "kind": "group",
                "mentions": &record.mentions,
            }),
        )
        .await?;
        Ok(surface_group_inbox_message(record))
    }

    async fn check_inbox(
        &self,
        agent_id: &str,
        drain: bool,
    ) -> Result<Vec<SurfaceInboxMessage>, CoreError> {
        let rows = message_repo::read_agent_inbox(
            self.store.pool(),
            agent_id,
            message_repo::InboxReadOptions { drain },
        )
        .await?;
        let mut messages = rows
            .dm
            .into_iter()
            .map(surface_inbox_message)
            .collect::<Vec<_>>();
        messages.extend(rows.group.into_iter().map(surface_group_inbox_message));
        Ok(messages)
    }

    async fn read_group_messages(
        &self,
        input: SurfaceGroupReadRequest,
    ) -> Result<SurfaceGroupReadResult, CoreError> {
        let result = message_repo::read_group_messages(
            self.store.pool(),
            &input.group,
            &input.agent_id,
            message_repo::GroupReadOptions {
                limit: input.limit,
                unread_limit: input.unread_limit,
                advance: match input.advance {
                    SurfaceGroupReadAdvance::None => message_repo::GroupReadAdvance::None,
                    SurfaceGroupReadAdvance::All => message_repo::GroupReadAdvance::All,
                },
            },
        )
        .await?;
        Ok(SurfaceGroupReadResult {
            group: result.group,
            unread: result
                .unread
                .into_iter()
                .map(surface_group_inbox_message)
                .collect(),
            read: result
                .read
                .into_iter()
                .map(surface_group_inbox_message)
                .collect(),
            unread_total: result.unread_total,
            unread_returned: result.unread_returned,
            unread_omitted: result.unread_omitted,
            advance: result.advance.as_str().to_string(),
        })
    }
}

fn surface_agent_record(record: agent_repo::AgentRecord) -> SurfaceAgentRecord {
    SurfaceAgentRecord {
        id: record.id,
        name: record.name,
        role: record.role,
        capability: record.capability,
        runtime: record.runtime,
        model: record.model,
        tmux_target: record.tmux_target,
        home_dir: record.home_dir,
        workdir: record.workdir,
        state_dir: record.state_dir,
        server: record.server,
        status: record.status,
        offline_reason: record.offline_reason,
        last_seen_at: record.last_seen_at,
        registered_at: record.registered_at,
        updated_at: record.updated_at,
        runtime_profile: record.runtime_profile,
        runtime_state: record.runtime_state,
    }
}

fn surface_relay_server(record: relay_repo::RelayServerRecord) -> SurfaceRelayServerRecord {
    SurfaceRelayServerRecord {
        id: record.id,
        instance_id: record.instance_id,
        boot_ts: record.boot_ts,
        agents: record.agents,
        sessions: record.sessions,
        agent_count: record.agent_count,
        online: record.online,
        maintenance: record.maintenance,
        last_seen_at: record.last_seen_at,
        heartbeat_at: record.heartbeat_at,
        updated_at: record.updated_at,
    }
}

fn surface_delivery_event(record: relay_repo::DeliveryEventRecord) -> SurfaceDeliveryEventRecord {
    SurfaceDeliveryEventRecord {
        id: record.id,
        seq: record.seq,
        event_type: record.event_type,
        message_id: record.message_id,
        queue_entry_id: record.queue_entry_id,
        agent: record.agent,
        target: record.target,
        reason: record.reason,
        source: record.source,
        context: record.context,
        created_at: record.created_at,
    }
}

fn surface_relay_stream_event(
    record: relay_repo::RelayStreamEventRecord,
) -> SurfaceRelayStreamEventRecord {
    SurfaceRelayStreamEventRecord {
        seq: record.seq,
        event: record.event,
        payload: record.payload,
        created_at: record.created_at,
    }
}

fn surface_matrix_bridge_room(
    record: matrix_bridge_repo::MatrixBridgeRoomRecord,
) -> SurfaceMatrixBridgeRoomRecord {
    SurfaceMatrixBridgeRoomRecord {
        room_id: record.room_id,
        project_id: record.project_id,
        group: record.group_name,
        agent: record.agent_name,
        trusted: record.trusted,
        trust_reason: record.trust_reason,
        inviter_mxid: record.inviter_mxid,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn surface_scheduler_pool(
    snapshot: agent_scheduler_repo::PoolSnapshot,
) -> SurfaceSchedulerPoolSnapshot {
    SurfaceSchedulerPoolSnapshot {
        grid: snapshot
            .grid
            .into_iter()
            .map(|(role, by_tier)| {
                (
                    role,
                    by_tier
                        .into_iter()
                        .map(|(tier, agents)| {
                            (
                                tier,
                                agents.into_iter().map(surface_scheduler_agent).collect(),
                            )
                        })
                        .collect(),
                )
            })
            .collect(),
        counts: snapshot.counts,
        total: snapshot.total,
        agents: snapshot
            .agents
            .into_iter()
            .map(surface_scheduler_agent)
            .collect(),
    }
}

fn surface_scheduler_agent(agent: agent_scheduler_repo::PoolAgent) -> SurfaceSchedulerPoolAgent {
    SurfaceSchedulerPoolAgent {
        name: agent.name,
        role: agent.role,
        capability: agent.capability,
        online: agent.online,
        busy: agent.busy,
    }
}

fn surface_scheduler_reservation(
    reservation: agent_scheduler_repo::SchedulerReservation,
) -> SurfaceSchedulerReservation {
    SurfaceSchedulerReservation {
        id: reservation.id,
        role: reservation.role,
        tier: reservation.tier,
        agent: reservation.agent,
        provisioned_name: reservation.provisioned_name,
        status: reservation.status,
        task: reservation.task,
        room: reservation.room,
        runtime: reservation.runtime,
        ticket: reservation.ticket,
        created_at: reservation.created_at,
        updated_at: reservation.updated_at,
        released_at: reservation.released_at,
    }
}

fn surface_scheduler_dispatch(
    result: agent_scheduler_repo::DispatchResult,
) -> SurfaceSchedulerDispatchResult {
    SurfaceSchedulerDispatchResult {
        status: result.status,
        role: result.role,
        tier: result.tier,
        agent: result.agent,
        reservation: result.reservation.map(surface_scheduler_reservation),
        ticket: result.ticket,
        queue_depth: result.queue_depth,
        name: result.name,
        runtime: result.runtime,
    }
}

fn surface_scheduler_release(
    result: agent_scheduler_repo::ReleaseResult,
) -> SurfaceSchedulerReleaseResult {
    SurfaceSchedulerReleaseResult {
        status: result.status,
        agent: result.agent,
        reservation: result.reservation.map(surface_scheduler_reservation),
        ticket: result.ticket,
        role: result.role,
        tier: result.tier,
        task: result.task,
        room: result.room,
    }
}

fn surface_agent_chat_task(
    record: agent_chat_task_repo::AgentChatTaskRecord,
) -> SurfaceAgentChatTaskRecord {
    SurfaceAgentChatTaskRecord {
        id: record.id,
        title: record.title,
        description: record.description,
        status: record.status,
        priority: record.priority,
        granularity: record.granularity,
        assignee: record.assignee,
        created_by: record.created_by,
        created_at: record.created_at,
        updated_at: record.updated_at,
        started_at: record.started_at,
        completed_at: record.completed_at,
        heartbeat_at: record.heartbeat_at,
        waiting_reason: record.waiting_reason,
        waiting_until: record.waiting_until,
        parent_id: record.parent_id,
        labels: record.labels,
        health: record.health,
        comments: record
            .comments
            .into_iter()
            .map(surface_agent_chat_task_comment)
            .collect(),
    }
}

fn surface_agent_chat_task_comment(
    comment: agent_chat_task_repo::AgentChatTaskComment,
) -> SurfaceAgentChatTaskComment {
    SurfaceAgentChatTaskComment {
        author: comment.author,
        text: comment.text,
        ts: comment.ts,
    }
}

fn surface_agent_chat_task_graph(
    graph: agent_chat_task_graph_repo::AgentChatTaskGraphRecord,
) -> SurfaceAgentChatTaskGraphRecord {
    SurfaceAgentChatTaskGraphRecord {
        id: graph.id,
        owner: graph.owner,
        label: graph.label,
        status: graph.status,
        nodes: graph
            .nodes
            .into_iter()
            .map(|(id, node)| (id, surface_agent_chat_task_graph_node(node)))
            .collect(),
        created_at: graph.created_at,
        updated_at: graph.updated_at,
        completed_at: graph.completed_at,
    }
}

fn surface_agent_chat_task_graph_node(
    node: agent_chat_task_graph_repo::AgentChatTaskGraphNode,
) -> SurfaceAgentChatTaskGraphNode {
    SurfaceAgentChatTaskGraphNode {
        id: node.id,
        assignee: node.assignee,
        role: node.role,
        capability: node.capability,
        tier: node.tier,
        scheduler_reservation_id: node.scheduler_reservation_id,
        scheduler_ticket: node.scheduler_ticket,
        scheduler_status: node.scheduler_status,
        provisioned_name: node.provisioned_name,
        runtime: node.runtime,
        description: node.description,
        depends_on: node.depends_on,
        status: node.status,
        result: node.result,
        error: node.error,
        condition: node.condition,
        message_id: node.message_id,
        started_at: node.started_at,
        dispatched_at: node.dispatched_at,
        completed_at: node.completed_at,
    }
}

fn surface_group_record(record: message_repo::GroupRecord) -> SurfaceGroupRecord {
    SurfaceGroupRecord {
        name: record.name,
        members: record.members,
        created_at: record.created_at,
    }
}

fn surface_inbox_message(record: message_repo::DirectMessageRecord) -> SurfaceInboxMessage {
    SurfaceInboxMessage {
        id: record.id,
        ts: record.ts,
        at: record.at,
        time: record.time,
        from: record.from,
        to: record.to,
        message_type: record.message_type,
        priority: record.priority,
        summary: record.summary,
        full: record.full,
        mentions: Vec::new(),
        attachments: record.attachments,
        reply_to: record.reply_to,
        group: None,
        source: record.source,
        source_room: record.source_room,
        sender_mxid: record.sender_mxid,
        trust_level: record.trust_level,
        from_id: record.from_id,
        schema: record.schema,
    }
}

fn surface_group_inbox_message(record: message_repo::GroupMessageRecord) -> SurfaceInboxMessage {
    SurfaceInboxMessage {
        id: record.id,
        ts: record.ts,
        at: record.at,
        time: record.time,
        from: record.from,
        to: String::new(),
        message_type: record.message_type,
        priority: record.priority,
        summary: record.summary,
        full: record.full,
        mentions: record.mentions,
        attachments: record.attachments,
        reply_to: record.reply_to,
        group: Some(record.group),
        source: record.source,
        source_room: None,
        sender_mxid: None,
        trust_level: None,
        from_id: None,
        schema: record.schema,
    }
}

fn clean_required(value: Option<&str>, message: &str) -> Result<String, CoreError> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CoreError::Invariant(message.to_string()))
}

fn clean_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
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

fn runtime_cli_kind(agent: &agent_repo::AgentRecord) -> Result<CliKind, CoreError> {
    let runtime = agent
        .runtime
        .as_deref()
        .or_else(|| {
            agent
                .runtime_profile
                .get("primary")
                .and_then(|primary| primary.get("framework"))
                .and_then(|framework| framework.as_str())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CoreError::Invariant(
                "agent has no valid framework; update agent runtime to codex or claude".to_string(),
            )
        })?;
    match runtime.to_ascii_lowercase().as_str() {
        "codex" => Ok(CliKind::Codex),
        "claude" | "claude-code" | "claude_code" => Ok(CliKind::ClaudeCode),
        other => Err(CoreError::Invariant(format!(
            "unsupported agent runtime '{other}'"
        ))),
    }
}

fn launch_env_overrides(runtime_profile: &Value) -> HashMap<String, String> {
    let mut env = HashMap::new();
    let Some(primary) = runtime_profile.get("primary") else {
        return env;
    };
    push_profile_env(&mut env, primary, "apiBaseUrl", "ANTHROPIC_BASE_URL");
    push_profile_env(&mut env, primary, "apiKey", "ANTHROPIC_API_KEY");
    push_profile_env(&mut env, primary, "model", "AGENTCHAT_LAUNCH_MODEL");
    push_profile_env(
        &mut env,
        primary,
        "extraArgs",
        "AGENTCHAT_LAUNCH_EXTRA_ARGS",
    );
    env
}

fn push_profile_env(env: &mut HashMap<String, String>, primary: &Value, key: &str, env_key: &str) {
    if let Some(value) = primary
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        env.insert(env_key.to_string(), value.to_string());
    }
}

fn surface_agent_handle(handle: AgentHandle) -> SurfaceAgentStartHandle {
    SurfaceAgentStartHandle {
        agent_id: handle.agent_id.as_str().to_string(),
        backend: match handle.backend {
            BackendKind::Tmux => "tmux".to_string(),
        },
        address: handle.address,
        pane_id: handle.pane_id,
        pid: handle.pid,
        session_name: handle.session_name,
    }
}

fn core_from_store_error(error: StoreError) -> CoreError {
    match error {
        StoreError::Invariant(message) => CoreError::Invariant(message),
        other => other.into(),
    }
}

fn clean_string_vec(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .filter_map(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .collect()
}

fn session_agent_name(session: &str) -> Option<String> {
    let name = session.split(':').next()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn agent_handle_from_record(agent: &agent_repo::AgentRecord, target: &str) -> AgentHandle {
    let session_name = target.split(':').next().unwrap_or(target).to_string();
    AgentHandle {
        agent_id: AgentId::parsed(agent.name.clone()),
        backend: BackendKind::Tmux,
        address: target.to_string(),
        pane_id: None,
        pid: None,
        session_name,
        spawned_at: SystemTime::now(),
    }
}

fn agent_down_archive_path(
    agent: &agent_repo::AgentRecord,
    now: i64,
) -> Result<PathBuf, CoreError> {
    let safe_name = safe_path_fragment(&agent.name);
    let dir = if let Some(state_dir) = agent
        .state_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        PathBuf::from(state_dir).join("lifecycle")
    } else if let Some(workdir) = agent
        .workdir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        PathBuf::from(workdir).join(".agentd").join("lifecycle")
    } else {
        std::env::temp_dir()
            .join("agentd")
            .join("agents")
            .join(&safe_name)
            .join("lifecycle")
    };
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("{safe_name}-{now}-down.log")))
}

fn safe_path_fragment(value: &str) -> String {
    let out = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if out.is_empty() {
        "agent".to_string()
    } else {
        out
    }
}

fn checkpoint_context_string(checkpoint: Option<&Checkpoint>, key: &str) -> Option<String> {
    checkpoint?
        .context_snapshot
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn run_context_from_value(value: Value) -> Result<RunContext, CoreError> {
    match value {
        Value::Null => Ok(RunContext::new()),
        Value::Object(map) => Ok(RunContext(map)),
        other => Err(CoreError::Invariant(format!(
            "initial workflow context must be a JSON object or null, got {}",
            json_type_name(&other)
        ))),
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Map a wire flow name to its shipped `.dot` file, or `None` for an unknown
/// flow. The single source of truth for the daemon side of the flow triple
/// (clap-derived value / `agentctl::cli::Flow::name` / this arm) — kept as a
/// pure fn so the mapping is unit-testable without starting a run (§7.3 P1.7).
fn flow_to_file(flow: &str) -> Option<&'static str> {
    match flow {
        "draft" => Some("draft.dot"),
        "execute" => Some("execute.dot"),
        "spike" => Some("spike.dot"),
        "docs-only" => Some("docs-only.dot"),
        "bugfix-rapid" => Some("bugfix-rapid.dot"),
        "refactor-only" => Some("refactor-only.dot"),
        "bootstrap" => Some("bootstrap.dot"),
        _ => None,
    }
}

/// Lowercase hex SHA-256 of `data` (the workflow content sha).
fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{ProductionRunHost, RunLockRegistry, flow_to_file};
    use crate::SystemClock;
    use agentd_core::engine::{ParkReason, RunProgress};
    use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
    use agentd_core::types::{NodeId, ReviewRunId, RunId, TaskRunId};
    use agentd_specify::{
        AcceptanceReport, DraftReceipt, DraftSpec, FrozenSpec, IssueContext, SemanticEvent,
        SpecifyClient, SpecifyError,
    };
    use agentd_store::{SqliteStore, run_repo};
    use agentd_surface::host::RunHost;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex as StdMutex};

    fn workflows_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows")
    }

    #[derive(Debug, Clone)]
    struct RuntimeSpecifyClient {
        events: Arc<StdMutex<Vec<SemanticEvent>>>,
        fail_report: bool,
    }

    impl RuntimeSpecifyClient {
        fn recording() -> Self {
            Self {
                events: Arc::new(StdMutex::new(Vec::new())),
                fail_report: false,
            }
        }

        fn failing() -> Self {
            Self {
                events: Arc::new(StdMutex::new(Vec::new())),
                fail_report: true,
            }
        }

        fn events(&self) -> Vec<SemanticEvent> {
            self.events.lock().expect("events lock").clone()
        }
    }

    #[async_trait::async_trait]
    impl SpecifyClient for RuntimeSpecifyClient {
        async fn pull_issue_context(&self, _issue_id: &str) -> Result<IssueContext, SpecifyError> {
            Err(SpecifyError::MissingScriptedResponse {
                operation: "pull_issue_context",
            })
        }

        async fn push_draft(&self, _draft: DraftSpec) -> Result<DraftReceipt, SpecifyError> {
            Err(SpecifyError::MissingScriptedResponse {
                operation: "push_draft",
            })
        }

        async fn pull_frozen_spec(
            &self,
            _spec_id: &str,
            _version: &str,
        ) -> Result<FrozenSpec, SpecifyError> {
            Err(SpecifyError::MissingScriptedResponse {
                operation: "pull_frozen_spec",
            })
        }

        async fn report_event(&self, event: SemanticEvent) -> Result<(), SpecifyError> {
            if self.fail_report {
                return Err(SpecifyError::Transport("runtime report failed".to_string()));
            }
            self.events.lock().expect("events lock").push(event);
            Ok(())
        }

        async fn report_acceptance(&self, _report: AcceptanceReport) -> Result<(), SpecifyError> {
            Err(SpecifyError::MissingScriptedResponse {
                operation: "report_acceptance",
            })
        }
    }

    async fn host_for_emit_tests() -> (ProductionRunHost, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::connect(&dir.path().join("agentd.db"))
            .await
            .expect("connect");
        let host = ProductionRunHost::new(
            store,
            Box::new(FakeBackend::new()),
            Box::new(RecordingCommandRunner::new()),
            Box::new(MempalStub::new()),
            Box::new(SystemClock),
            workflows_dir(),
        );
        (host, dir)
    }

    #[test]
    fn run_lock_is_per_run() {
        // The mechanism the per-run serialization rests on: concurrent callers for
        // ONE run must contend on the SAME lock; different runs must not.
        let reg = RunLockRegistry::default();
        let a1 = reg.lock_for("r1");
        let a2 = reg.lock_for("r1");
        let b = reg.lock_for("r2");
        assert!(Arc::ptr_eq(&a1, &a2), "same run id -> the same lock");
        assert!(
            !Arc::ptr_eq(&a1, &b),
            "different run id -> a different lock (cross-run delivery stays concurrent)"
        );
    }

    #[test]
    fn flow_to_file_resolves_every_shipped_flow() {
        // Every shipped flow name maps to a file that EXISTS — catches a
        // start_workflow arm that drifts from the actual file or the CLI name.
        for flow in [
            "draft",
            "execute",
            "spike",
            "docs-only",
            "bugfix-rapid",
            "refactor-only",
            "bootstrap",
        ] {
            let mapped = flow_to_file(flow);
            assert!(
                mapped.is_some(),
                "flow '{flow}' must map to a file, not 'unknown flow'"
            );
            let file = mapped.expect("just asserted Some");
            assert!(
                workflows_dir().join(file).exists(),
                "flow '{flow}' -> '{file}' must exist under workflows/"
            );
        }
        assert!(flow_to_file("bogus").is_none(), "unknown flow -> None");
    }

    #[tokio::test]
    async fn production_runhost_emits_review_reparks_when_round_differs() {
        let (host, _dir) = host_for_emit_tests().await;
        let run = RunId::from_string("round-dedup");
        run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
            .await
            .expect("record");

        for round in [1, 2] {
            host.emit(
                &run,
                &RunProgress::Parked {
                    run_id: run.clone(),
                    node_id: NodeId::parsed("review"),
                    reason: ParkReason::ReviewVerdicts {
                        review_run_id: ReviewRunId::from_string(format!("rr{round}")),
                        expected: 3,
                        round,
                    },
                },
            )
            .await
            .expect("emit");
        }

        let events = host.events_from(&run, 0).await.expect("events");
        let payloads: Vec<&str> = events
            .iter()
            .filter(|e| e.kind == "run_parked")
            .map(|e| e.payload.as_str())
            .collect();
        assert_eq!(
            payloads,
            vec![
                r#"{"node":"review","round":1}"#,
                r#"{"node":"review","round":2}"#
            ],
            "different review rounds survive payload-based dedup"
        );
    }

    #[tokio::test]
    async fn production_runhost_reports_appended_events_to_specify_client() {
        let (host, _dir) = host_for_emit_tests().await;
        let specify = RuntimeSpecifyClient::recording();
        let host = host.with_specify_client(Arc::new(specify.clone()));
        let run = RunId::from_string("specify-report");
        run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
            .await
            .expect("record");

        host.emit(
            &run,
            &RunProgress::Parked {
                run_id: run.clone(),
                node_id: NodeId::parsed("implement"),
                reason: ParkReason::AgentOutcome {
                    task_run_id: TaskRunId::from_string("tr-specify-report"),
                },
            },
        )
        .await
        .expect("emit");

        let durable = host.events_from(&run, 0).await.expect("events");
        assert_eq!(durable.len(), 1);
        assert_eq!(durable[0].kind, "run_parked");
        assert_eq!(durable[0].payload, r#"{"node":"implement"}"#);

        let reported = specify.events();
        assert_eq!(reported.len(), 1);
        assert_eq!(reported[0].workflow_id, "specify-report");
        assert_eq!(reported[0].kind, "agent.blocked");
        assert_eq!(
            reported[0].payload,
            serde_json::json!({
                "run_id": "specify-report",
                "seq": durable[0].seq,
                "agentd_event_kind": "run_parked",
                "payload": { "node": "implement" }
            })
        );
    }

    #[tokio::test]
    async fn production_runhost_does_not_report_deduped_reparks() {
        let (host, _dir) = host_for_emit_tests().await;
        let specify = RuntimeSpecifyClient::recording();
        let host = host.with_specify_client(Arc::new(specify.clone()));
        let run = RunId::from_string("specify-dedup");
        run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
            .await
            .expect("record");

        for _ in 0..2 {
            host.emit(
                &run,
                &RunProgress::Parked {
                    run_id: run.clone(),
                    node_id: NodeId::parsed("implement"),
                    reason: ParkReason::AgentOutcome {
                        task_run_id: TaskRunId::from_string("tr-specify-dedup"),
                    },
                },
            )
            .await
            .expect("emit");
        }

        let durable = host.events_from(&run, 0).await.expect("events");
        assert_eq!(durable.len(), 1, "second same-node re-park is suppressed");
        assert_eq!(
            specify.events().len(),
            1,
            "suppressed re-park is not semantically reported"
        );
    }

    #[tokio::test]
    async fn production_runhost_ignores_specify_report_errors_after_durable_emit() {
        let (host, _dir) = host_for_emit_tests().await;
        let specify = RuntimeSpecifyClient::failing();
        let host = host.with_specify_client(Arc::new(specify));
        let run = RunId::from_string("specify-fail");
        run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
            .await
            .expect("record");
        let mut live = host.subscribe_events();

        host.emit(
            &run,
            &RunProgress::Finished {
                run_id: run.clone(),
            },
        )
        .await
        .expect("emit ignores optional report error");

        let durable = host.events_from(&run, 0).await.expect("events");
        assert_eq!(durable.len(), 1);
        assert_eq!(durable[0].kind, "run_finished");

        let received = live.recv().await.expect("live event");
        assert_eq!(received.run_id, "specify-fail");
        assert_eq!(received.event.kind, "run_finished");
    }

    #[tokio::test]
    async fn production_runhost_default_specify_reporting_preserves_standalone_mode() {
        let (host, _dir) = host_for_emit_tests().await;
        let run = RunId::from_string("specify-default");
        run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
            .await
            .expect("record");

        host.emit(
            &run,
            &RunProgress::Failed {
                run_id: run.clone(),
                reason: "boom".to_string(),
            },
        )
        .await
        .expect("default offline specify reporter is no-op");

        let durable = host.events_from(&run, 0).await.expect("events");
        assert_eq!(durable.len(), 1);
        assert_eq!(durable[0].kind, "run_failed");
        assert_eq!(durable[0].payload, r#"{"reason":"boom"}"#);
    }

    #[test]
    fn runtime_specify_reporting_keeps_boundary() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace = manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root");
        let bin_manifest =
            std::fs::read_to_string(manifest_dir.join("Cargo.toml")).expect("bin manifest");
        let core_manifest = std::fs::read_to_string(
            workspace
                .join("crates")
                .join("agentd-core")
                .join("Cargo.toml"),
        )
        .expect("core manifest");
        let surface_manifest = std::fs::read_to_string(
            workspace
                .join("crates")
                .join("agentd-surface")
                .join("Cargo.toml"),
        )
        .expect("surface manifest");

        assert!(
            bin_manifest.contains("agentd-specify"),
            "agentd-bin owns the runtime Specify dependency"
        );
        assert!(
            !core_manifest.contains("agentd-specify"),
            "agentd-core stays free of runtime Specify wiring"
        );
        assert!(
            !surface_manifest.contains("agentd-specify"),
            "agentd-surface stays free of runtime Specify wiring"
        );
        for forbidden in ["reqwest", "tokio-tungstenite", "url"] {
            assert!(
                !bin_manifest.contains(forbidden),
                "P145 does not add network dependency {forbidden}"
            );
        }
    }
}
