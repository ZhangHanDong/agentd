spec: task
name: "Codex launcher permits local daemon proxy"
tags: [agent-chat-replacement, real-execute, codex, tmux, p212]
---

## Intent

The p204 r8 Codex-only real execute smoke proved that p211's central daemon
proxy path is wired into spawned agents, but the implementer could not submit
`tools/call` through `mcp-stdio --proxy-url http://127.0.0.1:18789`: Codex's
default command sandbox blocked the helper's loopback connection with
`Operation not permitted`. Falling back to local stdio dispatch reached the old
readonly SQLite failure. Codex real-agent launchers need an explicit sandbox
mode that permits local daemon proxy I/O while keeping unattended approval
behavior and without using the broad bypass flag.

## Decisions

- Managed Codex launchers include `--ask-for-approval never --sandbox
  danger-full-access`.
- The launcher still must not include
  `--dangerously-bypass-approvals-and-sandbox`.
- The change applies to both plain Codex launchers and Codex launchers with
  agentd MCP config overrides.
- This slice does not change Claude launchers, publish/open-pr helpers, or the
  smoke artifact task.

## Boundaries

### Allowed Changes

- specs/e2e/p212-codex-launcher-sandbox-for-local-daemon-proxy.spec.md
- specs/e2e/p202-codex-mcp-launcher-parity.spec.md
- specs/e2e/p207-codex-unattended-launcher.spec.md
- crates/agentd-tmux/src/backend.rs
- crates/agentd-tmux/tests/spawn.rs
- docs/plans/p204-real-codex-execute-smoke-gate.spec.md

### Forbidden

- Do not run real Claude.
- Do not use `--dangerously-bypass-approvals-and-sandbox`.
- Do not change publish/open-pr helper behavior in this slice.
- Do not weaken p204's requirement that real Codex execute must reach
  `finished`.

## Out of Scope

- Retrying the real smoke gate.
- Replacing the HTTP proxy transport.
- Changing the frozen smoke task.

## Completion Criteria

Scenario: Codex launcher permits local proxy while avoiding bypass
  Test:
    Package: agentd-tmux
    Filter: spawn_launcher_execs_codex_for_codex_cli
  Level: launcher unit
  Test Double: RecordingCommandRunner
  Given a Codex spawn request
  When the launcher script is written
  Then it execs `codex --ask-for-approval never --sandbox danger-full-access`
  And it does not include `--dangerously-bypass-approvals-and-sandbox`

Scenario: Codex MCP launcher permits local proxy
  Test:
    Package: agentd-tmux
    Filter: spawn_launcher_configures_codex_mcp_with_config_overrides_when_stdio_command_is_present
  Level: launcher unit
  Test Double: RecordingCommandRunner
  Given a Codex spawn request with `AGENTD_MCP_STDIO_CMD`
  When the launcher script is written
  Then it includes `--ask-for-approval never --sandbox danger-full-access`
  And it keeps the agentd MCP command, args, and per-tool approval overrides

Scenario: plain Codex launcher carries sandbox mode
  Test:
    Package: agentd-tmux
    Filter: spawn_launcher_execs_plain_codex_without_stdio_command
  Level: launcher unit
  Test Double: RecordingCommandRunner
  Given a Codex spawn request without `AGENTD_MCP_STDIO_CMD`
  When the launcher script is written
  Then it includes `--ask-for-approval never --sandbox danger-full-access`
  And it does not configure agentd MCP
