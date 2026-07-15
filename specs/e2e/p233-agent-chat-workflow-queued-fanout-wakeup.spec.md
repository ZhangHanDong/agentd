spec: task
name: "agent-chat workflow queued fan_out wakeup"
tags: [agent-chat-replacement, workflow, scheduler, queue, phase-e, p233]
---

## Intent

Close the remaining Phase E queued workflow wakeup gap after p232 for
`parallel.fan_out`: when scheduler-backed reviewer allocation queues, the
workflow must park with a durable reviewer ticket and dispatch that reviewer only
after a later scheduler release drains the ticket. This slice keeps the
implementation local and fake-backend driven; it does not claim complete
agent-chat replacement.

## Decisions

- Scheduler-backed `parallel.fan_out` allocation treats `status="queued"` as a
  parked reviewer state, not as permission to dispatch the requested role.
- A queued reviewer stores enough checkpoint context to wake later: review-run
  id, requested reviewer role, round, context sha, source implementation
  worktree, and the reviewer prompt base that is independent of the final agent
  id.
- Scheduler queue task payloads for workflow reviewers include
  `kind="workflow_fan_out_reviewer"`, run id, node id, review-run id,
  requested role, round, and context sha, so release-drain wakeup can validate
  the exact parked review node.
- `FanOutHandler::resume` stages any drained scheduler allocation returned by
  release, matching the p232 `codergen` wakeup path so the production host can
  notice queued workflow work after a reviewer frees an agent.
- When a queued reviewer ticket drains, the production host validates that the
  target review run is still the current open park, allocates or reuses a review
  worktree for the drained agent id, registers that worktree for later reviewer
  release, dispatches through `dispatch_allocated`, updates checkpoint
  scheduler metadata to `drained`, and emits a fresh `run_parked` event for the
  awakened review node.
- Repeated release or stale drained tickets must not duplicate backend dispatch,
  create a second scheduler ticket, or advance an unrelated run/node.
- p233 updates the parity map and roadmap to record queued `fan_out` reviewer
  wakeup progress while keeping replacement partial until dashboard views,
  Matrix/remote relay, cutover, rollback, notification gates, token
  provisioning, and broader lifecycle gaps are complete.

## Boundaries

### Allowed Changes

- specs/e2e/p233-agent-chat-workflow-queued-fanout-wakeup.spec.md
- crates/agentd-core/src/handler/fan_out.rs
- crates/agentd-core/src/handler/mod.rs
- crates/agentd-core/tests/handlers_park.rs
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
- Do not implement dashboard rendering, Matrix bridge state, remote relay state,
  service cutover state, rollback automation, notification gates, or token
  provisioning.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Queued startup from scheduler provision plans.
- Cross-host or remote relay scheduler coordination.
- Operator dashboard queue/agent panels.
- Agent lifecycle kill/rebind/session recovery.
- Token provisioning, rotation, notification gates, or import-time secret
  migration.

## Completion Criteria

<!-- lint-ack: decision-coverage - p233 binds queued fan_out park behavior, durable reviewer task payload fields, drain staging, drain validation, allocation-aware dispatch, duplicate suppression, and docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify fake backend counts, durable SQLite scheduler rows, checkpoint/event JSON, review-run ownership, and parity Markdown. -->

Scenario: fan_out queues scheduler reviewer without backend dispatch
  Test:
    Package: agentd-core
    Filter: fan_out_queues_scheduler_reviewer_without_dispatching_backend
  Level: core handler
  Test Double: recording allocator and recording backend
  Given a `parallel.fan_out` node that requests reviewer role `review` with no
  idle scheduler agent available
  When the allocator returns a queued allocation with a scheduler ticket
  Then the handler parks on the original review-run id
  And the backend receives zero plain spawn or allocation-aware dispatch calls
  And checkpoint-staged context records the scheduler ticket, review-run id,
  requested role, source worktree, round, context sha, and prompt base needed
  for a later drain wakeup

Scenario: production release drains queued fan_out reviewer ticket and dispatches once
  Test:
    Package: agentd-bin
    Filter: production_workflow_scheduler_drains_queued_fanout_reviewer_ticket_on_release
  Level: daemon integration
  Test Double: real SqliteStore and recording fake backend
  Given a scheduler-backed `parallel.fan_out` run parks queued because the only
  review agent is busy with an earlier scheduler-backed review run
  When the earlier reviewer submits a verdict and release drains the queued
  reviewer ticket
  Then the parked queued run remains parked on its original review-run id
  And the backend records exactly one allocation-aware dispatch to the freed
  Codex review agent and zero duplicate plain spawns for the queued reviewer
  And the queued reviewer prompt, checkpoint scheduler metadata, and latest
  `run_parked` event all identify the drained agent and reservation

Scenario: repeated scheduler release does not duplicate queued fan_out wakeup
  Test:
    Package: agentd-bin
    Filter: production_workflow_scheduler_queued_fanout_wakeup_is_idempotent
  Level: daemon integration
  Test Double: real SqliteStore and recording fake backend
  Given a queued `parallel.fan_out` reviewer has already been awakened by a
  drained scheduler ticket
  When the same release path is observed again through a replayed or unrelated
  reviewer completion
  Then no second backend dispatch is recorded for the queued review run
  And the scheduler queue keeps one drained ticket rather than creating another
  queued ticket

Scenario: parity docs record p233 queued fan_out wakeup progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p233_queued_fanout_wakeup_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the pool scheduler, task graph, migration, runtime launch, and Phase E
  sections are inspected
  Then they mention p233 queued fan_out reviewer wakeup, release-drain dispatch,
  and duplicate-dispatch suppression
  And they remain partial because dashboard views, Matrix/remote relay, cutover,
  rollback, notification gates, token provisioning, and lifecycle gaps are not
  complete
