spec: task
name: "cancellation-safe HTTP tool dispatch"
tags: [surface, http, tools, recovery, p154]
---

## Intent

Prevent a central daemon run from becoming stranded when an agent disconnects
after `submit_outcome` has been accepted but before the synchronous workflow
advancement triggered by that request returns. Once `/tools/call` dispatch
starts, daemon-owned work must continue independently of the client request.

## Decisions

- The HTTP `/tools/call` route starts dispatch in a daemon-owned Tokio task.
- The route awaits that task while the client remains connected and preserves
  the existing success and error response contract.
- Dropping the HTTP request future detaches the dispatch task instead of
  cancelling it.
- A dispatch task panic maps to the existing internal-error HTTP response while
  the client remains connected.

## Boundaries

### Allowed Changes

- specs/e2e/p154-cancellation-safe-http-tool-dispatch.spec.md
- crates/agentd-surface/src/http.rs
- crates/agentd-bin/tests/daemon_http.rs
- docs/superpowers/specs/2026-07-15-cancellation-safe-http-tool-dispatch-design.md
- docs/superpowers/plans/2026-07-15-cancellation-safe-http-tool-dispatch.md

### Forbidden

- Do not change workflow definitions or node retry policy.
- Do not add a database migration or new persisted state.
- Do not acknowledge `submit_outcome` before its normal dispatch result is
  available to a connected client.
- Do not invoke Claude in acceptance tests.

## Out of Scope

- Automatically resuming runs already stranded by an older daemon process.
- Making local in-process MCP dispatch survive termination of the daemon.
- Changing command timeout or process-tree termination behavior.

## Completion Criteria

Scenario: client cancellation does not cancel accepted workflow advancement
  Test:
    Package: agentd-bin
    Filter: http_tool_call_continues_after_client_cancellation
  Level: production host integration
  Test Double: blocking command runner and fake agent backend
  Given a run parked at an agent node followed by a blocking tool node
  When `/tools/call` accepts the agent outcome and enters the blocking tool
  And the client request future is cancelled before the tool returns
  Then releasing the tool still completes the daemon-owned dispatch
  And the run reaches its terminal node without resubmitting the agent outcome
