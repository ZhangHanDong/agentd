spec: task
name: "MCP stdio proxies central daemon"
tags: [agent-chat-replacement, real-execute, mcp, daemon, p211]
---

## Intent

The p204 r7 Codex-only real execute smoke reached implementation, lifecycle
verification, and three passing Codex reviewer verdicts. After p210 fixed
transient cwd inheritance, the run advanced to `publish_branch` but hung in
`git push` asking for `https://github.com` credentials. The publish command was
running inside the reviewer Codex `mcp-stdio` process, so it did not have the
operator daemon process's GitHub credential environment. Agent-facing MCP stdio
must submit events to the central daemon, not continue the workflow locally.

## Decisions

- The daemon HTTP surface exposes a JSON `POST /tools/call` endpoint that routes
  through the same agentd tool dispatcher as stdio MCP.
- Spawned agents receive an `mcp-stdio --proxy-url http://127.0.0.1:<port>`
  command, so their stdio MCP server proxies `tools/call` to the central daemon.
- In proxy mode, stdio MCP still answers `initialize` and `tools/list` locally,
  but `tools/call` is executed by the central daemon process.
- This slice keeps the existing local-host stdio mode for tests and offline
  compatibility when no proxy URL is supplied.

## Boundaries

### Allowed Changes

- specs/e2e/p211-mcp-stdio-proxies-central-daemon.spec.md
- crates/agentd-bin/src/agent_mcp_context.rs
- crates/agentd-bin/src/cli.rs
- crates/agentd-bin/src/main.rs
- crates/agentd-bin/src/stdio_mcp.rs
- crates/agentd-bin/tests/agent_mcp_context.rs
- crates/agentd-bin/tests/mcp_stdio.rs
- crates/agentd-bin/tests/daemon_http.rs
- crates/agentd-surface/src/http.rs
- docs/plans/p204-real-codex-execute-smoke-gate.spec.md

### Forbidden

- Do not run real Claude in tests.
- Do not add a GitHub token to prompts, logs, or state files.
- Do not change publish/open-pr helper behavior in this slice.
- Do not remove local stdio MCP dispatch mode.

## Out of Scope

- Retrying the real smoke gate.
- General remote relay or Matrix transport.
- Changing dashboard UI.
- Adding authentication to the local-only HTTP tool-call endpoint.

## Completion Criteria

Scenario: spawned MCP command proxies to daemon URL
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_command_includes_proxy_url_to_daemon
  Level: command rendering unit
  Test Double: static DaemonConfig
  Given a daemon config with port 8787
  When the agent MCP stdio command is rendered
  Then it ends with `mcp-stdio --proxy-url 'http://127.0.0.1:8787'`
  And config paths remain absolute and shell-quoted

Scenario: daemon HTTP tools call routes through central host
  Test:
    Package: agentd-bin
    Filter: daemon_router_tools_call_routes_to_dispatch
  Level: HTTP router integration
  Test Double: real SqliteStore, fake backend, recording command runner
  Given a central daemon router with a parked draft run
  When `POST /tools/call` calls `query_run`
  Then the response is 200
  And the JSON response reports the parked node from the central host

Scenario: stdio proxy forwards tools/call over HTTP
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_proxy_tools_call_posts_to_http_daemon
  Level: stdio MCP proxy unit
  Test Double: loopback HTTP test listener
  Given a proxy URL pointing at a test HTTP listener
  When stdio MCP handles a `tools/call` request
  Then it POSTs `/tools/call` with the tool name and arguments
  And the JSON-RPC response wraps the HTTP JSON as structured MCP content
