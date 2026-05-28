//! The persistence seam. The engine and handlers record run/outcome/park state
//! through this trait; P0.2 backs it with sqlite, P0.1 with `InMemoryStore`.
//!
//! The trait error type is [`CoreError`] (NOT a store-local error): the sqlite
//! impl maps its own `StoreError -> CoreError`, the in-memory fake returns
//! `CoreError` directly. The three `lookup_park_by_*` methods are what
//! `Engine::deliver_event` uses to resolve an incoming event back to the
//! `(run_id, node_id)` that parked (D8/M4).

use crate::CoreError;
use crate::engine::Checkpoint;
use crate::types::{AgentId, NodeId, Outcome, ReviewRunId, RunId, TaskRunId};

/// Lifecycle state of a run row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Parked,
    Finished,
    Failed,
}

/// A reviewer's vote in a fan-out review. `fan_in` aggregates these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerdictValue {
    Pass,
    Fail,
    Block,
}

/// One recorded review verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewVerdict {
    pub reviewer_id: AgentId,
    pub value: VerdictValue,
}

/// Durable run/outcome/park state the engine depends on. All methods are
/// fallible and return [`CoreError`]; the in-memory fake never actually fails
/// I/O but still returns `Result` to match the real store's signature.
#[async_trait::async_trait]
pub trait Store: Send + Sync {
    // ---- runs -------------------------------------------------------------
    async fn insert_run(&self, run_id: &RunId, workflow_sha: &str) -> Result<(), CoreError>;
    async fn update_run_status(&self, run_id: &RunId, status: RunStatus) -> Result<(), CoreError>;
    async fn set_current_node(&self, run_id: &RunId, node_id: &NodeId) -> Result<(), CoreError>;

    // ---- node outcomes ----------------------------------------------------
    async fn insert_node_outcome(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
        outcome: &Outcome,
    ) -> Result<(), CoreError>;
    async fn latest_outcome(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
    ) -> Result<Option<Outcome>, CoreError>;
    async fn count_attempts(&self, run_id: &RunId, node_id: &NodeId) -> Result<usize, CoreError>;

    // ---- human waits ------------------------------------------------------
    /// Open a human-wait row and return its generated `wait_id`.
    async fn open_human_wait(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
        prompt: &str,
    ) -> Result<String, CoreError>;
    /// Answer an open wait. Errors with [`CoreError::Store`] if already answered.
    async fn answer_human_wait(
        &self,
        wait_id: &str,
        answer: &str,
        feedback: Option<&str>,
    ) -> Result<(), CoreError>;
    async fn lookup_park_by_wait_id(
        &self,
        wait_id: &str,
    ) -> Result<Option<(RunId, NodeId)>, CoreError>;

    // ---- review runs (fan_out / fan_in) ----------------------------------
    /// Insert a review run and return its generated id. `context_sha` pins the
    /// context the reviewers saw (D7 — computed in memory, no disk bundle).
    async fn insert_review_run(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
        expected: usize,
        context_sha: &str,
    ) -> Result<ReviewRunId, CoreError>;
    async fn lookup_park_by_review_run(
        &self,
        review_run_id: &ReviewRunId,
    ) -> Result<Option<(RunId, NodeId)>, CoreError>;
    async fn insert_review_verdict(
        &self,
        review_run_id: &ReviewRunId,
        verdict: ReviewVerdict,
    ) -> Result<(), CoreError>;
    async fn count_verdicts(&self, review_run_id: &ReviewRunId) -> Result<usize, CoreError>;
    async fn list_verdicts(
        &self,
        review_run_id: &ReviewRunId,
    ) -> Result<Vec<ReviewVerdict>, CoreError>;

    // ---- task runs (codergen) --------------------------------------------
    /// Insert a task run and return its generated id.
    async fn insert_task_run(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
    ) -> Result<TaskRunId, CoreError>;
    async fn lookup_park_by_task_run(
        &self,
        task_run_id: &TaskRunId,
    ) -> Result<Option<(RunId, NodeId)>, CoreError>;

    // ---- checkpoints ------------------------------------------------------
    async fn write_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), CoreError>;
    async fn load_checkpoint(&self, run_id: &RunId) -> Result<Option<Checkpoint>, CoreError>;
}
