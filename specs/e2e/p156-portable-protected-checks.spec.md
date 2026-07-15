spec: task
name: "portable protected checks"
tags: [ci, portability, parity, agent-spec, p156]
---

## Intent

Make the protected PR checks reproducible on clean Linux and macOS runners.
Tests and governance gates must own their fixtures and tools, while design-only
contracts remain validated without being misreported as implemented.

## Decisions

- Store agent-chat capability sources as safe paths relative to the supplied
  agent-chat root; retain compatible absolute paths only when they remain under
  that root, and reject empty paths or parent traversal.
- Run parity audit tests against a temporary agent-chat fixture instead of a
  developer workstation checkout.
- Give the prepare-only real-execute smoke test a fake `agent-spec` plan tool in
  its isolated `PATH`; no provider runtime is started. Strip `template-only`
  from the rendered run contract so it returns to lifecycle verification.
- Use one shared spec verifier in local and GitHub gates. An explicit
  `design-only` or `template-only` tag runs parse and lint; every other
  contract runs lifecycle.
- Require `agent-spec 1.0.0` in local and GitHub gates. Missing or mismatched
  tooling is a gate failure, not a skipped check.
- Keep implemented P100, P120, and P130 contracts on lifecycle by updating
  their stale selectors to current equivalent integration tests.
- Verify the complete spec set recursively, then require one changed
  implementation contract to accept every staged, HEAD-commit, or PR-range
  change through explicit `--change` arguments. Deletions and type changes
  remain visible; renames contribute both their old and new paths.
- Bootstrap the existing reconciliation PR at the commit that introduces P156.
  Once the base contains P156, GitHub checks audit the complete base-to-head
  range; main pushes audit the event's before-to-head range.
- Run Matrix service smoke tests with the macOS system Bash so empty repeatable
  option arrays remain portable under Bash 3.2 nounset semantics.

## Boundaries

### Allowed Changes

- specs/e2e/p156-portable-protected-checks.spec.md
- docs/superpowers/specs/2026-07-15-portable-protected-checks-design.md
- docs/superpowers/plans/2026-07-15-portable-protected-checks.md
- specs/e2e/ad-e1-minimum-security-baseline.spec.md
- specs/e2e/p100-worktree-pr-publication.spec.md
- specs/e2e/p120-agent-mcp-stdio-startup-context.spec.md
- specs/e2e/p130-open-pr-preflight.spec.md
- specs/e2e/p272-runtime-compatibility-port.spec.md
- specs/e2e/real-execute-smoke-template.spec.md
- docs/parity/agent-chat-capability-map.md
- crates/agentctl/src/parity.rs
- crates/agentctl/tests/parity_cli.rs
- crates/agentd-bin/tests/contract.rs
- crates/agentd-bin/tests/matrix_service_smoke.rs
- crates/agentd-bin/tests/real_execute_smoke.rs
- crates/agentd-core/tests/ci_clippy.rs
- crates/agentd-core/tests/spec_guard.rs
- scripts/agentd_guard_changed_contract.sh
- scripts/agentd_matrix_client_bridge_service_smoke.sh
- scripts/agentd_verify_spec.sh
- scripts/agentd_real_execute_smoke.sh
- scripts/check.sh
- .github/workflows/ci.yml

### Forbidden

- Do not skip lifecycle for an implementation contract.
- Do not treat a design-only contract as implemented or passing lifecycle.
- Do not run an unrendered smoke template as an implementation contract.
- Do not weaken agent-spec lint, lifecycle, guard, Clippy, or dependency gates.
- Do not silently skip a missing or mismatched agent-spec installation.
- Do not depend on a developer-specific absolute checkout path.
- Do not invoke Claude or another real provider in tests.

## Out of Scope

- Implementing AD-E1 or recording FSF-0 acceptance.
- Changing parity capability status or replacement decisions.
- Dereferencing parity source citations or enforcing filesystem-level symlink
  containment; parity audit treats source values as read-only map references.
- Merging the pull request.

## Completion Criteria

Rule: portable-gates  clean runners enforce honest contract state

Scenario: parity audit is portable and rejects unsafe sources
  Test:
    Package: agentctl
    Filter: parity_audit_
  Level: CLI integration
  Test Double: temporary agent-chat repository and parity map
  Given capability sources relative to an arbitrary agent-chat checkout
  When parity audit runs on a clean runner
  Then required gaps return the stable gap exit code without mutating the source
  And empty or parent-traversal sources return the invalid-input exit code
  And `crates/agentctl/tests/parity_cli.rs` owns the temporary source fixture

Scenario: prepare-only smoke owns its planning tool
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_prepare_only_renders_isolated_contract
  Level: shell integration
  Test Double: temporary state and fake agent-spec executable
  Given no host agent-spec installation and no provider runtime
  When prepare-only renders the execution contract
  Then the frozen spec plan and workflow are created under temporary state
  And the frozen spec no longer contains the template-only tag
  And Claude Codex tmux Matrix and remote services are not started
  And `crates/agentd-bin/tests/real_execute_smoke.rs` owns the fake tool path

Scenario: design-only and implementation contracts use distinct gates
  Test:
    Package: agentd-core
    Filter: ci_and_local_gates_classify_non_implementation_specs
  Level: repository artifact inspection
  Test Double: workflow script and contract text
  Given AD-E1 and P272 explicitly declare design-only status
  And the unrendered real-execute contract declares template-only status
  When local and GitHub spec gates inspect all contracts
  Then design and template contracts run parse and lint through the shared verifier
  And untagged implementation contracts run lifecycle
  And P100 P120 and P130 use current equivalent lifecycle selectors
  And local and GitHub gates require agent-spec 1.0.0
  And both gates invoke the changed-contract boundary guard

Scenario: changed implementation contract governs the complete delta
  Test:
    Package: agentd-core
    Filter: changed_contract_guard_passes_changes_and_propagates_failure
  Level: shell integration
  Test Double: temporary git repository and fake agent-spec executable
  Given a staged implementation contract with modified deleted and renamed task files
  And a base revision before the P156 adoption contract
  When the changed-contract guard evaluates the staged and base-to-head deltas
  Then every changed path is passed to lifecycle through repeated --change arguments
  And a rename contributes both its old and new paths
  And range verification includes follow-up commits after P156 adoption
  And lifecycle success passes the guard
  And lifecycle failure fails the guard

Scenario: Matrix service smoke is portable to macOS system Bash
  Test:
    Package: agentd-bin
    Filter: matrix_service_smoke_execute_invokes_preflight_then_service_and_writes_evidence
  Level: shell integration
  Test Double: fake agentd service executable and temporary state
  Given macOS runs the smoke script with its system Bash 3.2 and nounset enabled
  And optional repeatable Matrix arguments are empty
  When the execute smoke builds the shared service arguments
  Then empty arrays are not expanded under nounset
  And preflight runs before service execution
  And the smoke writes its expected evidence
