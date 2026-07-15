spec: task
name: "p204 real Codex execute smoke gate"
tags: [agent-chat-replacement, real-execute, codex, p204, manual-gate]
---

## Intent

Run the Phase B real execute gate with Codex-only agents after p201-p203 made
Codex role selection, Codex MCP launch config, and runtime matrix selection
available. This is a live operator gate rather than a CI spec: it consumes
authenticated local Codex and GitHub state, writes evidence under `.agentd/`,
and determines whether the `real_codex_execution` parity row can advance.

## Decisions

- Use `AGENTD_REAL_EXECUTE_RUNTIMES=codex,codex,codex,codex`.
- Use fixed, suffixed run ids such as `p204-codex-matrix-r6` and matching state
  directories under `.agentd/real-execute-smoke/` so each failed attempt remains
  auditable without rewriting prior evidence.
- Use the default frozen smoke spec `.agentd/run/frozen.spec.md`; it is the
  low-risk docs-plus-test artifact task.
- Do not use real Claude for this gate.
- A passing gate requires the run to reach `finished`, not merely park or fail
  after manual intervention.
- If the gate fails, preserve the evidence directory and treat the failure as
  the next p204 implementation/debugging input rather than weakening the gate.

## Live Evidence

- `p204-codex-matrix` failed before durable structured failure handling.
- `p204-codex-matrix-r2` reached a structured failed terminal state for Codex
  readiness timeout; p205 preserved launch-failure evidence.
- `p204-codex-matrix-r3` reached Codex implementation but stopped at edit
  confirmation; p207 added unattended Codex launch flags.
- `p204-codex-matrix-r4` reached agentd MCP submission but exposed Codex MCP
  tool approval and raw stdio readonly-DB behavior; p208 added per-launch
  agentd MCP approval overrides.
- `p204-codex-matrix-r5` reached implementation and three reviewers but failed
  because launcher artifacts polluted tracked `.gitignore`; p209 moved launcher
  artifact ignore patterns to git `info/exclude`.
- `p204-codex-matrix-r6` reached implementation, lifecycle verification, three
  passing Codex reviewer verdicts, and then failed at `publish_branch`. Manual
  replay of `scripts/agentd_publish_worktree.sh` from the repository root
  succeeded, showing the helper and remote push path are functional. The root
  cause is that event-driven `mcp-stdio` continuations can inherit a transient
  reviewer worktree cwd that is released before subsequent tool nodes run; p210
  fixes this by forcing production tool commands to run from the configured
  repository root.
- `p204-codex-matrix-r7` reached implementation, lifecycle verification, three
  passing Codex reviewer verdicts, and then hung in `publish_branch` while `git
  push` prompted for HTTPS credentials inside the reviewer Codex `mcp-stdio`
  process. This showed the deeper boundary issue: agent-facing stdio was
  continuing the workflow locally instead of submitting tool calls to the
  central daemon process. p211 makes spawned `mcp-stdio` commands proxy
  `tools/call` to the central daemon through HTTP `/tools/call`, while
  preserving local stdio mode for offline tests.
- `p204-codex-matrix-r8` proved the p211 proxy command is injected, but the
  implementer Codex sandbox blocked the stdio helper's loopback connection to
  `127.0.0.1:18789` with `Operation not permitted`; direct local stdio fallback
  again hit readonly SQLite. p212 updates managed Codex launchers to use
  `--ask-for-approval never --sandbox danger-full-access` while still forbidding
  `--dangerously-bypass-approvals-and-sandbox`.
- `p204-codex-matrix-r9` finished successfully. Evidence in
  `.agentd/real-execute-smoke/p204-codex-matrix-r9/summary.txt` records
  `result: finished`, and `run_snapshot.json` records `status: finished` at
  `done` after `implement`, `verify_lifecycle`, `review`, `aggregate`,
  `publish_branch`, `open_pr`, and `report_acceptance`. The three Codex
  reviewer verdicts (`codex-sec`, `codex-perf`, `codex-readability`) were all
  `pass`; the `real_codex_execution` parity row can advance to `covered`.

## Boundaries

### Allowed Changes

