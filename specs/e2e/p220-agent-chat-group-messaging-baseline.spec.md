spec: task
name: "agent-chat group messaging baseline"
tags: [agent-chat-replacement, messaging, groups, mcp, http, phase-d, p220]
---

## Intent

Close the next required agent-chat replacement gap by adding durable group
membership, group message history, group @mention inbox rows, and the
agent-facing MCP `post` and `check_group` tools. This follows p219's
identity-bound direct messaging work and gives local agents the same core group
collaboration loop they use in agent-chat, while keeping attachment, Matrix,
remote relay, notification, and import/cutover work out of this slice.

## Decisions

- Add a new additive SQLite migration for group collaboration state.
- Preserve the existing p217/p218 direct-message table and JSON shape; group
  messages use separate group tables instead of rewriting direct messages.
- Groups persist as a name plus deduplicated member list. Blank member names are
  ignored, and duplicate members are removed case-insensitively while keeping
  the first spelling.
- `POST /api/groups`, `GET /api/groups`, `GET /api/groups/:name`, and
  `POST /api/groups/:name/members` provide local HTTP group administration.
- `POST /api/messages` accepts exactly one destination: either `to` for direct
  messages or `group` for group messages.
- Group messages require an existing group, a registered sender, and sender
  membership unless the sender is `system`.
- Explicit group `mentions` persist on the message. Mentioned registered
  members see the row in `check_inbox.group`; out-of-group registered mentions
  are reported as `mentions_not_in_group`; unknown mentions are reported as
  `mentions_unknown`.
- `check_inbox(agent_id, drain=false)` previews both direct messages and group
  mentions. `drain=true` marks returned direct rows and group mention rows read.
- `GET /api/groups/:name/messages?agent=<agent>` and MCP `check_group` require
  a registered group member and return agent-chat-style `read`, `unread`,
  `unread_total`, `unread_returned`, `unread_omitted`, and `advance` fields.
- Group history preview is non-destructive by default. `read_all=true` in MCP
  or `advance=all` in HTTP consumes all unread group history for that member.
- MCP `post` accepts `from`/`from_agent`, `group`, `summary`, `full`, optional
  `type`, `priority`, `mentions`, `reply_to`, and `schema`, and returns
  `ok`, `id`, `warnings`, `delivery`, and `taskGraph`.
- MCP `check_group` accepts `group`, `agent_id`, optional `limit`,
  `unread_limit`, and `read_all`.
- Identity-bound stdio sessions make MCP `post.from_agent` and
  `check_group.agent_id` implicit, with spoof rejection matching p219.
- The surface remains store-free. Persistence goes through `RunHost`; the
  production host maps to `agentd-store`, and `FakeRunHost` mirrors the behavior
  for in-process surface tests.
- The parity map records p220 group messaging progress but still keeps
  `messaging_inbox` and `group_messaging` partial until attachments/media,
  Matrix/remote relay delivery, notification gates, dashboard message views,
  and import/cutover are complete.

## Boundaries

### Allowed Changes

- specs/e2e/p220-agent-chat-group-messaging-baseline.spec.md
- crates/agentd-store/migrations/0006_group_messages.sql
- crates/agentd-store/src/message_repo.rs
- crates/agentd-store/tests/messages.rs
- crates/agentd-store/tests/migration.rs
- crates/agentd-surface/src/error.rs
- crates/agentd-surface/src/host.rs
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/src/mcp_server.rs
- crates/agentd-surface/src/test_support.rs
- crates/agentd-surface/src/tools/check_group.rs
- crates/agentd-surface/src/tools/check_inbox.rs
- crates/agentd-surface/src/tools/mod.rs
- crates/agentd-surface/src/tools/post.rs
- crates/agentd-surface/src/tools/send_message.rs
- crates/agentd-surface/tests/http.rs
- crates/agentd-surface/tests/tools.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/src/stdio_mcp.rs
- crates/agentd-bin/tests/daemon_http.rs
- crates/agentd-bin/tests/mcp_stdio.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not modify the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout or any source JSON file.
- Do not start Matrix, tmux, Claude, systemd, launchd, or remote relay
  processes in automated tests.
- Do not add attachment/media staging, Matrix bridge delivery, dashboard
  message pages, notification gates, agent-chat JSON group import/cutover, or
  service rollback automation in this slice.
