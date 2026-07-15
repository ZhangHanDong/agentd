spec: task
name: "agent-chat Matrix bot group management effects"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, bot-commands, group-management, phase-g, p261]
---

## Intent

Continue the p258/p260 Matrix bot management-command path by executing the
agent-chat group management commands that map cleanly to daemon group APIs.
Authorized Matrix bot `!mkgroup`, `!addmember`, `!rmember`, and `!rmgroup`
commands must mutate agentd backend group state through explicit effect ports
instead of returning unsupported-command notices, while default tests continue
to use fake Matrix clients and no real agent runtime.

## Decisions

- `!mkgroup <name> [member...]` calls a backend group-create effect and replies
  with the agent-chat-compatible "Group \"<name>\" created with members: ..."
  text.
- `!addmember` and `!rmember` accept either explicit room-agnostic arguments
  (`!addmember <group> <name>`) or one-argument group-room context
  (`!addmember <name>` when `MatrixBotCommandContext.group_name` is set).
- `!rmgroup` accepts an explicit group name or falls back to group-room context,
  verifies the backend group exists, deletes the daemon group record, and
  reports that Matrix room cleanup remains outside p261.
- Agentd's HTTP backend maps these effects to `POST /api/groups`,
  `POST /api/groups/:name/members`, `GET /api/groups/:name`, and
  `DELETE /api/groups/:name` using the existing operator bearer token path.
- The daemon HTTP surface gains a minimal `DELETE /api/groups/:name` route that
  deletes the durable group and cascades group membership/messages through the
  existing SQLite foreign keys.

## Boundaries

### Allowed Changes

- specs/e2e/p261-agent-chat-matrix-bot-group-management-effects.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/bot_commands.rs
- crates/agentd-matrix/tests/http_backend.rs
- crates/agentd-surface/src/host.rs
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/src/test_support.rs
- crates/agentd-surface/tests/http.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-store/src/message_repo.rs
- crates/agentd-store/tests/messages.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run a real Matrix homeserver smoke test for p261.
- Do not use a real Claude process for p261 tests.
- Do not implement `!joingroup`, `!spy`, `!agentctl`, or `!ctl` in p261.
- Do not implement Matrix room leave/kick cleanup for `!rmgroup` in p261.
- Do not implement Matrix media transfer, token rotation, service packaging, or
  cutover/rollback in p261.

## Acceptance Criteria

Scenario: executor creates a group through backend effects
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_executor_with_effects_creates_group
  Given an authorized Matrix bot command `!mkgroup ops codex-worker codex-reviewer`
  When command replies are executed with effects
  Then the backend effect receives group `ops` and both members
  And the reply contains `Group "ops" created with members: codex-worker, codex-reviewer`
  And the execution declares `MutatesBackend`

Scenario: executor updates group membership with explicit and contextual syntax
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_executor_with_effects_updates_group_members
  Given an authorized command `!addmember ops codex-reviewer`
  And an authorized command `!rmember codex-worker` in a Matrix room mapped to group `ops`
  When command replies are executed with effects
  Then the backend effect receives an add request for `ops/codex-reviewer`
  And the backend effect receives a remove request for `ops/codex-worker`
  And both executions declare `MutatesBackend`

Scenario: executor removes a group after backend existence check
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_executor_with_effects_removes_group
  Given the backend reports group `ops` exists
  And an authorized Matrix bot command `!rmgroup ops`
  When command replies are executed with effects
  Then the backend effect deletes group `ops`
  And the reply contains `Group "ops" removed`
  And the reply states Matrix room cleanup is not included in p261

Scenario: executor validates group management command usage and backend errors
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_executor_with_effects_rejects_invalid_group_management
  Given missing command arguments, unknown groups, and backend mutation errors
  When command replies are executed with effects
  Then usage errors do not declare side effects
  And backend errors reply with agent-chat-compatible `Failed: ...` text
  And no extra backend mutations are attempted after a failed existence check

Scenario: HTTP backend sends agent-chat group management request shapes
  Test:
    Package: agentd-matrix
    Filter: agentd_http_backend_executes_group_management_effect_requests
  Given a fake agentd HTTP endpoint with an operator token
  When the Matrix backend effect port creates, updates, looks up, and deletes a group
  Then requests use `POST /api/groups`, `POST /api/groups/ops/members`,
  `GET /api/groups/ops`, and `DELETE /api/groups/ops`
  And JSON bodies match the daemon group API
  And the authorization header is preserved on all requests

Scenario: daemon group delete route removes durable group state
  Test:
    Package: agentd-surface
    Filter: http_delete_group_removes_group_and_members
  Given a daemon HTTP app with group `factory` and members
  When `DELETE /api/groups/factory` is called
  Then the response is successful
  And `GET /api/groups/factory` returns `group_not_found`
  And deleting an unknown group returns `404`

Scenario: store group delete cascades membership and messages
  Test:
    Package: agentd-store
    Filter: group_delete_cascades_members_and_messages
  Given a durable group with members and a group message
  When the group is deleted through the message repository
  Then the group is no longer listed
  And group message reads for the deleted group return no messages

Scenario: parity docs record p261 without declaring full Matrix replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p261_matrix_group_management_progress
  Given the agent-chat replacement parity map and roadmap
  When p261 progress is inspected
  Then the Matrix bridge row mentions p261 group management effects for
  `!mkgroup`, `!addmember`, `!rmember`, and `!rmgroup`
  And the row remains partial
  And the row still names `!joingroup`, admin commands, Matrix room cleanup,
  Matrix media, real homeserver evidence, service packaging, cutover, rollback,
  token rotation, bridge operations, and dashboard/operator visibility gaps

## Out of Scope

- Real homeserver operator smoke evidence.
- Matrix group-room creation, leave, kick, invite, or cleanup lifecycle.
- Human self-join semantics for `!joingroup`.
- Admin command execution for `!spy`, `!agentctl`, or `!ctl`.
- Per-agent puppet client execution evidence.
