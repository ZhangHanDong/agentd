//! An in-memory [`Store`]. `HashMap`s behind a single `Mutex`; implements every
//! `Store` method including the three `lookup_park_by_*` reverse lookups that
//! `Engine::deliver_event` depends on. Ids are generated from a monotonic
//! counter so they are deterministic across a test run.
//!
//! Park semantics mirror the real store: a `lookup_park_by_*` returns `Some`
//! only while the child row is still *open* (human wait unanswered; review run
//! short of its expected verdict count), so a stale/replayed event resolves to
//! `None` and the engine treats it as a no-op.
//!
//! The row structs model the P0.2 sqlite schema for fidelity; not every column
//! is read back by a P0.1 test, hence the module-level `dead_code` allowance.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Mutex;

use crate::CoreError;
use crate::engine::Checkpoint;
use crate::ports::store::{RunStatus, Store};
use crate::types::{NodeId, Outcome, ReviewRunId, ReviewVerdict, RunId, TaskRunId};

#[derive(Debug)]
struct RunRow {
    workflow_sha: String,
    status: RunStatus,
    current_node: Option<String>,
}

#[derive(Debug)]
struct HumanWaitRow {
    run_id: String,
    node_id: String,
    prompt: String,
    answer: Option<String>,
    feedback: Option<String>,
}

#[derive(Debug)]
struct ReviewRunRow {
    run_id: String,
    node_id: String,
    expected: usize,
    context_sha: String,
}

#[derive(Debug)]
struct TaskRunRow {
    run_id: String,
    node_id: String,
    finished: bool,
}

#[derive(Debug, Default)]
struct Inner {
    counter: u64,
    runs: HashMap<String, RunRow>,
    outcomes: HashMap<(String, String), Vec<Outcome>>,
    human_waits: HashMap<String, HumanWaitRow>,
    review_runs: HashMap<String, ReviewRunRow>,
    verdicts: HashMap<String, Vec<ReviewVerdict>>,
    task_runs: HashMap<String, TaskRunRow>,
    checkpoints: HashMap<String, Checkpoint>,
}

impl Inner {
    fn next_id(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}_{}", self.counter)
    }
}

/// In-memory implementation of [`Store`].
#[derive(Debug, Default)]
pub struct InMemoryStore {
    inner: Mutex<Inner>,
}

impl InMemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("store lock")
    }

    /// Test-only read of a run's persisted status (no `Store`-trait getter exists).
    #[must_use]
    pub fn run_status(&self, run_id: &RunId) -> Option<RunStatus> {
        self.lock().runs.get(run_id.as_str()).map(|r| r.status)
    }
}

#[async_trait::async_trait]
impl Store for InMemoryStore {
    async fn insert_run(&self, run_id: &RunId, workflow_sha: &str) -> Result<(), CoreError> {
        self.lock().runs.insert(
            run_id.as_str().to_string(),
            RunRow {
                workflow_sha: workflow_sha.to_string(),
                status: RunStatus::Running,
                current_node: None,
            },
        );
        Ok(())
    }

    async fn update_run_status(&self, run_id: &RunId, status: RunStatus) -> Result<(), CoreError> {
        let mut inner = self.lock();
        let row = inner
            .runs
            .get_mut(run_id.as_str())
            .ok_or_else(|| CoreError::Store(format!("unknown run {}", run_id.as_str())))?;
        row.status = status;
        Ok(())
    }

    async fn set_current_node(&self, run_id: &RunId, node_id: &NodeId) -> Result<(), CoreError> {
        let mut inner = self.lock();
        let row = inner
            .runs
            .get_mut(run_id.as_str())
            .ok_or_else(|| CoreError::Store(format!("unknown run {}", run_id.as_str())))?;
        row.current_node = Some(node_id.as_str().to_string());
        Ok(())
    }

    async fn insert_node_outcome(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
        outcome: &Outcome,
    ) -> Result<(), CoreError> {
        let key = (run_id.as_str().to_string(), node_id.as_str().to_string());
        self.lock()
            .outcomes
            .entry(key)
            .or_default()
            .push(outcome.clone());
        Ok(())
    }

    async fn latest_outcome(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
    ) -> Result<Option<Outcome>, CoreError> {
        let key = (run_id.as_str().to_string(), node_id.as_str().to_string());
        Ok(self
            .lock()
            .outcomes
            .get(&key)
            .and_then(|v| v.last().cloned()))
    }

    async fn count_attempts(&self, run_id: &RunId, node_id: &NodeId) -> Result<usize, CoreError> {
        let key = (run_id.as_str().to_string(), node_id.as_str().to_string());
        Ok(self.lock().outcomes.get(&key).map_or(0, Vec::len))
    }

    async fn open_human_wait(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
        prompt: &str,
    ) -> Result<String, CoreError> {
        let mut inner = self.lock();
        let wait_id = inner.next_id("wait");
        inner.human_waits.insert(
            wait_id.clone(),
            HumanWaitRow {
                run_id: run_id.as_str().to_string(),
                node_id: node_id.as_str().to_string(),
                prompt: prompt.to_string(),
                answer: None,
                feedback: None,
            },
        );
        Ok(wait_id)
    }

