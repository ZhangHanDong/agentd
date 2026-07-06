//! The workflow run loop and event delivery (design §2, Engine Execution Model
//! D8). `execute` drives a run from its start node, running each node's handler
//! until the run finishes, fails, or parks. `deliver_event` resolves an inbound
//! event back to its parked node, resumes that handler, and continues the loop.
//!
//! Replay-safety lives in the store (a stale/replayed event resolves to `None`),
//! so `deliver_event` treats an unmatched event as `RunProgress::Ignored`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::CoreError;
use crate::engine::{Checkpoint, EngineEvent, HandlerStep, RunProgress, goal_gate};
use crate::graph::edge_select::select_next_edge;
use crate::graph::{NodeDef, NodeGraph, NodeShape};
use crate::handler::{HandlerCtx, HandlerRegistry, Ports};
use crate::ports::RunStatus;
use crate::ports::WorktreeAllocator;
use crate::types::{NodeId, Outcome, RunContext, RunId, Status};

/// Hard backstop: a single run may execute at most this many nodes before the
/// engine bails with an invariant error. Guards against a pathological graph
/// (e.g. a `goal_gate` recovery edge that is not attempts-gated) looping forever.
const STEP_CEILING: u32 = 10_000;

/// Workflow execution engine over one graph. Holds borrows of the graph, the
/// handler registry, and the ports; one instance drives one workflow's runs.
#[derive(Debug)]
pub struct Engine<'a> {
    graph: &'a NodeGraph,
    registry: &'a HandlerRegistry,
    ports: Ports<'a>,
    workflow_sha: String,
    /// Optional per-task_run worktree allocator (P2 C1' R3a). Default `None`
    /// leaves codergen spawning in `"."`.
    worktree_allocator: Option<&'a dyn WorktreeAllocator>,
}

/// Mutable per-run state threaded through the loop and snapshotted into a
/// checkpoint after every node. `attempts` is the single retry counter (per
/// node id); it is serialized as the checkpoint's `retry_counts` and restored on
/// resume, and is the same map `select_next_edge` consults for the `retry_target`
/// ceiling — so nothing double-counts.
struct RunState {
    current: NodeId,
    context: RunContext,
    attempts: HashMap<String, u32>,
    completed: Vec<NodeId>,
}

/// Outcome of processing one finished node: keep looping, or fail the run.
enum Flow {
    Continue,
    Fail(String),
}

/// Where edge selection sends the run next.
enum Advanced {
    To(NodeId),
    Stuck(String),
}

impl<'a> Engine<'a> {
    #[must_use]
    pub fn new(
        graph: &'a NodeGraph,
        registry: &'a HandlerRegistry,
        ports: Ports<'a>,
        workflow_sha: impl Into<String>,
    ) -> Self {
        Self {
            graph,
            registry,
            ports,
            workflow_sha: workflow_sha.into(),
            worktree_allocator: None,
        }
    }

    /// Thread an optional per-task_run worktree allocator into handlers.
    #[must_use]
    pub fn with_worktree_allocator(mut self, allocator: Option<&'a dyn WorktreeAllocator>) -> Self {
        self.worktree_allocator = allocator;
        self
    }

    /// Run `run_id` from the graph's start node until it finishes, fails, or parks.
    ///
    /// # Errors
    /// Returns [`CoreError`] on a store/handler failure or an engine invariant
    /// violation (missing start node, unknown handler, step ceiling exceeded).
    pub async fn execute(&self, run_id: &RunId) -> Result<RunProgress, CoreError> {
        let start = self
            .graph
            .starts()
            .into_iter()
            .next()
            .ok_or_else(|| CoreError::Invariant("graph has no start node".to_string()))?;
        self.ports
            .store
            .insert_run(run_id, &self.workflow_sha)
            .await?;
        tracing::info!(run_id = %run_id.as_str(), "run started");
        let state = RunState {
            current: NodeId::parsed(start.id.as_str()),
            context: RunContext::new(),
            attempts: HashMap::new(),
            completed: Vec::new(),
        };
        self.run_loop(run_id, state).await
    }

