spec: task
name: "Agent-chat parity baseline"
tags: [agent-chat-replacement, parity, p200]
---

## Intent

Create the first hard baseline for replacing `/Users/zhangalex/Work/Projects/consult/agent-chat`.
This slice must turn the replacement target into a maintained capability map and
a read-only `agentctl parity audit` gate. It does not implement missing
capabilities; it makes those gaps explicit, repeatable, and tied to future
replacement phases.

## Decisions

- Add `docs/parity/agent-chat-capability-map.md` as the authoritative local
  parity map for agent-chat replacement work.
- Add `agentctl parity audit --agent-chat <path>` as a read-only command that
  validates the map and prints missing required capabilities.
- The audit exits `1` when required capabilities are not covered, exits `2`
  for invalid inputs or map shape, and exits `0` only when all required rows are
  `covered`, `external`, or `deferred` with an explicit replacement decision.
- Required rows must use one of these statuses only: `covered`, `partial`,
  `missing`, `deferred`, or `external`; `unknown` is forbidden for required
  capabilities.
- The baseline must cover these required categories: registry, messaging,
  task graph, scheduler, runtime launch, dashboard/CLI operations, Matrix/remote,
  migration/cutover, auth, and real execution.

## Boundaries

### Allowed Changes

- specs/e2e/p200-agent-chat-parity-baseline.spec.md
- docs/parity/agent-chat-capability-map.md
- crates/agentctl/src/cli.rs
- crates/agentctl/src/main.rs
- crates/agentctl/src/parity.rs
- crates/agentctl/tests/parity_cli.rs

### Forbidden

- Do not start real Claude, Codex, tmux, Matrix, or remote relay processes.
- Do not write to, import from, or mutate the agent-chat checkout.
- Do not claim full agent-chat replacement in this slice.
- Do not add new Cargo dependencies for Markdown parsing.

## Out of Scope

- Implementing durable messages, agent lifecycle APIs, pool scheduling, Matrix
  relay, import, cutover, dashboard pages, or Codex runtime launch changes.
- Running `scripts/agentd_real_execute_smoke.sh --execute`.
- Migrating existing agent-chat JSON state.

## Completion Criteria

<!-- lint-ack: decision-coverage - the map shape and audit tests cover the listed status and required-category decisions. -->
<!-- lint-ack: observable-decision-coverage - this slice exposes one CLI output mode and one Markdown artifact. -->
<!-- lint-ack: boundary-entry-point - `agentctl parity audit` is the only new entry point. -->

Scenario: parity map names every required replacement category
  Test:
    Package: agentctl
    Filter: parity_capability_map_has_required_rows_without_unknowns
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the agent-chat replacement roadmap names required replacement categories
  When the parity map is parsed
  Then every required category has at least one row
  And no required row has status `unknown`
  And every row includes an agent-chat source path and a replacement decision

Scenario: audit reports current required gaps without mutating agent-chat
  Test:
    Package: agentctl
    Filter: parity_audit_reports_required_gaps_from_map
  Level: CLI
  Test Double: local agent-chat checkout path
  Given `/Users/zhangalex/Work/Projects/consult/agent-chat` exists
  When `agentctl parity audit --agent-chat /Users/zhangalex/Work/Projects/consult/agent-chat` runs
  Then it exits `1`
  And stdout includes a required summary
  And stdout names missing or partial required rows such as `messaging_inbox`, `pool_scheduler`, and `migration_shadow_cutover`
  And it does not create files under the agent-chat checkout

Scenario: audit rejects invalid agent-chat path
  Test:
    Package: agentctl
    Filter: parity_audit_rejects_missing_agent_chat_path
  Level: CLI
  Test Double: nonexistent path
  Given the requested `--agent-chat` path does not exist
  When `agentctl parity audit` runs
  Then it exits `2`
  And stderr explains that the agent-chat path is invalid
