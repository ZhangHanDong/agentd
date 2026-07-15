spec: task
name: "Runhost tool cwd stability"
tags: [agent-chat-replacement, real-execute, runhost, p210]
---

## Intent

The p204 r6 Codex-only real execute smoke reached implementation, lifecycle
verification, and three passing Codex reviewer verdicts, then failed at
`publish_branch`. Manual replay of `scripts/agentd_publish_worktree.sh` from the
repository root succeeded. The failure happens because the final reviewer MCP
process can advance the workflow from a transient reviewer worktree cwd; that
review worktree is released before subsequent tool nodes run, so relative tool
commands such as `bash scripts/agentd_publish_worktree.sh ...` can resolve from
an invalid or deleted directory instead of the daemon repository root.

## Decisions

- Production runhost tool commands with no explicit cwd run from the configured
  repository root, not from the current cwd of whichever MCP process delivered
  the event.
- `build_production_host` wires the configured `repo_dir` as the stable tool cwd
  for daemon and `mcp-stdio` execution.
- The core `tool` handler remains context-driven and does not learn about
  daemon configuration; cwd stabilization is a production-host composition
  responsibility.
- Existing argv substitution for `${worktree}` and `${task_run_id}` is
  unchanged.

## Boundaries

### Allowed Changes

- specs/e2e/p210-runhost-tool-cwd-stability.spec.md
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/src/daemon.rs
- crates/agentd-bin/tests/contract.rs
- docs/plans/p204-real-codex-execute-smoke-gate.spec.md

### Forbidden

- Do not run real Claude in tests.
- Do not change workflow DOT command strings in this slice.
- Do not change the publish/open-pr helper scripts.
- Do not move cwd policy into `agentd-core`.

## Out of Scope

- Retrying the real smoke gate.
- Improving stderr/stdout artifact capture for failed tool commands.
- Changing reviewer worktree release timing.
- Opening or updating GitHub PRs.

## Completion Criteria

Scenario: production tool commands use stable repo cwd after review fan-in
  Test:
    Package: agentd-bin
    Filter: production_runhost_execute_tools_use_stable_repo_cwd_after_review_fan_in
  Level: production host integration
  Test Double: real SqliteStore, fake backend, recording command runner
  Given an `execute.dot` run that parks at implement and then review
  When the third reviewer submits a passing verdict through the production host
  Then `publish_branch` is invoked with `cwd` equal to the repository root
  And `open_pr` is invoked with `cwd` equal to the repository root
  And both commands still receive the expected argv with `${worktree}` and
      `${task_run_id}` substituted
