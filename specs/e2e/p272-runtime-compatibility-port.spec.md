spec: task
name: "agent runtime control compatibility port"
tags: [e2e, agent-chat-replacement, runtime, compatibility, tmux, cli, p272, design-only]
---

## Intent

Port the sibling P205 registered-runtime status, capture, shutdown, and rebind
capability into the authoritative base namespace. Reuse the existing tmux
primitives and P234 lifecycle state transitions through one injected runtime
control port while keeping `AgentBackend` spawn-only and preserving current
`/api/agents` compatibility.

This contract is design-complete but implementation-paused. P263-P271 now exist
as reviewable feature-branch commits and the canonical roadmap records the
FSF/AD-E dependency gates. Execution remains blocked because P272-P275 are
FSF-0 transitional parity candidates; resume requires an explicit human scope
decision and is not implied by those completed prerequisites.

## Decisions

- Rename and extend the P234 composition-root `AgentLifecycle` as
  `AgentRuntimeControl` with status, capture, archive-first shutdown, and
  rebind operations. Production delegates to existing `TmuxBackend` methods;
  tests use recording fakes.
- Add typed surface requests/results for runtime status, capture, explicit
  shutdown, and optional-target rebind. Do not add these methods to
  `agentd_core::ports::AgentBackend`.
- Add `GET /api/agents/:name/status`,
  `GET /api/agents/:name/capture?lines=<u32>&ansi=<bool>`, and
  `POST /api/agents/:name/shutdown`. Capture defaults to 200 lines and
  `ansi=false`; shutdown rejects a missing or blank `archive_to`.
- Extend the existing `POST /api/agents/:name/rebind` body with optional
  `target`. Omission uses the stored `tmux_target`; a missing backend session
  preserves P234 HTTP 200 with `rebound=false` and offline reason
  `rebind-missing-session`.
- Reconstruct compatibility handles only from a non-empty stored or explicit
  tmux target. Paths, pane ids, PIDs, provider sessions, and tmux values remain
  runtime metadata and never become enterprise canonical ids.
- Map status to `gone`, `unexpected_shell`, `idle`, `busy`, or `starting`.
  `gone` records lifecycle metadata and marks the agent offline with
  `runtime-gone`; a live status marks it online without spawning.
- Explicit shutdown uses the backend archive-before-stop result, records
  method/path/final SHA, marks the agent offline, and clears the dead target.
  Existing P234 `down` continues to choose its own archive path through the
  same port.
- Add `agentctl agent status`, `capture`, and `shutdown`, plus optional
  `rebind --target`. These use operator bearer auth, forward success bodies,
  return exit 3 for daemon/transport rejection, and reject empty or slash
  containing names with exit 2 before network I/O.
- If implementation is explicitly resumed, update roadmap and parity evidence
  to close the P263 sibling P205 queue item while keeping native runtime, remote
  lifecycle, migration, cutover, rollback, and broader operational parity
  partial.

## Boundaries

### Allowed Changes

- specs/e2e/p272-runtime-compatibility-port.spec.md
- docs/specs/2026-07-11-agent-runtime-control-compatibility.md
- docs/superpowers/plans/2026-07-11-p272-runtime-compatibility-port.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md
- docs/parity/agent-chat-capability-map.md
- crates/agentd-surface/src/host.rs
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/src/test_support.rs
- crates/agentd-surface/tests/http.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/src/daemon.rs
- crates/agentd-bin/tests/contract.rs
- crates/agentd-bin/tests/daemon_http.rs
- crates/agentctl/src/cli.rs
- crates/agentctl/src/agent.rs
- crates/agentctl/tests/agent_cli.rs
- crates/agentctl/tests/parity_cli.rs
- crates/agentctl/tests/runtime_compatibility_contract.rs

### Forbidden

- Do not add a migration, table, dependency, native PTY/process runtime,
  provider-session persistence, worker protocol, scheduler reconciliation,
  dashboard behavior, Matrix behavior, remote lifecycle control, dual-write,
  cutover, or rollback.
- Do not add status, capture, shutdown, or rebind methods to
  `agentd_core::ports::AgentBackend`.
- Do not change P234 `down` response semantics or missing-session rebind from
  HTTP 200 `rebound=false` to an incompatible error.
- Do not treat tmux target, pane, PID, archive path, provider session, or CLI
  name as a P265/P267 canonical runtime, attempt, worker, or lease identity.
- Do not start real tmux, Claude, Codex, Matrix, Specify, OpenFab, or remote
  services in tests.

## Out of Scope

- Automatic daemon-boot rebind sweeps and remote-host process control.
- Runtime output streaming, terminal input, resize, signal APIs, or native PTY
  ownership; the historical P276 label maps to AD-E5 after AD-E2.
- Provider resume identifiers and native logical-session recovery; the
  historical P277 label maps to AD-E5 after AD-E2.
- Provision-registration reconciliation and scheduler-driven Codex auto-spawn;
  P273 is a paused FSF-0 transitional candidate.