- Do not make `agentd-surface` depend on `agentd-store` or runtime backends.
- Do not change the existing direct-message JSON shape for HTTP or MCP callers.
- Do not make `/tools/call` trust arbitrary HTTP headers as sender identity.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Attachment/media persistence, staging, localization, or Matrix transfer.
- Matrix bridge room membership, delivery, trust policy, and remote relay replay.
- Dashboard message pages and HTML rendering.
- Importing existing agent-chat `groups.json`, group `messages.json`, or group
  cursors.
- HTTP bearer-token, bridge-secret, or Matrix credential enforcement.
- Native notification queues, gates, or tmux push delivery.

## Completion Criteria

<!-- lint-ack: decision-coverage - p220 binds schema, HTTP group administration, direct-vs-group validation, mention delivery/warnings, inbox drain, group history cursors, MCP post/check_group, stdio identity injection, and docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify database tables/version, store behavior, HTTP behavior, MCP dispatcher/tool behavior, stdio schemas/injection, production persistence, and docs. -->

Scenario: migration creates durable group tables
  Test:
    Package: agentd-store
    Filter: migration_creates_group_message_tables
  Level: store migration
  Test Double: real SqliteStore temp database
  Given a newly migrated database
  When the schema is inspected
  Then `groups`, `group_members`, `group_messages`,
  `group_message_reads`, and `group_mention_reads` exist
  And reopening the store reports a schema version that includes the p220 group
  migration

Scenario: store group mention inbox is durable and drainable
  Test:
    Package: agentd-store
    Filter: group_message_mentions_appear_in_inbox_and_are_drainable
  Level: store integration
  Test Double: real SqliteStore temp database
  Given group `factory` with members `codex-a`, `codex-b`, and `codex-c`
  When `codex-a` creates two group messages and only the first mentions `codex-b`
  Then an inbox preview for `codex-b` returns the first message in `group`
  And an inbox preview for `codex-c` returns no group messages
  When `codex-b` drains the inbox
  Then reopening the store and draining `codex-b` again returns no group
  messages

Scenario: store group history previews and read_all advances
  Test:
    Package: agentd-store
    Filter: group_messages_preview_and_read_all_advances
  Level: store integration
  Test Double: real SqliteStore temp database
  Given group `factory` with members `codex-a` and `codex-b`
  And three group messages in message order
  When `codex-b` reads group `factory` with `advance=none`, `limit=1`, and
  `unread_limit=2`
  Then the response reports `unread_total=3`, `unread_returned=2`, and
  `unread_omitted=1`
  And a later preview still reports `unread_total=3`
  When `codex-b` reads group `factory` with `advance=all`
  Then a later preview reports `unread_total=0`

Scenario: HTTP group message lands in mentioned member inbox
  Test:
    Package: agentd-surface
    Filter: http_group_message_mentions_land_in_group_inbox
  Level: HTTP integration
  Test Double: FakeRunHost and in-process axum router
  Given registered agents `codex-a`, `codex-b`, and `codex-c`
  And group `factory` has members `codex-a` and `codex-b`
  When `POST /api/messages` posts to group `factory` with mentions
  `codex-b` and `codex-c`
  Then status is 200
  And the response contains `delivery.targetKind=null`
  And warnings include `mentions_not_in_group` for `codex-c`
  And `GET /api/inbox/codex-b` returns the message in `group`
  And `GET /api/inbox/codex-c` returns an empty `group`

Scenario: HTTP group history previews and advances
  Test:
    Package: agentd-surface
    Filter: http_group_messages_preview_and_advance_cursor
  Level: HTTP integration
  Test Double: FakeRunHost and in-process axum router
  Given registered agents `codex-a` and `codex-b`
  And group `factory` has both agents as members
  And three group messages exist
  When `GET /api/groups/factory/messages?agent=codex-b&advance=none&limit=1&unread_limit=2`
  is requested
  Then status is 200
  And the response reports `unread_total=3`, `unread_returned=2`,
  `unread_omitted=1`, and `advance=none`
  When `GET /api/groups/factory/messages?agent=codex-b&advance=all` is requested
  Then the next preview reports `unread_total=0`

Scenario: HTTP group endpoints reject invalid access
  Test:
    Package: agentd-surface
    Filter: http_group_message_rejects_unknown_group_and_non_member_sender
  Level: HTTP integration
  Test Double: FakeRunHost and in-process axum router
  Given registered agents `codex-a`, `codex-b`, and `codex-c`
  And group `factory` has members `codex-a` and `codex-b`
  When `POST /api/messages` contains both `to` and `group`
  Then status is 400
  When `POST /api/messages` posts to unknown group `ghost`
  Then status is 404
  When non-member `codex-c` posts to group `factory`
  Then status is 403
  When `GET /api/groups/factory/messages?agent=codex-c` is requested
  Then status is 403
  When `GET /api/groups/factory/messages` omits `agent`
  Then status is 400

