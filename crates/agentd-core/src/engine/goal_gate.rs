//! Goal-gate evaluation (design §2.7, Engine Execution Model D8a).
//! See `specs/core/p5-goal-gate-enforcement.spec.md`.
//!
//! This is a *pure diagnostic*: it reports whether every `goal_gate=true` node
//! reached a satisfying outcome and lists the ones that didn't. It never errors
//! and never routes — the engine (Task 9) consults the status before a terminal
//! transition and decides how to recover.

use std::collections::BTreeMap;

use crate::graph::NodeGraph;
use crate::types::{NodeId, Outcome, Status};

/// The result of a goal-gate check. `met` is true iff every gate node has a
/// satisfying outcome; `missing` lists the unsatisfied gate nodes in graph order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalGateStatus {
    pub met: bool,
    pub missing: Vec<NodeId>,
}

/// Evaluate every `goal_gate=true` node in `graph` against the recorded
/// `outcomes`. A gate is satisfied iff its recorded outcome is `Success` or
/// `PartialSuccess`; a missing entry or a `Fail`/`Retry` leaves it unsatisfied.
///
/// A graph with no gate nodes is trivially met.
#[must_use]
pub fn evaluate(graph: &NodeGraph, outcomes: &BTreeMap<NodeId, Outcome>) -> GoalGateStatus {
    let missing: Vec<NodeId> = graph
        .nodes
        .iter()
        .filter(|n| n.goal_gate)
        .filter(|n| !is_satisfied(outcomes.get(&NodeId::parsed(&n.id))))
        .map(|n| NodeId::parsed(&n.id))
        .collect();
    GoalGateStatus {
        met: missing.is_empty(),
        missing,
    }
}

fn is_satisfied(outcome: Option<&Outcome>) -> bool {
    matches!(
        outcome.map(|o| o.status),
        Some(Status::Success | Status::PartialSuccess)
    )
}
