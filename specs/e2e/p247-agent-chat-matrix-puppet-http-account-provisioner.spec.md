spec: task
name: "agent-chat Matrix puppet HTTP account provisioner assembly"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p247]
---

## Intent

Move the Matrix bridge one slice closer to agent-chat replacement by assembling
the p245 puppet account executor with the p246 Matrix Client-Server HTTP account
port behind one reusable library entry point. This slice proves the account
provisioning state machine can run end-to-end against local fake homeservers and
a fake token sink without requiring daemon service wiring, a real homeserver, or
durable token-store backends.

## Decisions

- Add `MatrixPuppetHttpAccountProvisioner` in `agentd-matrix`.
- The provisioner must construct `MatrixPuppetHttpAccountPort` from
  `MatrixPuppetHttpAccountConfig` and execute `MatrixPuppetAccountExecutor`
  against `MatrixPuppetDirectory`, `MatrixPuppetProvisioningConfig`, and
  `MatrixPuppetTokenState`.
- The provisioner must pass the configured Matrix registration token from the
  HTTP account config into the account port, so registration-token UIA can run
  through the assembled path.
- The provisioner must preserve p245 executor behavior: validate reusable tokens
  with whoami, fall back to login/register, save successful session tokens,
  prune stale token names, report per-agent failures, and continue remaining
  agents.
- Tests must use only local fake HTTP homeservers and in-memory token sinks.
- Keep daemon configuration, SDK account wiring, durable token-store backends,
  token rotation, display-name/avatar sync, service packaging, cutover, and
  rollback out of this slice.

## Boundaries

### Allowed Changes

- specs/e2e/p247-agent-chat-matrix-puppet-http-account-provisioner.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/puppet_http_account_provisioner.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not connect to a real Matrix homeserver in tests.
- Do not persist Matrix passwords or access tokens outside local test doubles.
- Do not add a new HTTP client dependency.
- Do not upload or download Matrix media.
- Do not add Matrix bot command handling.
- Do not add long-running bridge service packaging, cutover, rollback, token
  rotation, invite polling, display-name sync, or avatar sync.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

Daemon CLI/env configuration for account provisioning, SDK-backed account
provisioning, durable token storage backends, token rotation, bridge service
installation, encrypted-room verification, DM/group room lifecycle, media
transfer, bot commands, operator cutover, rollback automation, dashboard
rendering, and Matrix profile/avatar updates.

## Completion Criteria

<!-- lint-ack: decision-coverage - p247 binds HTTP account port assembly, executor reuse, registration-token UIA, stale-token pruning, per-agent continuation, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify fake HTTP method/path/header/body, token-sink saves/deletes, provisioning outcomes, and repository Markdown/source state. -->
<!-- lint-ack: error-path - p247 covers stale token pruning, login-to-registration fallback, per-agent HTTP failure continuation, and partial parity assertions. -->

Scenario: HTTP account provisioner reuses valid tokens and prunes stale token names
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_http_account_provisioner_reuses_valid_tokens_and_prunes_stale
  Level: integration unit
  Test Double: local fake HTTP homeserver and in-memory token sink
  Given a p243 `MatrixPuppetDirectory` with one known non-skipped agent
  And token state containing one matching token and one stale token name
  When `MatrixPuppetHttpAccountProvisioner` provisions the directory
  Then the fake homeserver receives `GET /_matrix/client/v3/account/whoami`
  And the request includes `Authorization: Bearer existing-token`
  And the report includes a reused-token outcome
  And the fake token sink deletes the stale token name

Scenario: HTTP account provisioner logs in and registers through the assembled path
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_http_account_provisioner_logs_in_and_registers_via_http
  Level: integration unit
  Test Double: local fake HTTP homeserver and in-memory token sink
  Given two known p243 agents with no stored tokens
  And provisioning config derives password candidates
  When one fake homeserver login succeeds
  And another fake homeserver login fails before registration-token UIA succeeds
  Then the report includes logged-in and registered outcomes
  And both returned access tokens are saved under canonical p243 agent names
  And registration completion uses `m.login.registration_token`

Scenario: HTTP account provisioner reports one account failure without stopping the rest
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_http_account_provisioner_reports_http_errors_without_stopping
  Level: integration unit
  Test Double: local fake HTTP homeserver and in-memory token sink
  Given two known p243 agents with no stored tokens
  And the first account receives an unusable registration probe response
  When the second account can log in successfully
  Then the report includes a failed outcome for the first account
  And the report still includes a logged-in outcome for the second account
  And only the second account token is saved

Scenario: parity docs record p247 HTTP account provisioner assembly without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p247_matrix_puppet_http_account_provisioner_progress
  Level: artifact inspection
  Test Double: repository Markdown files and Rust source text
  Given the agent-chat replacement parity map, roadmap, and `agentd-matrix` source
  When p247 progress is inspected
  Then the Matrix bridge row mentions p247 and `MatrixPuppetHttpAccountProvisioner`
  And the Matrix bridge row remains partial
  And the row still names durable token-store backends, Matrix media, cutover, rollback, token rotation, and service packaging gaps
  And the source mentions `MatrixPuppetHttpAccountProvisioner` and `MatrixPuppetHttpAccountPort`
