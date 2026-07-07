spec: task
name: "Real SIGKILL recovery harness via human answer MCP"
tags: [e2e, p1, recovery, mcp, surface]
---

## Intent

The P0.9 deployment checklist still leaves the real SIGKILL drill as a manual
operator step. Core and production delivery already support `HumanAnswered`, but
the MCP surface only exposes agent and review submissions, leaving no local
process-boundary way to resume a `wait.human` park without a real coding-agent
CLI. This slice exposes the existing human-answer event as a small MCP tool and
adds a guarded local harness that starts a temporary daemon, kills only that
daemon with SIGKILL, restarts it on the same SQLite DB, and resumes through
`agentd mcp-stdio`.

## Decisions

- Add an agentd MCP tool named `submit_human_answer`.
- `submit_human_answer` accepts `wait_id`, `answer`, and optional `feedback`, then
  delivers `EngineEvent::HumanAnswered` through the existing `RunHost::deliver`
  seam.
- A moved or already-answered wait returns `already_submitted`; a successful
  answer reports `accepted=true` and the next node when the run parks again.
- Do not add HTTP routes for human answers in this slice; the operator harness
  uses the existing MCP stdio boundary.
- Add `scripts/agentd_real_sigkill_smoke.sh`, dry-run by default, with real
  execution requiring both `--execute` and `AGENTD_REAL_SIGKILL_SMOKE=1`.
- The harness uses a temporary `wait.human` workflow under its state directory,
  queries the local SQLite DB for the open `wait_id`, kills only the daemon PID it
  started, restarts the daemon with the same `--db-path`, and submits the answer
  through `agentd mcp-stdio`.
- The harness writes auditable evidence under its state dir: workflow, preflight
  log, first daemon log, restarted daemon log, agentctl output, MCP response,
  run snapshot, events snapshot, and summary.
- Update the deployment checklist so the real SIGKILL item points to the guarded
  harness while retaining the existing proven-by-test notes.

## Boundaries

### Allowed Changes

- specs/e2e/p141-real-sigkill-human-answer-harness.spec.md
- docs/p0.9-deployment-checklist.md
- scripts/agentd_real_sigkill_smoke.sh
- specs/surface/p70-mcp-tool-schemas.spec.md
- crates/agentd-surface/src/mcp_server.rs
- crates/agentd-surface/src/tools/mod.rs
- crates/agentd-surface/src/tools/submit_human_answer.rs
- crates/agentd-surface/tests/tools.rs
- crates/agentd-bin/src/stdio_mcp.rs
- crates/agentd-bin/tests/mcp_stdio.rs
- crates/agentd-bin/tests/real_sigkill_smoke.rs

### Forbidden

- Do not change HTTP route shapes.
- Do not change `EngineEvent`, checkpoint, or database schemas.
- Do not start Claude, Codex, Gemini, tmux, GitHub, or any external coding-agent
  smoke as part of tests or dry-run/preflight modes.
- Do not kill any process except the daemon PID started by the harness itself.
- Do not make the SIGKILL harness execute without the explicit environment opt-in.

## Out of Scope

- Full real execute smoke with coding agents and `open_pr`.
- Matrix or Specify relay for human decisions.
- Human-answer authorization policy beyond this local MCP operator surface.
- Changing how `wait.human` stores prompts or wait metadata.

## Completion Criteria

Scenario: submit_human_answer delivers the human event
  Test:
    Package: agentd-surface
    Filter: submit_human_answer_delivers_and_reports_next
  Level: MCP tool contract
  Test Double: FakeRunHost
  Given an open `wait.human` park identified by `wait_id="hw1"`
  When `submit_human_answer` is called with answer "approve"
  Then it delivers `EngineEvent::HumanAnswered` with the same wait id and answer
  And it returns `accepted=true`
  And it reports the next parked node when the run parks again

Scenario: submit_human_answer maps replayed waits to already_submitted
  Test:
    Package: agentd-surface
    Filter: submit_human_answer_stale_wait_is_already_submitted
  Level: MCP tool contract
  Test Double: FakeRunHost
  Given the host reports `RunProgress::Ignored` for the human answer event
  When `submit_human_answer` is called
  Then the tool returns the `already_submitted` surface error code

Scenario: MCP dispatcher lists the human answer tool
  Test:
    Package: agentd-surface
    Filter: dispatch_lists_six_tools_with_submit_human_answer
  Level: dispatcher contract
  Test Double: pure descriptor list
  Given the MCP tool registry
  When tool descriptors are listed
  Then the six tools include `submit_human_answer`
  And the P70 tool-count contract is updated from five tools to six tools

Scenario: stdio schema exposes submit_human_answer arguments
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_tools_list_includes_submit_human_answer_schema
  Level: stdio MCP schema contract
  Test Double: in-process production host with fake ports
  Given `tools/list` is requested over the stdio handler
  When the `submit_human_answer` tool schema is inspected
  Then it requires `wait_id` and `answer`
  And `feedback` is optional

Scenario: real SIGKILL smoke dry-run is inert and documents the plan
  Test:
    Package: agentd-bin
    Filter: real_sigkill_smoke_dry_run_prints_plan_without_starting
  Level: script contract
  Test Double: subprocess dry-run
  Given `scripts/agentd_real_sigkill_smoke.sh --dry-run`
  When the script prints its plan
  Then it names the temporary `wait.human` workflow and `agentd mcp-stdio`
  And it does not create the state directory
  And it does not mention Claude, tmux, or GitHub as prerequisites

Scenario: real SIGKILL smoke execute requires explicit opt-in
  Test:
    Package: agentd-bin
    Filter: real_sigkill_smoke_execute_requires_explicit_opt_in
  Level: script safety contract
  Test Double: subprocess execute without env
  Given `--execute` is passed without `AGENTD_REAL_SIGKILL_SMOKE=1`
  When the script starts
  Then it exits non-zero before building, starting, or killing a daemon
  And stderr names the required opt-in environment variable

Scenario: real SIGKILL smoke preflight avoids external agent prerequisites
  Test:
    Package: agentd-bin
    Filter: real_sigkill_smoke_preflight_accepts_fake_local_tools
  Level: script preflight contract
  Test Double: fake PATH tools
  Given fake `cargo`, `curl`, `sqlite3`, and `agent-spec` tools on PATH
  When `--preflight-only` runs
  Then it reports `preflight ok`
  And it does not require `claude`, `tmux`, `gh`, `codex`, or `gemini`

Scenario: deployment checklist points SIGKILL drill at guarded harness
  Test:
    Package: agentd-bin
    Filter: deployment_checklist_mentions_real_sigkill_harness
  Level: docs regression
  Test Double: source inspection
  Given docs/p0.9-deployment-checklist.md and the P141 spec
  When the real SIGKILL section is inspected
  Then it names `scripts/agentd_real_sigkill_smoke.sh`
  And it states the `AGENTD_REAL_SIGKILL_SMOKE=1` opt-in
