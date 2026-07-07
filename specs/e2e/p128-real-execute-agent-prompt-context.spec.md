spec: task
name: "real execute agent prompt context"
tags: [e2e, workflow, p2, execute, prompt]
---

## Intent

Make real `execute.dot` agents able to find the frozen spec, generated plan, and
implementation worktree from their initial prompts. The daemon keeps tool nodes
running in its cwd and agents still run inside allocated git worktrees, so the
prompt must state how to resolve `.agentd/run/*` runtime paths and what each
agent is expected to review or implement.

## Decisions

- `codergen` prompts include `agentd_daemon_cwd` and a runtime-path rule stating
  that relative runtime paths are resolved from the daemon cwd, while code
  changes happen in the current worktree.
- `codergen` prompts include a short role task telling the agent to read listed
  inputs, complete the node role, and submit the outcome through MCP.
- `parallel.fan_out` reviewer prompts include `agentd_daemon_cwd`, `spec_path`,
  `plan_path`, the implementation worktree from run context, and the reviewer
  worktree used for that reviewer process.
- Reviewer prompts include a short review task telling the agent to review the
  current worktree against the listed spec/plan and submit `pass|concern|blocker`.

## Boundaries

### Allowed Changes

- crates/agentd-core/src/handler/codergen.rs
- crates/agentd-core/src/handler/fan_out.rs
- crates/agentd-core/tests/handlers_park.rs
- specs/e2e/p128-real-execute-agent-prompt-context.spec.md

### Forbidden

- Do not add schema columns for prompt context.
- Do not change tool-node cwd behavior.
- Do not change reviewer aggregation semantics or review verdict values.
- Do not change `execute.dot` topology in this slice.

## Out of Scope

- Running real Claude reviewers or creating a real PR.
- Copying `.agentd/run/*` files into every allocated worktree.
- Persisting prompt bodies to the database.

## Completion Criteria

Scenario: implementer prompt explains daemon-relative runtime paths
  Test:
    Package: agentd-core
    Filter: codergen_prompt_explains_daemon_relative_runtime_paths
  Level: core handler unit
  Test Double: FakeBackend + HandlerCtx
  Given a codergen node includes `spec_path` and `plan_path`
  When the handler spawns the implementer
  Then the prompt includes `agentd_daemon_cwd`
  And it states that relative runtime paths resolve from the daemon cwd
  And it tells the agent to complete the node role before submitting outcome

Scenario: reviewer prompt includes spec, plan, and worktree context
  Test:
    Package: agentd-core
    Filter: fan_out_prompt_includes_review_runtime_context
  Level: core handler unit
  Test Double: FakeBackend + fake WorktreeAllocator
  Given fan_out context contains `spec_path`, `plan_path`, and the implementation `worktree`
  When reviewer snapshot worktrees are allocated and reviewers spawn
  Then each reviewer prompt includes `agentd_daemon_cwd`
  And it includes the spec path, plan path, implementation worktree, and that reviewer worktree
  And it tells the reviewer to submit `pass|concern|blocker`
