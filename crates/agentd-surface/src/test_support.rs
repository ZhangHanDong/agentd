//! `FakeRunHost` — an in-memory [`RunHost`] for tool tests. Records delivered
//! events and replays scripted `RunProgress`; serves set snapshots / tasks /
//! review counts. Compiled only under `test-support`/`cfg(test)`.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use agentd_core::CoreError;
use agentd_core::types::{NodeId, ReviewRunId, RunId};
use agentd_core::{EngineEvent, RunProgress};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::host::{EventRecord, LiveEvent, RunHost, RunSnapshot, RunSummary, TaskAssignment};

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
}

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
}