    async fn answer_human_wait(
        &self,
        wait_id: &str,
        answer: &str,
        feedback: Option<&str>,
    ) -> Result<(), CoreError> {
        let mut inner = self.lock();
        let row = inner
            .human_waits
            .get_mut(wait_id)
            .ok_or_else(|| CoreError::Store(format!("unknown wait {wait_id}")))?;
        if row.answer.is_some() {
            return Err(CoreError::Store(format!("wait {wait_id} already answered")));
        }
        row.answer = Some(answer.to_string());
        row.feedback = feedback.map(ToString::to_string);
        Ok(())
    }

    async fn lookup_park_by_wait_id(
        &self,
        wait_id: &str,
    ) -> Result<Option<(RunId, NodeId)>, CoreError> {
        let inner = self.lock();
        Ok(inner
            .human_waits
            .get(wait_id)
            .filter(|r| r.answer.is_none())
            .map(|r| {
                (
                    RunId::from_string(r.run_id.clone()),
                    NodeId::from_string(r.node_id.clone()),
                )
            }))
    }

    async fn insert_review_run(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
        expected: usize,
        context_sha: &str,
    ) -> Result<ReviewRunId, CoreError> {
        let mut inner = self.lock();
        let id = inner.next_id("rr");
        inner.review_runs.insert(
            id.clone(),
            ReviewRunRow {
                run_id: run_id.as_str().to_string(),
                node_id: node_id.as_str().to_string(),
                expected,
                context_sha: context_sha.to_string(),
            },
        );
        inner.verdicts.entry(id.clone()).or_default();
        Ok(ReviewRunId::from_string(id))
    }

    async fn lookup_park_by_review_run(
        &self,
        review_run_id: &ReviewRunId,
    ) -> Result<Option<(RunId, NodeId)>, CoreError> {
        let inner = self.lock();
        let id = review_run_id.as_str();
        let Some(row) = inner.review_runs.get(id) else {
            return Ok(None);
        };
        let collected = inner.verdicts.get(id).map_or(0, Vec::len);
        if collected >= row.expected {
            return Ok(None); // no longer open — all verdicts already in
        }
        Ok(Some((
            RunId::from_string(row.run_id.clone()),
            NodeId::from_string(row.node_id.clone()),
        )))
    }

    async fn insert_review_verdict(
        &self,
        review_run_id: &ReviewRunId,
        verdict: ReviewVerdict,
    ) -> Result<(), CoreError> {
        let mut inner = self.lock();
        let id = review_run_id.as_str().to_string();
        if !inner.review_runs.contains_key(&id) {
            return Err(CoreError::Store(format!("unknown review run {id}")));
        }
        let bucket = inner.verdicts.entry(id).or_default();
        // Idempotent per reviewer (PRIMARY KEY (review_run_id, reviewer_id)):
        // a replayed verdict from a reviewer who already voted is a no-op, so
        // the count stays a count of DISTINCT reviewers.
        if bucket.iter().any(|v| v.reviewer_id == verdict.reviewer_id) {
            return Ok(());
        }
        bucket.push(verdict);
        Ok(())
    }

    async fn count_verdicts(&self, review_run_id: &ReviewRunId) -> Result<usize, CoreError> {
        Ok(self
            .lock()
            .verdicts
            .get(review_run_id.as_str())
            .map_or(0, Vec::len))
    }

    async fn review_expected(
        &self,
        review_run_id: &ReviewRunId,
    ) -> Result<Option<usize>, CoreError> {
        Ok(self
            .lock()
            .review_runs
            .get(review_run_id.as_str())
            .map(|r| r.expected))
    }

    async fn list_verdicts(
        &self,
        review_run_id: &ReviewRunId,
    ) -> Result<Vec<ReviewVerdict>, CoreError> {
        Ok(self
            .lock()
            .verdicts
            .get(review_run_id.as_str())
            .cloned()
            .unwrap_or_default())
    }

    async fn insert_task_run(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
    ) -> Result<TaskRunId, CoreError> {
        let mut inner = self.lock();
        let id = inner.next_id("tr");
        inner.task_runs.insert(
            id.clone(),
            TaskRunRow {
                run_id: run_id.as_str().to_string(),
                node_id: node_id.as_str().to_string(),
                finished: false,
            },
        );
        Ok(TaskRunId::from_string(id))
    }

    async fn complete_task_run(&self, task_run_id: &TaskRunId) -> Result<(), CoreError> {
        let mut inner = self.lock();
        let row = inner
            .task_runs
            .get_mut(task_run_id.as_str())
            .ok_or_else(|| {
                CoreError::Store(format!("unknown task run {}", task_run_id.as_str()))
            })?;
        row.finished = true;
        Ok(())
    }

    async fn lookup_park_by_task_run(
        &self,
        task_run_id: &TaskRunId,
    ) -> Result<Option<(RunId, NodeId)>, CoreError> {
        let inner = self.lock();
        Ok(inner
            .task_runs
            .get(task_run_id.as_str())
            .filter(|r| !r.finished)
            .map(|r| {
                (
                    RunId::from_string(r.run_id.clone()),
                    NodeId::from_string(r.node_id.clone()),
                )
            }))
    }

    async fn write_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), CoreError> {
        self.lock()
            .checkpoints
            .insert(checkpoint.run_id.as_str().to_string(), checkpoint.clone());
        Ok(())
    }

    async fn load_checkpoint(&self, run_id: &RunId) -> Result<Option<Checkpoint>, CoreError> {
        Ok(self.lock().checkpoints.get(run_id.as_str()).cloned())
    }
}
