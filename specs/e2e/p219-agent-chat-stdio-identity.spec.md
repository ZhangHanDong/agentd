spec: task
name: "agent-chat stdio MCP identity baseline"
tags: [agent-chat-replacement, messaging, mcp, identity, phase-d, p219]
---

## Intent

Make each agent-facing `agentd mcp-stdio` session identity-bound so direct
messaging can match agent-chat's collaboration model without agents manually
passing their sender name. This continues Phase D after p218 by letting the
stdio session fill `send_message.from_agent` and `check_inbox.agent_id` from a
known agent identity while preserving explicit HTTP `/tools/call` behavior for
operator or bridge callers.

## Decisions

- `agentd mcp-stdio` accepts an optional `--agent-id` identity flag.
- The stdio identity fallback order is `--agent-id`, then `AGENTD_AGENT_ID`,
  then `AGENTD_AGENT_NAME`, then `AGENT_NAME` for agent-chat compatibility.
- The agent launcher context appends `--agent-id '<spawn agent id>'` to each
  spawned agent's `AGENTD_MCP_STDIO_CMD`, so different agents do not share a
  generic sender command.
- When a stdio session has an identity, `tools/list` marks `send_message` as
  requiring only `to`, `summary`, and `full`, and marks `check_inbox` as not
  requiring `agent_id`.
- When a stdio session has an identity, `tools/call send_message` without
  `from_agent` or `from` writes a message whose sender is that identity.
- When a stdio session has an identity, `tools/call check_inbox` without
  `agent_id` reads that identity's direct inbox.
- Identity-bound stdio sessions reject attempts to send as or read inbox for a
  different agent instead of silently spoofing.
- Proxy mode injects the same identity into the forwarded `/tools/call` body
  before it reaches the central daemon.
- Direct central daemon `/tools/call` without an identity remains compatible
  with p218 and still requires explicit sender or inbox agent id.
- The parity map records p219 implicit stdio identity progress; messaging
  remains partial until group messaging, mentions, attachments, Matrix/remote
  relay, notification gates, and message import/shadow coverage are complete.

## Boundaries

### Allowed Changes

- specs/e2e/p219-agent-chat-stdio-identity.spec.md
- crates/agentd-bin/src/cli.rs
- crates/agentd-bin/src/main.rs
- crates/agentd-bin/src/stdio_mcp.rs
- crates/agentd-bin/src/agent_mcp_context.rs
- crates/agentd-bin/tests/mcp_stdio.rs
- crates/agentd-bin/tests/agent_mcp_context.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not modify the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout or any source JSON file.
- Do not run real Claude.
- Do not add third-party crates that are not already workspace dependencies.
- Do not add MCP `post`, `check_group`, group mentions, attachments, Matrix
  relay, remote relay, notification queue/gate, or task graph behavior in this
  slice.
- Do not make `/tools/call` trust arbitrary HTTP headers as sender identity in
  this slice.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- tmux session or pane TTY auto-detection for identity.
- Per-agent token provisioning or token-to-identity inference for direct HTTP
  `/tools/call`.
- Group `post`, `check_group`, @mentions, attachments, and structured message
  `schema` payloads.
- Message import, migration shadow audits for messages, service cutover, and
  rollback automation.
- Push notification delivery, tmux injection, inbox gates, and relay ack state.

## Completion Criteria

<!-- lint-ack: decision-coverage - p219 binds identity source precedence, launcher injection, schema changes, local/proxy injection, spoof rejection, and parity documentation through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify CLI/env behavior, generated command text, stdio schema, local durable side effects, proxy body side effects, spoof rejection, and docs. -->

Scenario: mcp-stdio accepts explicit agent identity
  Test:
    Package: agentd-bin
    Filter: agentd_cli_mcp_stdio_accepts_agent_id
  Level: CLI unit
  Given the agentd CLI parser
  When `agentd mcp-stdio --agent-id codex-worker` is parsed
  Then the command is `mcp-stdio`
  And the parsed identity is "codex-worker"

Scenario: launcher command is bound to the spawned agent id
  Test:
    Package: agentd-bin
    Filter: mcp_context_backend_exports_agent_bound_stdio_command
  Level: launcher unit
  Test Double: RecordingBackend
  Given an agent spawn request for "implementer"
  When the MCP stdio context backend injects the command
  Then `AGENTD_MCP_STDIO_CMD` contains `--agent-id 'implementer'`
  And the prompt shows the same identity-bound command

Scenario: identity-bound tools/list makes sender and inbox agent implicit
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_tools_list_with_identity_makes_send_and_inbox_identity_implicit
  Level: MCP stdio unit
  Test Double: production host with temp SQLite and fake backend
  Given a stdio MCP handler bound to "codex-worker"
  When `tools/list` is requested
  Then `send_message` does not require `from_agent`
  And `check_inbox` does not require `agent_id`

Scenario: identity-bound stdio sends without from_agent and reads own inbox
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_identity_sends_without_from_and_reads_own_inbox
  Level: MCP stdio integration
  Test Double: production host with temp SQLite and fake backend
  Given a stdio MCP handler bound to "codex-worker"
  When it calls `send_message` to "codex-reviewer" without `from_agent`
  Then the stored message sender is "codex-worker"
  When a handler bound to "codex-reviewer" calls `check_inbox` without `agent_id`
  Then it receives the message

Scenario: identity-bound stdio rejects sender and inbox spoofing
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_identity_rejects_sender_and_inbox_spoofing
  Level: MCP stdio unit
  Test Double: production host with temp SQLite and fake backend
  Given a stdio MCP handler bound to "codex-worker"
  When it calls `send_message` with `from_agent` "other-agent"
  Then the JSON-RPC error has data code "bad_request"
  When it calls `check_inbox` with `agent_id` "other-agent"
  Then the JSON-RPC error has data code "bad_request"

Scenario: proxy mode injects identity before forwarding tools/call
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_proxy_injects_identity_into_forwarded_send_message
  Level: MCP stdio proxy unit
  Test Double: local TCP listener
  Given a proxy stdio handler bound to "codex-worker"
  When it forwards `send_message` without `from_agent`
  Then the HTTP request body sent to `/tools/call` contains `"from_agent":"codex-worker"`

Scenario: parity map records p219 implicit identity progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p219_stdio_identity_progress
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the agent-chat replacement parity map
  When the `messaging_inbox` row is inspected
  Then its status is "partial"
  And its decision mentions p219 stdio identity, implicit direct sender,
  implicit own inbox reads, spoof rejection, and remaining group messaging,
  attachments, Matrix/remote relay, notification gate, and message import gaps
