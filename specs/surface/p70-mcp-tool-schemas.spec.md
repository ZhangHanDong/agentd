spec: task
name: "MCP tools over the RunHost seam: query_run + submit_outcome"
tags: [surface, mvp, p0, mcp]
---

## Intent

`agentd-surface` exposes the agentd MCP tools (design §4.12.1) to agents. Each
tool is a pure function over an injected `RunHost` seam — the agent↔engine/store
boundary — so the tools are testable against a `FakeRunHost` with no real engine,
MCP client, or socket. This task lands the seam, the error taxonomy, and the
first two tools: `query_run` (read) and `submit_outcome` (write). The remaining
tools (`submit_review`, `assign_task`, `check_inbox`) extend this spec in 7a
Tasks 2–3; the production `RunHost` (real `Engine` + store) is the daemon's job,
wired in P0.9.

## Decisions

- `RunHost` (object-safe) is the seam: `deliver(EngineEvent) -> RunProgress`, `run_snapshot(run_id) -> Option<RunSnapshot>`, `open_task(run_id, node_id) -> Option<TaskAssignment>`, `review_counts(review_run_id) -> (expected, got)`. Tests inject a `FakeRunHost`.
- `query_run { run_id }` → `run_snapshot`; returns `{ status, current_node, completed_nodes, context }`; a missing run is `SurfaceError::NotFound`.
- `submit_outcome { run_id, node_id, attempt, status, context_updates, preferred_label?, suggested_next? }` → resolve the open task via `open_task` (the inputs carry `(run, node)` but `deliver` routes by `task_run_id`), build an `Outcome`, then `deliver(AgentOutcomeSubmitted { task_run_id, outcome })`. Returns `{ recorded, next_node? }` from the `RunProgress`.
- `submit_outcome` error mapping: no open task → `not_assigned`; `RunProgress::Ignored` (the park already moved) → `stale_attempt`. The two `Ignored` causes are not collapsed — a missing task is distinguished by the `open_task` miss.
- `SurfaceError` maps to the §4.12.1 MCP codes (`not_assigned` / `already_submitted` / `stale_attempt` / `not_found`) via `code()`; a `CoreError` becomes `Internal`.

## Boundaries

### Allowed Changes

- crates/agentd-surface/**
- specs/surface/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not construct a real `Engine`, open a socket, or speak real rmcp in v0 — tools run over the `RunHost` seam.
- Do not reference, open, or write mempal's on-disk database (MCP-only, §3.1).

## Out of Scope

- `submit_review` / `assign_task` / `check_inbox` (7a Tasks 2–3); the rmcp server wiring (Task 3); HTTP+SSE (7b); the production `RunHost` (P0.9).

## Completion Criteria

Scenario: query_run returns the run snapshot
  Test: query_run_returns_snapshot
  Given a RunHost with a snapshot for run "r1" (status "parked", current node "review")
  When query_run runs for "r1"
  Then it returns status "parked", current_node "review", and the snapshot's completed nodes

Scenario: query_run on an unknown run is not_found
  Test: query_run_unknown_is_not_found
  Given a RunHost with no snapshot for "ghost"
  When query_run runs for "ghost"
  Then it returns Err whose code is "not_found"

Scenario: submit_outcome delivers the outcome and reports the next node
  Test: submit_outcome_delivers_and_reports_next
  Given a RunHost with an open task for run "r1" node "implement" and a scripted Parked progress on node "review"
  When submit_outcome runs for "r1"/"implement" with status "success"
  Then it returns recorded true and next_node "review", and the host received an AgentOutcomeSubmitted for that task

Scenario: submit_outcome with no open task is not_assigned
  Test: submit_outcome_no_task_is_not_assigned
  Given a RunHost with no open task for run "r1" node "implement"
  When submit_outcome runs
  Then it returns Err whose code is "not_assigned" and the host received no delivered event
