spec: task
name: "Daemon stdio MCP dispatcher entrypoint"
tags: [e2e, p0.9, mcp, stdio]
---

## Intent

The P0.9 real-agent checklist still lacks the process boundary between a spawned
agent and agentd's existing MCP tool dispatcher. This task adds the daemon-side
stdio entrypoint that a local agent process can be pointed at: JSON-RPC requests
on stdin reach the production `RunHost` through the existing
`agentd_surface::mcp_server::dispatch` path, and responses are written to stdout.

## Decisions

- Add an `agentd mcp-stdio` maintenance subcommand that reuses the shared
  `DaemonConfig`, builds the same production host as the HTTP daemon, and serves
  stdin/stdout until EOF.
- Keep this slice line-delimited JSON-RPC over stdio and dependency-free inside
  `agentd-bin`; adopting or upgrading the external `rmcp` crate is a follow-up
  compatibility task after the local version target is settled.
- Add a pure request handler helper so tests can exercise `tools/list`,
  `tools/call`, and error responses without opening sockets, tmux, or a real
  agent process.
- `tools/list` returns the five existing tool descriptors from
  `tool_descriptors()`.
- `tools/call` accepts MCP-style params `{ "name": "...", "arguments": {...} }`
  and routes to the existing dispatcher; it does not invent a second tool
  registry.
- JSON-RPC responses preserve the request `id`. Unknown methods return
  `code=-32601`; malformed or missing params return `code=-32602`; tool failures
  return `code=-32000` with `data.code` set to the existing `SurfaceError::code()`.

## Boundaries

### Allowed Changes

- specs/e2e/p119-stdio-mcp-dispatch.spec.md
- crates/agentd-bin/src/cli.rs
- crates/agentd-bin/src/lib.rs
- crates/agentd-bin/src/main.rs
- crates/agentd-bin/src/stdio_mcp.rs
- crates/agentd-bin/tests/mcp_stdio.rs

### Forbidden

- Do not modify crates/agentd-core/**.
- Do not modify crates/agentd-store/**.
- Do not modify crates/agentd-surface/**.
- Do not modify crates/agentd-tmux/**.
- Do not add new dependencies.
- Do not start tmux, spawn a real agent, or require a real MCP client in tests.

## Out of Scope

- Wiring spawned agents' initial prompts to this command.
- Replacing the dependency-free stdio harness with the external `rmcp` crate.
- Streaming notifications, progress, cancellation, or multi-message batching.
- Mempal, Matrix, Specify, or GitHub integration.

## Completion Criteria

Scenario: CLI parses the stdio subcommand with shared daemon options
  Test: agentd_cli_mcp_stdio_accepts_shared_options
  Level: unit parser
  Given the `agentd` CLI parser
  When `agentd --db-path state.db --workflows-dir workflows mcp-stdio` is parsed
  Then the command is `mcp-stdio`
  And the shared daemon options are preserved

Scenario: tools/list exposes the existing five tool descriptors
  Test: mcp_stdio_tools_list_returns_registered_tools
  Level: in-process JSON-RPC handler
  Given a production RunHost assembled with fakes
  When a JSON-RPC `tools/list` request is handled
  Then the response has `jsonrpc="2.0"` and the same `id`
  And the result contains exactly assign_task, submit_outcome, submit_review, check_inbox, and query_run

Scenario: tools/call reaches the existing dispatcher
  Test: mcp_stdio_tools_call_routes_to_dispatch
  Level: in-process JSON-RPC handler
  Given a production RunHost with a started draft run
  When a JSON-RPC `tools/call` request invokes `query_run`
  Then the response result contains the run snapshot parked at `propose_spec`

Scenario: unknown JSON-RPC methods are rejected
  Test: mcp_stdio_unknown_method_returns_json_rpc_error
  Level: in-process JSON-RPC handler
  Given a production RunHost assembled with fakes
  When a JSON-RPC request uses method `resources/list`
  Then the response error code is `-32601`
  And the response preserves the request id

Scenario: tool failures preserve the surface error code
  Test: mcp_stdio_tool_failure_preserves_surface_code
  Level: in-process JSON-RPC handler
  Given a production RunHost assembled with fakes
  When a JSON-RPC `tools/call` request invokes `query_run` for an unknown run
  Then the response error code is `-32000`
  And `error.data.code` is `not_found`

Scenario: stdio loop writes JSON-RPC responses to stdout
  Test: mcp_stdio_loop_writes_json_lines_to_stdout
  Level: in-memory stdio loop
  Given an in-memory stdin stream containing one line-delimited `tools/list` request
  When the stdio loop drains that stream
  Then stdout contains one JSON-RPC response line
  And the response line contains no tracing text or non-JSON prefix
