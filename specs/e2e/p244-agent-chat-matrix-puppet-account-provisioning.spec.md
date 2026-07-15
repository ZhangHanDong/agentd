spec: task
name: "agent-chat Matrix puppet account provisioning plan"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p244]
---

## Intent

Move the Matrix bridge one slice closer to agent-chat replacement by adding a
local, deterministic puppet account provisioning plan. This slice converts the
p243 puppet identity directory into login/register decisions, password
candidate derivation, registration-auth choice, existing token reuse, and stale
token pruning without contacting a real Matrix homeserver.

## Decisions

- Add `MatrixPuppetProvisioningConfig` in `agentd-matrix` to model the
  agent-chat account settings `MATRIX_AGENT_PASSWORD_SECRET`,
  `MATRIX_AGENT_PASSWORD_TEMPLATE`, `MATRIX_ALLOW_LEGACY_AGENT_PASSWORD`, and
  `MATRIX_REG_TOKEN`.
- Derive the primary agent password as lowercase hex SHA-256 of
  `<password_secret>:<agent_name>`, matching agent-chat's
  `deriveAgentPassword` behavior.
- Include a legacy password candidate only when legacy fallback is explicitly
  enabled and a non-empty template is configured; preserve agent-chat's
  replacement order by expanding `{name}` before `${name}`.
- De-duplicate password candidates while preserving preference order: derived
  password first, then legacy template password.
- Add `MatrixPuppetTokenState`, `MatrixPuppetProvisioningPlan`, and
  `MatrixPuppetProvisioningAction` so a caller can distinguish existing-token
  reuse, login/register attempts, and missing-password failures per puppet.
- Resolve token names case-insensitively so existing `agentTokens` entries keep
  their stored spelling while still matching canonical p243 agent names.
- Report stale token names for entries that no longer match any planned
  non-skipped puppet account, mirroring agent-chat's cleanup of tokens for
  non-agent users and skipped service agents.
- Add a local registration-auth helper that chooses `m.login.registration_token`
  when a registration token is configured, falls back to `m.login.dummy` only
  when the server probe supports dummy auth, and otherwise returns
  `BridgeError::InvalidConfig`.
- Keep actual Matrix HTTP login/register, whoami validation, token persistence,
  display-name/avatar sync, invite polling, service packaging, cutover, and
  rollback out of this slice.

## Boundaries

### Allowed Changes

- specs/e2e/p244-agent-chat-matrix-puppet-account-provisioning.spec.md
- crates/agentd-matrix/Cargo.toml
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
- Do not perform real Matrix login/register/whoami requests.
- Do not persist Matrix passwords or access tokens.
- Do not upload or download Matrix media.
- Do not add Matrix bot command handling.
- Do not add long-running bridge service packaging, cutover, rollback, token
  rotation, invite polling, display-name sync, or avatar sync.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

- Real homeserver account creation, UIA probe HTTP execution, token storage
  backends, token rotation, bridge service installation, encrypted-room
  verification, DM/group room lifecycle, media transfer, bot commands, operator
  cutover, rollback automation, and dashboard rendering.

## Completion Criteria

<!-- lint-ack: decision-coverage - p244 binds provisioning config, sha256 derivation, legacy fallback, candidate de-duplication, token-state lookup, stale-token pruning, registration-auth choice, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify derived password strings, action kinds, token names, stale token names, auth variants, error variants, Cargo/source bindings, and repository Markdown state. -->
<!-- lint-ack: error-path - p244 includes missing-password action, no-usable-registration-flow error, stale-token pruning, and partial parity assertions. -->

Scenario: puppet provisioning config derives ordered password candidates
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_provisioning_config_derives_ordered_password_candidates
  Level: library unit
  Test Double: local secret and legacy template strings
  Given a canonical agent name, a password secret, and an enabled legacy template
  When `MatrixPuppetProvisioningConfig` derives password candidates
  Then the first candidate is lowercase hex SHA-256 of `<secret>:<agent_name>`
  And the legacy candidate preserves agent-chat's `{name}` then `${name}` replacement order
  And duplicate candidates are removed without changing preference order
  And empty secrets or disabled legacy settings add no candidate

Scenario: puppet provisioning plan reuses tokens and schedules login/register
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_provisioning_plan_reuses_tokens_and_schedules_missing_accounts
  Level: library unit
  Test Double: p243 puppet directory and in-memory token map
  Given a p243 `MatrixPuppetDirectory` with two non-skipped agents and one skipped service agent
  And token state containing one existing token with different casing plus one stale token
  When `MatrixPuppetProvisioningPlan` is built
  Then the matching token is reused without exposing the token value in the action
  And the agent without a token is scheduled for login/register with password candidates
  And the skipped service agent is not scheduled
  And the stale token name is reported for pruning

Scenario: puppet provisioning plan reports missing password instead of registering blindly
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_provisioning_plan_reports_missing_password_candidates
  Level: library unit
  Test Double: provisioning config without password settings
  Given a p243 `MatrixPuppetDirectory` with a non-skipped agent and no stored token
  And `MatrixPuppetProvisioningConfig` has no password secret and no enabled legacy template
  When `MatrixPuppetProvisioningPlan` is built
  Then the agent action is `MissingPassword`
  And the action includes the agent name and MXID for operator diagnostics
  And no login/register password is generated

Scenario: registration auth plan chooses token, dummy, or InvalidConfig deterministically
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_registration_auth_plan_selects_token_dummy_or_error
  Level: library unit
  Test Double: local UIA session id and dummy-flow boolean
  Given a registration UIA session id
  When `MatrixPuppetProvisioningConfig` has a registration token
  Then registration auth uses `m.login.registration_token`
  And when no token is configured but dummy auth is supported, auth uses `m.login.dummy`
  And when neither token nor dummy auth is available, `BridgeError::InvalidConfig` is returned

Scenario: parity docs record p244 provisioning progress without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p244_matrix_puppet_provisioning_progress
  Level: artifact inspection
  Test Double: repository Markdown files, Cargo manifest, and Rust source text
  Given the agent-chat replacement parity map, roadmap, `agentd-matrix` manifest, and source
  When p244 progress is inspected
  Then the Matrix bridge row mentions p244 and puppet account provisioning plan
  And the Matrix bridge row remains partial
  And the row still names real account registration, Matrix media, cutover, rollback, and token rotation gaps
  And the source mentions `MatrixPuppetProvisioningPlan`, `MatrixPuppetTokenState`, and `MatrixPuppetProvisioningConfig`
  And the manifest includes the SHA-256 dependency needed for agent-chat-compatible password derivation
