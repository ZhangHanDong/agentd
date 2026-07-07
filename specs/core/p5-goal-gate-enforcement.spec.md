spec: task
name: "Goal-gate evaluation (diagnostic, not routing)"
tags: [core, mvp, p0, workflow]
---

## Intent

A workflow may mark nodes `goal_gate=true` to declare "this run has not achieved
its goal until these nodes succeed". Per Engine Execution Model D8a, the gate is
a *diagnostic*: `goal_gate::evaluate` reports whether every gate node has a
satisfying outcome and lists the ones that don't. It NEVER errors and NEVER
routes — the engine (Task 9) consults the status before a terminal transition
and decides what to do (synthesize a `goal_gate_unmet` Fail and re-run edge
selection, or raise `CoreError::GoalGateNotMet` when no recovery edge exists).

## Decisions

- `GoalGateStatus { met: bool, missing: Vec<NodeId> }`
- `fn evaluate(graph: &NodeGraph, outcomes: &BTreeMap<NodeId, Outcome>) -> GoalGateStatus`
- A gate node is *satisfied* iff `outcomes` carries an outcome for it whose status is `Success` or `PartialSuccess`. `PartialSuccess` counts as met (design §2.3: partial credit still advances the goal).
- A gate node with no recorded outcome, or a `Fail`/`Retry` outcome, is *missing*.
- `missing` preserves graph node order (deterministic); `met == missing.is_empty()`.
- A graph with zero gate nodes is trivially met (`missing` empty).
- `evaluate` is pure and side-effect free — no filesystem, no error, no logging.

## Boundaries

### Allowed Changes

- crates/agentd-core/src/engine/goal_gate.rs
- crates/agentd-core/src/engine/mod.rs
- crates/agentd-core/tests/goal_gate.rs

### Forbidden

- Do not have `evaluate` return a `Result` or route/transition — it is a pure diagnostic (D8a).
- Do not treat `Fail` or `Retry` as satisfying a gate.

## Completion Criteria

Scenario: All gate nodes succeeded so the gate is met
  Test: goal_gate_evaluate_met_when_all_gates_success_or_partial
  Given a graph with two goal_gate nodes and outcomes recording Success for both
  When evaluate runs
  Then met is true and missing is empty

Scenario: A failed gate node makes the gate unmet and is listed
  Test: goal_gate_evaluate_unmet_lists_missing_gate_nodes
  Given a graph with two goal_gate nodes where one recorded a Fail outcome
  When evaluate runs
  Then met is false and missing lists exactly the failed gate node

Scenario: A gate node with no recorded outcome is missing
  Test: goal_gate_evaluate_unmet_when_a_gate_has_no_outcome
  Given a graph with a goal_gate node that has no entry in outcomes
  When evaluate runs
  Then met is false and missing lists that gate node

Scenario: PartialSuccess satisfies a gate
  Test: goal_gate_evaluate_partial_success_counts_as_met
  Given a graph with one goal_gate node whose recorded outcome is PartialSuccess
  When evaluate runs
  Then met is true and missing is empty