    /// Resolve `event` to its parked node, resume that handler, and continue the
    /// run. An event with no open park is a no-op ([`RunProgress::Ignored`]).
    ///
    /// # Errors
    /// Returns [`CoreError`] on a store/handler failure or a missing checkpoint.
    pub async fn deliver_event(&self, event: EngineEvent) -> Result<RunProgress, CoreError> {
        let resolved = match &event {
            EngineEvent::HumanAnswered { wait_id, .. } => {
                self.ports.store.lookup_park_by_wait_id(wait_id).await?
            }
            EngineEvent::ReviewVerdictSubmitted { review_run_id, .. } => {
                self.ports
                    .store
                    .lookup_park_by_review_run(review_run_id)
                    .await?
            }
            EngineEvent::AgentOutcomeSubmitted { task_run_id, .. } => {
                self.ports
                    .store
                    .lookup_park_by_task_run(task_run_id)
                    .await?
            }
        };
        let Some((run_id, node_id)) = resolved else {
            return Ok(RunProgress::Ignored {
                reason: "no open park matches this event (unknown or already resolved)".to_string(),
            });
        };

        let checkpoint = self
            .ports
            .store
            .load_checkpoint(&run_id)
            .await?
            .ok_or_else(|| {
                CoreError::Invariant(format!("parked run {} has no checkpoint", run_id.as_str()))
            })?;
        let mut state = RunState {
            current: node_id.clone(),
            context: checkpoint.context_snapshot,
            attempts: checkpoint
                .retry_counts
                .iter()
                .map(|(k, v)| (k.as_str().to_string(), *v))
                .collect(),
            completed: checkpoint.completed_nodes,
        };

        let (step, staged) = {
            let node = self.graph.node(node_id.as_str()).ok_or_else(|| {
                CoreError::Invariant(format!(
                    "checkpointed node '{}' not in graph",
                    node_id.as_str()
                ))
            })?;
            let kind = node.handler.ok_or_else(|| {
                CoreError::Invariant(format!(
                    "node '{}' has no handler to resume",
                    node_id.as_str()
                ))
            })?;
            let handler = self
                .registry
                .get(kind)
                .ok_or_else(|| CoreError::UnknownHandler(format!("{kind:?}")))?;
            let mut ctx = HandlerCtx::new(&run_id, self.graph, node, &state.context, self.ports)
                .with_worktree_allocator(self.worktree_allocator);
            let step = handler.resume(&mut ctx, event).await?;
            let staged = ctx.staged_updates().clone();
            (step, staged)
        };
        // ctx-staged updates merge on every step, including resume (e.g.
        // wait.human stages `answer` here so `condition="answer=approve"` routes).
        state.context.merge(&staged);

        match step {
            HandlerStep::Park(reason) => {
                self.ports.store.set_current_node(&run_id, &node_id).await?;
                self.write_checkpoint(&run_id, &state).await?;
                Ok(RunProgress::Parked {
                    run_id,
                    node_id,
                    reason,
                })
            }
            HandlerStep::Done(outcome) => match self
                .process_done(&run_id, &node_id, outcome, &mut state)
                .await?
            {
                Flow::Continue => self.run_loop(&run_id, state).await,
                Flow::Fail(reason) => self.fail(&run_id, reason).await,
            },
        }
    }

    async fn run_loop(
        &self,
        run_id: &RunId,
        mut state: RunState,
    ) -> Result<RunProgress, CoreError> {
        let mut steps: u32 = 0;
        loop {
            steps += 1;
            if steps > STEP_CEILING {
                // Mark the run Failed before surfacing the error so the store
                // never leaves a ceiling-exhausted run orphaned as `Running`
                // (every other run-ending path updates the status).
                self.ports
                    .store
                    .update_run_status(run_id, RunStatus::Failed)
                    .await?;
                return Err(CoreError::Invariant(format!(
                    "run {} exceeded the {STEP_CEILING}-node step ceiling",
                    run_id.as_str()
                )));
            }
            if let Some(progress) = self.step_once(run_id, &mut state).await? {
                return Ok(progress);
            }
        }
    }

