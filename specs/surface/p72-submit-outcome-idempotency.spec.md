spec: task
name: "submit_outcome append-once: stale park → stale_attempt"
tags: [surface, mvp, p0, mcp, idempotency]
---

## Intent

`submit_outcome` is the one strictly append-once tool (design §4.12.1): a node
outcome is recorded once per `(run_id, node_id, attempt)`. Append-once is the
engine's job — a stale or replayed submission resolves to a park that has already
moved, which `deliver` returns as `RunProgress::Ignored`. The tool maps that to a
`stale_attempt` error rather than silently double-advancing the run. A first,
valid submission that finishes the run reports `recorded` with no next node.

## Decisions

- A `deliver` that returns `RunProgress::Ignored` (the `(run, node, attempt)` park is gone — already answered or replayed) → `SurfaceError::StaleAttempt`. The tool never double-delivers.
- A `deliver` returning `RunProgress::Finished` → `{ recorded: true, next_node: None }`; `Parked` → `{ recorded: true, next_node: Some(parked_node) }`; `Failed` → `{ recorded: true, next_node: None }` (recorded, run failed).

## Boundaries

### Allowed Changes

- crates/agentd-surface/**
- specs/surface/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not retry or re-deliver after an `Ignored` result — surface it as `stale_attempt`.

## Out of Scope

- The other tools; the real engine append-once logic (covered by core's engine tests); the rmcp server.

## Completion Criteria

Scenario: a stale submission whose park already moved is stale_attempt
  Test: submit_outcome_stale_park_is_stale_attempt
  Given a RunHost with an open task for run "r1" node "implement" and a scripted Ignored progress
  When submit_outcome runs
  Then it returns Err whose code is "stale_attempt"

Scenario: a submission that finishes the run reports recorded with no next node
  Test: submit_outcome_finished_has_no_next
  Given a RunHost with an open task for run "r1" node "implement" and a scripted Finished progress
  When submit_outcome runs with status "success"
  Then it returns recorded true and next_node None
