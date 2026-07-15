spec: task
name: "agent-chat Matrix puppet identity map"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p243]
---

## Intent

Move the Matrix bridge one slice closer to agent-chat replacement by making
agent puppet identity mapping explicit and testable inside `agentd-matrix`.
This slice covers local deterministic mapping only: known agent names become
planned Matrix puppet MXIDs, skipped service agents are excluded, and the client
transport can distinguish known puppet MXIDs from arbitrary `@ac_*` senders.

## Decisions

- Add a local `MatrixPuppetDirectory` and `MatrixPuppetAccount` model in
  `agentd-matrix` for deterministic puppet identity planning.
- Use an explicit Matrix `server_name` plus `agent_user_prefix` to generate
  local puppet MXIDs in the form `@<prefix><agent_name>:<server_name>`.
- De-duplicate known agents case-insensitively while preserving the first
  observed canonical spelling.
- Exclude `skip_agent_names` case-insensitively from planned puppet accounts;
  this mirrors agent-chat's `MATRIX_BRIDGE_SKIP_AGENTS` behavior for service
  relays such as `openfab-bridge`.
- Reject empty server names, empty prefixes, empty agent names, agent names that
  already look like MXIDs, and names that contain Matrix separators.
- Extend `MatrixClientTransportConfig` with optional `matrix_server_name`,
  `known_agent_names`, and `skip_agent_names`; when `matrix_server_name` is set,
  loop suppression must use `MatrixPuppetDirectory` and suppress only known
  non-skipped puppet MXIDs.
- Keep the old prefix-only suppression fallback when `matrix_server_name` is not
  configured so existing p240 behavior remains compatible.
- Keep account registration, token provisioning, password derivation, avatar
  sync, room lifecycle, Matrix media, bot commands, service packaging, cutover,
  and rollback out of this slice.

## Boundaries

### Allowed Changes

- specs/e2e/p243-agent-chat-matrix-puppet-identity-map.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/client_transport.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md
- Cargo.lock

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not connect to a real Matrix homeserver in tests.
- Do not register Matrix bot or puppet accounts.
- Do not generate or persist Matrix passwords or access tokens.
- Do not upload or download Matrix media.
- Do not add Matrix bot command handling.
- Do not add long-running bridge service packaging, cutover, rollback, token
  provisioning, or token rotation.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

- Real homeserver registration/login, password derivation, encrypted-room
  verification, agent avatar sync, DM/group room lifecycle, media transfer, bot
  commands, service installation, operator cutover, rollback automation, and
  dashboard rendering.

## Completion Criteria

<!-- lint-ack: decision-coverage - p243 binds MatrixPuppetDirectory, MatrixPuppetAccount, mxid generation, skip-agent behavior, validation, transport suppression, compatibility fallback, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify generated MXIDs, reverse lookup, invalid local config, transport inbound events, fallback source inspection, and repository Markdown state. -->
<!-- lint-ack: error-path - p243 includes invalid config rejection, unknown/skipped puppet non-suppression, and not-covered parity scenarios. -->

Scenario: puppet directory plans known non-skipped agent MXIDs
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_directory_plans_known_non_skipped_agent_mxids
  Level: library unit
  Test Double: local agent names and Matrix server name
  Given known agents with duplicate casing and a service agent in `skip_agent_names`
  When `MatrixPuppetDirectory` plans puppet accounts
  Then non-skipped agents are de-duplicated case-insensitively
  And each account preserves the canonical agent name
  And each account has a localpart using `agent_user_prefix`
  And each account has an MXID using the configured Matrix `server_name`

Scenario: puppet directory resolves only known local puppet MXIDs
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_directory_resolves_only_known_local_puppet_mxids
  Level: library unit
  Test Double: local Matrix MXID strings
  Given a `MatrixPuppetDirectory` with known agents and skipped service agents
  When Matrix sender MXIDs are classified
  Then a known local puppet MXID resolves to its agent name
  And an unknown `@ac_*` MXID does not resolve
  And a skipped service-agent MXID does not resolve
  And a same-prefix MXID from another server does not resolve

Scenario: puppet directory rejects invalid identity inputs
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_directory_rejects_invalid_identity_inputs
  Level: library unit
  Test Double: invalid local config strings
  Given empty server names, empty prefixes, empty agent names, MXID-like names, and names with Matrix separators
  When `MatrixPuppetDirectory` is built
  Then invalid inputs return a `BridgeError::InvalidConfig`
  And no partial puppet plan is returned

Scenario: Matrix client transport suppresses known puppet loops without hiding unknown prefix users
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_suppresses_known_puppets_not_unknown_prefix_users
  Level: library unit
  Test Double: fake Matrix client sync snapshot
  Given `MatrixClientTransportConfig` includes `matrix_server_name`, known agents, and skipped agents
  When inbound Matrix text events are normalized
  Then bot echoes and known non-skipped puppet MXIDs are suppressed
  And unknown `@ac_*` senders and skipped service-agent MXIDs are still forwarded
  And existing `[AGENTIGNORE]` suppression remains active

Scenario: compatibility fallback and parity docs record p243 without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p243_matrix_puppet_identity_progress
  Level: artifact inspection
  Test Double: repository Markdown files and Rust source text
  Given the agent-chat replacement parity map, roadmap, and Matrix client transport source
  When p243 progress is inspected
  Then the Matrix bridge row mentions p243 and puppet identity mapping
  And the Matrix bridge row remains partial
  And the row still names account registration, Matrix media, cutover, rollback, and token gaps
  And the source still contains the prefix-only fallback for configs without `matrix_server_name`
