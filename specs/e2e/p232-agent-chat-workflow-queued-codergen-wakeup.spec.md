spec: task
name: "agent-chat workflow queued codergen wakeup"
tags: [agent-chat-replacement, workflow, scheduler, queue, phase-e, p232]
---

## Intent

Close the next Phase E scheduler execution gap after p231 for the single-agent
`codergen` workflow path: when scheduler-backed workflow allocation cannot find
an idle compatible agent, the workflow must park with one durable scheduler
ticket instead of failing, and a later scheduler release that drains that ticket
must dispatch the parked `codergen` prompt to the freed online agent. This slice
keeps the implementation spec-gated, local, and fake-backend driven; it does not
claim complete agent-chat replacement.

## Decisions

- Production scheduler-backed `codergen` allocation treats `status="queued"` as
  a parked workflow state, not a terminal backend error.
- A queued `codergen` node keeps the original task-run id open, stores the
  allocated worktree and base prompt in checkpoint context, records the scheduler
  ticket in scheduler metadata, and does not call the backend until a drain
  occurs.
- Scheduler queue task payloads for workflow `codergen` include
  `kind="workflow_codergen"`, run id, node id, task-run id, requested role, and
  worktree, so release-drain wakeup can validate and dispatch the exact parked
  node.
- When `AgentAllocator::release` drains a queued workflow `codergen` ticket, the
  production host validates that the target task run is still the current open
  park, enriches the drained allocation with the registry runtime fields, sets
  the task-run owner to the freed agent, dispatches through
  `dispatch_allocated`, updates checkpoint scheduler metadata to `drained`, and
  emits a fresh `run_parked` event for the awakened node.
- Repeated release or stale drained tickets must not duplicate backend dispatch,
  create a second scheduler ticket, or advance an unrelated run/node.
- p232 updates the parity map and roadmap to record queued `codergen` workflow
  wakeup progress while keeping replacement partial until fan_out queued
  wakeups, dashboard views, Matrix/remote relay, cutover, rollback, notification
  gates, and token provisioning are complete.

## Boundaries

### Allowed Changes

- specs/e2e/p232-agent-chat-workflow-queued-codergen-wakeup.spec.md
- crates/agentd-core/src/ports/agent_allocator.rs
- crates/agentd-core/src/handler/codergen.rs
- crates/agentd-core/src/handler/mod.rs
- crates/agentd-core/tests/handlers_park.rs
- crates/agentd-store/src/agent_scheduler_repo.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/contract.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not modify the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout or any source JSON file.
- Do not run real Claude, Matrix, systemd, launchd, remote relay, or real tmux
  panes in automated tests.
- Do not use real Claude in tests; use Codex-prefixed agents, fake backends, or
  scripted local command runners only.
- Do not implement `parallel.fan_out` queued reviewer wakeup in this slice.
- Do not implement dashboard rendering, Matrix bridge state, remote relay state,
  service cutover state, rollback automation, notification gates, or token
  provisioning.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Queued reviewer wakeup for `parallel.fan_out`.
- Provisioned agent startup from scheduler provision plans.
- Cross-host or remote relay scheduler coordination.
- Operator dashboard queue/agent panels.
- Token provisioning, rotation, notification gates, or import-time secret
  migration.

## Completion Criteria

<!-- lint-ack: decision-coverage - p232 binds queued codergen park behavior, durable task payload fields, drain validation, allocation-aware dispatch, duplicate suppression, and docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify fake backend counts, durable SQLite scheduler rows, checkpoint/event JSON, task-run ownership, and parity Markdown. -->

Scenario: codergen queues scheduler allocation without backend dispatch
  Test:
    Package: agentd-core
    Filter: codergen_queues_scheduler_allocation_without_dispatching_backend
  Level: core handler
  Test Double: recording allocator and recording backend
  Given a `codergen` node that requests role `coding` with no idle scheduler
  agent available
  When the allocator returns a queued allocation with a scheduler ticket
  Then the handler parks on the original task-run id
  And the backend receives zero plain spawn or allocation-aware dispatch calls
  And checkpoint-staged context records the scheduler ticket, task-run id,
  worktree, and base prompt needed for a later drain wakeup

Scenario: production release drains queued codergen ticket and dispatches once
  Test:
    Package: agentd-bin
    Filter: production_workflow_scheduler_drains_queued_codergen_ticket_on_release
  Level: daemon integration
  Test Double: real SqliteStore and recording fake backend
  Given a scheduler-backed `codergen` run parks queued because the only coding
  agent is busy with an earlier scheduler-backed run
  When the earlier agent submits a successful outcome and release drains the
  queued ticket
  Then the parked queued run remains parked on its original task-run id
  And the backend records exactly one allocation-aware dispatch to the freed
  Codex agent and zero duplicate plain spawns for the queued run
  And the queued task-run owner, checkpoint scheduler metadata, and latest
  `run_parked` event all identify the drained agent and reservation

Scenario: repeated scheduler release does not duplicate queued codergen wakeup
  Test:
    Package: agentd-bin
    Filter: production_workflow_scheduler_queued_codergen_wakeup_is_idempotent
  Level: daemon integration
  Test Double: real SqliteStore and recording fake backend
  Given a queued `codergen` run has already been awakened by a drained scheduler
  ticket
  When the same release path is observed again through a replayed or unrelated
  completion
  Then no second backend dispatch is recorded for the queued task run
  And the scheduler queue keeps one drained ticket rather than creating another
  queued ticket

Scenario: parity docs record p232 queued codergen wakeup progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p232_queued_codergen_wakeup_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the pool scheduler, task graph, migration, runtime launch, and Phase E
  sections are inspected
  Then they mention p232 queued codergen workflow wakeup, release-drain dispatch,
  and duplicate-dispatch suppression
  And they remain partial because fan_out queued wakeups, dashboard views,
  Matrix/remote relay, cutover, rollback, notification gates, and token
  provisioning are not complete
