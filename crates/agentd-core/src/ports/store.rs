//! The persistence seam. The engine and handlers record run/outcome/park state
//! through this trait; P0.2 backs it with sqlite, P0.1 with `InMemoryStore`.
//!
//! The trait error type is [`CoreError`] (NOT a store-local error): the sqlite
//! impl maps its own `StoreError -> CoreError`, the in-memory fake returns
//! `CoreError` directly. The three `lookup_park_by_*` methods are what
//! `Engine::deliver_event` uses to resolve an incoming event back to the
//! `(run_id, node_id)` that parked (D8/M4).

use std::path::{Path, PathBuf};

use crate::CoreError;
use crate::engine::Checkpoint;
use crate::types::{AgentId, NodeId, Outcome, ReviewRunId, ReviewVerdict, RunId, TaskRunId};

/// Lifecycle state of a run row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Parked,
    Finished,
    Failed,
}

/// Durable run/outcome/park state the engine depends on. All methods are
/// fallible and return [`CoreError`]; the in-memory fake never actually fails
/// I/O but still returns `Result` to match the real store's signature.
#[async_trait::async_trait]
pub trait Store: Send + Sync {
    // ---- runs -------------------------------------------------------------
    /// Create the run row. **Idempotent first-wins**: a re-insert of an existing
    /// `run_id` preserves the existing row (it does NOT reset status/cursor), so
    /// a daemon-pre-created rich run row survives the engine's `insert_run`.
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
        round: u32,
        context_sha: &str,
    ) -> Result<ReviewRunId, CoreError>;
    async fn lookup_park_by_review_run(
        &self,
        review_run_id: &ReviewRunId,
    ) -> Result<Option<(RunId, NodeId)>, CoreError>;
    /// Record a reviewer's verdict. **Idempotent per reviewer**: a second verdict
    /// from the same `reviewer_id` on the same review run is a no-op (first wins),
    /// mirroring the real store's `PRIMARY KEY (review_run_id, reviewer_id)`
    /// (design §3.3). This keeps `count_verdicts` a count of *distinct* reviewers
    /// so a replayed event cannot reach quorum with fewer than N reviewers.
    async fn insert_review_verdict(
        &self,
        review_run_id: &ReviewRunId,
        verdict: ReviewVerdict,
    ) -> Result<(), CoreError>;
    /// Number of *distinct* reviewers who have submitted on this review run.
    async fn count_verdicts(&self, review_run_id: &ReviewRunId) -> Result<usize, CoreError>;
    /// The reviewer count this review run is waiting for (`None` if unknown).
    async fn review_expected(
        &self,
        review_run_id: &ReviewRunId,
    ) -> Result<Option<usize>, CoreError>;
    /// The Delphi round this review run belongs to (`None` if unknown).
    async fn review_round(&self, review_run_id: &ReviewRunId) -> Result<Option<u32>, CoreError>;
    async fn list_verdicts(
        &self,
        review_run_id: &ReviewRunId,
    ) -> Result<Vec<ReviewVerdict>, CoreError>;
    /// Persist the worktree assigned to one reviewer in a review run.
    async fn set_review_worktree(
        &self,
        review_run_id: &ReviewRunId,
        reviewer_id: &AgentId,
        path: &Path,
    ) -> Result<(), CoreError>;
    /// Return and mark consumed the reviewer worktree, if one is still pending
    /// release. This is intentionally take-once so replayed reviewer verdicts
    /// cannot release the same worktree twice.
    async fn take_review_worktree(
        &self,
        review_run_id: &ReviewRunId,
        reviewer_id: &AgentId,
    ) -> Result<Option<PathBuf>, CoreError>;

    // ---- task runs (codergen) --------------------------------------------
    /// Insert a task run and return its generated id.
    async fn insert_task_run(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
    ) -> Result<TaskRunId, CoreError>;
    /// Atomically insert a task run and its validated execution input.
    async fn insert_task_run_with_spec(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
        spec: &crate::types::NativeExecutionSpec,
    ) -> Result<TaskRunId, CoreError> {
        let _ = (run_id, node_id, spec);
        Err(CoreError::Invariant(
            "atomic task execution spec insertion is unsupported by this store".into(),
        ))
    }
    /// Persist the agent id that owns a task run.
    async fn set_task_run_agent(
        &self,
        task_run_id: &TaskRunId,
        agent_id: &AgentId,
    ) -> Result<(), CoreError>;
    /// Persist the immutable, versioned provider execution input for a task.
    /// Implementations without enterprise task execution support fail closed.
    async fn set_task_execution_spec(
        &self,
        task_run_id: &TaskRunId,
        spec: &crate::types::NativeExecutionSpec,
    ) -> Result<(), CoreError> {
        let _ = (task_run_id, spec);
        Err(CoreError::Invariant(
            "task execution specs are unsupported by this store".into(),
        ))
    }
    /// Read the immutable execution input for a task, if one has been set.
    async fn get_task_execution_spec(
        &self,
        task_run_id: &TaskRunId,
    ) -> Result<Option<crate::types::NativeExecutionSpec>, CoreError> {
        let _ = task_run_id;
        Err(CoreError::Invariant(
            "task execution specs are unsupported by this store".into(),
        ))
    }
    /// Persist the worktree assigned to a task run.
    async fn set_task_run_worktree(
        &self,
        task_run_id: &TaskRunId,
        path: &Path,
    ) -> Result<(), CoreError>;
    /// Mark a task run finished so a replayed `AgentOutcomeSubmitted` is a no-op
    /// (mirrors `wait.human`'s close-on-answer; the real store sets `finished_at`).
    async fn complete_task_run(&self, task_run_id: &TaskRunId) -> Result<(), CoreError>;
    /// Returns the parked `(run_id, node_id)` only while the task run is still
    /// open (not yet completed); `None` once completed or unknown (replay no-op).
    async fn lookup_park_by_task_run(
        &self,
        task_run_id: &TaskRunId,
    ) -> Result<Option<(RunId, NodeId)>, CoreError>;

    // ---- checkpoints ------------------------------------------------------
    async fn write_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), CoreError>;
    async fn load_checkpoint(&self, run_id: &RunId) -> Result<Option<Checkpoint>, CoreError>;
}
