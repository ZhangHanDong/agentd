spec: task
name: "Real agent stdio smoke wiring"
tags: [e2e, p0.9, mcp, stdio, tmux, real-agent]
---

## Intent

Close the next P0.9 real-environment gap after P122: a spawned default
Claude Code agent must start with an actual MCP client configuration for the
daemon-side `agentd mcp-stdio` server, not only with a prompt that mentions a
command. This slice makes the tmux launcher load a per-spawn Claude MCP config
and leaves the paid/authenticated real Claude smoke as an explicit operator
step over that wiring.

## Decisions

- The default production writer agent remains `CliKind::ClaudeCode`.
- When `AGENTD_MCP_STDIO_CMD` is present for a Claude Code spawn, the launcher
  writes a per-agent MCP config file in the worktree and starts `claude` with
  `--mcp-config <file>` plus `--strict-mcp-config`.
- The config file uses a single server named `agentd` with Claude's project
  `.mcp.json` shape: `mcpServers.agentd.type = "stdio"`, command `sh`, and
  args `["-lc", <AGENTD_MCP_STDIO_CMD>]`.
- The generated MCP config file is excluded from git via local `info/exclude`
  alongside the launcher script, without modifying tracked `.gitignore`.
- Absence of `AGENTD_MCP_STDIO_CMD` keeps existing tmux launcher behavior.

## Boundaries

### Allowed Changes
- specs/e2e/p123-real-agent-stdio-smoke.spec.md
- crates/agentd-tmux/**
- crates/agentd-bin/src/agent_mcp_context.rs
- crates/agentd-bin/tests/agent_mcp_context.rs
- docs/p0.9-deployment-checklist.md

### Forbidden
- Do not add a new agent-facing MCP tool.
- Do not replace the `agentd_surface::dispatch` registry.
- Do not require network access, Claude auth, or a live tmux server in automated
  tests.
- Do not change the default production writer CLI away from Claude Code in this
  slice.

## Completion Criteria

Scenario: Claude launcher loads a per-spawn agentd MCP config
  Test:
    Package: agentd-tmux
    Filter: spawn_launcher_configures_claude_mcp_when_stdio_command_is_present
  Given a Claude Code `SpawnRequest` contains `AGENTD_MCP_STDIO_CMD`
  When `TmuxBackend::spawn` writes the launcher
  Then the worktree contains a per-agent MCP config file
  And the config file defines `mcpServers.agentd` as stdio command `sh`
  And the config file passes `["-lc", <AGENTD_MCP_STDIO_CMD>]`
  And the launcher execs `claude --mcp-config <file> --strict-mcp-config`

Scenario: MCP config artifacts are excluded from git
  Test:
    Package: agentd-tmux
    Filter: spawn_git_exclude_excludes_launcher_and_mcp_config_artifacts
  Given a Claude Code `SpawnRequest` contains `AGENTD_MCP_STDIO_CMD`
  When `TmuxBackend::spawn` succeeds twice in the same worktree
  Then git `info/exclude` contains exactly one `.agentd-launcher-*.sh` line
  And git `info/exclude` contains exactly one `.agentd-mcp-*.json` line
  And tracked `.gitignore` is not created or modified

Scenario: Existing no-MCP launcher behavior is preserved
  Test:
    Package: agentd-tmux
    Filter: spawn_without_stdio_command_keeps_plain_claude_launcher
  Given a Claude Code `SpawnRequest` does not contain `AGENTD_MCP_STDIO_CMD`
  When `TmuxBackend::spawn` writes the launcher
  Then no per-agent MCP config file is written
  And the launcher execs plain `claude`

Scenario: Existing-session fast path writes no artifacts
  Test:
    Package: agentd-tmux
    Filter: spawn_existing_session_skips_mcp_config_artifacts
  Given a tmux session already exists for the requested agent
  When `TmuxBackend::spawn` returns the recoverable rebind error
  Then neither the launcher script nor the MCP config file exists in the worktree

Scenario: Startup prompt names the configured MCP server
  Test:
    Package: agentd-bin
    Filter: mcp_context_backend_prompt_names_agentd_server
  Given the production MCP context backend injects `AGENTD_MCP_STDIO_CMD`
  When it appends startup context to a spawned agent prompt
  Then the prompt names the `agentd` MCP server
  And the prompt still mentions `tools/list` and `tools/call`

## Out of Scope

- Running a paid/authenticated Claude Code network request in automated tests.
- Codex-specific one-shot MCP config flags; Codex remains unchanged until its
  launcher config shape is specified separately.
- Full `execute.dot` reviewer fan-out, PR creation, real SIGKILL, or the
  90-second MVP demo.
