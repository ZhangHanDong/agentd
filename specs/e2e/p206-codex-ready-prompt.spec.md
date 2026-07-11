spec: task
name: "Codex ready prompt compatibility"
tags: [agent-chat-replacement, real-execute, codex, tmux, p206]
---

## Intent

The p204 Codex-only real execute smoke now reaches a structured failed terminal
state, but Codex v0.143 is visibly idle after the backend times out because the
tmux readiness defaults still expect an older prompt hint and a 15 second
startup budget. This slice updates the readiness contract for current Codex so
agentd can inject the task prompt after Codex has finished slow MCP startup.

## Decisions

- Codex readiness keeps the old `? for shortcuts` substring for compatibility
  and also accepts the current idle prompt marker rendered as `>` shaped
  U+203A followed by a space.
- The default `Config::ready_deadline` is at least 45 seconds so a Codex CLI
  startup that waits for 30 second MCP server timeouts can still become ready.
- Claude readiness patterns and workspace-trust confirmation behavior are not
  changed by this slice.
- p206 does not change Codex launcher flags, MCP server configuration,
  role-prefix runtime selection, or real execute workflow semantics.

## Boundaries

### Allowed Changes

- specs/e2e/p206-codex-ready-prompt.spec.md
- crates/agentd-tmux/src/config.rs
- crates/agentd-tmux/tests/inject.rs
- crates/agentd-tmux/tests/skeleton.rs

### Forbidden

- Do not start real Claude, Codex, tmux, Matrix, or remote relay processes in
  tests.
- Do not remove existing Claude or Codex ready patterns.
- Do not change `scripts/agentd_real_execute_smoke.sh`.
- Do not change Codex MCP launcher arguments or generated config behavior.
- Do not alter run failure persistence semantics from p205.

## Out of Scope

- Proving a full real execute run succeeds; that remains the p204 manual gate.
- Disabling user MCP servers such as playwright or shadcn.
- Adding configurable per-runtime deadlines from files or environment.

## Completion Criteria

Scenario: current Codex idle prompt marker is ready
  Test:
    Package: agentd-tmux
    Filter: wait_for_ready_accepts_current_codex_prompt_marker
  Level: tmux readiness unit
  Test Double: RecordingCommandRunner
  Given a Codex pane capture containing the current idle prompt marker
  When `wait_for_ready` runs for `CliKind::Codex`
  Then it returns Ok after one capture

Scenario: Codex banner alone is not ready
  Test:
    Package: agentd-tmux
    Filter: config_codex_ready_patterns_do_not_match_banner_only
  Level: config unit
  Test Double: in-memory Config
  Given a Codex pane capture containing the startup banner without the idle prompt marker
  When `Config::main_prompt_visible` checks the Codex patterns
  Then it returns false

Scenario: default ready deadline covers slow Codex MCP startup
  Test:
    Package: agentd-tmux
    Filter: config_default_ready_deadline_covers_slow_codex_mcp_startup
  Level: config unit
  Test Double: in-memory Config
  Given the default tmux backend `Config`
  When the ready deadline is inspected
  Then it is at least 45 seconds

Scenario: existing Claude readiness still works
  Test:
    Package: agentd-tmux
    Filter: wait_for_ready_accepts_claude_auto_mode_prompt
  Level: regression unit
  Test Double: RecordingCommandRunner
  Given a Claude pane capture containing `auto mode on`
  When `wait_for_ready` runs for `CliKind::ClaudeCode`
  Then it still returns Ok
