# Cancellation-safe HTTP tool dispatch design

## Problem

`POST /tools/call` currently awaits MCP dispatch directly in the Axum request
future. `submit_outcome` may persist the agent result and then synchronously run
several tool nodes before returning. If the client closes that request during a
long tool node, Axum drops the request future, which also drops command
execution. The checkpoint has already advanced past the agent park, so there is
no event left to resume the stranded tool node.

The P153 Codex-only real execute run `ad-e0-p153-20260715-r2` reproduced this:
the implement outcome and task-delta outcome were durable, the checkpoint moved
to `verify_lifecycle`, and closing the pending stdio proxy request removed the
tool child while leaving the run `running`.

## Design

The central HTTP route is the ownership boundary. It clones the `Arc<dyn
RunHost>`, moves the owned tool request into `tokio::spawn`, and awaits the join
handle for the normal response. Dropping the Axum request drops only the join
handle; Tokio keeps the spawned dispatch task alive.

Normal success and `SurfaceError` responses remain unchanged. A panic in the
spawned task is returned as the existing generic internal error when a client
is still waiting.

## Verification

A production-host integration test uses a workflow with an agent park followed
by a blocking tool. It waits until the tool starts, aborts the HTTP request,
releases the runner, and verifies both command completion and terminal run
state. The test fails against direct request-owned dispatch because aborting the
request drops the blocking command future.
