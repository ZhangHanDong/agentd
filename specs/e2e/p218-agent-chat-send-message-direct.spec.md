spec: task
name: "agent-chat direct send_message MCP baseline"
tags: [agent-chat-replacement, messaging, mcp, phase-d, p218]
---

## Intent

Add the first agent-facing direct `send_message` MCP tool so agents can write
durable direct messages without going through an operator-only HTTP call. This
continues Phase D from p217 by making direct-message inbox recovery usable from
the MCP surface while keeping group `post`, `check_group`, attachments, schema
payloads, and implicit per-session identity out of scope.

## Decisions

- `send_message` writes into the existing p217 direct-message store through the
  `RunHost::post_direct_message` seam.
- The tool accepts `from_agent` (with `from` as a compatibility alias), `to`,
  `summary`, `full`, optional `type`, `priority`, and `reply_to`.
- Because the current central daemon `/tools/call` path has no per-stdio-session
  agent identity yet, p218 requires an explicit sender instead of silently
  guessing it.
- Default message `type` is `inform`; accepted values are `request`, `inform`,
  and `reply`.
- Default `priority` is `normal`; accepted values are `normal`, `high`, and
  `urgent`.
- The tool returns `{ ok: true, message }`, where `message` is the same
  agent-facing direct inbox row shape used by `check_inbox`.
- The stdio MCP `tools/list` schema advertises the new `send_message` tool and
  its required fields.
- Invalid or blank sender/recipient/summary/full, invalid `type`, and invalid
  `priority` are rejected before a message is written.
- The parity map keeps `messaging_inbox` at `partial` and records p218
  direct `send_message` progress; it remains partial until implicit identity,
  group messages, `post`, `check_group`, attachments, Matrix/remote relay
  delivery, notification gates, and message import/shadow coverage are complete.

## Boundaries

### Allowed Changes

- specs/e2e/p218-agent-chat-send-message-direct.spec.md
- crates/agentd-surface/src/error.rs
- crates/agentd-surface/src/mcp_server.rs
- crates/agentd-surface/src/tools/mod.rs
- crates/agentd-surface/src/tools/send_message.rs
- crates/agentd-surface/tests/tools.rs
- crates/agentd-bin/src/stdio_mcp.rs
- crates/agentd-bin/tests/daemon_http.rs
- crates/agentd-bin/tests/mcp_stdio.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not modify the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout or any source JSON file.
- Do not run real Claude.
- Do not add third-party crates that are not already workspace dependencies.
- Do not add group messaging, `post`, `check_group`, attachments, Matrix relay,
  remote relay, notification queue/gate, or task graph behavior in this slice.
- Do not change the store schema or duplicate the p217 direct-message table.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Implicit sender identity from the stdio session or agent token.
- MCP `post`, `check_group`, filtered inbox reads, and group @mentions.
- Attachments and structured `schema` payloads.
- Message import, migration shadow audits for messages, and service cutover.
- Push notification delivery, tmux injection, inbox gates, and relay ack state.

## Completion Criteria

<!-- lint-ack: decision-coverage - p218 binds sender aliasing, direct persistence, defaults, schema visibility, and validation through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify tool output, inbox side effects, HTTP /tools/call behavior, stdio tools/list schema, and no-write validation. -->

Scenario: send_message writes one durable direct message visible through check_inbox
  Test:
    Package: agentd-surface
    Filter: send_message_writes_direct_message_visible_through_check_inbox
  Level: surface unit
  Test Double: FakeRunHost
  Given a fake host with an empty inbox
  When `send_message` runs from "codex-worker" to "codex-reviewer"
  Then the output has `ok` true and a message addressed to "codex-reviewer"
  And a `check_inbox` preview for "codex-reviewer" returns that message
  And the message has default type "inform" and priority "normal"

Scenario: send_message rejects invalid input before writing
  Test:
    Package: agentd-surface
    Filter: send_message_rejects_invalid_input_before_writing
  Level: surface unit
  Test Double: FakeRunHost
  Given a fake host with an empty inbox
  When `send_message` runs with blank `from_agent`
  Then it returns Err whose code is "bad_request"
  And a `check_inbox` preview for the target returns no messages
  When `send_message` runs with invalid priority "panic"
  Then it returns Err whose code is "bad_request"
  And no message is written

Scenario: dispatcher lists and routes the send_message tool
  Test:
    Package: agentd-surface
    Filter: dispatch_lists_and_routes_send_message_tool
  Level: surface unit
  Test Double: FakeRunHost
  Given the tool dispatcher
  When descriptors are listed
  Then `send_message` is present
  When dispatch runs `send_message` with `from` alias arguments
  Then it returns `ok` true and writes a message to the target inbox

Scenario: daemon tools/call send_message feeds the durable inbox
  Test:
    Package: agentd-bin
    Filter: daemon_router_tools_call_send_message_feeds_check_inbox
  Level: daemon integration
  Test Double: real SqliteStore, fake backend, HTTP router
  Given the daemon has an empty database
  When `/tools/call` invokes `send_message`
  Then the response contains `ok` true and the message id
  When `/tools/call` invokes `check_inbox` for the target with drain
  Then the returned `dm` row contains the sent summary and full body
  And a second inbox preview returns no unread messages

Scenario: stdio tools/list advertises send_message input schema
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_tools_list_advertises_send_message_schema
  Level: MCP stdio unit
  Test Double: production host with temp SQLite and fake backend
  Given the stdio MCP handler
  When `tools/list` is requested
  Then the response contains `send_message`
  And its schema requires `from_agent`, `to`, `summary`, and `full`
  And its schema enumerates `type` and `priority` values

Scenario: parity map records p218 send_message progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p218_send_message_progress
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the agent-chat replacement parity map
  When the `messaging_inbox` row is inspected
  Then its status is "partial"
  And its decision mentions p218 direct `send_message`, explicit sender,
  and remaining implicit identity, group messaging, `post`, `check_group`,
  attachments, Matrix/remote relay, notification gates, and message import gaps
