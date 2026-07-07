//! `RunHost` — the agent↔engine/store seam the MCP tools sit on. The production
//! host (constructs an `Engine` with the real ports + the run's graph and reads
//! the store/checkpoint) is the daemon's job, wired in P0.9; tests inject a
//! `FakeRunHost`. The engine is `Engine<'a>` (per-call, borrow-based), so the
//! tools must not hold it directly — they hold `Arc<dyn RunHost>`.

use agentd_core::CoreError;
use agentd_core::types::{NodeId, ReviewRunId, RunId, TaskRunId};
use agentd_core::{EngineEvent, RunProgress};
use serde::Serialize;
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
}
