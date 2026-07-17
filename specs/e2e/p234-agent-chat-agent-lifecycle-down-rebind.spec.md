spec: task
name: "agent-chat agent lifecycle down and rebind recovery"
tags: [agent-chat-replacement, registry, lifecycle, runtime, phase-c, p234]
---

## Intent

Advance the agent-chat replacement path by wiring the next local agent lifecycle
operations above the native runtime boundary: stop an online agent session,
record the stopped state durably, and recover a surviving native session after a
daemon restart through explicit rebind. This slice keeps the work local and
Codex/fake-testable; it does not invoke non-Codex providers in automated tests.

## Decisions

- Add an operator-local `POST /api/agents/:name/down` endpoint compatible with
  the agent-chat dashboard action. It stops the registered runtime through an
  injectable lifecycle port, archives before shutdown, marks the agent offline,
  clears the native runtime reference, and records lifecycle metadata in
  `runtime_state`.
- Keep `POST /api/agents/:name/offline` as the agent/server-reported state
  endpoint. `down` is an operator action that performs runtime shutdown first.
- Add an operator-local `POST /api/agents/:name/rebind` endpoint. It reads the
  stored native runtime reference, probes/reconstructs a handle through the
  lifecycle port, and marks the agent online when the target is live.
- Rebind returns HTTP 200 with `rebound=false` when the stored target no longer
  exists; in that case agentd marks the agent offline with reason
  `rebind-missing-session` and preserves the recovery observation in
  `runtime_state`.
- Rebind after a daemon host rebuild uses the same SQLite registry state; no
  in-memory runtime handle is required to recover the session view.
- Add `agentctl agent down` and `agentctl agent rebind` as dependency-free HTTP
  clients for the new daemon endpoints. These are operator commands and use the
  bearer token path, not the per-agent token path.
- Real daemon assembly uses `NativeAgentLifecycle` behind the lifecycle port.
  Tests use Codex records, fake backends, and recording fake lifecycle ports
  only.
- Keep replacement parity partial after this slice. Dashboard panels,
  Matrix/remote relay state, service cutover, rollback automation, token
  provisioning/rotation, and full agent home/profile management remain open.

## Boundaries

### Allowed Changes

- specs/e2e/p234-agent-chat-agent-lifecycle-down-rebind.spec.md
- crates/agentd-store/src/agent_repo.rs
- crates/agentd-store/tests/agent_registry.rs
- crates/agentd-surface/src/host.rs
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/src/test_support.rs
- crates/agentd-bin/src/daemon.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/daemon_http.rs
- crates/agentctl/src/agent.rs
- crates/agentctl/src/cli.rs
- crates/agentctl/tests/agent_cli.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not implement dashboard rendering, Matrix bridge state, remote relay state,
  service cutover state, rollback automation, token provisioning, or token
  rotation.
- Do not turn `offline` into a shutdown endpoint; it remains a state report.
- Do not claim that agentd can fully replace agent-chat after this slice.

## Out of Scope

- Force-deleting agent registry rows and tombstone rollback.
- Agent-chat `agent-down` active-work guards and Claude resume-hint refusal.
- Automatic rebind sweep on daemon boot.
- Provisioning missing agents from scheduler provision plans.
- Remote-host lifecycle control.

## Completion Criteria

<!-- lint-ack: decision-coverage - p234 binds store runtime-state lifecycle metadata, daemon down/rebind behavior, daemon rebuild recovery, CLI paths, real tmux adapter wiring, and parity docs through explicit tests or compile-bound assembly. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify SQLite registry state, fake lifecycle calls, HTTP JSON responses, CLI request paths, and parity Markdown without real Claude/tmux process execution. -->
<!-- lint-ack: boundary-entry-point - daemon and CLI entry points are verified through bound `agentd-bin` and `agentctl` package filters. -->

Scenario: store merges lifecycle metadata into runtime state
  Test:
    Package: agentd-store
    Filter: agent_registry_lifecycle_patch_merges_runtime_state
  Level: store integration
  Test Double: tempfile SQLite database
  Given a registered Codex agent with existing runtime observation state
  When the store merges a lifecycle observation for a down or rebind action
  Then the existing runtime observation remains readable
  And the lifecycle metadata is persisted under `runtime_state.lifecycle`

Scenario: daemon down stops runtime and marks the agent offline
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_down_stops_runtime_and_marks_offline
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite, fake backend, recording lifecycle port
  Given an online registered Codex agent with a stored native runtime reference and state dir
  When a client posts `/api/agents/codex-worker/down`
  Then the recording lifecycle port sees one shutdown request for that target
  And the response reports `action="agent-down-kill"`
  And the agent status becomes "offline", `native_runtime_ref` is null, and
  `runtime_state.lifecycle.state` is "down"

Scenario: daemon rebind recovers live sessions and marks missing targets offline
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_rebind_recovers_live_session_and_marks_missing_offline
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite, fake backend, scripted lifecycle port
  Given one online Codex agent whose stored target is live
  And another online Codex agent whose stored target is gone
  When clients post `/api/agents/:name/rebind`
  Then the live target response reports `rebound=true` and returns a handle
  And the missing target response reports `rebound=false`
  And the missing target agent is marked offline with reason
  `rebind-missing-session`

Scenario: daemon rebuild can recover a stored session through rebind
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_rebind_recovers_after_host_rebuild
  Level: HTTP integration
  Test Double: two ProductionRunHost instances over one SQLite file and one scripted lifecycle port
  Given an online registered Codex agent persisted by one daemon host
  When a second daemon host opens the same database and posts
  `/api/agents/codex-worker/rebind`
  Then the session is recovered from the stored native runtime reference
  And no backend spawn is required

Scenario: agentctl calls down and rebind endpoints
  Test:
    Package: agentctl
    Filter: agent_cli_down_and_rebind_use_lifecycle_api_agents
  Level: CLI
  Test Double: one-shot local TCP daemon
  Given a local fake HTTP daemon that records requests
  When `agentctl agent down codex-sec` and `agentctl agent rebind codex-sec`
  run against it
  Then the fake daemon observes `/api/agents/codex-sec/down` and
  `/api/agents/codex-sec/rebind`
  And the CLI exits successfully and prints the daemon response body

Scenario: parity docs record p234 lifecycle progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p234_agent_lifecycle_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the registry lifecycle, runtime launch, and migration cutover rows are
  inspected
  Then they mention p234 down, rebind, session recovery, and runtime lifecycle
  metadata
  And they remain partial because dashboard, Matrix/remote relay, cutover,
  rollback, token provisioning, and full agent home/profile management are not
  complete
