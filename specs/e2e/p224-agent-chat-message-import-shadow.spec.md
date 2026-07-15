spec: task
name: "agent-chat message JSON import and shadow audit"
tags: [agent-chat-replacement, migration, shadow, messaging, phase-d, phase-h, p224]
---

## Intent

Advance the agent-chat replacement cutover path after p223 by importing the
existing agent-chat `data/messages.json`, `data/groups.json`, and
`data/cursors.json` state into agentd's durable direct/group message tables.
This gives agentd a non-destructive way to preserve message history and unread
cursor semantics during migration, while keeping task graphs, Matrix state,
remote relay state, service cutover, rollback automation, and token
provisioning out of this slice.

## Decisions

- Operator entry points are `agentctl parity import-messages` and
  `agentctl parity shadow-messages`.
- `agentctl parity import-messages --agent-chat <path> --db-path <path>` is
  dry-run by default: it validates the checkout, reads message/group/cursor JSON
  state, reports planned direct/group message counts, and does not create or
  mutate the target SQLite database.
- `agentctl parity import-messages --agent-chat <path> --db-path <path>
  --execute` opens and migrates the target SQLite database, creates referenced
  groups from `groups.json`, imports direct and group messages through the
  existing message repository semantics, and applies imported read cursors.
- `agentctl parity shadow-messages --agent-chat <path> --db-path <path>` is
  read-only: it compares supported message ids from `messages.json` against the
  target database and exits 0 only when no supported direct/group message is
  missing.
- Direct messages are imported when a row has `to` and no `group`; group
  messages are imported when a row has `group` and no `to`.
- Imported fields preserve `id`, `ts`, `from`, `to`/`group`, `type`,
  `priority`, `summary`, `full`, `mentions`, `reply_to`, `source`,
  `sourceRoom`, `senderMxid`, `trustLevel`, `fromId`, `schema`, and
  `attachments` where present.
- Cursor import maps agent-chat inbox cursors to `direct_messages.read_at` and
  `group_mention_reads`, and maps per-group cursors to `group_message_reads`,
  so imported history does not reappear as unread after cutover.
- The importer is additive and non-destructive: it never writes to the
  agent-chat checkout and never removes target rows absent from the source.
- Malformed supported message/group/cursor JSON rejects execute mode without
  partial direct/group message writes.
- The parity map keeps `messaging_inbox`, `group_messaging`, and
  `migration_shadow_cutover` partial until Matrix/remote relay delivery,
  notification gates, dashboard message views, task/task-graph import, service
  cutover, rollback automation, and token provisioning are done.

## Boundaries

### Allowed Changes

- specs/e2e/p224-agent-chat-message-import-shadow.spec.md
- crates/agentctl/src/cli.rs
- crates/agentctl/src/parity.rs
- crates/agentctl/tests/parity_cli.rs
- crates/agentd-store/src/agent_chat_import.rs
- crates/agentd-store/tests/agent_chat_import.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not modify the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout or any source JSON file.
- Do not run real Claude, Matrix, tmux, systemd, launchd, or remote relay in
  automated tests.
- Do not import tasks, task graphs, Matrix bridge state, remote relay state,
  service cutover state, rollback plans, or token provisioning in this slice.
- Do not add new database tables or third-party crates.
- Do not change daemon HTTP routes, workflow engine behavior, MCP tool schemas,
  or tmux launch behavior.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Importing `tasks.json`, task graphs, Matrix bridge state, remote relay state,
  alert state, runtime sessions, or delivery-event history.
- Copying attachment bytes or message media files.
- Starting imported agents, resurrecting tmux sessions, or changing runtime
  launch policy.
- Browser/dashboard import UI.
- Service cutover and rollback automation.

## Completion Criteria

<!-- lint-ack: decision-coverage - p224 binds CLI dry-run/execute/audit, store message/group/cursor import, atomic error handling, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify stdout, exit codes, db existence, persisted rows, read cursor side effects, source checkout non-mutation, and docs. -->

Scenario: message import dry-run reports a plan without opening the database
  Test:
    Package: agentctl
    Filter: parity_message_import_dry_run_reports_counts_without_creating_db
  Level: CLI integration
  Test Double: temp agent-chat fixture and temp db path
  Given an agent-chat fixture with one direct message, one group message,
  `groups.json`, and `cursors.json`
  When `agentctl parity import-messages --agent-chat <path> --db-path <db>`
  runs
  Then stdout reports dry-run mode and planned direct/group message counts
  And the target database file does not exist
  And the source `messages.json`, `groups.json`, and `cursors.json` files are
  unchanged

Scenario: message import execute writes messages, groups, and read cursors
  Test:
    Package: agentctl
    Filter: parity_message_import_execute_writes_messages_groups_and_cursors
  Level: CLI integration
  Test Double: temp agent-chat fixture and real SqliteStore
  Given an empty target SQLite database path
  When `agentctl parity import-messages --agent-chat <path> --db-path <db>
  --execute` runs
  Then exit status is 0
  And stdout reports imported direct/group message counts
  And a follow-up `shadow-messages` audit exits 0
  And imported cursor state prevents already-read direct and group messages from
  appearing as unread

Scenario: message shadow audit reports drift without mutating
  Test:
    Package: agentctl
    Filter: parity_message_shadow_audit_reports_missing_messages_without_mutating
  Level: CLI integration
  Test Double: temp agent-chat fixture and real SqliteStore
  Given a target SQLite database containing only one of two source messages
  When `agentctl parity shadow-messages --agent-chat <path> --db-path <db>`
  runs
  Then exit status is 1
  And stdout names the missing message id
  And a second audit reports the same missing message id

Scenario: store message importer rejects malformed messages atomically
  Test:
    Package: agentd-store
    Filter: agent_chat_message_import_rejects_malformed_messages_without_partial_writes
  Level: store integration
  Test Double: temp malformed agent-chat fixture and real SqliteStore
  Given source `messages.json` is malformed
  When the store message importer runs with execute mode
  Then the import returns an error
  And no direct or group message rows are written to the target database

Scenario: parity map records p224 message import shadow progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p224_message_import_shadow_progress
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the agent-chat replacement parity map
  When the messaging, group, and migration rows are inspected
  Then they mention p224 message import, shadow audit, and cursor preservation
  And they remain partial because Matrix/remote relay delivery, notification
  gates, dashboard message views, task/task-graph import, service cutover,
  rollback, and token provisioning are not complete