    /// Run one node. Returns `Some(progress)` when the run ends (terminal, park,
    /// or fail) or `None` to keep looping after advancing `state.current`.
    async fn step_once(
        &self,
        run_id: &RunId,
        state: &mut RunState,
    ) -> Result<Option<RunProgress>, CoreError> {
        let node = self.graph.node(state.current.as_str()).ok_or_else(|| {
            CoreError::Invariant(format!(
                "current node '{}' not in graph",
                state.current.as_str()
            ))
        })?;
        match node.shape {
            NodeShape::Terminal => {
                self.ports
                    .store
                    .update_run_status(run_id, RunStatus::Finished)
                    .await?;
                self.release_worktree_after_success(run_id, &state.context)
                    .await;
                tracing::info!(run_id = %run_id.as_str(), "run finished");
                Ok(Some(RunProgress::Finished {
                    run_id: run_id.clone(),
                }))
            }
            NodeShape::Start => {
                // Start does no work; advance with a synthetic success.
                let current = state.current.clone();
                match self
                    .advance(run_id, current.as_str(), &Outcome::success(), state)
                    .await?
                {
                    Advanced::To(next) => {
                        state.completed.push(current);
                        state.current = next;
                        self.write_checkpoint(run_id, state).await?;
                        Ok(None)
                    }
                    Advanced::Stuck(reason) => Ok(Some(self.fail(run_id, reason).await?)),
                }
            }
            NodeShape::Regular => {
                let kind = node.handler.ok_or_else(|| {
                    CoreError::Invariant(format!("node '{}' has no handler", node.id))
                })?;
                let handler = self
                    .registry
                    .get(kind)
                    .ok_or_else(|| CoreError::UnknownHandler(format!("{kind:?}")))?;
                let (step, staged) = {
                    let mut ctx =
                        HandlerCtx::new(run_id, self.graph, node, &state.context, self.ports)
                            .with_worktree_allocator(self.worktree_allocator);
                    let step = handler.run(&mut ctx).await?;
                    let staged = ctx.staged_updates().clone();
                    (step, staged)
                };
                state.context.merge(&staged);
                match step {
                    HandlerStep::Park(reason) => {
                        self.ports
                            .store
                            .set_current_node(run_id, &state.current)
                            .await?;
                        self.write_checkpoint(run_id, state).await?;
                        Ok(Some(RunProgress::Parked {
                            run_id: run_id.clone(),
                            node_id: state.current.clone(),
                            reason,
                        }))
                    }
                    HandlerStep::Done(outcome) => {
                        let current = state.current.clone();
                        match self.process_done(run_id, &current, outcome, state).await? {
                            Flow::Continue => Ok(None),
                            Flow::Fail(reason) => Ok(Some(self.fail(run_id, reason).await?)),
                        }
                    }
                }
            }
        }
    }

    /// Mark the run Failed in the store and build the `Failed` progress.
    async fn fail(&self, run_id: &RunId, reason: String) -> Result<RunProgress, CoreError> {
        self.ports
            .store
            .update_run_status(run_id, RunStatus::Failed)
            .await?;
        Ok(RunProgress::Failed {
            run_id: run_id.clone(),
            reason,
        })
    }

    async fn release_worktree_after_success(&self, run_id: &RunId, context: &RunContext) {
        let Some(allocator) = self.worktree_allocator else {
            return;
        };
        let Some(task_run_id) = context
            .0
            .get("task_run_id")
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        let Some(worktree) = context
            .0
            .get("worktree")
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        let path = Path::new(worktree);
        if let Err(err) = allocator.release(task_run_id, path).await {
            tracing::warn!(
                run_id = %run_id.as_str(),
                task_run_id = %task_run_id,
                worktree = %path.display(),
                error = %err,
                "worktree release failed after successful run; boot-GC remains the fallback"
            );
        }
    }

    /// Record a finished node's outcome, then either re-run it (Retry under
    /// budget) or advance. The caller's loop continues from `state.current`.
    async fn process_done(
        &self,
        run_id: &RunId,
        node_id: &NodeId,
        mut outcome: Outcome,
        state: &mut RunState,
    ) -> Result<Flow, CoreError> {
        // Outcome.context_updates merge on Done (ctx-staged already merged by caller).
        state.context.merge(&outcome.context_updates);
        self.ports
            .store
            .insert_node_outcome(run_id, node_id, &outcome)
            .await?;
        let attempts_here = {
            let counter = state
                .attempts
                .entry(node_id.as_str().to_string())
                .or_insert(0);
            *counter += 1;
            *counter
        };

        if outcome.status == Status::Retry {
            let max = retry_max(self.graph.node(node_id.as_str()));
            if attempts_here < max {
                // Re-run the same node; current is unchanged.
                self.write_checkpoint(run_id, state).await?;
                return Ok(Flow::Continue);
            }
            // Retry budget exhausted — route as a Fail.
            outcome.status = Status::Fail;
        }

        match self
            .advance(run_id, node_id.as_str(), &outcome, state)
            .await?
        {
            Advanced::To(next) => {
                state.completed.push(node_id.clone());
                state.current = next;
                self.write_checkpoint(run_id, state).await?;
                Ok(Flow::Continue)
            }
            // The store status is set by the caller via `fail()`; process_done
            // stays pure so both run_loop and deliver_event reconcile it once.
            Advanced::Stuck(reason) => Ok(Flow::Fail(reason)),
        }
    }

