spec: task
name: "real execute smoke artifact __AGENTD_REAL_EXECUTE_RUN_ID__"
tags: [smoke, execute, real-agent, run-unique, template-only]
---

## Intent

Create a tiny, run-specific artifact that proves the current real
`execute.dot` run produced and verified a new task delta before publication.
The task is intentionally limited to one document and one Rust integration
test so the workflow, rather than product behavior, remains under test.

## Decisions

- Add `__AGENTD_REAL_EXECUTE_DOC_PATH__`.
- Add `__AGENTD_REAL_EXECUTE_TEST_PATH__`.
- The document contains the exact marker `__AGENTD_REAL_EXECUTE_MARKER__`.
- The Rust integration test uses
  `include_str!("../../../docs/real-execute-smoke/__AGENTD_REAL_EXECUTE_RUN_ID__.md")`
  to read the document from the repository root.
- The Rust tests are named `__AGENTD_REAL_EXECUTE_EXISTS_FILTER__` and
  `__AGENTD_REAL_EXECUTE_MARKER_FILTER__`.
- Do not add dependencies or modify existing runtime behavior.

## Boundaries

### Allowed Changes

- __AGENTD_REAL_EXECUTE_DOC_PATH__
- __AGENTD_REAL_EXECUTE_TEST_PATH__

### Forbidden

- Do not modify workflow files, daemon code, tmux code, store migrations, or
  GitHub publishing scripts.
- Do not change existing tests.
- Do not add external dependencies.

## Out of Scope

- Product behavior changes.
- Real SIGKILL recovery drills.
- Manual reviewer policy changes.

## Completion Criteria

Scenario: run-specific smoke artifact document exists
  Test:
    Package: agentd-bin
    Filter: __AGENTD_REAL_EXECUTE_EXISTS_FILTER__
  Level: integration test
  Test Double: filesystem include_str
  Given the implementation worktree for run "__AGENTD_REAL_EXECUTE_RUN_ID__"
  When `cargo test -p agentd-bin __AGENTD_REAL_EXECUTE_EXISTS_FILTER__` runs
  Then `__AGENTD_REAL_EXECUTE_TEST_PATH__` passes by reading `__AGENTD_REAL_EXECUTE_DOC_PATH__`

Scenario: run-specific smoke artifact contains the ready marker
  Test:
    Package: agentd-bin
    Filter: __AGENTD_REAL_EXECUTE_MARKER_FILTER__
  Level: integration test
  Test Double: filesystem include_str
  Given the implementation worktree for run "__AGENTD_REAL_EXECUTE_RUN_ID__"
  When `cargo test -p agentd-bin __AGENTD_REAL_EXECUTE_MARKER_FILTER__` runs
  Then the test passes by finding `__AGENTD_REAL_EXECUTE_MARKER__`
