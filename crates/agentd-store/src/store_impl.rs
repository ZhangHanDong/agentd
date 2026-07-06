//! `impl agentd_core::ports::Store for SqliteStore` — delegates each engine-facing
//! method to its repo. Repos return `StoreError`; `?` converts to the trait's
//! `CoreError` via the `From` impl in `error.rs`.

use agentd_core::CoreError;
use agentd_core::engine::Checkpoint;
use agentd_core::ports::{RunStatus, Store};
use agentd_core::types::{AgentId, NodeId, Outcome, ReviewRunId, ReviewVerdict, RunId, TaskRunId};
use std::path::{Path, PathBuf};

use crate::store::SqliteStore;
use crate::{checkpoint_repo, human_wait_repo, outcome_repo, review_repo, run_repo, task_repo};

#[async_trait::async_trait]
impl Store for SqliteStore {
    async fn insert_run(&self, run_id: &RunId, workflow_sha: &str) -> Result<(), CoreError> {
        Ok(run_repo::insert_run(self.pool(), run_id, workflow_sha).await?)
    }

    async fn update_run_status(&self, run_id: &RunId, status: RunStatus) -> Result<(), CoreError> {
        Ok(run_repo::update_run_status(self.pool(), run_id, status).await?)
    }

    async fn set_current_node(&self, run_id: &RunId, node_id: &NodeId) -> Result<(), CoreError> {
        Ok(run_repo::set_current_node(self.pool(), run_id, node_id).await?)
    }

    async fn insert_node_outcome(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
        outcome: &Outcome,
    ) -> Result<(), CoreError> {
        Ok(outcome_repo::insert_node_outcome(self.pool(), run_id, node_id, outcome).await?)
    }

    async fn latest_outcome(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
    ) -> Result<Option<Outcome>, CoreError> {
        Ok(outcome_repo::latest_outcome(self.pool(), run_id, node_id).await?)
    }

    async fn count_attempts(&self, run_id: &RunId, node_id: &NodeId) -> Result<usize, CoreError> {
        Ok(outcome_repo::count_attempts(self.pool(), run_id, node_id).await?)
    }

    async fn open_human_wait(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
        prompt: &str,
    ) -> Result<String, CoreError> {
        Ok(human_wait_repo::open_human_wait(self.pool(), run_id, node_id, prompt).await?)
    }

    async fn answer_human_wait(
        &self,
        wait_id: &str,
        answer: &str,
        feedback: Option<&str>,
    ) -> Result<(), CoreError> {
        Ok(human_wait_repo::answer_human_wait(self.pool(), wait_id, answer, feedback).await?)
    }

    async fn lookup_park_by_wait_id(
        &self,
        wait_id: &str,
    ) -> Result<Option<(RunId, NodeId)>, CoreError> {
        Ok(human_wait_repo::lookup_park_by_wait_id(self.pool(), wait_id).await?)
    }

    async fn insert_review_run(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
        expected: usize,
        round: u32,
        context_sha: &str,
    ) -> Result<ReviewRunId, CoreError> {
        Ok(review_repo::insert_review_run(
            self.pool(),
            run_id,
            node_id,
            expected,
            round,
            context_sha,
        )
        .await?)
    }

    async fn lookup_park_by_review_run(
        &self,
        review_run_id: &ReviewRunId,
    ) -> Result<Option<(RunId, NodeId)>, CoreError> {
        Ok(review_repo::lookup_park_by_review_run(self.pool(), review_run_id).await?)
    }

    async fn insert_review_verdict(
        &self,
        review_run_id: &ReviewRunId,
        verdict: ReviewVerdict,
    ) -> Result<(), CoreError> {
        Ok(review_repo::insert_review_verdict(self.pool(), review_run_id, verdict).await?)
    }

    async fn count_verdicts(&self, review_run_id: &ReviewRunId) -> Result<usize, CoreError> {
        Ok(review_repo::count_verdicts(self.pool(), review_run_id).await?)
    }

    async fn review_expected(
        &self,
        review_run_id: &ReviewRunId,
    ) -> Result<Option<usize>, CoreError> {
        Ok(review_repo::review_expected(self.pool(), review_run_id).await?)
    }

    async fn review_round(&self, review_run_id: &ReviewRunId) -> Result<Option<u32>, CoreError> {
        Ok(review_repo::review_round(self.pool(), review_run_id).await?)
    }

    async fn list_verdicts(
        &self,
        review_run_id: &ReviewRunId,
    ) -> Result<Vec<ReviewVerdict>, CoreError> {
        Ok(review_repo::list_verdicts(self.pool(), review_run_id).await?)
    }

    async fn set_review_worktree(
        &self,
        review_run_id: &ReviewRunId,
        reviewer_id: &AgentId,
        path: &Path,
    ) -> Result<(), CoreError> {
        let path = path.to_string_lossy();
        Ok(
            review_repo::set_review_worktree(self.pool(), review_run_id, reviewer_id, &path)
                .await?,
        )
    }

    async fn take_review_worktree(
        &self,
        review_run_id: &ReviewRunId,
        reviewer_id: &AgentId,
    ) -> Result<Option<PathBuf>, CoreError> {
        Ok(review_repo::take_review_worktree(self.pool(), review_run_id, reviewer_id).await?)
    }

    async fn insert_task_run(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
    ) -> Result<TaskRunId, CoreError> {
        Ok(task_repo::insert_task_run(self.pool(), run_id, node_id).await?)
    }

    async fn set_task_run_agent(
        &self,
        task_run_id: &TaskRunId,
        agent_id: &AgentId,
    ) -> Result<(), CoreError> {
        Ok(task_repo::set_task_run_agent(self.pool(), task_run_id, agent_id).await?)
    }

    async fn set_task_run_worktree(
        &self,
        task_run_id: &TaskRunId,
        path: &Path,
    ) -> Result<(), CoreError> {
        let path = path.to_string_lossy();
        Ok(task_repo::set_task_run_worktree(self.pool(), task_run_id, &path).await?)
    }

    async fn complete_task_run(&self, task_run_id: &TaskRunId) -> Result<(), CoreError> {
        Ok(task_repo::complete_task_run(self.pool(), task_run_id).await?)
    }

    async fn lookup_park_by_task_run(
        &self,
        task_run_id: &TaskRunId,
    ) -> Result<Option<(RunId, NodeId)>, CoreError> {
        Ok(task_repo::lookup_park_by_task_run(self.pool(), task_run_id).await?)
    }

    async fn write_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), CoreError> {
        Ok(checkpoint_repo::write_checkpoint(self.pool(), checkpoint).await?)
    }

    async fn load_checkpoint(&self, run_id: &RunId) -> Result<Option<Checkpoint>, CoreError> {
        Ok(checkpoint_repo::load_checkpoint(self.pool(), run_id).await?)
    }
}
