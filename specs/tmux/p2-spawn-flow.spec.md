spec: task
name: "TmuxBackend spawn flow"
tags: [tmux, mvp, p0, backend, spawn]
---

## Intent

Implement `AgentBackend::spawn` for `TmuxBackend` (design §4.5) — the only trait
method P0.3 widens to a real caller. `spawn` probes for an existing session,
writes a launcher script into the worktree, launches tmux (directly or wrapped
in a transient systemd scope), probes the new pane's id and pid, and returns an
`AgentHandle`. Every step runs through the injected runner, so the whole flow is
exercised against a `RecordingCommandRunner` with no real tmux server.

## Decisions

- Naming (§4.3): `session_name = "agentd-<agent_id>"`, `address = "<session_name>:0.0"`. `AgentHandle.backend` is `BackendKind::Tmux`.
- Step order is fixed (§4.5): `has-session -t <session>` probe FIRST; if it exits zero the session exists and `spawn` returns `BackendError::Recoverable` (mapped to `CoreError::Backend`) telling the caller to `rebind`, BEFORE any launcher is written or any `new-session` is run.
- The launcher `<worktree>/.agentd-launcher-<agent_id>.sh` (shebang, `cd` to the worktree, exported env, `exec` the CLI) is written before launch, and git `info/exclude` is amended idempotently with `.agentd-launcher-*.sh` without changing the tracked worktree `.gitignore`.
- Launch argv: Direct runs `tmux new-session -d -s <session> -c <worktree> bash <launcher>`; Systemd wraps it as `systemd-run --user --scope --unit=<scope_name> --collect --quiet tmux new-session …`.
- The pane is probed with `display-message -p -t <address> "#{pane_id} #{pane_pid}"`; the first whitespace token is the `pane_id` and the second (if present and numeric) is the pid. Output with no `pane_id` token is `BackendError::Invariant`.
- A non-zero `new-session` exit (the launch failed) is surfaced as an error, before the pane is probed.
- The launcher's env exports are safe: each `env_overrides` value is POSIX single-quoted, and each key must be a shell identifier (`[A-Za-z_][A-Za-z0-9_]*`) — an invalid key is `BackendError::Invariant` rather than emitted raw into the script.
- `spawn` maps its internal `BackendError` to `CoreError::Backend` at the trait boundary (D2). The agentd-core trait is not widened beyond `spawn` (D1).

## Boundaries

### Allowed Changes

- crates/agentd-tmux/**
- specs/tmux/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not embed the prompt payload in any command string here — prompt delivery is Task 3 and uses the buffer path.
- Do not run `new-session` or write a launcher when the `has-session` probe shows the session already exists.

## Out of Scope

- Prompt injection, `wait_for_ready`, capture, status, shutdown, and rebind (Tasks 3–5). When `initial_prompt` is set, wiring `wait_for_ready` + `send_prompt` into `spawn` lands in Task 3.

## Completion Criteria

Scenario: spawn returns a handle with the parsed pane id and address
  Test: spawn_returns_handle_with_parsed_pane_id
  Given a backend whose runner scripts a non-existent session then a pane probe of "%3 12345"
  When spawn runs for agent "claude-impl-a" with the Direct strategy and no initial prompt
  Then the handle session_name is "agentd-claude-impl-a", its address is "agentd-claude-impl-a:0.0", its pane_id is "%3", its pid is 12345, and the recorded calls run has-session then new-session then display-message in that order

Scenario: spawn writes the launcher script and amends git exclude
  Test: spawn_writes_launcher_and_amends_git_exclude
  Given a backend whose runner scripts a successful Direct launch
  When spawn runs against a temporary worktree
  Then a file ".agentd-launcher-claude-impl-a.sh" exists in the worktree and git "info/exclude" contains ".agentd-launcher-*.sh"

Scenario: spawn with the Systemd strategy wraps the launch in systemd-run
  Test: spawn_systemd_strategy_wraps_launch
  Given a backend whose runner scripts a successful launch
  When spawn runs with the Systemd strategy whose scope is "agentd-claude-impl-a.scope"
  Then the launch call program is "systemd-run" and its args include "--scope", "--unit=agentd-claude-impl-a.scope", the tmux binary path, and "new-session"

Scenario: spawn on an existing session is recoverable
  Test: spawn_on_existing_session_is_recoverable
  Given a backend whose runner scripts the has-session probe exiting zero
  When spawn runs
  Then it returns Err(CoreError::Backend) whose text mentions rebinding

Scenario: spawn does not write a launcher when the session already exists
  Test: spawn_existing_session_skips_launcher
  Given a backend whose runner scripts the has-session probe exiting zero
  When spawn runs against a temporary worktree
  Then no launcher file is written in the worktree and only the has-session call was recorded

Scenario: spawn maps an unparseable pane probe to an error
  Test: spawn_unparseable_pane_info_is_error
  Given a backend whose runner scripts a non-existent session then an empty pane probe
  When spawn runs
  Then it returns Err(CoreError::Backend)

Scenario: a pane probe without a pid yields a handle with no pid
  Test: spawn_handle_has_no_pid_when_probe_omits_it
  Given a backend whose runner scripts a pane probe of "%7" with no second token
  When spawn runs
  Then the handle pane_id is "%7" and its pid is None

Scenario: a non-zero new-session exit surfaces as an error before the pane probe
  Test: spawn_surfaces_launch_failure
  Given a backend whose runner scripts a non-existent session then a new-session exit of 2
  When spawn runs
  Then it returns Err(CoreError::Backend) and only the has-session and new-session calls were recorded

Scenario: spawn exports env overrides into the launcher
  Test: spawn_launcher_exports_env_overrides
  Given a spawn request whose env_overrides has key "AGENTD_ROLE"
  When spawn runs against a temporary worktree
  Then the launcher script contains an "export AGENTD_ROLE=" line

Scenario: spawn rejects an env override key that is not a shell identifier
  Test: spawn_rejects_invalid_env_key
  Given a spawn request whose env_overrides has the key "BAD KEY"
  When spawn runs
  Then it returns Err(CoreError::Backend) and no launch call was recorded
