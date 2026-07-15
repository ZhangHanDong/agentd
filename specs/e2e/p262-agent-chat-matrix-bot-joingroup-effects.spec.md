spec: task
name: "agent-chat Matrix bot joingroup effects"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, bot-commands, group-management, phase-g, p262]
---

## Intent

Close the next Matrix bot management-command gap by executing agent-chat's
`!joingroup` command in agentd. The command must add the human sender to the
daemon group using the same human-name rule as agent-chat, and the SDK-facing
transport must invite the sender's full Matrix MXID when a trusted Matrix group
room mapping is already known.

## Decisions

- `!joingroup <group>` and contextual `!joingroup` resolve the group the same
  way agent-chat does: explicit argument first, then `MatrixBotCommandContext.group_name`.
- The daemon group member added by `!joingroup` is
  `MatrixBotCommand.sender_human_localpart`, not the full Matrix MXID, matching
  agent-chat's `humanName` backend membership behavior.
- Matrix room invite effects use `MatrixBotCommand.sender_mxid`, so federated
  humans are invited with their complete Matrix id.
- A successful backend mutation attempts a group-room invite only when a trusted
  Matrix room registration exists for the target group; missing mappings are
  reported without contacting a homeserver.
- p262 keeps `!spy`, `!agentctl`, `!ctl`, Matrix media, real homeserver smoke,
  service packaging, cutover, and rollback out of scope.

## Boundaries

### Allowed Changes

- specs/e2e/p262-agent-chat-matrix-bot-joingroup-effects.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/bot_commands.rs
- crates/agentd-matrix/tests/client_transport.rs
- crates/agentd-matrix/tests/http_backend.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run a real Matrix homeserver smoke test for p262.
- Do not use a real Claude process for p262 tests.
- Do not implement `!spy`, `!agentctl`, or `!ctl` in p262.
- Do not implement Matrix media transfer, service packaging, cutover, rollback,
  token rotation, or dashboard work in p262.
- Do not change p261 `!mkgroup`, `!addmember`, `!rmember`, or `!rmgroup`
  command semantics except where shared helpers require non-behavioral cleanup.

## Acceptance Criteria

Scenario: executor joins an explicit group with backend and room effects
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_executor_with_effects_joins_group
  Given an authorized Matrix bot command `!joingroup ops` from `@alex:matrix.test`
  When command replies are executed with effects
  Then the backend effect receives an add-member request for group `ops` and member `alex`
  And the room effect receives group `ops` and human MXID `@alex:matrix.test`
  And the reply contains `Added you (alex) to group "ops"`
  And the execution declares `MutatesBackend` and `ChangesMatrixRooms`

Scenario: executor joins the contextual group when no argument is supplied
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_executor_with_effects_joins_contextual_group
  Given an authorized Matrix bot command `!joingroup` in a room mapped to group `ops`
  When command replies are executed with effects
  Then the backend effect receives an add-member request for group `ops` and member `alex`
  And the room effect receives group `ops` and human MXID `@alex:matrix.test`
  And the reply contains `Added you (alex) to group "ops"`

Scenario: executor rejects invalid joingroup usage and backend failures
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_executor_with_effects_rejects_invalid_joingroup
  Given a `!joingroup` command without an argument or group-room context
  And a backend mutation result with error `group_not_found`
  When command replies are executed with effects
  Then the missing-group command replies with `Usage: !joingroup <group> (or use inside a group room)`
  And the failed backend mutation replies with `Failed: group_not_found`
  And no Matrix room invite effect runs after the failed backend mutation

Scenario: SDK-facing transport invites a joingroup sender to the trusted group room
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_joingroup_invites_sender_to_trusted_group_room
  Given a fake Matrix sync with a trusted joined room mapped to group `ops`
  And a `!joingroup ops` bot command from `@alex:matrix.test` in another trusted command room
  When management command replies are executed with effects
  Then the fake backend adds member `alex` to group `ops`
  And the fake Matrix client invites `@alex:matrix.test` to the trusted group room
  And the command reply is sent to the command room

Scenario: SDK-facing transport reports missing group-room mapping without Matrix invite
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_joingroup_reports_missing_group_room_mapping
  Given a fake Matrix sync with a `!joingroup ops` bot command but no trusted room mapped to group `ops`
  When management command replies are executed with effects
  Then the fake backend adds member `alex` to group `ops`
  And no Matrix invite request is sent
  And the command reply reports that no trusted Matrix group room is mapped

Scenario: HTTP backend joingroup uses existing group member update route
  Test:
    Package: agentd-matrix
    Filter: agentd_http_backend_executes_joingroup_member_update_request
  Given a fake agentd HTTP endpoint with an operator token
  When the Matrix backend effect port updates group `ops` with add member `alex`
  Then the request uses `POST /api/groups/ops/members`
  And the JSON body is `{ "add": ["alex"], "remove": [] }`
  And the authorization header is preserved

Scenario: parity docs record p262 without declaring full Matrix replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p262_matrix_joingroup_progress
  Given the agent-chat replacement parity map and roadmap
  When p262 progress is inspected
  Then the Matrix bridge row mentions p262 `!joingroup` backend and trusted group-room invite effects
  And the row remains partial
  And the row still names admin commands, Matrix media, real homeserver evidence,
  service packaging, cutover, rollback, token rotation, bridge operations, and
  dashboard/operator visibility gaps

## Out of Scope

- Real homeserver operator smoke evidence.
- Admin command execution for `!spy`, `!agentctl`, or `!ctl`.
- Matrix media transfer.
- Service packaging, cutover, rollback, and dashboard work.
- General room lifecycle parity beyond inviting the `!joingroup` sender to an
  already trusted group room mapping.
