spec: task
name: "agent-chat workflow scheduler allocation"
tags: [agent-chat-replacement, workflow, scheduler, dispatch, phase-e, p230]
---

## Intent

Advance Phase E after p229 by making DOT workflow execution request agents
through the same scheduler boundary instead of treating `codergen.role` and
`parallel.fan_out.reviewers` as hardcoded runtime identities. This slice makes
workflow allocation decisions visible in run checkpoints/events and preserves the
fake/local test harness boundary; it does not claim complete agent-chat
replacement.

## Decisions

- Add a core `AgentAllocator` port between workflow handlers and the concrete
  scheduler. `codergen` and `parallel.fan_out` call this port before spawning or
  prompting an agent.
- The default allocator is direct/backward-compatible: it returns the requested
  role as the selected agent id and records `status="direct"`, so existing unit
  tests, examples, and non-scheduler workflows keep their old behavior unless a
  scheduler-backed allocator is installed.
- `codergen` still creates the task-run row before dispatch, but now persists the
  allocated agent id, not the requested role. Its prompt includes the allocated
  `agentd_agent_id` plus scheduler metadata when present.
- `parallel.fan_out` allocates each reviewer independently. Reviewer prompts and
  verdict ids use the allocated reviewer id, while stance query/profile lookup
  remains keyed by the declared reviewer role so existing graph attributes stay
  stable.
- Scheduler allocation metadata is staged into the run context before parking so
  the checkpoint and `run_parked` event payload expose the scheduler status,
  selected agent id, requested role, tier, reservation id, ticket, and
  provisioned name when available.
- Production `ProductionRunHost` wires the allocator to the durable p228
  scheduler repo so workflow allocation produces durable scheduler reservations
  or tickets. This slice exercises that wiring with fake backends only.
- On `codergen` outcome or reviewer verdict resume, workflow handlers release the
  allocated agent through the allocator. Drained queued workflow tickets are
  recorded by the scheduler release response, but this slice does not wake or
  dispatch queued workflow nodes.
- p230 updates the parity map and roadmap to show workflow scheduler allocation
  progress, while `pool_scheduler`, `task_graph_coordination`, and
  `migration_shadow_cutover` remain partial until dashboard views, Matrix/remote
  relay, online-agent prompt reuse, queued workflow wakeups, service cutover,
  rollback, and token provisioning are complete.

## Boundaries

### Allowed Changes

- specs/e2e/p230-agent-chat-workflow-scheduler-allocation.spec.md
- crates/agentd-core/src/ports/*
- crates/agentd-core/src/handler/codergen.rs
- crates/agentd-core/src/handler/fan_out.rs
- crates/agentd-core/src/handler/mod.rs
- crates/agentd-core/src/test_support/*
- crates/agentd-core/tests/handlers_park.rs
- crates/agentd-core/tests/engine_execute.rs
- crates/agentd-core/tests/handlers.rs
- crates/agentd-core/tests/worktree_threading.rs
- crates/agentd-core/examples/minimal_engine_run.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/contract.rs
- crates/agentd-bin/tests/daemon_http.rs
- crates/agentctl/tests/parity_cli.rs
- crates/agentctl/tests/workflows.rs
- crates/agentd-store/tests/store_trait.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not modify the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout or any source JSON file.
- Do not run real Claude, Matrix, tmux, systemd, launchd, or remote relay in
  automated tests.
- Do not use real Claude in tests; use Codex-prefixed roles, fake backends, or
  local non-launching harnesses only.
- Do not require a real online tmux pane for workflow allocation tests.
- Do not implement existing-pane prompt injection for routed online agents.
- Do not implement queued workflow wakeup/drain execution.
- Do not implement browser/dashboard queue UI, Matrix bridge state, remote relay
  state, service cutover state, rollback plans, or token provisioning.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Reusing an existing registered tmux pane without spawning a new backend
  session.
- Waking a parked workflow when a queued scheduler ticket drains.
- Provisioned agent startup, idle reaping, and per-cell token provisioning.
- Cross-host or remote relay scheduler coordination.
- Dashboard rendering of workflow scheduler reservations.

## Completion Criteria

<!-- lint-ack: decision-coverage - p230 binds core allocation, codergen task ownership, fan_out reviewer ownership, production scheduler wiring, run-event observability, release behavior, and docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify fake core allocation records, checkpoint/event JSON, durable scheduler rows, and parity Markdown. -->

Scenario: codergen uses allocated scheduler identity instead of requested role
  Test:
    Package: agentd-core
    Filter: codergen_allocates_agent_before_spawn_and_task_ownership
  Level: core handler
  Test Double: recording allocator and fake backend
  Given a workflow node `handler="codergen"` with `role="coding"` and
  `capability="medium"`
  When the allocator returns selected agent `codex-coding-1` with a reservation
  id and tier
  Then the task-run owner is persisted as `codex-coding-1`
  And the spawn request uses `codex-coding-1`
  And the prompt and staged context expose the scheduler allocation metadata

Scenario: fan_out allocates every reviewer before spawning reviewers
  Test:
    Package: agentd-core
    Filter: fan_out_allocates_reviewers_and_uses_selected_reviewer_ids
  Level: core handler
  Test Double: recording allocator and fake backend
  Given a `parallel.fan_out` node with reviewers `review,testing`
  When the allocator returns `codex-review-1` and `codex-testing-1`
  Then two reviewer spawn requests use the selected agent ids
  And each prompt uses the selected reviewer id for `submit_review`
  And the staged run context records one scheduler allocation per reviewer

Scenario: production workflow start writes scheduler allocation into run events
  Test:
    Package: agentd-bin
    Filter: production_workflow_scheduler_allocation_is_visible_in_run_events
  Level: daemon integration
  Test Double: real SqliteStore and fake agent backend
  Given a production host with online scheduler agents for coding and review
  When a DOT workflow parks at a scheduler-backed `codergen` or `fan_out` node
  Then the durable scheduler reservation exists
  And the `run_parked` event payload includes scheduler allocation metadata
  And no real Claude, tmux, Matrix, systemd, launchd, or remote relay process is
  started

Scenario: workflow allocation releases scheduler reservation on completion
  Test:
    Package: agentd-bin
    Filter: production_workflow_scheduler_release_on_agent_completion
  Level: daemon integration
  Test Double: real SqliteStore and fake agent backend
  Given a scheduler-backed workflow node parked on an allocated agent
  When that task submits a successful outcome through the production host
  Then the scheduler reservation for that agent is released
  And replaying the same outcome does not release or advance the run twice

Scenario: parity docs record p230 workflow scheduler progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p230_workflow_scheduler_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the pool scheduler, task graph, migration, and Phase E sections are
  inspected
  Then they mention p230 workflow scheduler allocation, codergen/fan_out
  scheduler requests, scheduler metadata in run events, and release behavior
  And they remain partial because dashboard views, Matrix/remote relay, existing
  pane prompt reuse, queued workflow wakeups, cutover, rollback, and token
  provisioning are not complete
