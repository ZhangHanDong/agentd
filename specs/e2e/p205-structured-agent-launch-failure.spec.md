spec: task
name: "Structured agent launch failure"
tags: [agent-chat-replacement, real-execute, p205, blocker]
---

## Intent

Fix the p204 Codex-only real execute failure path so an agent launch failure is
durable and diagnosable. The real run exposed that a spawned agent's MCP stdio
process can boot a production host before `codergen` has persisted the allocated
task worktree; boot-GC can then remove the agent's own worktree, and the run is
left `running` when spawn returns a backend error.

## Decisions

- `codergen` persists an allocated task worktree before calling the backend
  `spawn`, so any agent-side `mcp-stdio` production host sees the worktree as
  active during boot-GC.
- If spawn fails after allocation, the task row keeps the worktree reference so
  failure evidence and failed-run cleanup can find it.
- `ProductionRunHost::start_run` converts run-execution errors after graph
  resolution into `RunProgress::Failed`, updates the run row to `failed`, and
  emits one `run_failed` event.
- HTTP `POST /runs` may report a created run with status `failed` for a
  structured launch blocker instead of returning a bare 500 that leaves the run
  `running`.
- p205 does not lengthen Codex readiness timeout or change Codex launcher flags;
  those remain follow-up fixes if they still block after worktree preservation.

## Boundaries

### Allowed Changes

- specs/e2e/p205-structured-agent-launch-failure.spec.md
- crates/agentd-core/src/handler/codergen.rs
- crates/agentd-core/src/test_support/in_memory_store.rs
- crates/agentd-core/tests/handlers_park.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/contract.rs
- docs/plans/p204-real-codex-execute-smoke-gate.spec.md

### Forbidden

- Do not use Claude in tests.
- Do not run the real execute smoke from this spec's automated tests.
- Do not change Codex launcher MCP config generation in this slice.
- Do not delete p204 evidence.

## Out of Scope

- Retrying the p204 real Codex execute gate.
- Changing the runtime matrix semantics.
- Agent registry, scheduler, messaging, Matrix, remote relay, or migration
  parity.

## Completion Criteria

<!-- lint-ack: observable-decision-coverage - The durable DB status/event and task worktree ordering are directly asserted. -->
<!-- lint-ack: error-path - Both scenarios are failure-path behavior. -->

Scenario: codergen persists task worktree before spawn
  Test:
    Package: agentd-core
    Filter: codergen_persists_allocated_worktree_before_spawn
  Level: handler unit
  Test Double: in-memory store, recording allocator, failing backend
  Given a codergen node with a task worktree allocator
  When backend spawn fails
  Then the task run row still contains the allocated worktree path
  And the backend error is surfaced

Scenario: production start records backend launch failure as terminal run failure
  Test:
    Package: agentd-bin
    Filter: production_runhost_backend_failure_marks_run_failed_and_emits_event
  Level: production host integration
  Test Double: real SqliteStore, failing backend
  Given an `execute.dot` run whose implementer backend spawn fails
  When `ProductionRunHost::start_run` runs
  Then it returns `RunProgress::Failed`
  And the run snapshot status is `failed`
  And `events_from` includes one `run_failed` event with the backend error
