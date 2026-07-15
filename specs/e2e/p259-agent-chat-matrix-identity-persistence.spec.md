spec: task
name: "agent-chat Matrix identity persistence"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, bot-commands, phase-g, p259]
---

## Intent

Continue the p258 Matrix bot management-command work by making the daemon accept
and persist the `PATCH /api/agents/:name` identity update that `!identity`
already sends through `AgentdHttpBackend`. This closes the fake-server-only
identity gap without broadening scope into Matrix room lifecycle, remaining bot
admin commands, CLI UX, or real homeserver evidence.

## Decisions

- Add daemon-side identity persistence for `PATCH /api/agents/:name` with a
  JSON body containing `identity`.
- Store the identity text in the existing agent `runtime_profile` JSON as the
  top-level `identity` field so p259 does not require a schema migration or a
  new agent table column.
- Preserve existing `runtime_profile` keys when identity is changed.
- Return the updated agent record on success, return `agent_not_found` for an
  unknown agent, and reject empty or whitespace-only identity text before
  mutating state.
- Treat identity patching as an operator-managed route; use the same local
  operator authorization boundary as other operator agent routes.
- Keep default tests fake/local only; do not use real Claude, real Matrix
  homeservers, real daemon supervision, or real execute smoke.

## Boundaries

### Allowed Changes
- specs/e2e/p259-agent-chat-matrix-identity-persistence.spec.md
- crates/agentd-store/src/agent_repo.rs
- crates/agentd-store/tests/agent_registry.rs
- crates/agentd-surface/src/host.rs
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/src/test_support.rs
- crates/agentd-surface/tests/http.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/daemon_http.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden
- Do not add a new database migration or new agent table column for p259.
- Do not implement real Matrix room creation/invite lifecycle.
- Do not implement additional bot/admin commands beyond making p258
  `!identity` persist through the daemon.
- Do not add new cargo dependencies.
- Do not contact a real Matrix homeserver or start real Claude/agent runtimes in
  tests.
- Do not claim full agent-chat Matrix replacement in parity documentation.

## Completion Criteria

Scenario: store updates identity in runtime profile without losing existing profile keys
  Test:
    Package: agentd-store
    Filter: agent_registry_identity_patch_persists_runtime_profile_text
  Given a registered `codex-worker` agent with an existing `runtime_profile.primary.framework`
  When the store identity update runs with `Be concise and report blockers`
  Then the returned agent has `runtime_profile.identity` set to that text
  And the existing `runtime_profile.primary.framework` key remains unchanged
  And inspecting the agent again returns the same persisted identity text

Scenario: store rejects empty identity and unknown agents without mutation
  Test:
    Package: agentd-store
    Filter: agent_registry_identity_patch_rejects_empty_and_unknown_agents
  Given a registered `codex-worker` agent with no `runtime_profile.identity`
  When the store identity update runs with whitespace-only text
  Then it returns a validation error and leaves the runtime profile unchanged
  And updating identity for `ghost` returns `None`

Scenario: HTTP surface patches agent identity through the host
  Test:
    Package: agentd-surface
    Filter: http_agent_identity_patch_persists_profile_and_reports_errors
  Level: HTTP route integration
  Test Double: in-process `FakeRunHost`
  Targets: crates/agentd-surface/tests/http.rs
  Given a fake host with a registered `codex-worker`
  And the requested identity text is `Review carefully`
  When an operator sends `PATCH /api/agents/codex-worker` with the requested identity text
  Then the response is successful and includes the updated agent record
  And `GET /api/agents/codex-worker` returns `runtime_profile.identity` as `Review carefully`
  And an empty identity patch is rejected before mutating the profile
  And patching an unknown agent returns `agent_not_found`

Scenario: production daemon route persists identity after router rebuild
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_identity_patch_persists_after_router_rebuild
  Level: daemon assembly integration
  Test Double: production host with fake backend/lifecycle
  Targets: crates/agentd-bin/tests/daemon_http.rs
  Given a production daemon router backed by a temporary SQLite store
  And a registered `codex-worker` with an existing runtime profile
  When an operator sends `PATCH /api/agents/codex-worker` with an identity text
  Then the response includes `runtime_profile.identity`
  And rebuilding the router over the same database preserves the identity
  And the existing profile keys remain present

Scenario: parity docs record p259 without declaring full Matrix replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p259_matrix_identity_persistence_progress
  Targets: crates/agentctl/tests/parity_cli.rs
  Given the agent-chat replacement parity map and roadmap
  When p259 progress is inspected
  Then the Matrix bridge row mentions daemon identity persistence for `PATCH /api/agents/:name`
  And the row remains partial
  And the row still names real SDK DM room lifecycle, remaining management commands, Matrix media, service packaging, cutover, rollback, token rotation, bridge operations, and dashboard/operator visibility gaps
