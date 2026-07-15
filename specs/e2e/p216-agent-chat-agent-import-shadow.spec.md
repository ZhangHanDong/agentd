spec: task
name: "agent-chat agent JSON import and shadow audit"
tags: [agent-chat-replacement, migration, shadow, registry, phase-h, p216]
---

## Intent

Add the first non-destructive migration path from agent-chat local JSON state
into agentd by importing `data/agents.json` and auditing supported agent ids
against an agentd SQLite database. This advances replacement cutover work while
keeping messages, task graphs, Matrix relay state, and service cutover out of
scope until their durable schemas are present in this worktree.

## Decisions

- Operator entry points are `agentctl parity import-agents` and
  `agentctl parity shadow-agents`.
- `agentctl parity import-agents --agent-chat <path> --db-path <path>` is
  dry-run by default: it validates the checkout, reads `data/agents.json`,
  reports source/planned/skipped counts, and does not create or mutate the
  SQLite database.
- `agentctl parity import-agents --agent-chat <path> --db-path <path>
  --execute` opens and migrates the target SQLite database, then upserts
  supported agent records through the existing `agent_repo`.
- `agentctl parity shadow-agents --agent-chat <path> --db-path <path>` is
  read-only: it compares supported agent names from `agents.json` against the
  target database and exits 0 only when no supported agent is missing.
- The importer preserves agent-chat names as agentd agent ids/names, maps
  `type` to runtime, `tmux` to `tmux_target`, `homeDir`/`workdir`/`stateDir`,
  `server`, `capability`, and `agentModelVersion` where present, and stores the
  source `agentId` under the runtime profile for later cutover diagnostics.
- The importer is additive and non-destructive: it never writes to the
  agent-chat checkout and never removes target rows absent from the source.
- Malformed supported JSON rejects execute mode without partial writes.
- The parity map moves `migration_shadow_cutover` from `missing` to `partial`;
  full cutover still requires messages, tasks, task graphs, Matrix/remote relay
  state, service cutover, rollback automation, and token provisioning.

## Boundaries

### Allowed Changes

- specs/e2e/p216-agent-chat-agent-import-shadow.spec.md
- crates/agentctl/Cargo.toml
- crates/agentctl/src/cli.rs
- crates/agentctl/src/parity.rs
- crates/agentctl/tests/parity_cli.rs
- crates/agentd-store/src/lib.rs
- crates/agentd-store/src/agent_chat_import.rs
- crates/agentd-store/tests/agent_chat_import.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not modify the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout or any source JSON file.
- Do not run real Claude.
- Do not add third-party crates that are not already workspace dependencies.
- Do not add message, task, task graph, Matrix, remote relay, service cutover,
  rollback, or token provisioning import in this slice.
- Do not change daemon HTTP routes, workflow engine behavior, or tmux launch
  behavior.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Importing `messages.json`, `tasks.json`, `task_graphs.json`, attachments, or
  Matrix/remote relay state.
- Starting imported agents or resurrecting tmux sessions.
- Browser/dashboard import UI.
- Service cutover and rollback automation.

## Completion Criteria

<!-- lint-ack: decision-coverage - p216 binds CLI dry-run/execute/audit, store import atomicity, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify stdout, exit codes, db existence, persisted rows, and source checkout non-mutation. -->

Scenario: agent import dry-run reports a plan without opening the database
  Test:
    Package: agentctl
    Filter: parity_agent_import_dry_run_reports_counts_without_creating_db
  Level: CLI integration
  Test Double: temp agent-chat fixture and temp db path
  Given an agent-chat fixture with two supported agents in `data/agents.json`
  When `agentctl parity import-agents --agent-chat <path> --db-path <db>` runs
  Then stdout reports dry-run mode and planned agent count "2"
  And the target database file does not exist
  And the source `agents.json` file is unchanged

Scenario: agent import execute writes supported agents into SQLite
  Test:
    Package: agentctl
    Filter: parity_agent_import_execute_writes_supported_agents
  Level: CLI integration
  Test Double: temp agent-chat fixture and real SqliteStore
  Given an empty target SQLite database path
  When `agentctl parity import-agents --agent-chat <path> --db-path <db>
  --execute` runs
  Then exit status is 0
  And a follow-up shadow audit exits 0
  And stdout reports imported agent count "2"

Scenario: agent shadow audit reports drift without mutating
  Test:
    Package: agentctl
    Filter: parity_agent_shadow_audit_reports_missing_agents_without_mutating
  Level: CLI integration
  Test Double: temp agent-chat fixture and real SqliteStore
  Given a target SQLite database containing only one of two source agents
  When `agentctl parity shadow-agents --agent-chat <path> --db-path <db>` runs
  Then exit status is 1
  And stdout names the missing agent
  And a second audit reports the same missing agent

Scenario: store importer rejects malformed agents JSON atomically
  Test:
    Package: agentd-store
    Filter: agent_chat_agent_import_rejects_malformed_agents_without_partial_writes
  Level: store integration
  Test Double: temp malformed agent-chat fixture and real SqliteStore
  Given source `agents.json` is malformed
  When the store importer runs with execute mode
  Then the import returns an error
  And no agent rows are written to the target database

Scenario: parity map records p216 migration progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p216_agent_import_shadow_progress
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the agent-chat replacement parity map
  When the `migration_shadow_cutover` row is inspected
  Then its status is "partial"
  And its decision mentions p216 agents.json import, shadow audit, and remaining
  messages, tasks, task graphs, Matrix, remote relay, service cutover, rollback,
  and token provisioning gaps
