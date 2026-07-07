=== Contract ===

# Task Contract: real execute smoke artifact

## Intent
Create a tiny, low-risk artifact that proves the real `execute.dot` path can
take a frozen spec, have an implementer make a verified change in an allocated
worktree, and publish that change for PR creation. This is intentionally a
minimal docs-plus-test task so the workflow, not product behavior, is under
test.

## Decisions
- Add `docs/agentd-real-execute-smoke.md`.
- Add `crates/agentd-bin/tests/real_execute_smoke_artifact.rs`.
- The document contains the exact marker `AGENTD_REAL_EXECUTE_SMOKE_READY`.
- The Rust integration test uses `include_str!("../../../docs/agentd-real-execute-smoke.md")` to read the document from the repository root and check both existence and marker content.
- Do not add dependencies or modify existing runtime behavior.

## Boundaries
Allowed changes:
- docs/agentd-real-execute-smoke.md
- crates/agentd-bin/tests/real_execute_smoke_artifact.rs
Forbidden:
- Do not modify workflow files, daemon code, tmux code, store migrations, or
- Do not change existing tests.
- Do not add external dependencies.
Out of scope:
- Product behavior changes.
- Real SIGKILL recovery drills.
- Manual reviewer policy changes.

## Completion Criteria
Scenario: smoke artifact document exists
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_artifact_exists
    Level: integration test
    Test Double: filesystem include_str
  Given the implementation worktree
  When `cargo test -p agentd-bin real_execute_smoke_artifact_exists` runs
  Then `crates/agentd-bin/tests/real_execute_smoke_artifact.rs` passes by reading `docs/agentd-real-execute-smoke.md`

Scenario: smoke artifact contains the ready marker
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_artifact_mentions_ready_marker
    Level: integration test
    Test Double: filesystem include_str
  Given the implementation worktree
  When `cargo test -p agentd-bin real_execute_smoke_artifact_mentions_ready_marker` runs
  Then the test passes by finding `AGENTD_REAL_EXECUTE_SMOKE_READY`

=== Codebase Context ===

(no matching files found)

=== Task Sketch ===

Group 1 (order 1):
  Scenarios:
    - smoke artifact document exists
    - smoke artifact contains the ready marker
  Boundary paths:
    - docs/agentd-real-execute-smoke.md
    - crates/agentd-bin/tests/real_execute_smoke_artifact.rs
  Test selectors:
    - real_execute_smoke_artifact_exists
    - real_execute_smoke_artifact_mentions_ready_marker

=== Warnings ===

  - Allowed Changes path not found: docs/agentd-real-execute-smoke.md (resolved to ./docs/agentd-real-execute-smoke.md)
  - Allowed Changes path not found: crates/agentd-bin/tests/real_execute_smoke_artifact.rs (resolved to ./crates/agentd-bin/tests/real_execute_smoke_artifact.rs)
