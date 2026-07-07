spec: task
name: "Real execute smoke harness"
tags: [e2e, p0.9, real-agent, execute, smoke, script]
---

## Intent

Turn the manually-driven real `execute.dot` attempt into a repeatable operator
harness. The prior run proved useful boundaries but depended on ad hoc commands;
this slice adds a guarded script that dry-runs the concrete plan, preflights the
real tools, and in explicit execute mode starts the daemon with absolute
`repo_dir` and `worktree_base` paths, triggers `execute.dot`, and captures the
evidence needed to diagnose real-agent, worktree, and PR failures.

## Decisions

- The harness is `scripts/agentd_real_execute_smoke.sh`.
- Real execution requires both `--execute` and `AGENTD_REAL_EXECUTE_SMOKE=1`.
- `--dry-run` is the default mode and prints the exact build, plan-generation,
  daemon, and `agentctl run start --flow execute` commands without creating the
  state directory or starting real processes.
- `--preflight-only` checks local prerequisites without starting the daemon:
  `cargo`, `tmux`, `claude`, `agent-spec`, `curl`, `git`, `gh`, Claude help
  output containing `--mcp-config`, `gh auth status`, and current `HEAD`
  common history with `origin/main`.
- The daemon command uses absolute `--repo-dir` and absolute `--worktree-base`
  paths so launcher cwd handling cannot repeat the relative nested-path failure.
- Real execution writes evidence under a configurable `--state-dir`:
  frozen spec copy, generated plan copy, preflight log, daemon log, agentctl
  output, run snapshot, event snapshot, and summary file.

## Boundaries

### Allowed Changes

- specs/e2e/p131-real-execute-smoke-harness.spec.md
- scripts/agentd_real_execute_smoke.sh
- crates/agentd-bin/tests/real_execute_smoke.rs
- docs/p0.9-deployment-checklist.md

### Forbidden

- Do not make automated tests call paid/authenticated Claude or GitHub services.
- Do not start a real daemon, tmux session, or agent process in automated tests.
- Do not change `execute.dot` topology in this slice.
- Do not modify or delete existing `.agentd/real-execute-smoke/*` evidence.

## Out of Scope

- Solving Claude account quota or GitHub remote-history mismatch.
- Replacing the real reviewers with test doubles inside the script.
- Creating or pushing commits from the harness itself.
- Changing branch publication or open PR helper semantics.

## Completion Criteria

<!-- lint-ack: error-path — the execute opt-in guard and missing-gh preflight scenarios are the script's explicit failure paths. -->

Scenario: default dry-run prints the real execute plan without side effects
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_dry_run_prints_plan_without_starting
  Level: script integration
  Test Double: process invocation in dry-run mode
  Given no opt-in environment variable is set
  When the harness runs with `--dry-run`, `--run-id`, `--port`, and `--state-dir`
  Then it exits 0
  And stdout lists plan generation, daemon command, `agentctl run start --flow execute`, health check URL, and evidence directory
  And the state directory is not created

Scenario: execute mode requires explicit opt-in
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_execute_requires_explicit_opt_in
  Level: script integration
  Test Double: process invocation without opt-in environment
  Given `AGENTD_REAL_EXECUTE_SMOKE` is unset
  When the harness runs with `--execute`
  Then it exits non-zero
  And stderr names `AGENTD_REAL_EXECUTE_SMOKE=1`

Scenario: preflight reports missing required tools
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_preflight_fails_when_tool_is_missing
  Level: script integration
  Test Double: fake PATH missing gh
  Given the process `PATH` contains no `gh` executable
  When the harness runs with `--preflight-only`
  Then it exits non-zero
  And stderr names the missing `gh` prerequisite

Scenario: preflight accepts fake local prerequisites without starting daemon
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_preflight_accepts_fake_tools
  Level: script integration
  Test Double: fake PATH with local tool shims
  Given fake local `cargo`, `tmux`, `claude`, `agent-spec`, `curl`, `git`, and `gh` executables
  And fake `claude --help` prints `--mcp-config`
  And fake `gh auth status` exits 0
  And fake git fetch, rev-parse, and merge-base checks exit 0
  When the harness runs with `--preflight-only`
  Then it exits 0
  And stdout contains `preflight ok`
  And no daemon log exists in the state directory

Scenario: dry-run uses absolute repo and worktree-base paths
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_dry_run_uses_absolute_daemon_paths
  Level: script integration
  Test Double: process invocation in dry-run mode
  Given a relative state directory
  When the harness prints the dry-run plan
  Then the daemon command contains absolute `--repo-dir` and `--worktree-base` arguments

Scenario: real execution plan writes named evidence artifacts
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_script_declares_evidence_artifacts
  Level: source inspection
  Test Double: script source text
  Given the harness source is inspected
  When its artifact paths are checked
  Then it declares `frozen.spec.md`, `plan.md`, `preflight.log`, `daemon.log`, `agentctl.out`, `run_snapshot.json`, `events.snapshot`, and `summary.txt`
