spec: task
name: "Real Claude stdio smoke harness"
tags: [e2e, p0.9, real-agent, claude, smoke, script]
---

## Intent

Turn the remaining P0.9 real-agent stdio smoke into a repeatable operator
harness. P123 made the Claude launcher load the `agentd` MCP server; this slice
adds a guarded script that can either dry-run/preflight locally or, with explicit
opt-in, start a real daemon, trigger `draft.dot`, and capture the evidence needed
to prove a real authenticated Claude process advanced the run.

## Decisions

- The harness is `scripts/agentd_real_claude_smoke.sh`.
- Real execution requires both `--execute` and `AGENTD_REAL_CLAUDE_SMOKE=1`.
- `--dry-run` is the default mode and prints the concrete run plan without
  starting the daemon, invoking `agentctl run start`, spawning tmux, or calling
  Claude.
- `--preflight-only` checks local prerequisites without starting the daemon:
  `cargo`, `tmux`, `claude`, `agent-spec`, `curl`, and Claude help output
  containing `--mcp-config`.
- Real execution writes evidence under a configurable `--state-dir`: issue file,
  preflight log, daemon log, agentctl output, run snapshot, event snapshot, and a
  summary file.
- The harness uses the existing live path: `target/debug/agentd` serves the
  daemon and `target/debug/agentctl run start --flow draft` triggers the run.

## Boundaries

### Allowed Changes
- specs/e2e/p124-real-claude-smoke-harness.spec.md
- scripts/agentd_real_claude_smoke.sh
- crates/agentd-bin/tests/real_claude_smoke.rs
- docs/p0.9-deployment-checklist.md

### Forbidden
- Do not make automated tests call a paid/authenticated Claude network request.
- Do not start a real tmux session in automated tests.
- Do not add new Rust production dependencies.
- Do not change `draft.dot`, `execute.dot`, or the MCP tool schemas in this
  slice.

## Completion Criteria

Scenario: default dry-run prints the real smoke plan without side effects
  Test:
    Package: agentd-bin
    Filter: real_claude_smoke_dry_run_prints_plan_without_starting
  Given no opt-in environment variable is set
  When the harness runs with `--dry-run`, `--run-id`, `--port`, and `--state-dir`
  Then it exits 0
  And stdout lists the daemon command, agentctl command, health check URL, and
  evidence directory
  And the state directory is not created

Scenario: execute mode requires explicit opt-in
  Test:
    Package: agentd-bin
    Filter: real_claude_smoke_execute_requires_explicit_opt_in
  Given `AGENTD_REAL_CLAUDE_SMOKE` is unset
  When the harness runs with `--execute`
  Then it exits non-zero
  And stderr names `AGENTD_REAL_CLAUDE_SMOKE=1`

Scenario: preflight reports missing required tools
  Test:
    Package: agentd-bin
    Filter: real_claude_smoke_preflight_fails_when_tool_is_missing
  Given the process `PATH` contains no `claude` executable
  When the harness runs with `--preflight-only`
  Then it exits non-zero
  And stderr names the missing `claude` prerequisite

Scenario: preflight accepts fake local prerequisites without starting daemon
  Test:
    Package: agentd-bin
    Filter: real_claude_smoke_preflight_accepts_fake_tools
  Given fake local `cargo`, `tmux`, `claude`, `agent-spec`, and `curl` executables
  And fake `claude --help` prints `--mcp-config`
  When the harness runs with `--preflight-only`
  Then it exits 0
  And stdout contains `preflight ok`
  And no daemon log exists in the state directory

Scenario: real execution plan writes named evidence artifacts
  Test:
    Package: agentd-bin
    Filter: real_claude_smoke_script_declares_evidence_artifacts
  Given the harness source is inspected
  When its artifact paths are checked
  Then it declares `issue.md`, `preflight.log`, `daemon.log`, `agentctl.out`,
  `run_snapshot.json`, `events.snapshot`, and `summary.txt`

## Out of Scope

- Completing the paid/authenticated real Claude call inside automated tests.
- Full `execute.dot` reviewer fan-out and PR creation.
- Real SIGKILL recovery or the 90-second MVP demo.
