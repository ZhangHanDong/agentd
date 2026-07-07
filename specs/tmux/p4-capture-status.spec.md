spec: task
name: "Pane capture + status detection"
tags: [tmux, mvp, p0, backend, status]
---

## Intent

Expose the pane capture surface and the two-step status detector (design §4.8).
`capture` reads a pane's buffer (optionally with ansi escapes); `status` reads
`pane_current_command` and, when an agent CLI is running, diffs two captures to
tell Idle from Busy. Both are inherent methods on `TmuxBackend` (D1) and run
entirely through the injected runner, so the FakeRunner drives every branch.

## Decisions

- `CaptureOpts { lines: u32, ansi: bool }`. `capture` runs `capture-pane -p -t <address> -S -<lines>`, adding `-e` when `ansi` is set, and returns the captured buffer string.
- `status` step 1 reads `display-message -p -t <address> "#{pane_current_command}"`. An empty or non-zero result is `Gone` (no addressable pane).
- `status` step 2: when `pane_current_command` is a login-stripped `bash`/`zsh`/`sh`, the pane is `Starting` if the capture is empty, else `Gone` (a shell showing prior output means the CLI is no longer running).
- `status` step 3: otherwise the CLI is running — capture twice with a `Config::status_diff_gap` pause; identical captures are `Idle { last_output_age: status_diff_gap }`, differing captures are `Busy { last_output_age: 0 }` (a v0 approximation; per-CLI statusLine probes are deferred).
- `capture` and `status` map a runner launch/timeout failure to `BackendError`.

## Boundaries

### Allowed Changes

- crates/agentd-tmux/**
- specs/tmux/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not add a `StatusProbe`-style per-CLI hook — that is a later trait extension (§4.8).

## Out of Scope

- Shutdown and rebind (Task 5); the `UnexpectedShell` status variant (a later, more precise probe).

## Completion Criteria

Scenario: capture returns the pane buffer
  Test: capture_returns_pane_buffer
  Given a backend whose runner scripts a capture-pane output of "screen contents"
  When capture runs with 200 lines and no ansi
  Then it returns "screen contents" and the recorded call is capture-pane with -S and "-200"

Scenario: capture includes ansi escapes when requested
  Test: capture_with_ansi_includes_escapes
  Given a backend with a recording runner
  When capture runs with ansi enabled
  Then the recorded capture-pane call args include "-e"

Scenario: capture surfaces a runner failure as an error
  Test: capture_surfaces_runner_error
  Given a backend whose runner is scripted to fail the command
  When capture runs
  Then it returns Err(BackendError)

Scenario: status is Gone when the pane has no command
  Test: status_gone_when_pane_absent
  Given a backend whose runner scripts an empty pane_current_command
  When status runs
  Then it returns AgentStatus::Gone

Scenario: status is Starting for a booting shell with no output
  Test: status_starting_for_booting_shell
  Given a backend whose runner scripts pane_current_command "bash" then an empty capture
  When status runs
  Then it returns AgentStatus::Starting

Scenario: status is Gone for a shell showing prior output
  Test: status_gone_for_shell_with_output
  Given a backend whose runner scripts pane_current_command "bash" then a non-empty capture
  When status runs
  Then it returns AgentStatus::Gone

Scenario: status is Idle when two captures are identical
  Test: status_idle_when_output_unchanged
  Given a backend whose runner scripts a CLI command then two identical captures
  When status runs
  Then it returns AgentStatus::Idle

Scenario: status is Busy when two captures differ
  Test: status_busy_when_output_changes
  Given a backend whose runner scripts a CLI command then two differing captures
  When status runs
  Then it returns AgentStatus::Busy
