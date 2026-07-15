spec: task
name: "agent-chat workflow existing-pane prompt reuse"
tags: [agent-chat-replacement, workflow, scheduler, runtime, phase-e, p231]
---

## Intent

Close the next Phase E scheduler execution gap after p230: when workflow
allocation routes to an already-online registered agent, agentd must reuse that
agent's tmux pane and deliver the workflow prompt instead of attempting to spawn
a duplicate session. This slice makes existing-pane reuse explicit in the core
backend seam, the real tmux backend, and the production scheduler-backed
workflow allocator while keeping tests local and non-launching.

## Decisions

- Add an allocation-aware backend entrypoint for workflow dispatch. Its default
  behavior remains backward-compatible spawn so non-scheduler workflows and fake
  tests keep their current behavior.
- `codergen` and `parallel.fan_out` call the allocation-aware backend entrypoint
  with the `AgentAllocation` returned by `AgentAllocator`; they still persist
  task/review ownership and worktree state before dispatch.
- `TmuxBackend` handles `status="routed"` allocations that include a registered
  `tmuxTarget` or `tmux_target` by rebinding that tmux session, pasting the
  prompt with the existing `send_prompt` buffer path, and returning the rebound
  handle without running `new-session` or writing launcher files.
- A routed allocation without a usable tmux target is a hard backend error for
  workflow dispatch; agentd must not silently fall back to spawning a duplicate
  session for an online scheduler-selected agent.
- Production `SchedulerWorkflowAllocator` enriches routed allocations with the
  selected agent's registry runtime fields, including `tmuxTarget`,
  `tmux_target`, runtime, model, workdir, and runtime profile/state when
  present.
- `McpStdioContextBackend` and `PooledBackend` preserve their existing spawn
  decorations when dispatching through the allocation-aware entrypoint.
- p231 updates the parity map and roadmap to record existing-pane prompt reuse
  progress while keeping replacement partial until dashboard views,
  Matrix/remote relay, queued workflow wakeups, cutover, rollback, and token
  provisioning are complete.

## Boundaries

### Allowed Changes

- specs/e2e/p231-agent-chat-workflow-existing-pane-reuse.spec.md
- crates/agentd-core/src/ports/backend.rs
- crates/agentd-core/src/handler/codergen.rs
- crates/agentd-core/src/handler/fan_out.rs
- crates/agentd-core/src/test_support/fake_backend.rs
- crates/agentd-core/tests/handlers_park.rs
- crates/agentd-bin/src/agent_mcp_context.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/agent_mcp_context.rs
- crates/agentd-bin/tests/contract.rs
- crates/agentd-tmux/src/backend.rs
- crates/agentd-tmux/src/pool.rs
- crates/agentd-tmux/tests/inject.rs
- crates/agentd-tmux/tests/pool.rs
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
- Do not implement queued workflow wakeup/drain execution.
- Do not implement dashboard rendering, Matrix bridge state, remote relay state,
  service cutover state, rollback automation, or token provisioning.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Starting provisioned agents from scheduler provision plans.
- Waking a parked workflow after a queued scheduler ticket drains.
- Cross-host or remote relay scheduler coordination.
- Operator dashboard queue/agent panels.
- Token provisioning, rotation, or import-time secret migration.

## Completion Criteria

<!-- lint-ack: decision-coverage - p231 binds the allocation-aware backend seam, handler dispatch calls, tmux rebind+prompt behavior, missing-target failure, production registry runtime enrichment, decorator forwarding, and docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify fake handler dispatch records, scripted tmux command calls, durable SQLite registry data, backend spawn/dispatch counts, and parity Markdown. -->

Scenario: codergen dispatches routed allocation through the backend reuse seam
  Test:
    Package: agentd-core
    Filter: codergen_dispatches_allocated_agent_without_calling_plain_spawn
  Level: core handler
  Test Double: recording allocator and recording backend
  Given a `codergen` node that requests role `coding`
  When the allocator returns routed agent `codex-coding-1` with tmux runtime
  Then the backend receives one allocation-aware dispatch for `codex-coding-1`
  And the backend receives zero plain spawn calls
  And the task-run owner and staged scheduler metadata remain persisted before
  parking

Scenario: tmux backend reuses a routed online pane and pastes the workflow prompt
  Test:
    Package: agentd-tmux
    Filter: routed_allocation_rebinds_existing_pane_and_sends_prompt_without_spawn
  Level: backend
  Test Double: scripted RecordingCommandRunner
  Given a routed allocation with `tmuxTarget="agentd-codex-coding-1:0.0"`
  When the tmux backend dispatches a spawn request with an initial prompt
  Then it runs the rebind liveness probe and pane probe
  And it uses `set-buffer`, `paste-buffer`, and bare `Enter` to deliver the
  prompt
  And it does not run `new-session` or create launcher artifacts

Scenario: tmux backend rejects routed allocations that lack a tmux target
  Test:
    Package: agentd-tmux
    Filter: routed_allocation_without_tmux_target_does_not_fall_back_to_spawn
  Level: backend
  Test Double: scripted RecordingCommandRunner
  Given a routed allocation whose runtime has no `tmuxTarget` or `tmux_target`
  When the tmux backend dispatches that allocation
  Then it returns a backend error explaining the missing tmux target
  And no tmux command or launcher file is created

Scenario: production workflow routed allocation carries registry tmux target and avoids spawn
  Test:
    Package: agentd-bin
    Filter: production_workflow_scheduler_reuses_registered_pane_without_spawn
  Level: daemon integration
  Test Double: real SqliteStore and recording fake backend
  Given a production scheduler-backed workflow and an online registered Codex
  agent with `tmux_target="agentd-codex-coding-1:0.0"`
  When the workflow parks at a scheduler-backed `codergen` node
  Then the backend records one allocation-aware dispatch and zero plain spawns
  And the dispatched allocation runtime includes the registry tmux target
  And the `run_parked` event exposes the same tmux target in scheduler metadata

Scenario: parity docs record p231 existing-pane reuse progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p231_existing_pane_reuse_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the pool scheduler, task graph, migration, runtime launch, and Phase E
  sections are inspected
  Then they mention p231 existing-pane prompt reuse, tmux rebind, routed online
  agents, and no duplicate spawn
  And they remain partial because queued workflow wakeups, dashboard views,
  Matrix/remote relay, cutover, rollback, and token provisioning are not
  complete