    /// Select the next edge for `outcome`. If it targets a terminal node, gate on
    /// `goal_gate` (D8a): when unmet, discard that transition, synthesize a
    /// `goal_gate_unmet` Fail, and re-select once — a non-terminal recovery edge
    /// advances; otherwise the run is stuck.
    async fn advance(
        &self,
        run_id: &RunId,
        node_id: &str,
        outcome: &Outcome,
        state: &RunState,
    ) -> Result<Advanced, CoreError> {
        let Some(target) = select_next_edge(
            self.graph,
            node_id,
            outcome,
            &state.context,
            &state.attempts,
        )
        .map(|e| e.to.clone()) else {
            return Ok(Advanced::Stuck(format!(
                "no outgoing edge from '{node_id}' for status {:?}",
                outcome.status
            )));
        };

        if self.is_terminal(&target) {
            let gate = self.evaluate_goal_gate(run_id).await?;
            if !gate.met {
                let synthetic = Outcome {
                    status: Status::Fail,
                    preferred_label: Some("goal_gate_unmet".to_string()),
                    ..Outcome::success()
                };
                let recovery = select_next_edge(
                    self.graph,
                    node_id,
                    &synthetic,
                    &state.context,
                    &state.attempts,
                )
                .map(|e| e.to.clone());
                return match recovery {
                    Some(next) if !self.is_terminal(&next) => {
                        Ok(Advanced::To(NodeId::parsed(next)))
                    }
                    _ => Ok(Advanced::Stuck(format!(
                        "goal gate not met and no recovery edge: missing {:?}",
                        gate.missing
                    ))),
                };
            }
        }
        Ok(Advanced::To(NodeId::parsed(target)))
    }

    /// Build the `goal_gate` outcomes map from the STORE (not in-memory state):
    /// gate nodes may have completed in an earlier `deliver_event` segment.
    async fn evaluate_goal_gate(
        &self,
        run_id: &RunId,
    ) -> Result<goal_gate::GoalGateStatus, CoreError> {
        let mut outcomes: BTreeMap<NodeId, Outcome> = BTreeMap::new();
        for node in &self.graph.nodes {
            if node.goal_gate {
                let nid = NodeId::parsed(node.id.as_str());
                if let Some(outcome) = self.ports.store.latest_outcome(run_id, &nid).await? {
                    outcomes.insert(nid, outcome);
                }
            }
        }
        Ok(goal_gate::evaluate(self.graph, &outcomes))
    }

    fn is_terminal(&self, node_id: &str) -> bool {
        self.graph
            .node(node_id)
            .is_some_and(|n| n.shape == NodeShape::Terminal)
    }

    async fn write_checkpoint(&self, run_id: &RunId, state: &RunState) -> Result<(), CoreError> {
        let retry_counts: BTreeMap<NodeId, u32> = state
            .attempts
            .iter()
            .map(|(k, v)| (NodeId::parsed(k.as_str()), *v))
            .collect();
        let checkpoint = Checkpoint {
            run_id: run_id.clone(),
            current_node: state.current.clone(),
            completed_nodes: state.completed.clone(),
            retry_counts,
            context_snapshot: state.context.clone(),
            workflow_sha: self.workflow_sha.clone(),
        };
        self.ports.store.write_checkpoint(&checkpoint).await
    }
}

/// Per-node retry budget from `retry_policy="max=N"`; default 1 (no re-run).
fn retry_max(node: Option<&NodeDef>) -> u32 {
    node.and_then(|n| n.attrs.get("retry_policy"))
        .map_or(1, |policy| {
            policy
                .split(',')
                .find_map(|part| part.trim().strip_prefix("max="))
                .and_then(|n| n.trim().parse().ok())
                .unwrap_or(1)
        })
}
