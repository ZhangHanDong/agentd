spec: task
name: "Codex MCP launcher parity"
tags: [agent-chat-replacement, real-execute, codex, mcp, p202]
---

## Intent

Move Phase B closer to a real Codex-only execute run by giving Codex-spawned
agents the same agentd MCP callback path that Claude-spawned agents already
receive. p201 selected Codex by role prefix; this slice makes that selected
runtime able to discover `agentd mcp-stdio` without requiring a global Codex
configuration edit.

## Decisions

- Codex MCP configuration is per launch, not a persistent user config mutation:
  the launcher passes `codex -c mcp_servers.agentd.command=... -c
  mcp_servers.agentd.args=...`.
- The generated Codex command preserves the user's existing `CODEX_HOME`,
  profile, login, and default model settings by using config overrides instead
  of replacing the config directory. p207/p212 may add transient launcher flags
  for managed automation, but it still must not mutate persistent user config.
- Claude keeps the existing generated `.agentd-mcp-*.json` plus
  `--mcp-config ... --strict-mcp-config` path.
- Empty `AGENTD_MCP_STDIO_CMD` remains invalid for every runtime.
- p202 keeps `real_codex_execution` partial until a real
  `AGENTD_REAL_EXECUTE_SMOKE=1 --execute` Codex run succeeds.

## Boundaries

### Allowed Changes

- specs/e2e/p202-codex-mcp-launcher-parity.spec.md
- crates/agentd-tmux/src/backend.rs
- crates/agentd-tmux/tests/spawn.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md

### Forbidden

- Do not start real Claude, Codex, tmux, Matrix, or remote relay processes in
  tests.
- Do not mutate the user's Codex home or global Codex config.
- Do not change `scripts/agentd_real_claude_smoke.sh`.
- Do not change p201 role-prefix runtime selection semantics.

## Out of Scope

- Running `scripts/agentd_real_execute_smoke.sh --execute`.
- Adding `AGENTD_REAL_EXECUTE_RUNTIMES` matrix parsing.
- Implementing agent registry, scheduler, messaging, Matrix, or migration parity.
- Changing Codex authentication, profile, model, or sandbox defaults beyond the
  managed-launcher sandbox flag covered by p212.

## Completion Criteria

<!-- lint-ack: decision-coverage - Codex launch config and Claude preservation are directly tested in tmux spawn tests. -->
<!-- lint-ack: observable-decision-coverage - launcher script text and on-disk config artifacts are asserted. -->
<!-- lint-ack: output-mode-coverage - file output is covered by launcher script reads plus Claude JSON presence and Codex JSON absence assertions. -->
<!-- lint-ack: boundary-entry-point - `crates/agentctl/tests/parity_cli.rs` is an artifact-inspection test target, not a runtime entry point. -->
<!-- lint-ack: error-path - the empty MCP command scenario is the failure path; the linter does not classify an invariant error assertion as an error-path keyword. -->

Scenario: Codex launcher injects agentd MCP through config overrides
  Test:
    Package: agentd-tmux
    Filter: spawn_launcher_configures_codex_mcp_with_config_overrides_when_stdio_command_is_present
  Level: tmux launcher artifact
  Test Double: RecordingCommandRunner
  Given a Codex spawn request with `AGENTD_MCP_STDIO_CMD`
  When the tmux backend writes the launcher
  Then the launcher execs `codex --ask-for-approval never --sandbox danger-full-access`
  And the launcher passes `-c mcp_servers.agentd.command="sh"`
  And the launcher passes `-c mcp_servers.agentd.args=["-lc", <command>]`
  And the launcher does not set `CODEX_HOME`
  And no `.agentd-mcp-*.json` file is written for Codex

Scenario: Claude launcher keeps generated MCP config file behavior
  Test:
    Package: agentd-tmux
    Filter: spawn_launcher_configures_claude_mcp_when_stdio_command_is_present
  Level: tmux launcher artifact
  Test Double: RecordingCommandRunner
  Given a Claude spawn request with `AGENTD_MCP_STDIO_CMD`
  When the tmux backend writes the launcher
  Then the launcher still uses `claude --mcp-config`
  And the generated `.agentd-mcp-*.json` file still defines `mcpServers.agentd`

Scenario: Codex launcher stays plain without MCP command
  Test:
    Package: agentd-tmux
    Filter: spawn_launcher_execs_plain_codex_without_stdio_command
  Level: tmux launcher artifact
  Test Double: RecordingCommandRunner
  Given a Codex spawn request without `AGENTD_MCP_STDIO_CMD`
  When the tmux backend writes the launcher
  Then the launcher is a Codex command with `--ask-for-approval never --sandbox danger-full-access`
  And it does not include `mcp_servers.agentd`

Scenario: Empty MCP command is rejected for Codex
  Test:
    Package: agentd-tmux
    Filter: spawn_rejects_empty_mcp_stdio_command_for_codex
  Level: tmux launcher validation
  Test Double: RecordingCommandRunner
  Given a Codex spawn request with an empty `AGENTD_MCP_STDIO_CMD`
  When the tmux backend attempts to spawn
  Then it returns a backend invariant error before `new-session`

Scenario: parity row records p202 while remaining partial
  Test:
    Package: agentctl
    Filter: parity_capability_map_marks_real_codex_execution_partial_after_p202
  Level: artifact inspection
  Test Double: repository Markdown file
  Given p202 adds Codex MCP launcher parity
  When the parity map is parsed
  Then the `real_codex_execution` row remains `partial`
  And its replacement decision mentions p202 Codex MCP launcher progress
