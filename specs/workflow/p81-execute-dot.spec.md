spec: task
name: "execute.dot — the standalone Path-B execute workflow"
tags: [workflow, dot, mvp, p0, path-b, goal-gate]
---

## Intent

Author `workflows/execute.dot`, the second standalone Path-B workflow (boundary
Δ1): take a frozen spec and walk it to a PR (pull frozen spec → plan → implement
→ verify → adversarial review → open PR → report). It parks at `implement` (the
implementer agent) and `review` (three reviewers). Two `goal_gate` nodes guard
the terminal transition; because the gate is a runtime terminal-guard that
`flow validate` does not fully check, the node routing to `done` carries a
`goal_gate_unmet` recovery edge to a non-terminal. This task lands the file, its
validation, and the static recovery-edge guarantee; the engine walks are p81's
later scenarios.

## Decisions

- `execute.dot`: `start` → `pull_frozen_spec`(tool) → `draft_plan`(tool, shells `agent-spec plan` — NOT a planner agent) → `implement`(codergen, role=implementer) → `verify_lifecycle`(tool, goal_gate=true) → `review`(parallel.fan_out, reviewers=3, bundle=frozen, visibility=blind) → `aggregate`(parallel.fan_in, aggregator=majority_pass, goal_gate=true) → `open_pr`(tool, `gh pr create`) → `report_acceptance`(tool) → `done`(Msquare).
- The terminal-routing node `report_acceptance` has two outgoing edges: `report_acceptance -> done [condition="outcome=success"]` and the recovery edge `report_acceptance -> implement [label="goal_gate_unmet"]` to a non-terminal (so an unmet gate recovers instead of going Stuck).
- Exactly one `parallel.fan_out` feeds the one `parallel.fan_in` (P0.1 supports a single unpaired fan_out per fan_in).
- All `cmd=` are static whitespace-split argv (no `${...}`); `open_pr` relies on ambient `gh` auth (standalone, D6).

## Boundaries

### Allowed Changes

- workflows/execute.dot
- crates/agentctl/tests/**
- specs/workflow/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1 — the grammar/engine are frozen).
- Do not use `${...}` / `$run_dir` substitution in any `cmd=`.

## Out of Scope

- Live execution / a production RunHost / real agents / real `gh` (P0.9). The walk-tests drive the real engine over in-memory fakes in-process.

## Completion Criteria

Scenario: execute.dot parses and validates with zero violations
  Test: execute_dot_validates
  Given the authored workflows/execute.dot
  When it is parsed and built with NodeGraph::from_ast
  Then it returns Ok with no validation violations

Scenario: execute.dot has exactly one start and one terminal
  Test: execute_dot_single_start_single_terminal
  Given the validated execute.dot graph
  When its start and terminal nodes are counted
  Then there is exactly one start node and exactly one terminal node

Scenario: execute.dot has a goal_gate_unmet recovery edge to a non-terminal
  Test: execute_dot_has_goal_gate_unmet_recovery_edge
  Given the validated execute.dot graph
  When the edge labelled "goal_gate_unmet" is located
  Then exactly one such edge exists and its target is not a terminal node

Scenario: an execute variant with two unpaired fan_outs into one fan_in is rejected
  Test: execute_dot_rejects_unpaired_double_fan_out_variant
  Given an execute-shaped graph with two parallel.fan_out nodes feeding one parallel.fan_in without pair_with
  When it is built with NodeGraph::from_ast
  Then it returns Err reporting the unpaired fan_out
