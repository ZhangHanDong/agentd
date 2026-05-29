spec: task
name: "Shutdown sequence + rebind"
tags: [tmux, mvp, p0, backend, lifecycle]
---

## Intent

Tear an agent down safely (design §4.9) and re-attach to a survivor on daemon
restart (§4.10). `shutdown` archives the pane transcript BEFORE any kill, then
escalates graceful → interrupt → kill; `rebind` re-probes a session and rebuilds
its handle, or reports that the session is gone. Both are inherent methods on
`TmuxBackend` (D1) and run through the injected runner.

## Decisions

- `ShutdownOpts { archive_to: PathBuf }`; `ShutdownReport { method: ShutdownMethod, final_capture_sha: String }`; `ShutdownMethod { Graceful, Interrupt, Kill }`.
- `shutdown` probes `has-session` first; a missing session is `BackendError::Recoverable` (case 5) before anything else runs.
- `shutdown` then ARCHIVES: `capture-pane` 5000 lines, writes the buffer to `archive_to`, and records `final_capture_sha` as its SHA-256 — all BEFORE any kill action (case 7).
- `shutdown` escalates: graceful `/exit` via `send_prompt` then wait `Config::graceful_timeout`, re-probe `has-session` (gone ⇒ `Graceful`); else interrupt with two `send-keys … C-c`, wait `Config::sigint_settle`, re-probe (gone ⇒ `Interrupt`); else `kill-session` (⇒ `Kill`). The interrupt uses the named `C-c` key, never `send-keys -l`.
- `rebind(target)` runs `has-session -t <target>`; missing ⇒ `Ok(None)`; else `display-message` re-probes the pane and rebuilds an `AgentHandle` (`session_name = target`, `address = <target>:0.0`, `agent_id` = `target` with any `agentd-` prefix stripped).

## Boundaries

### Allowed Changes

- crates/agentd-tmux/**
- specs/tmux/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- The literal `send-keys -l` must never appear in crates/agentd-tmux/src/**.
- Do not kill or interrupt the session before the transcript archive is written.

## Out of Scope

- DB reconciliation of a `lost` task_run (the engine/store own that); per-CLI graceful-exit commands beyond `/exit`.

## Completion Criteria

Scenario: shutdown archives the transcript before any kill
  Test: shutdown_archives_before_any_kill
  Given a backend whose runner keeps the session alive through graceful and interrupt so shutdown escalates to kill-session
  When shutdown runs with an archive path
  Then the recorded capture-pane call precedes every send-keys C-c and the kill-session call, the archive file is written, and the report sha is non-empty with method Kill

Scenario: a graceful exit reports the Graceful method
  Test: shutdown_graceful_reports_graceful
  Given a backend whose has-session probe shows the session gone after the graceful exit
  When shutdown runs
  Then the report method is Graceful and no kill-session or C-c call was recorded

Scenario: shutdown on a missing session is recoverable
  Test: shutdown_missing_session_is_recoverable
  Given a backend whose first has-session probe shows the session already gone
  When shutdown runs
  Then it returns Err(BackendError::Recoverable) and no capture-pane or kill call was recorded

Scenario: rebind reconstructs a handle for a live session
  Test: rebind_reconstructs_live_session
  Given a backend whose has-session probe succeeds and pane probe returns "%5 999"
  When rebind runs for target "agentd-claude-impl-a"
  Then it returns a handle whose session_name is "agentd-claude-impl-a", address is "agentd-claude-impl-a:0.0", and pane_id is "%5"

Scenario: rebind on a missing session returns None
  Test: rebind_missing_session_returns_none
  Given a backend whose has-session probe shows the session gone
  When rebind runs
  Then it returns Ok(None)
