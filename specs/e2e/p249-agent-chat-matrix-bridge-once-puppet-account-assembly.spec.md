spec: task
name: "agent-chat Matrix bridge-once puppet account assembly"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p249]
---

## Intent

Move the Matrix bridge one slice closer to replacing agent-chat by wiring the
p247 HTTP puppet account provisioner and the p248 bridge-state token file store
into the `agentd matrix-bridge-once` assembly path. The command remains a
deterministic one-shot runner, but operators can now opt into Matrix puppet
account provisioning through explicit CLI arguments and fake-tested local HTTP
servers.

## Decisions

- Add an optional puppet-account provisioning config to `BridgeOnceConfig`.
- `run_bridge_once` must execute puppet provisioning before the normal room,
  inbound, outbox, sent-log, and cursor pass when the optional config is present.
- `BridgeOnceReport` must retain the puppet-account provisioning report when
  provisioning is configured.
- Add `agentd matrix-bridge-once` CLI arguments for Matrix homeserver URL,
  Matrix server name, agent prefix, known agents, skipped agents, bridge-state
  puppet token file, password secret, legacy password template toggle, and
  optional registration token.
- Missing puppet-account arguments must fail with `BridgeError::InvalidConfig`
  before contacting either the Matrix homeserver or the agentd backend.
- Omitting all puppet-account CLI arguments must preserve the existing
  `matrix-bridge-once` behavior.
- Keep real daemon/service packaging, SDK account provisioning, token rotation,
  cutover, rollback, media transfer, bot commands, and Matrix profile sync out
  of this slice.

## Boundaries

### Allowed Changes

- specs/e2e/p249-agent-chat-matrix-bridge-once-puppet-account-assembly.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/file_transport.rs
- crates/agentd-bin/src/cli.rs
- crates/agentd-bin/src/matrix_bridge.rs
- crates/agentd-bin/tests/matrix_bridge_once.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not connect to a real Matrix homeserver in tests.
- Do not add a new persistence dependency or HTTP client dependency.
- Do not persist Matrix passwords.
- Do not add SQLite token tables or token rotation.
- Do not add long-running bridge service packaging, cutover, rollback, invite
  polling, display-name sync, or avatar sync.
- Do not upload or download Matrix media.
- Do not add Matrix bot command handling.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

Daemon long-running Matrix service configuration, SDK-backed account
provisioning, SQLite token storage, token rotation, encrypted-room
verification, DM/group room lifecycle, media transfer, bot commands, operator
cutover, rollback automation, dashboard rendering, and Matrix profile/avatar
updates.

## Completion Criteria

<!-- lint-ack: decision-coverage - p249 binds optional bridge-once assembly, CLI options, provisioning ordering, retained reports, missing-argument rejection, default behavior preservation, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify fake HTTP request ordering, on-disk bridge-state token updates, parsed CLI arguments, error variants, no backend contact on invalid config, and repository Markdown/source state. -->
<!-- lint-ack: error-path - p249 covers incomplete provisioning config and partial parity assertions. -->

Scenario: bridge-once config provisions puppet accounts before bridge runtime
  Test:
    Package: agentd-matrix
    Filter: matrix_bridge_once_runner_provisions_puppet_accounts_before_bridge_runtime
  Level: integration unit
  Test Double: local fake Matrix homeserver, local fake agentd backend, and temporary filesystem
  Given `BridgeOnceConfig` includes a puppet-account config with one known agent
  And the bridge-state token file contains one stale `agentTokens` entry
  And a local fake Matrix homeserver accepts one password login
  When `run_bridge_once` executes
  Then the Matrix login request is completed before the fake agentd backend receives bridge runtime calls
  And `BridgeOnceReport` contains a logged-in puppet-account outcome
  And the bridge-state token file contains the new token and no stale token
  And the normal bridge runtime still registers rooms, forwards inbound events, sends outbox events, and saves the cursor

Scenario: CLI accepts explicit puppet-account provisioning options
  Test:
    Package: agentd-bin
    Filter: agentd_cli_matrix_bridge_once_accepts_puppet_account_options
  Level: unit
  Test Double: clap parser
  Given `agentd matrix-bridge-once` is invoked with Matrix puppet-account options
  When `AgentdCli` parses the arguments
  Then the homeserver URL, server name, prefix, known agents, skipped agents, puppet state path, password secret, legacy template flag, legacy template string, and registration token are retained

Scenario: bin bridge-once assembly uses HTTP provisioner and token file store
  Test:
    Package: agentd-bin
    Filter: agentd_bin_matrix_bridge_once_provisions_puppets_with_file_store
  Level: integration unit
  Test Double: local fake Matrix homeserver, local fake agentd backend, and temporary filesystem
  Given CLI-style `MatrixBridgeOnceArgs` include Matrix puppet-account options
  And the fake Matrix homeserver accepts one Matrix password login
  When `run_matrix_bridge_once` executes
  Then the fake homeserver receives a login for the configured prefixed localpart
  And the returned report contains the logged-in puppet-account outcome
  And the bridge-state `agentTokens` object persists the new token while preserving unrelated state
  And the fake agentd backend still receives the normal room, inbound, and outbox calls

Scenario: incomplete puppet-account config fails before HTTP side effects
  Test:
    Package: agentd-bin
    Filter: agentd_bin_matrix_bridge_once_rejects_incomplete_puppet_config_without_backend_contact
  Level: integration unit
  Test Double: temporary filesystem
  Given CLI-style `MatrixBridgeOnceArgs` include a Matrix homeserver URL but omit required puppet-account fields
  When `run_matrix_bridge_once` validates the configuration
  Then it returns `BridgeError::InvalidConfig`
  And it does not create Matrix bridge cursor, sent-log, or puppet token files

Scenario: parity docs record p249 bridge-once assembly progress without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p249_matrix_bridge_once_puppet_assembly_progress
  Level: artifact inspection
  Test Double: repository Markdown files and Rust source text
  Given the agent-chat replacement parity map, roadmap, and Matrix bridge sources
  When p249 progress is inspected
  Then the Matrix bridge row mentions p249, `BridgeOncePuppetAccountConfig`, and `MatrixPuppetTokenFileStore`
  And the Matrix bridge row remains partial
  And the row still names daemon/SDK service assembly, Matrix media, cutover, rollback, token rotation, and service packaging gaps
  And the source mentions `MatrixPuppetHttpAccountProvisioner`, `MatrixPuppetTokenFileStore`, and `run_bridge_once`