- Agent-chat import, shadow comparison, dual-write, dashboard lifecycle UI,
  service cutover, rollback, and token provisioning/rotation.

## Completion Criteria

<!-- lint-ack: decision-coverage - eight scenarios cover the port, HTTP defaults and errors, durable status/shutdown/rebind behavior, CLI requests and validation, P234 compatibility, and roadmap evidence. -->
<!-- lint-ack: observable-decision-coverage - outputs are fake-port calls, HTTP status/body values, SQLite registry rows, CLI exit codes/raw requests, and inspected source/docs. -->
<!-- lint-ack: output-mode-coverage - both JSON HTTP and agentctl stdout/error/exit behavior are bound to explicit tests. -->
<!-- lint-ack: boundary-entry-point - scenarios bind surface routing, production host composition, CLI entry points, and documentation artifacts. -->
<!-- lint-ack: bdd-rule-grouping - all scenarios prove one runtime-control compatibility slice. -->

Scenario: runtime control port remains separate from the spawn backend
  Test:
    Package: agentd-bin
    Filter: runtime_control_port_status_capture_shutdown_rebind_is_spawn_independent
  Level: composition contract
  Test Double: recording runtime control and recording spawn backend
  Given one runtime control with scripted status capture shutdown and rebind results
  When all four control methods are invoked through the production host
  Then the recording runtime control sees all operations in order
  And the spawn backend sees no request
  And `AgentBackend` remains spawn-only

Scenario: HTTP status and capture expose registered runtime state
  Test:
    Package: agentd-surface
    Filter: http_agent_runtime_status_and_capture_return_typed_results
  Level: HTTP routing
  Test Double: surface FakeHost
  Given a registered agent with runtime status and capture results
  When operator requests status and capture with omitted or explicit query values
  Then both responses are HTTP 200 with the typed status and captured text
  And omitted capture values are 200 lines and no ANSI
  And explicit lines and ANSI reach the host unchanged

Scenario: HTTP runtime operations validate auth agents handles and archive paths
  Test:
    Package: agentd-surface
    Filter: http_agent_runtime_operations_validate_requests_before_control
  Level: HTTP routing
  Test Double: surface FakeHost with configured operator auth
  Given unknown agents missing runtime metadata invalid capture query and blank shutdown paths
  When status capture shutdown or rebind routes are requested
  Then unknown agents return 404 and invalid requests return 400
  And unauthorized requests return 401 without a host mutation

Scenario: production status persists live and gone registry observations
  Test:
    Package: agentd-bin
    Filter: production_agent_runtime_status_persists_live_and_gone_observations
  Level: production host SQLite integration
  Test Double: temporary SQLite and recording runtime control
  Given registered agents with stored tmux targets and scripted idle or gone probes
  When runtime status is requested
  Then idle returns online with milliseconds and no spawn
  And gone returns offline clears the dead target and records `runtime-gone`
  And both observations are retained under runtime lifecycle metadata

Scenario: explicit shutdown archives and rebind supports stored or supplied targets
  Test:
    Package: agentd-bin
    Filter: production_agent_runtime_shutdown_and_rebind_preserve_p234_semantics
  Level: production host SQLite integration
  Test Double: temporary SQLite and recording runtime control
  Given a registered runtime and an explicit archive path
  When shutdown runs and rebind later uses a supplied live target
  Then shutdown records method path and SHA before marking the agent offline
  And rebind stores the recovered target and marks the agent online without spawn
  When rebind uses a missing target
  Then it returns `rebound=false` with HTTP-compatible offline metadata

Scenario: agentctl runtime control commands call base daemon endpoints
  Test:
    Package: agentctl
    Filter: agent_cli_runtime_control_commands_use_api_agents
  Level: CLI integration
  Test Double: one-shot local fake HTTP daemon
  Given successful daemon responses
  When status capture shutdown and rebind commands run
  Then they call `/api/agents/:name/status`, capture, shutdown, and rebind
  And capture query values plus shutdown archive and rebind target JSON are exact
  And daemon response bodies are printed unchanged

Scenario: agentctl rejects invalid runtime names before network
  Test:
    Package: agentctl
    Filter: agent_cli_runtime_control_rejects_invalid_names_before_network
  Level: CLI validation
  Test Double: unreachable daemon address
  Given an empty or slash-containing runtime agent name
  When status capture shutdown or rebind is invoked
  Then each command exits 2
  And no daemon connection is attempted

Scenario: roadmap and parity keep P272 transitional without claiming native runtime
  Test:
    Package: agentctl
    Filter: p272_roadmap_and_parity_record_runtime_compatibility_without_native_claim
  Level: artifact inspection
  Test Double: repository Markdown and Rust files
  Given the P272 design and canonical roadmap are inspected
  When roadmap parity and dependency evidence are inspected
  Then they name status capture shutdown rebind and `AgentRuntimeControl`
  And P263 sibling P205 remains assigned to P272 but is not yet discharged
  And native runtime remote lifecycle cutover and rollback remain incomplete
  And Immediate Next Step remains the AD-E1 security baseline rather than P273
