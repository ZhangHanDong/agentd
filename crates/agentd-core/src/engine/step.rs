//! Engine control-flow vocabulary. See the "Engine Execution Model" section of
//! the P0.1 plan (D1–D3) for the authoritative rationale.

use crate::types::ids::{AgentId, NodeId, ReviewRunId, RunId, TaskRunId};
use crate::types::outcome::Outcome;
use crate::types::verdict::VerdictValue;

/// What a handler hands back to the engine after `run()` / `resume()`.
#[derive(Debug, Clone)]
pub enum HandlerStep {
    /// Synchronous completion (conditional, tool). Engine records the Outcome
    /// and immediately selects the next edge.
    Done(Outcome),
    /// Handler started external work; engine must checkpoint, stop the loop,
    /// and wait for a matching `EngineEvent`.
    Park(ParkReason),
}

/// Why a node parked, and what event will unpark it.
#[derive(Debug, Clone)]
pub enum ParkReason {
    HumanAnswer {
        wait_id: String,
    },
    ReviewVerdicts {
        review_run_id: ReviewRunId,
        expected: usize,
    },
    AgentOutcome {
        task_run_id: TaskRunId,
    },
}

/// Events delivered to `Engine::deliver_event` to unpark a run. In P0.1 tests
/// synthesize these directly; P0.6/P0.7 translate Matrix slashes / MCP calls
/// into them.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    HumanAnswered {
        wait_id: String,
        answer: String,
        feedback: Option<String>,
    },
    ReviewVerdictSubmitted {
        review_run_id: ReviewRunId,
        reviewer_id: AgentId,
        verdict: VerdictValue,
    },
    AgentOutcomeSubmitted {
        task_run_id: TaskRunId,
        outcome: Outcome,
    },
}

/// Result of advancing a run (returned by `execute()` and `deliver_event()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunProgress {
    /// Run reached a terminal node.
    Finished { run_id: RunId },
    /// Run parked on a node; caller should expect a future `deliver_event`.
    Parked { run_id: RunId, node_id: NodeId },
    /// Run failed terminally (no recovery edge, `goal_gate` unsatisfiable, etc.).
    Failed { run_id: RunId, reason: String },
}