Scenario: MCP dispatcher registers group tools
  Test:
    Package: agentd-surface
    Filter: dispatch_lists_group_tools_after_p220
  Level: MCP dispatcher unit
  Test Double: tool descriptor list only
  Given the transport-agnostic MCP dispatcher
  When tool descriptors are listed
  Then `post` and `check_group` are present
  And `send_message` and `check_inbox` remain present

Scenario: MCP post writes group message and mention inbox row
  Test:
    Package: agentd-surface
    Filter: post_group_message_mentions_member_and_warns_non_member
  Level: MCP tool integration
  Test Double: FakeRunHost
  Given registered agents `codex-a`, `codex-b`, and `codex-c`
  And group `factory` has members `codex-a` and `codex-b`
  When MCP tool `post` sends a group message from `codex-a` mentioning
  `codex-b` and `codex-c`
  Then the response contains `ok=true`, `delivery.targetKind=null`, and
  `delivery.suppressed=[codex-c]`
  And the response warnings include `mentions_not_in_group` for `codex-c`
  And MCP `check_inbox` for `codex-b` returns the message in `group`
  And MCP `check_inbox` for `codex-c` returns an empty `group`

Scenario: MCP check_group previews and read_all consumes cursor
  Test:
    Package: agentd-surface
    Filter: check_group_previews_and_read_all_consumes_cursor
  Level: MCP tool integration
  Test Double: FakeRunHost
  Given registered agents `codex-a` and `codex-b`
  And group `factory` has both agents as members
  And three group messages exist
  When MCP tool `check_group` runs for `codex-b` with `read_all=false`,
  `limit=1`, and `unread_limit=2`
  Then it reports `unread_total=3`, `unread_returned=2`,
  `unread_omitted=1`, and `advance=none`
  And a later preview still reports `unread_total=3`
  When MCP tool `check_group` runs for `codex-b` with `read_all=true`
  Then a later preview reports `unread_total=0`

Scenario: stdio group tools expose identity-aware schemas
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_tools_list_with_identity_makes_group_tools_identity_implicit
  Level: stdio JSON-RPC unit
  Test Double: production host with temp SQLite and fake backend
  Given a stdio MCP handler bound to "codex-a"
  When `tools/list` is requested
  Then `post` does not require `from_agent`
  And `check_group` does not require `agent_id`
  And `post` may advertise p221 `attachments`
  And `check_group` does not advertise `attachments`

Scenario: stdio post and check_group persist through production host
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_post_then_check_group_reads_and_consumes_group_message
  Level: stdio JSON-RPC integration
  Test Double: production host with temp SQLite and fake backend
  Given registered agents `codex-a` and `codex-b`
  And group `factory` has both agents as members
  When JSON-RPC `tools/call` invokes `post` for group `factory` from a
  handler bound to `codex-a`
  And JSON-RPC `tools/call` invokes `check_group` for a handler bound to
  `codex-b`
  Then the first group check returns one unread message
  When JSON-RPC `tools/call` invokes `check_group` with `read_all=true`
  Then a later group preview reports `unread_total=0`

Scenario: production router group state persists across rebuild
  Test:
    Package: agentd-bin
    Filter: daemon_router_group_message_persists_mentions_and_group_cursor
  Level: daemon HTTP integration
  Test Double: production host over temp SQLite store with fake backends
  Given registered agents `codex-a` and `codex-b`
  And group `factory` has both agents as members
  When `codex-a` posts a group message mentioning `codex-b`
  And the daemon router is rebuilt over the same database
  Then `GET /api/inbox/codex-b` returns the persisted message in `group`
  When `GET /api/groups/factory/messages?agent=codex-b&advance=all` is
  requested
  And the daemon router is rebuilt again over the same database
  Then the next group preview reports `unread_total=0`

Scenario: parity map records p220 group messaging progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p220_group_messaging_progress
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the agent-chat replacement parity map
  When the `messaging_inbox` and `group_messaging` rows are inspected
  Then they mention p220 durable groups, group mentions, MCP `post`, and
  MCP `check_group`
  And both rows remain partial
