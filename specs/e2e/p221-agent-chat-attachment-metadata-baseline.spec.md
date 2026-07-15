spec: task
name: "agent-chat attachment metadata baseline"
tags: [agent-chat-replacement, messaging, attachments, mcp, http, phase-d, p221]
---

## Intent

Close the next replacement gap after p220 by letting direct and group messages
carry agent-chat-style attachment metadata. This slice adds local readable-file
validation and durable metadata round trips for `send_message`, `post`, and
`POST /api/messages`, while deliberately stopping short of media staging,
localization, Matrix upload, remote fetch, and dashboard rendering. p222
supersedes only the local HTTP media staging part; the rest of this p221
metadata baseline remains valid.

## Decisions

- Add an additive SQLite migration that stores `attachments_json` on both
  `direct_messages` and `group_messages`, with schema version `7`.
- Attachment inputs accept either a string path or an object with `path`,
  optional `name`, optional `mime`, and optional `kind`.
- Local attachment validation requires each path to exist, be a regular file,
  be non-empty, and be no larger than `20 MiB`.
- Attachment count is limited to `8` items, matching agent-chat defaults.
- Persisted attachment metadata uses agent-chat-compatible fields: `path`,
  `name`, `mime`, `kind`, `size`, `staged`, and `source_path`.
- `staged=false` marks this p221 local metadata baseline; no code in this slice
  pretends that media was uploaded, copied, sanitized, localized, or bridged.
- `send_message`, `post`, and `/api/messages` all persist normalized attachment
  metadata and expose it through `check_inbox`, `check_group`, and HTTP inbox
  responses.
- MCP `send_message` and `post` input schemas advertise `attachments`; MCP
  `check_inbox` and `check_group` do not advertise attachment inputs.
- p220's group-tool schema assertion is updated only to acknowledge that p221
  adds `post.attachments`; `check_group` remains unchanged.
- The parity map records `attachments_media` as partial, not complete, because
  p221 did not include staging, Matrix transfer, remote relay media handling,
  dashboard previews, or import/cutover.

## Boundaries

### Allowed Changes

- specs/e2e/p221-agent-chat-attachment-metadata-baseline.spec.md
- specs/e2e/p220-agent-chat-group-messaging-baseline.spec.md
- crates/agentd-store/migrations/0007_message_attachments.sql
- crates/agentd-store/src/message_repo.rs
- crates/agentd-store/tests/messages.rs
- crates/agentd-store/tests/migration.rs
- crates/agentd-surface/src/host.rs
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/src/test_support.rs
- crates/agentd-surface/src/tools/attachments.rs
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
- Do not start Matrix, tmux, Claude, systemd, launchd, remote relay, or real
  media bridge processes in automated tests.
- p221 itself must not add a media staging endpoint, copy attachment bytes,
  sanitize images, fetch remote media, upload to Matrix, localize received
  media, or render attachment previews in the dashboard. Later specs may
  supersede specific local-media exclusions.
- Do not add new third-party dependencies for attachment metadata in this slice.
- Do not make `agentd-surface` depend on `agentd-store` or runtime backends.
- Do not change the direct-vs-group destination rule for `/api/messages`.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- `/api/media/stage` parity and content-addressed media storage. p222 follows
  up with the first local staging/fetch baseline, while content-addressed
  storage remains out of scope.
- Matrix bridge media upload/download and LocalPath localization.
- Image sanitization, MIME sniffing from file bytes, and preview generation.
- Dashboard message pages and attachment rendering.
- Importing existing agent-chat attachment files or cursors.

## Completion Criteria

<!-- lint-ack: decision-coverage - p221 binds migration version/columns, store round trip, MCP schemas, local validation errors, HTTP round trip, production stdio persistence, and docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios cover persisted state, MCP schema, MCP tool output, HTTP output, invalid-input failure, and docs. -->

Scenario: migration adds attachment columns
  Test:
    Package: agentd-store
    Filter: migration_adds_message_attachment_columns
  Level: store migration
  Test Double: real SqliteStore temp database
  Given a newly migrated database
  When the message tables are inspected
  Then `direct_messages.attachments_json` exists
  And `group_messages.attachments_json` exists
  And reopening the store reports schema version `7`

Scenario: store direct and group messages round trip attachments
  Test:
    Package: agentd-store
    Filter: direct_and_group_messages_round_trip_attachment_metadata
  Level: store integration
  Test Double: real SqliteStore temp database
  Given one direct message and one group message with attachment metadata
  When the direct inbox and group history are read
  Then both returned messages contain the original attachment metadata

Scenario: MCP tools accept local attachment metadata
  Test:
    Package: agentd-surface
    Filter: send_and_post_accept_readable_local_attachment_metadata
  Level: MCP tool integration
  Test Double: FakeRunHost and temp local files
  Given a readable local file attachment
  When MCP `send_message` sends a direct message with that attachment
  Then `check_inbox` for the receiver returns `attachments[0].staged=false`
  And the attachment includes `path`, `source_path`, `name`, `kind`, and `size`
  When MCP `post` sends a group message with that attachment
  Then `check_group` for another group member returns the same attachment fields

Scenario: MCP tools reject invalid local attachments before writing
  Test:
    Package: agentd-surface
    Filter: send_and_post_reject_invalid_attachment_before_writing
  Level: MCP tool integration
  Test Double: FakeRunHost and temp local files
  Given no readable file exists at `/tmp/agentd-missing-p221`
  When MCP `send_message` includes that missing attachment path
  Then it returns `bad_request`
  And the receiver inbox remains empty
  Given nine attachment inputs
  When MCP `post` submits them to a valid group
  Then it returns `bad_request`
  And the group history remains empty

Scenario: HTTP messages persist local attachment metadata
  Test:
    Package: agentd-surface
    Filter: http_messages_accept_attachment_metadata_for_direct_and_group
  Level: HTTP integration
  Test Double: FakeRunHost and in-process axum router
  Given registered agents `codex-a` and `codex-b`
  And group `factory` has both agents as members
  And a readable local file attachment exists
  When `POST /api/messages` sends a direct message to `codex-b`
  Then `GET /api/inbox/codex-b` returns the attachment metadata
  When `POST /api/messages` posts a group message to `factory`
  Then `GET /api/groups/factory/messages?agent=codex-b` returns the attachment
  metadata in unread group history

Scenario: stdio schemas advertise attachments only on write tools
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_schemas_advertise_attachment_inputs
  Level: stdio JSON-RPC unit
  Test Double: production host with temp SQLite and fake backend
  Given a stdio MCP handler
  When `tools/list` is requested
  Then `send_message.inputSchema.properties.attachments` exists
  And `post.inputSchema.properties.attachments` exists
  And `check_inbox.inputSchema.properties.attachments` is absent
  And `check_group.inputSchema.properties.attachments` is absent

Scenario: stdio send_message attachment persists through production host
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_send_message_attachment_round_trips_through_check_inbox
  Level: stdio JSON-RPC integration
  Test Double: production host with temp SQLite, temp local file, and fake backend
  Given a stdio MCP handler bound to `codex-a`
  And a readable local file attachment exists
  When JSON-RPC `tools/call` invokes `send_message` to `codex-b` with the
  attachment
  Then JSON-RPC `tools/call` invokes `check_inbox` for `codex-b` and returns
  the same attachment metadata

Scenario: parity map records p221 attachment metadata progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p221_attachment_metadata_progress
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the agent-chat replacement parity map
  When the `attachments_media` row is inspected
  Then it mentions p221 local-file attachment metadata
  And the row remains partial because Matrix media transfer and broader media
  parity are not complete
