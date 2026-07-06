spec: task
name: "execute.dot context and report readiness"
tags: [e2e, workflow, p2, execute, context]
---

## Intent

Make the standalone `execute.dot` path ready for a real frozen-spec run by
binding the implementer prompt to concrete local spec and plan paths, and by
ensuring the final acceptance-report tool has a report file to read. The
workflow must keep the existing PR publication shape while adding only the
minimal local runtime-state plumbing needed for a real end-to-end run.

## Decisions

- Add a `tool` node attribute named `context_updates` whose comma-separated
  `key=value` entries are staged into run context only when the command exits
  successfully.
- `execute.dot` stages `spec_path=.agentd/run/frozen.spec.md` after
  `pull_frozen_spec` succeeds.
- `execute.dot` writes the plan through `scripts/agentd_write_plan.sh` and then
  stages `plan_path=.agentd/run/plan.md` after `draft_plan` succeeds.
- Keep the existing publish-before-PR order; `open_pr` remains a single argv-safe
  tool command and PR-specific preflight lives in the open PR helper.
- `scripts/agentd_publish_worktree.sh` writes `.agentd/run/report.md` in the
  daemon cwd after the task branch is pushed, so `report_acceptance` does not
  fail on a missing local report file.

## Boundaries

### Allowed Changes

- crates/agentd-core/src/handler/tool.rs
- crates/agentd-core/tests/handlers.rs
- workflows/execute.dot
- scripts/agentd_write_plan.sh
- scripts/agentd_publish_worktree.sh
- crates/agentctl/tests/workflows.rs
- crates/agentd-bin/tests/**
- specs/e2e/p127-execute-context-report-readiness.spec.md

### Forbidden

- Do not change the publish-before-PR topology in this slice.
- Do not add schema columns for `spec_path`, `plan_path`, report path, or PR URL.
- Do not make `tool` nodes run in the allocated worktree cwd.

## Out of Scope

- Running the full real `execute.dot` flow with real reviewers and PR creation.
- Persisting PR metadata or acceptance reports in the database.
- Changing reviewer prompt contents beyond the existing context snapshot.

## Completion Criteria

Scenario: tool stages static context updates only on successful command exit
  Test:
    Package: agentd-core
    Filter: tool_handler_stages_static_context_updates_on_success
  Level: core handler unit
  Test Double: RecordingCommandRunner + HandlerCtx staged updates
  Given a tool node with `context_updates="spec_path=.agentd/run/frozen.spec.md,plan_path=.agentd/run/plan.md"`
  When its command exits with status 0
  Then the handler stages both context keys with string values

Scenario: malformed static context updates are rejected
  Test:
    Package: agentd-core
    Filter: static_context_updates_reject_malformed_entries
  Level: parser unit
  Test Double: pure parser helper
  Given a tool node `context_updates` entry without `=`
  When the static context-update parser reads it
  Then it returns an invariant error that names the required `key=value` syntax

Scenario: execute.dot declares the frozen spec and plan context bridge
  Test:
    Package: agentctl
    Filter: execute_dot_declares_spec_and_plan_context_bridge
  Level: workflow unit
  Test Double: DOT parser + NodeGraph inspector
  Given workflows/execute.dot
  When the graph is parsed and inspected
  Then `pull_frozen_spec` stages `spec_path`
  And `draft_plan` runs the plan-writing helper and stages `plan_path`
  And `implement` includes both keys in its initial prompt

Scenario: execute.dot implementer prompt receives concrete spec and plan paths
  Test:
    Package: agentctl
    Filter: execute_dot_implement_prompt_receives_spec_and_plan_paths
  Level: core workflow integration
  Test Double: FakeBackend + RecordingCommandRunner + fake WorktreeAllocator
  Given execute.dot on the real Engine with fake ports
  When the workflow reaches the implementer park
  Then the spawned implementer prompt includes `spec_path: .agentd/run/frozen.spec.md`
  And it includes `plan_path: .agentd/run/plan.md`

Scenario: publish helper writes the local acceptance report for report_acceptance
  Test:
    Package: agentd-bin
    Filter: publish_worktree_writes_local_acceptance_report
  Level: script integration
  Test Double: local git worktree + local bare origin
  Given a local git worktree with a bare origin and a valid task_run_id
  When `scripts/agentd_publish_worktree.sh` publishes the task branch
  Then `.agentd/run/report.md` exists in the daemon cwd
  And the report names the task_run_id and `agentd/${task_run_id}` branch
