spec: task
name: "Codex agentd MCP tool approval"
tags: [agent-chat-replacement, real-execute, codex, mcp, p208]
---

## Intent

The p204 Codex-only real execute smoke reached `submit_outcome`, but Codex
stopped at an MCP tool approval prompt for the managed `agentd` server. agentd
launches that MCP server per run against the run database, so the Codex launcher
should approve the agentd tool calls needed for outcome and review submission
without mutating the user's global Codex configuration.

## Decisions

- When `AGENTD_MCP_STDIO_CMD` is present for a Codex spawn, the launcher passes
  per-launch `mcp_servers.agentd.tools.<tool>.approval_mode="approve"`
  overrides for all agentd MCP tools.
- The approved agentd tools are `assign_task`, `submit_outcome`,
  `submit_review`, `check_inbox`, and `query_run`.
- Plain Codex launches without `AGENTD_MCP_STDIO_CMD` do not include any
  `mcp_servers.agentd` overrides.
- The launcher still does not use `--dangerously-bypass-approvals-and-sandbox`
  or mutate `CODEX_HOME`.

## Boundaries

### Allowed Changes

- specs/e2e/p208-codex-agentd-mcp-tool-approval.spec.md
- crates/agentd-tmux/src/backend.rs
- crates/agentd-tmux/tests/spawn.rs

### Forbidden

- Do not start real Claude, Codex, tmux, Matrix, or remote relay processes in
  tests.
- Do not use `--dangerously-bypass-approvals-and-sandbox`.
- Do not mutate the user's Codex home or global Codex config.
- Do not change Claude launcher flags.
- Do not change `scripts/agentd_real_execute_smoke.sh`.

## Out of Scope

- Changing the agentd MCP tool schema.
- Switching to `codex exec` non-interactive mode.
- Granting approvals for user/global MCP servers such as playwright or shadcn.

## Completion Criteria

Scenario: Codex agentd MCP tools are approved per launch
  Test:
    Package: agentd-tmux
    Filter: spawn_launcher_configures_codex_mcp_with_config_overrides_when_stdio_command_is_present
  Level: tmux launcher artifact
  Test Double: RecordingCommandRunner
  Given a Codex spawn request with `AGENTD_MCP_STDIO_CMD`
  When the tmux backend writes the launcher
  Then the launcher includes approval overrides for `assign_task`, `submit_outcome`, `submit_review`, `check_inbox`, and `query_run`
  And it still passes the per-launch `mcp_servers.agentd.command` and `mcp_servers.agentd.args` overrides

Scenario: plain Codex launch does not grant agentd MCP approvals
  Test:
    Package: agentd-tmux
    Filter: spawn_launcher_execs_plain_codex_without_stdio_command
  Level: tmux launcher artifact
  Test Double: RecordingCommandRunner
  Given a Codex spawn request without `AGENTD_MCP_STDIO_CMD`
  When the tmux backend writes the launcher
  Then it does not include any `mcp_servers.agentd` override
