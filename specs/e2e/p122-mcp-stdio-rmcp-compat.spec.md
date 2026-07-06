spec: task
name: "MCP stdio rmcp 2.x compatibility"
tags: [e2e, p0.9, mcp, stdio, rmcp, compatibility]
---

## Intent

P119 exposed a dependency-free JSON-RPC stdio entrypoint and P120 taught spawned
agents how to find it, but real MCP clients expect the MCP lifecycle and tool
payload shapes, not only raw `tools/list` and `tools/call`. This task makes the
daemon-side stdio boundary compatible with the current `rmcp` model layer while
preserving the existing `agentd_surface::dispatch` tool registry.

## Decisions

- Bump the workspace `rmcp` dependency from the old declared-but-unused `0.1`
  line to `2.1`, disable default features, enable only `server`, and make
  `agentd-bin` depend on it for model compatibility.
- Keep `agentd-surface` transport-agnostic; do not move the dispatcher or tool
  handlers behind an `rmcp` server abstraction in this slice.
- Support the MCP `initialize` request with `protocolVersion`, `capabilities`
  containing `tools`, `serverInfo`, and human-readable `instructions`.
- Accept the `notifications/initialized` notification without writing a response
  line, because MCP notifications do not have JSON-RPC ids.
- Return `tools/list` entries with MCP `inputSchema` objects for all five
  agentd tools; every schema has root type `object`.
- Return successful `tools/call` responses as MCP `CallToolResult` objects:
  `content` contains one text block, `structuredContent` contains the dispatcher
  JSON result, and `isError` is `false`.
- Preserve protocol-level JSON-RPC errors for malformed requests, unknown
  methods, and dispatcher failures; do not silently turn those into successful
  tool-level errors in this compatibility slice.

## Boundaries

### Allowed Changes

- **/Cargo.toml
- **/Cargo.lock
- docs/p0.9-deployment-checklist.md
- specs/e2e/p122-mcp-stdio-rmcp-compat.spec.md
- crates/agentd-bin/Cargo.toml
- crates/agentd-bin/src/stdio_mcp.rs
- crates/agentd-bin/tests/mcp_stdio.rs

### Forbidden

- Do not modify crates/agentd-core/**.
- Do not modify crates/agentd-store/**.
- Do not modify crates/agentd-surface/**.
- Do not modify crates/agentd-tmux/**.
- Do not change the existing agentd tool input or output structs.
- Do not add a second tool registry.

## Out of Scope

- A full `rmcp::ServerHandler` rewrite.
- Real tmux agent smoke execution.
- Matrix or mempal live transport.
- Changing prompt wording from P120 beyond references already present there.
- HTTP/SSE behavior changes.

## Completion Criteria

Scenario: initialize returns rmcp-compatible server metadata
  Test: mcp_stdio_initialize_returns_server_capabilities
  Level: agentd-bin stdio request handler unit
  Targets: crates/agentd-bin/src/stdio_mcp.rs
  Given a JSON-RPC MCP `initialize` request
  When the stdio request handler handles it
  Then the response result contains `protocolVersion`, `capabilities.tools`,
  `serverInfo.name`, and `instructions`
  And the `protocolVersion` matches `rmcp::model::ProtocolVersion::LATEST`

Scenario: initialized notification writes no response line
  Test: mcp_stdio_loop_ignores_initialized_notification
  Level: agentd-bin stdio loop unit
  Targets: crates/agentd-bin/src/stdio_mcp.rs and crates/agentd-bin/tests/mcp_stdio.rs
  Given an input stream containing a `notifications/initialized` notification
  When the stdio loop serves the stream
  Then stdout is empty

Scenario: tools/list includes MCP input schemas
  Test: mcp_stdio_tools_list_includes_input_schemas
  Level: agentd-bin stdio request handler unit
  Targets: crates/agentd-bin/src/stdio_mcp.rs
  Given a JSON-RPC `tools/list` request
  When the stdio request handler handles it
  Then it returns the five agentd tools
  And every tool has an `inputSchema` with root type `object`
  And `submit_outcome` declares `run_id`, `node_id`, `attempt`, and `status`

Scenario: tools/call returns MCP CallToolResult shape
  Test: mcp_stdio_tools_call_returns_call_tool_result_shape
  Level: production RunHost + stdio request handler integration
  Test Double: real SqliteStore with FakeBackend and RecordingCommandRunner
  Given a started draft run
  When a JSON-RPC `tools/call` request invokes `query_run`
  Then the response result contains text `content`
  And `structuredContent.current_node` is `propose_spec`
  And `isError` is `false`

Scenario: rmcp workspace dependency is version-aligned without macro defaults
  Test: rmcp_workspace_dependency_is_version_aligned
  Given the workspace Cargo.toml and agentd-bin Cargo.toml
  When the files are read as text
  Then the workspace dependency uses version `2.1`
  And it disables default features
  And it enables only the `server` feature
  And `agentd-bin` depends on workspace `rmcp`
