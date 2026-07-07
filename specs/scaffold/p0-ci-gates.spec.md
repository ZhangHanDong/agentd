spec: task
name: "P0 CI seven-layer gate"
tags: [ci, mvp, p0]
---

## Intent

Establish the CI quality gate matching design §6.4. A PR must not be mergeable
unless lint, unit, spec-lifecycle, and cross-deps-sanity all pass. The local
`scripts/check.sh` mirrors CI so contributors get the same result before
pushing. dot-validate and docs jobs are deferred until their dependencies land.

## Decisions

- GitHub Actions workflow at .github/workflows/ci.yml
- Jobs: lint, unit (matrix ubuntu+macos), spec-lifecycle, cross-deps-sanity
- cargo-nextest runs tests with --no-tests=warn so empty test sets do not fail early phases
- cargo-deny gates licenses and advisories
- Boundary checks scan only crates/*/src/** so test fixtures are not flagged

## Boundaries

### Allowed Changes

- .github/workflows/ci.yml
- deny.toml
- scripts/check.sh

### Forbidden

- Do not allow clippy warnings; clippy runs with -D warnings
- Do not allow palace.db references under crates/*/src/**
- Do not allow the literal send-keys -l under crates/*/src/**

## Completion Criteria

Scenario: Local check script mirrors CI and succeeds on a clean tree
  Test: scaffold_local_check_script_runs
  Given a clean working tree with the scaffold in place
  When scripts/check.sh runs
  Then it exits zero
  And it prints a ready-for-PR line as its final output

Scenario: A palace.db reference under crate source fails the boundary check
  Test: scaffold_palace_db_reference_fails_gate
  Given a fake crate tree whose src file contains the literal palace.db
  When the boundary source scan is applied to that tree
  Then the scan reports a match

Scenario: A send-keys literal under crate source fails the boundary check
  Test: scaffold_send_keys_dash_l_fails_gate
  Given a fake crate tree whose src file contains the forbidden send-keys literal
  When the boundary source scan is applied to that tree
  Then the scan reports a match

Scenario: The boundary check ignores forbidden strings under tests directories
  Test: scaffold_gate_does_not_flag_tests_directory
  Given a fake crate tree whose tests file contains the literal palace.db
  When the boundary source scan is applied to that tree
  Then the scan reports no match
