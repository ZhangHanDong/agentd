spec: task
name: "submit_review + assign_task over the RunHost seam (+ error codes)"
tags: [surface, mvp, p0, mcp]
---

## Intent

The `submit_review` and `assign_task` MCP tools (design §4.12.1), over the
`RunHost` seam. `submit_review` delivers a reviewer's verdict and reports how
many reviewers the fan-in still waits on; `assign_task` hands an agent the open
task it was assigned. Both surface the §4.12.1 error codes precisely. Tested
against a `FakeRunHost` — no real engine.

## Decisions

- `submit_review { review_run_id, reviewer_id, verdict, findings }` → `deliver(ReviewVerdictSubmitted { review_run_id, reviewer_id, verdict })`. Wire verdict maps `pass→Pass`, `concern→Fail`, `blocker→Block`. `findings` are accepted but opaque in v0 (the daemon persists them later). Output `{ accepted, fan_in_pending }`; `fan_in_pending = expected − got` from `review_counts` (saturating).
- A `submit_review` whose `deliver` returns `RunProgress::Ignored` (the review run already closed / re-submitted past aggregation) → `SurfaceError::AlreadySubmitted`.
- `assign_task { run_id, node_id, agent_id }` → `open_task(run, node)`; a miss, OR a task whose `agent_id` differs from the requester → `SurfaceError::NotAssigned`. Output `{ task_run_id, worktree?, spec_path?, plan_path?, context_pack? }`.

## Boundaries

### Allowed Changes

- crates/agentd-surface/**
- specs/surface/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not construct a real `Engine`, open a socket, or speak real rmcp in v0.
- Do not hand an agent a task that belongs to a different agent.

## Out of Scope

- `check_inbox` + the rmcp server wiring (Task 3); HTTP+SSE (7b); findings persistence (daemon, P0.9).

## Completion Criteria

Scenario: submit_review records the verdict and reports remaining reviewers
  Test: submit_review_records_and_reports_pending
  Given a RunHost scripting a Parked progress and review counts expected 3, got 1
  When submit_review runs with verdict "pass"
  Then it returns accepted true and fan_in_pending 2, and the host received a ReviewVerdictSubmitted

Scenario: a re-submitted or closed review is already_submitted
  Test: submit_review_on_closed_review_is_already_submitted
  Given a RunHost scripting an Ignored progress
  When submit_review runs
  Then it returns Err whose code is "already_submitted"

Scenario: assign_task returns the open task for the requesting agent
  Test: assign_task_returns_open_task
  Given a RunHost with an open task for run "r1" node "implement" assigned to agent "impl-a"
  When assign_task runs for agent "impl-a"
  Then it returns the task's task_run_id and worktree

Scenario: assign_task for a different agent is not_assigned
  Test: assign_task_other_agent_is_not_assigned
  Given a RunHost with an open task assigned to agent "impl-a"
  When assign_task runs for agent "impl-b"
  Then it returns Err whose code is "not_assigned"

Scenario: assign_task with no open task is not_assigned
  Test: assign_task_no_task_is_not_assigned
  Given a RunHost with no open task for run "r1" node "implement"
  When assign_task runs
  Then it returns Err whose code is "not_assigned"
