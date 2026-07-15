spec: task
name: "agent-chat Matrix DM room lifecycle"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, bot-commands, phase-g, p260]
---

## Intent

Continue the p258 `!dm` management-command path by replacing the placeholder
Matrix room effect with a fake-tested SDK-facing DM room lifecycle. The bridge
must derive the agent puppet MXID from the Matrix transport config, use the
full human sender MXID, reuse an existing trusted direct room when possible,
and create or invite through `MatrixClientPort` operations without requiring a
real homeserver in default tests.

## Decisions

- `!dm <agent>` room effects use the sender's full Matrix MXID, not only the
  localpart fallback, so federated humans are not silently rewritten to the
  local homeserver.
- The lifecycle derives the agent puppet account with the existing
  `MatrixPuppetDirectory` using `matrix_server_name`, `agent_user_prefix`,
  `known_agent_names`, and `skip_agent_names`.
- Existing trusted direct room registrations for the target agent are reused
  before creating any new room.
- Missing direct rooms are created through a new `MatrixClientPort` direct-room
  operation that invites both the human MXID and the agent puppet MXID and names
  the room `DM: <agent>`.
- Existing direct rooms inspect the human membership first; joined humans return
  `Joined`, absent humans trigger an invite, and invite failures return
  `InviteFailed` with the error text while preserving the room link.
- The real Matrix SDK adapter implements the new port operations behind the
  existing `matrix-sdk-adapter` feature; default builds and default tests remain
  SDK-free.

## Boundaries

### Allowed Changes
- specs/e2e/p260-agent-chat-matrix-dm-room-lifecycle.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/bot_commands.rs
- crates/agentd-matrix/tests/client_transport.rs
- crates/agentd-matrix/tests/client_bridge_once.rs
- crates/agentd-matrix/tests/sdk_adapter.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden
- Do not run a real Matrix homeserver smoke test for p260.
- Do not use a real Claude process for p260 tests.
- Do not add a new cargo dependency.
- Do not implement Matrix media transfer or additional bot/admin commands in
  p260.
- Do not change daemon identity persistence semantics from p259.

## Acceptance Criteria

Scenario: transport reuses a joined human-agent direct room
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_dm_lifecycle_reuses_joined_direct_room
  Given an authorized `!dm codex-worker` command from `@alex:matrix.test`
  And the sync snapshot has a trusted direct room mapped to `codex-worker`
  And the fake Matrix client reports `@alex:matrix.test` as joined in that room
  When management command replies are executed with effects
  Then the bridge sends a reply containing "already in the DM room"
  And no direct room is created
  And no invite request is sent

Scenario: transport creates a missing human-agent direct room
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_dm_lifecycle_creates_missing_direct_room
  Given an authorized `!dm codex-worker` command from `@alex:matrix.test`
  And the transport config has server `matrix.test`, agent prefix `ac_`, and known agent `codex-worker`
  And no trusted direct room exists for `codex-worker`
  When management command replies are executed with effects
  Then the fake Matrix client creates a direct room named `DM: codex-worker`
  And the direct-room invite list contains `@alex:matrix.test` and `@ac_codex-worker:matrix.test`
  And the bridge sends a reply containing "DM room ready"

Scenario: transport reports invite failure for an existing direct room
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_dm_lifecycle_reports_invite_failure_for_existing_room
  Given an authorized `!dm codex-worker` command from `@alex:matrix.test`
  And a trusted direct room exists for `codex-worker`
  And the fake Matrix client reports no human membership and rejects the invite
  When management command replies are executed with effects
  Then the bridge sends a reply containing "DM room exists but invite failed"
  And the reply includes the fake invite error text
  And no new direct room is created

Scenario: bridge once executes DM lifecycle without a real homeserver
  Test:
    Package: agentd-matrix
    Filter: matrix_client_bridge_once_executes_dm_lifecycle_with_fake_client
  Given a one-shot Matrix client bridge run with a fake SDK client and a `!dm codex-worker` command
  And the fake agentd backend reports `codex-worker` as an agent
  When `run_matrix_client_bridge_once` executes the command
  Then the fake Matrix client creates or reuses the DM room through port operations
  And the run reports one bot command reply
  And no real Matrix homeserver is contacted

Scenario: SDK adapter exposes feature-gated DM lifecycle operations
  Test:
    Package: agentd-matrix
    Filter: sdk_adapter_feature_path_compiles_dm_room_lifecycle_methods
  Given the `matrix-sdk-adapter` feature is enabled only for the nested check
  When the SDK adapter tests compile with that feature
  Then `SdkMatrixClient` implements direct-room creation, room member status lookup, and room invites through `MatrixClientPort`
  And the default `agentd-matrix` build remains SDK-free

Scenario: parity docs record p260 without declaring full Matrix replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p260_matrix_dm_lifecycle_progress
  Given the agent-chat replacement parity map and roadmap
  When p260 progress is inspected
  Then the Matrix bridge row mentions p260 SDK-facing DM room lifecycle for `!dm`
  And the row remains partial
  And the row still names Matrix media, remaining management commands, service packaging, cutover, rollback, token rotation, bridge operations, and dashboard/operator visibility gaps

## Out of Scope

- Real homeserver operator smoke evidence.
- Agent puppet token provisioning changes.
- Matrix media transfer.
- Remaining Matrix management/admin commands beyond `!dm`.
- Dashboard or service packaging changes.
