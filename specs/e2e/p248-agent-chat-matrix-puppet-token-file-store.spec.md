spec: task
name: "agent-chat Matrix puppet token file store"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p248]
---

## Intent

Move the Matrix bridge one slice closer to agent-chat replacement by adding a
durable Matrix puppet token backend for the p245-p247 provisioning path. This
slice gives agentd-matrix a file-backed store for agent-chat-style
`bridge-state.json` `agentTokens`, while preserving unrelated bridge state so a
later daemon/bridge assembly can reuse the same state file safely.

## Decisions

- Add `MatrixPuppetTokenFileStore` in `agentd-matrix`.
- The store must read agent-chat-compatible JSON with a top-level
  `agentTokens` object into `MatrixPuppetTokenState`.
- Missing state files must load as an empty token state so first-run provisioning
  can create the file.
- `save_agent_token` must preserve an existing case-insensitive token key when
  replacing a token; otherwise it writes the canonical p243 agent name.
- `delete_agent_token` must remove stale token names from the `agentTokens`
  object without touching unrelated fields.
- Writes must create missing parent directories, preserve unknown top-level
  bridge-state fields, and use a temp-file-then-rename write path.
- Malformed JSON must return `BridgeError::State` and must not overwrite the
  existing file.
- Keep daemon CLI/env configuration, SQLite token tables, token rotation,
  display-name/avatar sync, service packaging, cutover, and rollback out of this
  slice.

## Boundaries

### Allowed Changes

- specs/e2e/p248-agent-chat-matrix-puppet-token-file-store.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/puppet_token_file_store.rs
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
- Do not add daemon CLI/env wiring for Matrix account provisioning.
- Do not add SQLite token tables or token rotation.
- Do not upload or download Matrix media.
- Do not add Matrix bot command handling.
- Do not add long-running bridge service packaging, cutover, rollback, invite
  polling, display-name sync, or avatar sync.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

Daemon configuration for account provisioning, SDK-backed account provisioning,
SQLite token storage, token rotation, encrypted-room verification, DM/group room
lifecycle, media transfer, bot commands, operator cutover, rollback automation,
dashboard rendering, and Matrix profile/avatar updates.

## Completion Criteria

<!-- lint-ack: decision-coverage - p248 binds agent-chat bridge-state shape, first-run defaulting, case-insensitive replacement, stale deletion, malformed JSON protection, provisioner integration, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify on-disk JSON shape, preserved fields, temp-write side effects, error variants, fake HTTP request flow, and repository Markdown/source state. -->
<!-- lint-ack: error-path - p248 covers missing files, malformed JSON, stale-token deletion, and partial parity assertions. -->

Scenario: file token store loads agent-chat state and preserves unknown fields
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_token_file_store_loads_agent_chat_state_and_preserves_unknown_fields
  Level: integration unit
  Test Double: temporary filesystem
  Given a bridge-state JSON file with `botToken`, `agentTokens`, `roomGroupMap`, and an unknown field
  When `MatrixPuppetTokenFileStore` loads the token state
  Then it resolves token names case-insensitively through `MatrixPuppetTokenState`
  And saving a new token for the same agent updates the existing stored key
  And the file still contains the unrelated top-level fields

Scenario: file token store creates first-run state and parent directories
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_token_file_store_creates_missing_state_and_parent_dirs
  Level: integration unit
  Test Double: temporary filesystem
  Given the configured bridge-state path does not exist
  When the file store loads token state and saves a token
  Then loading returns an empty `MatrixPuppetTokenState`
  And the parent directory is created
  And the written JSON contains an `agentTokens` object with the saved token

Scenario: file token store deletes stale tokens without touching other state
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_token_file_store_deletes_stale_tokens_without_touching_other_state
  Level: integration unit
  Test Double: temporary filesystem
  Given a bridge-state JSON file with multiple `agentTokens` and unrelated fields
  When `delete_agent_token` removes one stale token name
  Then only that token name is absent
  And the remaining token names and unrelated fields are preserved

Scenario: file token store rejects malformed JSON without overwriting
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_token_file_store_rejects_malformed_json_without_overwriting
  Level: integration unit
  Test Double: temporary filesystem
  Given a bridge-state path containing malformed JSON
  When loading token state or saving a token through the file store
  Then each call returns `BridgeError::State`
  And the malformed file contents remain unchanged

Scenario: file token store persists HTTP provisioner token updates
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_token_file_store_persists_http_provisioner_updates
  Level: integration unit
  Test Double: local fake HTTP homeserver and temporary filesystem
  Given an agent-chat-style bridge-state file with one stale Matrix puppet token
  And a local fake homeserver that accepts one Matrix password login
  When `MatrixPuppetHttpAccountProvisioner` provisions using `MatrixPuppetTokenFileStore`
  Then the report includes a logged-in outcome
  And the saved bridge-state JSON contains the new agent token
  And the stale token name is deleted from `agentTokens`

Scenario: parity docs record p248 token file store progress without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p248_matrix_puppet_token_file_store_progress
  Level: artifact inspection
  Test Double: repository Markdown files and Rust source text
  Given the agent-chat replacement parity map, roadmap, and `agentd-matrix` source
  When p248 progress is inspected
  Then the Matrix bridge row mentions p248 and `MatrixPuppetTokenFileStore`
  And the Matrix bridge row remains partial
  And the row still names daemon/SDK account provisioning assembly, Matrix media, cutover, rollback, token rotation, and service packaging gaps
  And the source mentions `MatrixPuppetTokenFileStore` and `MatrixPuppetTokenSink`
