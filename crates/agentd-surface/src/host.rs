//! `RunHost` — the agent↔engine/store seam the MCP tools sit on. The production
//! host (constructs an `Engine` with the real ports + the run's graph and reads
//! the store/checkpoint) is the daemon's job, wired in P0.9; tests inject a
//! `FakeRunHost`. The engine is `Engine<'a>` (per-call, borrow-based), so the
//! tools must not hold it directly — they hold `Arc<dyn RunHost>`.

use agentd_core::CoreError;
use agentd_core::types::{NodeId, ReviewRunId, RunId, TaskRunId};
use agentd_core::{EngineEvent, RunProgress};
use serde_json::Value;

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

/// The seam: deliver engine events and read the bits the tools need.
#[async_trait::async_trait]
pub trait RunHost: Send + Sync {
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
}
