spec: task
name: "Handler trait + conditional and tool handlers"
tags: [core, mvp, p0, handlers]
---

## Intent

Define the `Handler` trait every node kind implements and the `HandlerCtx` the
engine threads into it, then land the two *synchronous* handlers — `conditional`
and `tool` — that return `HandlerStep::Done` without parking (D1). Park-style
handlers (wait.human/fan_out/fan_in/codergen) come in Task 8; the engine loop in
Task 9. A `HandlerRegistry` resolves a `HandlerKind` to an `Arc<dyn Handler>`,
which only works because the trait is object-safe via `#[async_trait]` (D4).

## Decisions

- `Handler` is `#[async_trait]`, `Send + Sync`, returns `Result<HandlerStep, CoreError>` from both `run` and a defaulted `resume` (default errors `CoreError::Invariant` — only park handlers override it).
- `HandlerCtx<'a>` carries: `run_id`, `&NodeGraph`, the current `&NodeDef`, a read-only `&RunContext`, a `Ports<'a>` bundle (`&dyn AgentBackend/CommandRunner/Store/MempalClient/Clock`), and a private staged `context_updates` map with `stage(k,v)` / `staged_updates()`.
- **Two context-update channels, one reconciliation rule (engine invariant, defined here, enforced in Task 9):**
  - *ctx-staged* (`HandlerCtx::stage`) = updates a handler computes locally before returning — including before a `Park`, so the Task 5 checkpoint's `context_snapshot` captures them.
  - *`Outcome.context_updates`* = updates arriving from outside (an agent's `submit_outcome` via MCP).
  - The engine merges ctx-staged into `RunContext` on **every** step (Done and Park) and additionally merges `Outcome.context_updates` on **Done**. A handler MUST NOT write the same key to both channels.
- `Ports<'a>` is a borrow bundle the engine builds once and threads to each node's `HandlerCtx`, keeping the constructor narrow.
- `conditional` (thin, per design §2.4 — edges already route): evaluate each outgoing edge's `condition=` against the run context via the Task 4 `eval_condition` (with a synthetic `Outcome::success()` — so scenario conditions use `kv("k")=="v"`, NOT `outcome=`/`answer=` which are meaningless on a branch node). Return `Done(Outcome::success())` with `preferred_label` = the first matching edge's `label` (falling back to its target id). If none match, use the first unconditional edge as the default branch. If none match and there is no default, return `Done(Outcome::fail())`.
- `tool`: read node `cmd` (split on whitespace into program + args) and optional `timeout_secs`; shell out via `CommandRunner`. Map result to `Status`: exit `0` → `Success`; non-zero exit → `Fail`; a `CommandError` (failed launch / timeout / killed — transient) → `Retry`. When `artifact_path=` is set on a `Success`, attach an `Artifact` pointer (kind `Transcript`, the declared path, `sha256` + byte-length of captured stdout). Synchronous — never parks. agentd-core writes no file here (pointer only, per design §3.1).

## Boundaries

### Allowed Changes

- crates/agentd-core/src/handler/{mod.rs,registry.rs,conditional.rs,tool.rs}
- crates/agentd-core/src/lib.rs
- crates/agentd-core/src/graph/node_graph.rs (add `Hash` to `HandlerKind` for registry keys)
- crates/agentd-core/tests/handlers.rs

### Forbidden

- Do not make `conditional` or `tool` park — they return `Done` synchronously (D1).
- Do not write the `artifact_path` file from agentd-core (I/O-free except checkpoints); record a pointer only.
- Do not let `Handler` become non-object-safe (no generics / `Self` returns) — the registry stores `Arc<dyn Handler>`.

## Completion Criteria

Scenario: A handler round-trips through the registry as a trait object
  Test: handler_trait_is_object_safe_in_registry
  Given a HandlerRegistry with the conditional handler registered
  When the handler is resolved by its HandlerKind and run on a context
  Then it returns Ok(HandlerStep::Done(_)) through the Arc<dyn Handler>

Scenario: conditional picks the first matching branch
  Test: conditional_picks_first_matching_branch
  Given a conditional node with two condition edges and a context where the first matches
  When the handler runs
  Then it returns Done(Success) with preferred_label equal to the first edge's label

Scenario: conditional fails when no branch matches and there is no default
  Test: conditional_returns_fail_when_no_branch_matches_and_no_default
  Given a conditional node whose only edges are conditioned and none match the context
  When the handler runs
  Then it returns Done with status Fail

Scenario: conditional uses the default branch when present
  Test: conditional_uses_default_branch_when_present
  Given a conditional node with one non-matching condition edge and one unconditional edge
  When the handler runs
  Then it returns Done(Success) with preferred_label equal to the unconditional edge's label

Scenario: tool captures stdout as an artifact pointer when artifact_path is set
  Test: tool_handler_captures_stdout_as_artifact_when_path_set
  Given a tool node with cmd and artifact_path and a runner scripted to exit 0 with stdout
  When the handler runs
  Then it returns Done(Success) with one Artifact whose path matches and whose bytes equal the stdout length

Scenario: tool maps a non-zero exit to Fail
  Test: tool_handler_maps_nonzero_exit_to_fail
  Given a tool node whose runner is scripted to exit non-zero
  When the handler runs
  Then it returns Done with status Fail

Scenario: tool maps a CommandError (transient, e.g. timeout) to Retry
  Test: tool_handler_maps_command_error_to_retry
  Given a tool node whose runner is scripted to return a CommandError
  When the handler runs
  Then it returns Done with status Retry
