//! `FakeRunHost` — an in-memory [`RunHost`] for tool tests. Records delivered
//! events and replays scripted `RunProgress`; serves set snapshots / tasks /
//! review counts. Compiled only under `test-support`/`cfg(test)`.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use agentd_core::CoreError;
use agentd_core::types::{NodeId, ReviewRunId, RunId};
use agentd_core::{EngineEvent, RunProgress};

use crate::host::{RunHost, RunSnapshot, TaskAssignment};

/// Scripted, recording [`RunHost`] for tests.
#[derive(Debug, Default)]
pub struct FakeRunHost {
    snapshots: Mutex<HashMap<String, RunSnapshot>>,
    tasks: Mutex<HashMap<(String, String), TaskAssignment>>,
    delivered: Mutex<Vec<EngineEvent>>,
    progress: Mutex<VecDeque<RunProgress>>,
    review_counts: Mutex<HashMap<String, (usize, usize)>>,
}

impl FakeRunHost {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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

    /// Every event delivered so far, in order.
    #[must_use]
    pub fn delivered(&self) -> Vec<EngineEvent> {
        self.delivered.lock().expect("delivered lock").clone()
    }
}

#[async_trait::async_trait]
impl RunHost for FakeRunHost {
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
}
