spec: task
name: "Prompt injection (buffer path) + wait_for_ready"
tags: [tmux, mvp, p0, backend, inject]
---

## Intent

Deliver prompts to a running agent through tmux's paste buffer (design §4.6) and
block until the CLI's main prompt is visible (§4.7), then wire both into
`spawn`'s step 5 so an `initial_prompt` is delivered after the agent is ready.
The hard rule is that the prompt payload is NEVER typed as keystrokes — it goes
into the buffer (argv for small prompts, stdin for large ones) and is pasted, so
the literal `send-keys -l` never appears. Everything is exercised against a
`RecordingCommandRunner`; timing comes from `Config` so tests run zero-delay.

## Decisions

- `send_prompt` (§4.6): stage 1 `set-buffer <prompt>` by argv when the prompt is at most 64 KiB, else `set-buffer -` with the prompt on stdin; stage 2 `paste-buffer -p -t <address> -d`; stage 3 sleep `Config::inject_delay`; stage 4 `send-keys -t <address> Enter`. The single Enter is a bare key, not `send-keys -l`.
- The payload is never embedded as keystrokes or in any command string. The only forbidden literal is `send-keys -l`; the bare `send-keys … Enter` call is legitimate.
- `wait_for_ready` (§4.7): capture the pane on a loop with exponential backoff from `Config::ready_probe_initial` doubling to `Config::ready_probe_max`, until `Config::ready_deadline`. It returns Ok as soon as `Config::main_prompt_visible` finds a CLI ready pattern (substring) in the capture; on deadline it returns `BackendError::Recoverable`.
- The capture primitive is `capture-pane -p -t <address> -S -<lines>` (with `-e` when ansi is requested). `wait_for_ready` captures 50 lines, no ansi.
- `spawn` wires step 5: when `initial_prompt` is set, it calls `wait_for_ready` then `send_prompt` before returning the handle.

## Boundaries

### Allowed Changes

- crates/agentd-tmux/**
- specs/tmux/**

### Forbidden

- The literal `send-keys -l` must never appear in crates/agentd-tmux/src/**.
- Do not modify any file under crates/agentd-core/** (D1).
- Do not embed the prompt payload in any shell command string — it lives only in the tmux buffer (via set-buffer argv or stdin).

## Out of Scope

- Full capture/status options and the public `capture` surface (Task 4); shutdown and rebind (Task 5).

## Completion Criteria

Scenario: send_prompt delivers the prompt through the buffer path
  Test: send_prompt_uses_buffer_path
  Given a backend with a recording runner and a handle addressing "agentd-x:0.0"
  When send_prompt runs with the prompt "hello world"
  Then the recorded calls include set-buffer with "hello world", then paste-buffer targeting the address, then send-keys with Enter

Scenario: send_prompt never sends the payload as keystrokes
  Test: send_prompt_never_sends_payload_as_keys
  Given a backend with a recording runner
  When send_prompt runs with the prompt "secret-payload"
  Then no send-keys call carries "secret-payload" and no call uses the -l flag

Scenario: a prompt over 64 KiB streams through stdin
  Test: send_prompt_large_prompt_uses_stdin
  Given a backend with a recording runner and a prompt larger than 64 KiB
  When send_prompt runs
  Then the set-buffer call argv is exactly set-buffer and "-", and the payload is not an argument of any recorded call

Scenario: wait_for_ready returns Ok when the main prompt is visible
  Test: wait_for_ready_returns_ok_when_visible
  Given a backend whose runner scripts a capture containing the claude_code ready pattern
  When wait_for_ready runs for the ClaudeCode CLI
  Then it returns Ok after one capture

Scenario: wait_for_ready re-polls until the prompt appears
  Test: wait_for_ready_loops_until_visible
  Given a backend whose runner scripts a capture without the ready pattern then one with it
  When wait_for_ready runs
  Then it returns Ok after the second capture

Scenario: wait_for_ready times out when the prompt never appears
  Test: wait_for_ready_times_out
  Given a backend whose ready_deadline is zero
  When wait_for_ready runs and the prompt is never visible
  Then it returns Err(BackendError::Recoverable) mentioning the main prompt

Scenario: spawn injects the initial prompt after readiness
  Test: spawn_injects_initial_prompt_after_ready
  Given a backend (zero delays) scripting a successful spawn, a ready capture, and the buffer-path calls
  When spawn runs with an initial_prompt
  Then the recorded calls end with set-buffer, paste-buffer, then send-keys Enter, after display-message
