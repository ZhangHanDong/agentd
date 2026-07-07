spec: task
name: "Spawned agent MCP stdio startup context"
tags: [e2e, p0.9, mcp, stdio, spawn]
---

## Intent

P119 gives the daemon a stdio JSON-RPC entrypoint for the existing MCP
dispatcher, but a spawned agent still needs startup context before it can use
that entrypoint. This task wires two pieces of context: the production daemon
exports and prompts the exact `agentd mcp-stdio` command, while core park
handlers include the run/node identifiers the agent must use when reporting
outcomes or reviews.

## Decisions

- `agentd-bin` owns the stdio command injection at the production composition
  root by wrapping the selected `AgentBackend`; `agentd-core` only emits
  task/review identity in the prompt.
- The stdio command is rendered from the current `agentd` executable plus the
  shared `DaemonConfig` values needed by `mcp-stdio`: `--db-path`,
  `--workflows-dir`, `--repo-dir`, `--worktree-base`, and `--log-level error`.
- Relative config paths are made absolute against the daemon cwd before being
  placed in the command, because spawned agents run from allocated worktrees.
- Shell values in the command string are POSIX single-quoted so paths containing
  spaces or apostrophes remain copy/paste safe.
- The backend wrapper exports `AGENTD_MCP_STDIO_CMD` and appends an
  `agentd_mcp_stdio` prompt block without replacing any existing prompt text or
  caller-provided environment values.
- `codergen` prompt text includes `agentd_run_id`, `agentd_node_id`,
  `agentd_agent_id`, `agentd_task_run_id`, and explicit `submit_outcome`
  guidance for the existing `tools/call` JSON-RPC method.
- `parallel.fan_out` reviewer prompt text includes `agentd_run_id`,
  `agentd_node_id`, `agentd_reviewer_id`, `agentd_review_run_id`, and explicit
  `submit_review` guidance for the existing `tools/call` JSON-RPC method.
- Keep the existing surface tool schemas unchanged; this task only makes the
  already shipped tools discoverable and usable from spawned agent context.

## Boundaries

### Allowed Changes

- specs/e2e/p120-agent-mcp-stdio-startup-context.spec.md
- crates/agentd-bin/src/agent_mcp_context.rs
- crates/agentd-bin/src/daemon.rs
- crates/agentd-bin/src/lib.rs
- crates/agentd-bin/tests/agent_mcp_context.rs
- crates/agentd-core/src/handler/codergen.rs
- crates/agentd-core/src/handler/fan_out.rs
- crates/agentd-core/tests/handlers_park.rs

### Forbidden

- Do not modify crates/agentd-store/**.
- Do not modify crates/agentd-surface/**.
- Do not modify crates/agentd-tmux/**.
- Do not add new dependencies.
- Do not change shipped workflow DOT files.
- Do not start tmux, spawn a real agent, or require a real MCP client in tests.

## Out of Scope

- Replacing the dependency-free stdio harness with the external `rmcp` crate.
- Changing `assign_task` ownership semantics or task-run schema.
- Review bundle files, Matrix/Mempal live integration, or a real-agent smoke.
- Teaching agents to parse arbitrary prompt text beyond the explicit keys added
  here.

## Completion Criteria

Scenario: stdio command rendering is absolute and shell-safe
  Test: mcp_stdio_command_uses_absolute_config_paths_and_shell_quotes
  Level: unit command renderer
  Given a daemon config with relative paths and a daemon cwd containing spaces
  When the stdio command is rendered for a known agentd executable path
  Then every config path in the command is absolute
  And shell-special values are POSIX single-quoted
  And the command ends with `mcp-stdio`

Scenario: backend wrapper exports the command and appends prompt context
  Test: mcp_context_backend_exports_command_and_appends_prompt
  Level: unit backend wrapper
  Given a SpawnRequest with an existing prompt and an existing env override
  When the MCP context backend spawns through a recording backend
  Then the forwarded request keeps the existing prompt and env override
  And it adds `AGENTD_MCP_STDIO_CMD`
  And its prompt contains `agentd_mcp_stdio`, `tools/list`, and `tools/call`

Scenario: backend wrapper creates prompt context when none exists
  Test: mcp_context_backend_creates_prompt_when_missing
  Level: unit backend wrapper
  Given a SpawnRequest with no initial prompt
  When the MCP context backend spawns through a recording backend
  Then the forwarded request has an initial prompt
  And the prompt contains the stdio command and `AGENTD_MCP_STDIO_CMD`

Scenario: codergen prompt includes outcome submission identity
  Test: codergen_prompt_includes_outcome_submission_context
  Level: core handler unit
  Given a codergen node with a run id and role
  When the handler runs and parks for an agent outcome
  Then the spawned prompt contains `agentd_run_id`, `agentd_node_id`,
  `agentd_agent_id`, `agentd_task_run_id`, and `submit_outcome`

Scenario: fan_out prompt includes review submission identity
  Test: fan_out_prompt_includes_review_submission_context
  Level: core handler unit
  Given a fan_out node with two reviewers
  When the handler runs and parks for review verdicts
  Then each reviewer prompt contains `agentd_run_id`, `agentd_node_id`,
  that reviewer's `agentd_reviewer_id`, the shared `agentd_review_run_id`, and
  `submit_review`
