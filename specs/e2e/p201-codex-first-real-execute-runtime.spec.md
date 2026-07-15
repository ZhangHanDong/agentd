spec: task
name: "Codex-first real execute runtime selection"
tags: [agent-chat-replacement, real-execute, codex, p201]
---

## Intent

Move the agent-chat replacement path one step beyond the p200 parity baseline by
making the real `execute.dot` smoke Codex-testable without requiring Claude.
Agentd already supports launching Codex at the tmux backend layer; this slice
connects workflow role names and the real-execute smoke harness to that runtime
selection so the user can test with Codex agents only.

## Decisions

- Runtime selection is role-name based for this slice: roles prefixed `codex-`
  spawn with `CliKind::Codex`; roles prefixed `claude-` and unprefixed legacy
  roles keep `CliKind::ClaudeCode` compatibility.
- `scripts/agentd_real_execute_smoke.sh` accepts `--implementer-role ROLE` and
  `--reviewers CSV` and uses those values to create a smoke-local workflow copy.
- A smoke run whose selected roles are all `codex-*` must require the `codex`
  executable and must not require `claude` or `claude --mcp-config` during
  preflight.
- The default smoke plan remains backward-compatible with the shipped
  `workflows/execute.dot` roles unless explicit runtime options are provided.
- p201 keeps the p200 parity row `real_codex_execution` at `partial`, because a
  fully successful live Codex run is still gated by the explicit
  `AGENTD_REAL_EXECUTE_SMOKE=1 --execute` environment and real host setup.

## Boundaries

### Allowed Changes

- specs/e2e/p201-codex-first-real-execute-runtime.spec.md
- scripts/agentd_real_execute_smoke.sh
- crates/agentd-bin/tests/real_execute_smoke.rs
- crates/agentd-core/src/handler/mod.rs
- crates/agentd-core/tests/handlers_park.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md

### Forbidden

- Do not start real Claude, Codex, tmux, Matrix, or remote relay processes in
  tests.
- Do not change `scripts/agentd_real_claude_smoke.sh`.
- Do not remove Claude compatibility for existing unprefixed or `claude-*`
  workflow roles.
- Do not mutate the agent-chat checkout.

## Out of Scope

- Running `scripts/agentd_real_execute_smoke.sh --execute`.
- Implementing registry, scheduler, messaging, Matrix, or migration parity.
- Adding runtime profile storage or a full runtime scheduler.

## Completion Criteria

<!-- lint-ack: decision-coverage - runtime-role mapping and smoke CLI behavior are covered by explicit tests. -->
<!-- lint-ack: observable-decision-coverage - the smoke script scenarios bind stdout/stderr and preflight behavior. -->
<!-- lint-ack: error-path - the mixed-role preflight scenario is the failure path; the linter does not classify `exits non-zero` as an error path. -->
<!-- lint-ack: boundary-entry-point - `crates/agentctl/tests/parity_cli.rs` is a test target, not a runtime entry point; the parity-row scenario binds it through the agentctl package selector. -->
<!-- lint-ack: bdd-rule-grouping - this p201 slice is a flat runtime-selection checklist; rule grouping would not add verification coverage. -->

Scenario: codex-prefixed workflow roles spawn Codex agents
  Test:
    Package: agentd-core
    Filter: codex_prefixed_roles_spawn_codex_cli
  Level: handler behavior
  Test Double: FakeBackend
  Given a codergen node with role `codex-impl`
  When the handler parks on agent execution
  Then the recorded spawn request uses `CliKind::Codex`
  And an unprefixed `implementer` role still uses `CliKind::ClaudeCode`

Scenario: mixed fan-out reviewers preserve per-role runtime selection
  Test:
    Package: agentd-core
    Filter: fan_out_prefixed_reviewers_select_matching_cli
  Level: handler behavior
  Test Double: FakeBackend
  Given a fan_out node with reviewers `claude-sec,codex-perf,gemini-readability`
  When the handler spawns reviewers
  Then `codex-perf` uses `CliKind::Codex`
  And `claude-sec` and `gemini-readability` use `CliKind::ClaudeCode`

Scenario: Codex-only smoke preflight does not require Claude
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_codex_only_preflight_accepts_fake_codex_without_claude
  Level: script preflight
  Test Double: fake PATH tools
  Given fake local `cargo`, `tmux`, `codex`, `agent-spec`, `curl`, `git`, and `gh` executables
  And no `claude` executable is present in `PATH`
  When `agentd_real_execute_smoke.sh --preflight-only --implementer-role codex-impl --reviewers codex-sec,codex-perf,codex-readability` runs
  Then it exits `0`
  And stdout includes `preflight ok`
  And stderr does not name a missing `claude` prerequisite

Scenario: mixed-role smoke preflight still requires Claude
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_mixed_roles_preflight_requires_claude
  Level: script preflight
  Test Double: fake PATH tools
  Given fake local `cargo`, `tmux`, `codex`, `agent-spec`, `curl`, `git`, and `gh` executables
  And no `claude` executable is present in `PATH`
  When `agentd_real_execute_smoke.sh --preflight-only --implementer-role codex-impl --reviewers claude-sec,codex-perf` runs
  Then it exits non-zero
  And stderr names the missing `claude` prerequisite

Scenario: dry-run documents Codex-only runtime choices
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_dry_run_prints_codex_runtime_roles
  Level: script dry-run
  Test Double: script output
  Given explicit Codex implementer and reviewer roles
  When `agentd_real_execute_smoke.sh --dry-run` runs
  Then stdout lists the selected implementer role
  And stdout lists the selected reviewers
  And stdout names the smoke-local workflow copy used for execution

Scenario: parity row remains partial after Codex runtime selection
  Test:
    Package: agentctl
    Filter: parity_capability_map_marks_real_codex_execution_partial_after_p201
  Level: artifact inspection
  Test Double: repository Markdown file
  Given p201 adds Codex-first runtime selection for the smoke harness
  When the parity map is parsed
  Then the `real_codex_execution` row remains `partial`
  And its replacement decision mentions the Codex runtime selection progress