- docs/plans/p204-real-codex-execute-smoke-gate.spec.md
- .agentd/real-execute-smoke/p204-codex-matrix/**
- .agentd/real-execute-smoke/p204-codex-matrix-r*/**
- .agentd/run/frozen.spec.md
- .agentd/run/plan.md
- .agentd/run/report.md
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md
- crates/agentctl/tests/parity_cli.rs
- scripts/agentd_real_execute_smoke.sh
- crates/agentd-bin/tests/real_execute_smoke.rs
- specs/e2e/p205-structured-agent-launch-failure.spec.md
- specs/e2e/p206-codex-ready-prompt.spec.md
- specs/e2e/p207-codex-unattended-launcher.spec.md
- specs/e2e/p208-codex-agentd-mcp-tool-approval.spec.md
- specs/e2e/p209-launcher-artifacts-use-git-exclude.spec.md
- specs/e2e/p210-runhost-tool-cwd-stability.spec.md
- specs/e2e/p211-mcp-stdio-proxies-central-daemon.spec.md
- specs/e2e/p212-codex-launcher-sandbox-for-local-daemon-proxy.spec.md

### Forbidden

- Do not run real Claude.
- Do not change the frozen smoke task to make the gate easier.
- Do not delete or rewrite prior smoke evidence directories.
- Do not mark `real_codex_execution` covered unless the Codex-only run reaches
  `finished` with evidence in the p204 state directory.

## Out of Scope

- Agent registry, scheduler, messaging, Matrix, remote relay, and migration
  parity work.
- Replacing the frozen smoke artifact task with a product behavior change.
- Cleaning unrelated dirty worktree files.

## Completion Criteria

<!-- lint-ack: boundary-entry-point - p204 is a manual live gate; test files are listed only as allowed follow-up patch points if the gate exposes a code bug. -->

Scenario: Codex-only preflight passes without Claude
  Test: manual:p204_codex_preflight
  Given the current repository root
  When the operator runs:
    | command |
    | AGENTD_REAL_EXECUTE_RUNTIMES=codex,codex,codex,codex bash scripts/agentd_real_execute_smoke.sh --preflight-only --run-id p204-codex-matrix --state-dir .agentd/real-execute-smoke/p204-codex-matrix |
  Then stdout includes `preflight ok`
  And stderr does not report a missing Claude prerequisite

Scenario: real Codex execute smoke reaches finished
  Test: manual:p204_codex_execute_finished
  Given Codex-only preflight has passed
  When the operator runs:
    | command |
    | AGENTD_REAL_EXECUTE_RUNTIMES=codex,codex,codex,codex AGENTD_REAL_EXECUTE_SMOKE=1 bash scripts/agentd_real_execute_smoke.sh --execute --run-id p204-codex-matrix --state-dir .agentd/real-execute-smoke/p204-codex-matrix |
  Then `.agentd/real-execute-smoke/p204-codex-matrix/summary.txt` contains `result: finished`
  And `.agentd/real-execute-smoke/p204-codex-matrix/run_snapshot.json` contains `"status":"finished"`

Scenario: evidence proves all selected agents are Codex roles
  Test: manual:p204_codex_role_evidence
  Given the p204 state directory exists
  And the runtime matrix maps the implementer slot to `codex-impl`
  When `.agentd/real-execute-smoke/p204-codex-matrix/workflows/execute.dot` is inspected
  Then it contains `role="codex-impl"`
  And it contains `reviewers="codex-sec,codex-perf,codex-readability"`
  And it does not contain `claude-sec` or `gemini-readability`

Scenario: failed real run preserves evidence and blocks parity advancement
  Test: manual:p204_failed_run_preserves_evidence
  Given the p204 execute command exits non-zero
  When the p204 state directory is inspected
  Then the state directory is preserved for debugging
  And the parity map is not advanced to covered
  And the next implementation step is based on the captured `summary.txt`, `run_snapshot.json`, `events.snapshot`, and `daemon.log`

Scenario: parity is advanced only after finished evidence
  Test: manual:p204_parity_update
  Given the p204 run reached `finished`
  When the parity map is updated
  Then the `real_codex_execution` row no longer cites missing real Codex execute
  evidence as the reason for remaining `partial`
  And any remaining `partial` status must name a different concrete Phase B gap
