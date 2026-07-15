spec: task
name: "agent-chat direct inbox baseline"
tags: [agent-chat-replacement, messaging, inbox, phase-d, p217]
---

## Intent

Replace the v0 empty `check_inbox` placeholder with a durable direct-message
inbox baseline that agents can recover from through the central daemon. This
slice intentionally covers only private/direct messages; group history,
@mentions, attachments, Matrix relay state, and full send/post MCP compatibility
remain separate Phase D work.

## Decisions

- Direct messages are durable SQLite rows keyed by a stable `message_id`.
- `message_id` is idempotent: accepting the same id twice does not duplicate
  inbox reads.
- Inbox messages preserve the agent-chat-facing fields agents depend on:
  `id`, `ts`, `at`, `time`, `from`, `to`, `type`, `priority`, `summary`,
  `full`, `reply_to`, `source`, `sourceRoom`, `senderMxid`, `trustLevel`, and
  `fromId`.
- `check_inbox(agent_id, drain=false)` is a non-destructive preview of unread
  direct messages for that agent.
- `check_inbox(agent_id, drain=true)` returns the same unread direct messages
  and marks those returned rows read, so the next drain/preview no longer
  returns them.
- The tool output includes both the existing flat `messages` field and
  agent-chat-compatible `dm` and `group` arrays. For this slice, `dm` equals
  `messages` and `group` is always empty.
- The daemon exposes an operator-controlled `POST /api/messages` direct-message
  write path guarded by the existing operator bearer policy when configured.
- The parity map moves `messaging_inbox` from `missing` to `partial`; it remains
  partial until group mentions, send/post MCP tools, attachments, Matrix/remote
  relay delivery, notification gating, and import/shadow coverage are complete.

## Boundaries

### Allowed Changes

- specs/e2e/p217-agent-chat-direct-inbox-baseline.spec.md
- crates/agentd-store/src/lib.rs
- crates/agentd-store/src/message_repo.rs
- crates/agentd-store/src/util.rs
- crates/agentd-store/migrations/0005_direct_messages.sql
- crates/agentd-store/tests/messages.rs
- crates/agentd-store/tests/migration.rs
- crates/agentd-surface/src/host.rs
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/src/test_support.rs
- crates/agentd-surface/src/tools/check_inbox.rs
- crates/agentd-surface/tests/tools.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/daemon_http.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not modify the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout or any source JSON file.
- Do not run real Claude.
- Do not add third-party crates that are not already workspace dependencies.
- Do not add group messaging, group mentions, attachments, Matrix relay, remote
  relay, notification queue/gate, or task graph behavior in this slice.
- Do not import `messages.json` or mutate agent-chat cursor files.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- MCP `send_message`, `post`, `check_group`, and filtered `kinds` previews.
- Group membership, group history, and @mention inbox semantics.
- Attachment staging/localization.
- Message import, migration shadow audits for messages, and service cutover.
- Push notification delivery, tmux injection, inbox gates, and relay ack state.

## Completion Criteria

<!-- lint-ack: decision-coverage - p217 binds durable direct message rows, idempotent ids, preview/drain behavior, HTTP write path, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify stored rows, returned message fields, read markers, HTTP status/body/auth, and parity row text. -->

Scenario: store direct inbox reads unread messages and drains returned rows
  Test:
    Package: agentd-store
    Filter: direct_inbox_reads_unread_messages_and_drain_marks_read
  Level: store integration
  Test Double: real SqliteStore in a temp directory
  Given a durable direct message from "alex" to "codex-worker"
  When the inbox is previewed without draining
  Then the message is returned with agent-chat-compatible fields
  And a second preview returns the same unread message
  When the inbox is read with drain enabled
  Then the message is returned and marked read
  And a follow-up preview returns no messages

Scenario: store direct inbox is idempotent by message id
  Test:
    Package: agentd-store
    Filter: direct_inbox_insert_is_idempotent_by_message_id
  Level: store integration
  Test Double: real SqliteStore in a temp directory
  Given the same direct `message_id` is accepted twice
  When the target agent previews the inbox
  Then only one unread message is returned

Scenario: check_inbox returns durable direct messages and drains by request
  Test:
    Package: agentd-surface
    Filter: check_inbox_returns_durable_direct_messages_and_drains
  Level: surface unit
  Test Double: FakeRunHost
  Given the fake host has one unread direct message for "codex-worker"
  When `check_inbox` runs with `drain=false`
  Then `messages` and `dm` each contain that message
  And `group` is empty
  When `check_inbox` runs with `drain=true`
  Then the message is returned and marked read
  And the next preview is empty

Scenario: daemon accepts operator direct messages and agents read them through tools
  Test:
    Package: agentd-bin
    Filter: daemon_router_operator_message_write_feeds_check_inbox_tool
  Level: daemon integration
  Test Double: real SqliteStore, fake backend, HTTP router, tools/call
  Given the daemon has an empty database
  When an operator posts a direct message to `/api/messages`
  Then the response includes the stable message id
  When the target agent calls `check_inbox` through `/tools/call` with drain
  Then the returned `dm` row contains the message summary and full body
  And a second tool call returns no unread messages

Scenario: daemon message write obeys the configured operator bearer guard
  Test:
    Package: agentd-bin
    Filter: daemon_router_message_write_requires_bearer_when_configured
  Level: daemon integration
  Test Double: real SqliteStore, fake backend, HTTP router
  Given the daemon is configured with an operator API token
  When `/api/messages` is called without a bearer token
  Then the route returns 401
  When `/api/messages` is called with the configured bearer token
  Then the route stores the direct message and returns 201

Scenario: parity map records p217 direct inbox progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p217_direct_inbox_progress
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the agent-chat replacement parity map
  When the `messaging_inbox` row is inspected
  Then its status is "partial"
  And its decision mentions p217 durable direct messages, `check_inbox`,
  preview/drain read semantics, and remaining group mentions, send/post MCP,
  attachments, Matrix/remote relay, notification gates, and message import gaps
