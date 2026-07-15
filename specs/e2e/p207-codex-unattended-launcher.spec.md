spec: task
name: "Codex unattended launcher"
tags: [agent-chat-replacement, real-execute, codex, tmux, p207]
---

## Intent

The p204 Codex-only real execute smoke reached the implementer task after p206,
but Codex stopped at an interactive edit confirmation prompt. agentd-spawned
Codex agents must be able to finish isolated worktree tasks without a human
pressing `y`, so the launcher should select Codex's supported non-approval
policy for these managed sessions.

## Decisions

- Every Codex launcher command includes `--ask-for-approval never --sandbox
  danger-full-access` so managed agents can call the local daemon proxy without
  an interactive approval prompt.
- The launcher does not use `--dangerously-bypass-approvals-and-sandbox`.
- Codex MCP configuration from p202 remains per launch via `-c
  mcp_servers.agentd.*` overrides and still avoids persistent user config
  mutation.
- Claude launcher behavior is unchanged.

## Boundaries

### Allowed Changes

- specs/e2e/p207-codex-unattended-launcher.spec.md
- specs/e2e/p202-codex-mcp-launcher-parity.spec.md
- crates/agentd-tmux/src/backend.rs
- crates/agentd-tmux/tests/spawn.rs

### Forbidden

- Do not start real Claude, Codex, tmux, Matrix, or remote relay processes in
  tests.
- Do not use `--dangerously-bypass-approvals-and-sandbox`.
- Do not mutate the user's Codex home or global Codex config.
- Do not change Claude launcher flags.
- Do not change p201 role-prefix runtime selection semantics.

## Out of Scope

- Switching to `codex exec` non-interactive mode.
- Changing model, profile, login, or user MCP server configuration.
- Proving the p204 real execute smoke succeeds; this slice only fixes launcher
  artifacts.

## Completion Criteria

Scenario: Codex launcher disables approval prompts
  Test:
    Package: agentd-tmux
    Filter: spawn_launcher_execs_codex_for_codex_cli
  Level: tmux launcher artifact
  Test Double: RecordingCommandRunner
  Given a Codex spawn request
  When the tmux backend writes the launcher
  Then the launcher execs `codex --ask-for-approval never --sandbox danger-full-access`
  And it does not include `--dangerously-bypass-approvals-and-sandbox`

Scenario: Codex MCP launcher remains unattended
  Test:
    Package: agentd-tmux
    Filter: spawn_launcher_configures_codex_mcp_with_config_overrides_when_stdio_command_is_present
  Level: tmux launcher artifact
  Test Double: RecordingCommandRunner
  Given a Codex spawn request with `AGENTD_MCP_STDIO_CMD`
  When the tmux backend writes the launcher
  Then the launcher includes `--ask-for-approval never --sandbox danger-full-access`
  And it still passes the per-launch `mcp_servers.agentd.*` overrides
  And it does not write a Claude-style MCP JSON file

Scenario: plain Codex launcher stays MCP-free
  Test:
    Package: agentd-tmux
    Filter: spawn_launcher_execs_plain_codex_without_stdio_command
  Level: tmux launcher artifact
  Test Double: RecordingCommandRunner
  Given a Codex spawn request without `AGENTD_MCP_STDIO_CMD`
  When the tmux backend writes the launcher
  Then it includes `--ask-for-approval never --sandbox danger-full-access`
  And it does not include any `mcp_servers.agentd` override
