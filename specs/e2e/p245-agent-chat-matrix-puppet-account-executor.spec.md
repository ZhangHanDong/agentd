spec: task
name: "agent-chat Matrix puppet account executor boundary"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p245]
---

## Intent

Move the Matrix bridge one slice closer to agent-chat replacement by executing
the p244 local puppet account provisioning decisions against fakeable local
ports. This slice covers the account provisioning order that agent-chat uses:
validate an existing token with whoami, otherwise try password login candidates,
register with the preferred password after login candidates fail, persist new
tokens, prune stale token entries, and continue remaining agents after an
individual account fails.

## Decisions

- Add an explicit `MatrixPuppetAccountPort` in `agentd-matrix` for Matrix
  account operations needed by puppet provisioning: `whoami`, `login`, and
  `register`.
- Add an explicit `MatrixPuppetTokenSink` for durable `agentTokens`-style
  updates without binding this slice to a concrete state file or database.
- Add `MatrixPuppetAccountExecutor` to execute `MatrixPuppetDirectory`,
  `MatrixPuppetProvisioningConfig`, and `MatrixPuppetTokenState` together.
- Preserve agent-chat's account order: validate a stored token first, fall back
  to password login candidates when the token is missing or invalid, then
  register with the first candidate only after all login candidates fail.
- Save a new token after successful login or registration under the canonical
  p243 agent name.
- Delete stale stored token names after planned non-skipped puppet accounts are
  processed.
- Report per-agent outcomes for reused, logged-in, registered,
  missing-password, and failed accounts so one bad puppet does not abort
  provisioning for the remaining agents.
- Keep real Matrix HTTP requests, Matrix SDK account registration wiring, UIA
  probe execution, display-name/avatar sync, token rotation, service packaging,
  cutover, and rollback out of this slice.

## Boundaries

### Allowed Changes

- specs/e2e/p245-agent-chat-matrix-puppet-account-executor.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/puppet_accounts.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not connect to a real Matrix homeserver in tests.
- Do not perform real Matrix login/register/whoami requests.
- Do not persist Matrix passwords or access tokens outside local test doubles.
- Do not upload or download Matrix media.
- Do not add Matrix bot command handling.
- Do not add long-running bridge service packaging, cutover, rollback, token
  rotation, invite polling, display-name sync, or avatar sync.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

Real homeserver account creation, UIA probe HTTP execution, registration auth
wire-format requests, SDK-backed account provisioning, token storage backends,
token rotation, bridge service installation, encrypted-room verification,
DM/group room lifecycle, media transfer, bot commands, operator cutover,
rollback automation, dashboard rendering, and Matrix profile/avatar updates.

## Completion Criteria

<!-- lint-ack: decision-coverage - p245 binds token validation, login fallback, registration fallback, token persistence, stale-token pruning, per-agent failure reporting, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify account-port calls, token-sink calls, outcome variants, password candidate ordering, failure continuation, and repository Markdown state. -->
<!-- lint-ack: error-path - p245 covers invalid existing tokens, missing passwords, login failures, register failures, save failures, stale-token pruning, and partial parity assertions. -->

Scenario: account executor reuses valid tokens and prunes stale token entries
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_account_executor_reuses_valid_tokens_and_prunes_stale_entries
  Level: library unit
  Test Double: fake Matrix account port and in-memory token sink
  Given a p243 `MatrixPuppetDirectory` with one known non-skipped agent and one skipped service agent
  And token state containing a case-insensitive matching token plus one stale token
  When `MatrixPuppetAccountExecutor` provisions the directory
  Then it validates the matching token through `whoami`
  And it reports a reused-token outcome without calling login or register
  And it deletes the stale token name through the token sink

Scenario: account executor logs in with password candidates and persists the token
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_account_executor_logs_in_then_persists_agent_token
  Level: library unit
  Test Double: fake Matrix account port and in-memory token sink
  Given a p243 `MatrixPuppetDirectory` with one known agent and no stored token
  And `MatrixPuppetProvisioningConfig` derives a password candidate
  When the fake account port accepts the first login candidate
  Then the executor reports a logged-in outcome
  And the access token is saved under the canonical p243 agent name
  And no registration call is made

Scenario: account executor reports missing password without Matrix account calls
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_account_executor_reports_missing_password_without_registering
  Level: library unit
  Test Double: fake Matrix account port and in-memory token sink
  Given a p243 `MatrixPuppetDirectory` with one known agent and no stored token
  And `MatrixPuppetProvisioningConfig` has no password secret and no enabled legacy template
  When `MatrixPuppetAccountExecutor` provisions the directory
  Then the executor reports a missing-password outcome
  And it does not call whoami, login, register, or the token sink for that agent

Scenario: account executor registers with the preferred password after login candidates fail
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_account_executor_registers_after_login_candidates_fail
  Level: library unit
  Test Double: fake Matrix account port and in-memory token sink
  Given a p243 `MatrixPuppetDirectory` with one known agent and no stored token
  And provisioning config derives a primary password and an enabled legacy fallback password
  When all login candidates fail and registration succeeds
  Then login is attempted in candidate order
  And register is called once with the first password candidate
  And the registration token is saved under the canonical p243 agent name

Scenario: account executor reports failures without stopping other agents
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_account_executor_reports_failures_without_stopping_other_agents
  Level: library unit
  Test Double: fake Matrix account port and in-memory token sink
  Given two known p243 agents
  And the first agent cannot be registered after login failures
  When the second agent can log in successfully
  Then the report includes a failed outcome for the first agent
  And the report still includes a logged-in outcome for the second agent
  And the second agent's token is persisted

Scenario: parity docs record p245 executor progress without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p245_matrix_puppet_account_executor_progress
  Level: artifact inspection
  Test Double: repository Markdown files and Rust source text
  Given the agent-chat replacement parity map, roadmap, and `agentd-matrix` source
  When p245 progress is inspected
  Then the Matrix bridge row mentions p245 and the puppet account executor boundary
  And the Matrix bridge row remains partial
  And the row still names real Matrix HTTP account registration, Matrix media, cutover, rollback, and token rotation gaps
  And the source mentions `MatrixPuppetAccountExecutor`, `MatrixPuppetAccountPort`, and `MatrixPuppetTokenSink`
