spec: task
name: "Tmux backend skeleton: discovery, error, config, runner"
tags: [tmux, mvp, p0, backend]
---

## Intent

Lay the `agentd-tmux` foundation that every later P0.3 task builds on: the
private `BackendError` (design §4.2) and its mapping to the public
`CoreError::Backend` boundary, tmux-binary discovery (§4.4), an in-memory
`Config` holding the per-CLI ready patterns and tunable delays (§4.6/§4.7), the
`TmuxBackend` struct that injects an `Arc<dyn CommandRunner>` (D3), and the
production `TokioCommandRunner` the daemon wires (design D6). No tmux server or
spawned child is touched by tests — every flow runs through the injected runner.

## Decisions

- `BackendError` stays private to `agentd-tmux`; a `From<BackendError> for CoreError` maps every variant to `CoreError::Backend(String)` via its `Display` text. The agentd-core trait is NOT widened (D1).
- tmux discovery order is: `AGENTD_TMUX_BIN` (returned verbatim, no existence check) > first *existing* of `/opt/homebrew/bin/tmux`, `/usr/local/bin/tmux`, `/usr/bin/tmux` > `tmux` resolved on `PATH`. Nothing found returns `BackendError::Fatal` whose text contains `tmux not found` and `AGENTD_TMUX_BIN`.
- `TokioCommandRunner` returns `Ok(CommandOutput)` for any command that ran to completion — including a non-zero exit code — and `Err(CommandError)` only when the program cannot be launched or exceeds `RunOpts::timeout`.
- `Config` holds per-CLI ready patterns (`claude_code`, `codex`) plus `inject_delay` (60ms) and the `wait_for_ready`/status/shutdown durations as public fields, so tests construct a zero-delay `Config`. Patterns are matched as substrings of a captured buffer.
- `TmuxBackend` injects `runner: Arc<dyn CommandRunner>`, `tmux_bin`, and `cfg`; its `tmux` helper runs the resolved binary and maps a launch/timeout `CommandError` to `BackendError`. `TmuxBackend` hand-writes `Debug` because `dyn CommandRunner` is not `Debug`.

## Boundaries

### Allowed Changes

- crates/agentd-tmux/**
- specs/tmux/**

### Forbidden

- Do not modify any file under crates/agentd-core/** — it is tagged and frozen for this phase (D1).
- Do not add dependencies beyond agentd-core, tokio, async-trait, thiserror, tracing, and the listed dev-dependencies.
- Do not read a TOML config file in v0; `Config` is constructed in memory.

## Out of Scope

- Loading ready patterns or delays from a `config.toml` file (deferred until a daemon consumer exists).
- The `spawn` flow, prompt injection, capture, status, shutdown, and rebind (Tasks 2–5).

## Completion Criteria

Scenario: AGENTD_TMUX_BIN is returned verbatim
  Test: discovery_honors_env_override
  Given an explicit env override path that does not exist on disk
  When tmux binary discovery runs with that override
  Then the override path is returned unchanged without an existence check

Scenario: The first existing candidate path is selected
  Test: discovery_selects_first_existing_candidate
  Given no env override and a candidate list where only the second path exists
  When tmux binary discovery runs
  Then the second candidate path is returned

Scenario: A missing tmux binary is a fatal error with an install hint
  Test: discovery_missing_tmux_is_fatal_with_hint
  Given no env override, no existing candidate, and an empty PATH lookup
  When tmux binary discovery runs
  Then it returns BackendError::Fatal whose text contains "tmux not found" and "AGENTD_TMUX_BIN"

Scenario: A recoverable backend error maps to CoreError::Backend
  Test: backend_error_recoverable_maps_to_core_backend
  Given a BackendError::Recoverable carrying a message
  When it is converted into CoreError
  Then the result is CoreError::Backend whose text contains the original message

Scenario: A fatal backend error maps to CoreError::Backend
  Test: backend_error_fatal_maps_to_core_backend
  Given a BackendError::Fatal carrying a message
  When it is converted into CoreError
  Then the result is CoreError::Backend whose text contains the original message

Scenario: Config exposes default ready patterns and accepts overrides
  Test: config_ready_patterns_default_and_override
  Given a default Config
  When the claude_code ready patterns are read and then overridden with a new pattern
  Then the default patterns are non-empty and a buffer containing the overridden pattern is recognized as the main prompt

Scenario: TokioCommandRunner captures stdout from a completed command
  Test: tokio_runner_captures_stdout
  Given a TokioCommandRunner
  When it runs a command that prints a known line to stdout
  Then the returned CommandOutput has status 0 and stdout containing that line

Scenario: A command that exits non-zero is returned as Ok output
  Test: tokio_runner_nonzero_exit_is_ok
  Given a TokioCommandRunner
  When it runs a command that exits with code 1
  Then it returns Ok(CommandOutput) whose status is non-zero

Scenario: A program that cannot be launched returns CommandError
  Test: tokio_runner_launch_failure_is_error
  Given a TokioCommandRunner
  When it runs a program name that does not exist on the system
  Then it returns Err(CommandError)

Scenario: TokioCommandRunner forwards stdin to the child
  Test: tokio_runner_forwards_stdin
  Given a TokioCommandRunner and RunOpts whose stdin is a known byte string
  When it runs a command that echoes its stdin to stdout
  Then the returned stdout contains that byte string

Scenario: The tmux helper runs the resolved binary and records argv
  Test: tmux_helper_runs_resolved_binary_and_records_argv
  Given a TmuxBackend built with a recording runner and a known tmux binary path
  When the tmux helper is invoked with a subcommand and arguments
  Then the recorded call program is the tmux binary path and its args are the subcommand and arguments
