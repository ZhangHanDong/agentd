spec: task
name: "CI clippy clean"
tags: [ci, clippy, p0.9, github, cleanup]
---

## Intent

Make the open PR pass the GitHub Actions lint job that runs
`cargo clippy --workspace --all-targets -- -D warnings`. The PR already opens
and is mergeable, but CI fails before test jobs because newer stable clippy
denies warning patterns in `agentd-core`; this slice removes those warning
patterns without changing workflow behavior. After fixing the first
`agentd-core` batch, local workspace clippy revealed additional denied warning
batches in `agentd-bin`, `agentd-tmux`, `agentd-surface`, `agentd-store`, and
`agentd-core` tests; this slice covers the full observed lint batch needed for
the same CI command to pass.

## Decisions

- Fix the current clippy failures directly instead of relaxing CI flags.
- Replace `map(...).unwrap_or_else(...)` patterns with `map_or_else`.
- Keep normalized text diff behavior unchanged while avoiding direct
  `usize as f64` casts.
- Replace the 9-argument `review_prompt` function signature with a small input
  struct rather than suppressing `too_many_arguments`.
- Backtick `task_run` in doc comments flagged by `doc_markdown`.
- Remove strict clippy warning patterns from tmux worktree pool code without
  changing allocation, snapshot, or launch behavior.
- Keep HTTP/store tests behaviorally identical while refactoring clippy-only
  test-shape warnings.
- Add a source-inspection regression test for these exact clippy patterns.

## Boundaries

### Allowed Changes

- specs/e2e/p135-ci-clippy-clean.spec.md
- crates/agentd-core/src/handler/codergen.rs
- crates/agentd-core/src/handler/fan_in.rs
- crates/agentd-core/src/handler/fan_out.rs
- crates/agentd-core/src/ports/worktree_allocator.rs
- crates/agentd-core/src/test_support/in_memory_store.rs
- crates/agentd-core/tests/ci_clippy.rs
- crates/agentd-core/tests/handlers_park.rs
- crates/agentd-bin/src/cli.rs
- crates/agentd-bin/src/main.rs
- crates/agentd-bin/src/stdio_mcp.rs
- crates/agentd-bin/tests/contract.rs
- crates/agentd-store/tests/store_trait.rs
- crates/agentd-surface/tests/http.rs
- crates/agentd-tmux/src/backend.rs
- crates/agentd-tmux/src/pool.rs
- crates/agentd-tmux/tests/pool.rs

### Forbidden

- Do not relax CI, clippy, rustfmt, or cargo-deny settings.
- Do not add broad `allow(clippy::...)` suppressions for these warnings.
- Do not change workflow DOT files or PR publication scripts in this slice.
- Do not modify `.agentd/*` evidence.

## Out of Scope

- Reworking Delphi convergence semantics.
- Changing reviewer prompt content.
- Fixing unrelated future clippy warnings not present in the observed local
  workspace clippy run for this PR branch.
- Changing GitHub Actions workflow topology.

## Completion Criteria

<!-- lint-ack: decision-coverage — the source-inspection scenario checks that review_prompt no longer has the 9-argument signature. -->
<!-- lint-ack: observable-decision-coverage — this slice is clippy-shape cleanup only; HTTP/store behavior remains covered by the unchanged existing test assertions and the workspace all-targets clippy compile. -->
<!-- lint-ack: output-mode-coverage — output is mentioned only as CI/source command output; no runtime file/stdout output mode changes in this slice. -->
<!-- lint-ack: boundary-entry-point — allowed Rust entry points are covered by `cargo clippy --workspace --all-targets -- -D warnings`; this slice does not change runtime entry behavior. -->

Scenario: known clippy regression markers are absent
  Test:
    Package: agentd-core
    Filter: ci_clippy_known_warning_patterns_are_absent
  Level: source inspection
  Test Double: repository source text
  Given the files named in the failing GitHub Actions clippy log
  When the regression test scans for the known denied patterns
  Then it finds no `map(...).unwrap_or_else(...)` current-dir or worktree fallback snippets
  And it finds no direct `usize as f64` normalized diff snippet
  And it finds no unbackticked `task_run` doc-comment snippets
  And it finds no observed tmux pool, surface-test, or handler-test clippy marker snippets

Scenario: workspace clippy command passes
  Test:
    Package: agentd-core
    Filter: ci_clippy_known_warning_patterns_are_absent
  Level: CI command
  Test Double: local cargo clippy verification
  Given the PR branch after the source cleanup
  When `cargo clippy --workspace --all-targets -- -D warnings` runs
  Then it exits 0 without clippy errors
