spec: task
name: "Real execute runtime matrix"
tags: [agent-chat-replacement, real-execute, codex, p203]
---

## Intent

Move Phase B from manual role spelling toward a Codex-first real execute smoke
path. p201 made role prefixes select runtimes and p202 gave Codex launches MCP
callback parity; this slice adds a single runtime matrix knob so the smoke can
request an all-Codex implementer plus reviewer set without requiring Claude.

## Decisions

- `AGENTD_REAL_EXECUTE_RUNTIMES` is a comma-separated matrix with exactly four
  entries: implementer, security reviewer, performance reviewer, readability
  reviewer.
- The only supported runtime value is `codex`; any other value is rejected
  before preflight.
- A `codex,codex,codex,codex` matrix maps to role names
  `codex-impl`, `codex-sec`, `codex-perf`, and `codex-readability`.
- The runtime matrix conflicts with explicit `--implementer-role` or
  `--reviewers` flags; ambiguous precedence is rejected before preflight.
- Dry-run and preflight-only must apply the same matrix parsing as execute mode
  without creating daemon, Claude, Codex, or GitHub side effects.
- p203 keeps `real_codex_execution` partial until a real
  `AGENTD_REAL_EXECUTE_SMOKE=1 --execute` Codex run succeeds.

## Boundaries

### Allowed Changes

- specs/e2e/p203-real-execute-runtime-matrix.spec.md
- scripts/agentd_real_execute_smoke.sh
- crates/agentd-bin/tests/real_execute_smoke.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md

### Forbidden

- Do not start real Claude, Codex, Matrix, or remote relay processes in tests.
- Do not run `scripts/agentd_real_execute_smoke.sh --execute`.
- Do not add a non-Codex launcher path to this smoke.
- Do not change p201 role-prefix runtime selection semantics.
- Do not add a Gemini runtime path in this slice.

## Out of Scope

- Running the authorized real execute smoke.
- Implementing agent registry, scheduler, messaging, Matrix, remote relay, or
  migration parity.
- Changing workflow graph semantics beyond the smoke-local role substitution.
- Adding persistent runtime profiles.

## Completion Criteria

<!-- lint-ack: decision-coverage - Each matrix decision is bound to one or more smoke-script tests. -->
<!-- lint-ack: observable-decision-coverage - stdout, stderr, state-dir absence, and preflight tool requirements are asserted. -->
<!-- lint-ack: output-mode-coverage - dry-run stdout, preflight stdout/stderr, and repository Markdown artifacts are covered. -->
<!-- lint-ack: flag-combination-coverage - matrix plus explicit role flags is the required conflict scenario. -->
<!-- lint-ack: boundary-entry-point - test file paths are artifact-inspection entry points bound through package/filter selectors. -->
<!-- lint-ack: error-path - invalid arity/value and explicit-role conflict are failure paths. -->

Scenario: all-Codex runtime matrix dry-run prints derived roles
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_runtime_matrix_dry_run_prints_codex_roles
  Level: shell smoke artifact
  Test Double: fake state directory, no real agents
  Given `AGENTD_REAL_EXECUTE_RUNTIMES=codex,codex,codex,codex`
  When the execute smoke script runs in dry-run mode
  Then stdout includes `runtime_matrix: codex,codex,codex,codex`
  And stdout includes `implementer_role: codex-impl`
  And stdout includes `reviewers: codex-sec,codex-perf,codex-readability`
  And the dry-run state directory is not created

Scenario: all-Codex runtime matrix preflight does not require Claude
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_runtime_matrix_codex_only_preflight_succeeds
  Level: shell smoke preflight
  Test Double: fake tools with Codex present and Claude absent
  Given `AGENTD_REAL_EXECUTE_RUNTIMES=codex,codex,codex,codex`
  When the execute smoke script runs in preflight-only mode
  Then preflight succeeds with fake Codex prerequisites
  And stderr does not mention a missing Claude prerequisite
  And the daemon log is not created

Scenario: non-Codex runtime matrix values are rejected
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_runtime_matrix_rejects_non_codex_runtime
  Level: shell smoke validation
  Test Double: no real agents
  Given `AGENTD_REAL_EXECUTE_RUNTIMES=codex,claude,codex,codex`
  When the execute smoke script validates options
  Then validation fails before starting the daemon
  And stderr states that only `codex` is supported

Scenario: invalid runtime matrix shape is rejected before preflight
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_runtime_matrix_rejects_wrong_arity_or_unknown_runtime
  Level: shell smoke validation
  Test Double: no real agents
  Given a matrix with too few entries or an unsupported `gemini` value
  When the execute smoke script validates options
  Then it exits non-zero before creating the state directory
  And stderr names `AGENTD_REAL_EXECUTE_RUNTIMES`

Scenario: runtime matrix conflicts with explicit role flags
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_runtime_matrix_conflicts_with_explicit_roles
  Level: shell smoke validation
  Test Double: no real agents
  Given a runtime matrix and an explicit `--implementer-role`
  When the execute smoke script validates options
  Then it exits non-zero
  And stderr explains the matrix cannot be combined with explicit roles

Scenario: parity row records p203 while remaining partial
  Test:
    Package: agentctl
    Filter: parity_capability_map_marks_real_codex_execution_partial_after_p203
  Level: artifact inspection
  Test Double: repository Markdown file
  Given p203 adds the real execute runtime matrix
  When the parity map is parsed
  Then the `real_codex_execution` row remains `partial`
  And its replacement decision mentions p203 runtime matrix progress
