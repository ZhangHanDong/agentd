spec: task
name: "start_workflow seeds initial run context"
tags: [e2e, p0.9, daemon, context, real-agent]
---

## Intent

The surface `RunHost::start_workflow` contract already accepts an initial
`context`, and HTTP `POST /runs` forwards it, but production currently discards
that value before the engine starts. This slice closes the P0.9 initial-context
gap so real run starts can pass issue/spec metadata into the first checkpoint and
the first agent prompt without relying only on ambient per-run files.

## Decisions

- `ProductionRunHost::start_workflow` converts a JSON object `context` into the
  engine `RunContext` before starting the run.
- A missing HTTP `context` deserializes to JSON null and remains an empty
  `RunContext`, preserving existing `POST /runs` behavior.
- Non-object, non-null `context` values are rejected before recording or
  executing the run, because the engine context is a top-level key/value map.
- `agentctl run start --context-file <path>` reads and validates that file as a
  JSON object or null, then posts it as the `context` field; without the flag it
  preserves the existing empty object body.
- Keep `Engine::execute` as the empty-context compatibility entry point and add
  an explicit seeded-context execution path for production run starts.
- Update the deployment checklist so the P0.9 `Initial run context` gap no
  longer says production `start_workflow` accepts but discards `context`.

## Boundaries

### Allowed Changes

- specs/e2e/p137-initial-run-context-seeding.spec.md
- docs/p0.9-deployment-checklist.md
- crates/agentctl/Cargo.toml
- crates/agentctl/src/run.rs
- crates/agentctl/tests/run_cli.rs
- crates/agentd-core/src/engine/execute.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/contract.rs
- crates/agentd-bin/tests/deployment_checklist.rs

### Forbidden

- Do not change HTTP `POST /runs` request or response field names.
- Do not add schema columns for context keys.
- Do not move runtime-state files into task worktrees or change tool-node cwd.
- Do not change workflow `.dot` files for this slice.

## Out of Scope

- Real authenticated Claude/Codex/Gemini smoke execution.
- Persisting issue documents, frozen specs, or context packs in a new file
  format.
- Boot-resume CLI flags or checkpoint transaction hardening.
- Changing `assign_task`, `submit_outcome`, or reviewer MCP JSON shapes.

## Completion Criteria

Scenario: production start_workflow seeds object context
  Test:
    Package: agentd-bin
    Filter: production_start_workflow_seeds_initial_context
  Level: daemon contract
  Test Double: real SqliteStore on tempfile plus fake backend and runner
  Given a production host and a `draft` run start with object context containing `issue_id`, `issue_title`, and `issue_body`
  When `ProductionRunHost::start_workflow` parks at `propose_spec`
  Then the run snapshot context contains those issue fields
  And the spawned spec-writer prompt includes the string fields requested by `draft.dot`

Scenario: production start_workflow rejects non-object context
  Test:
    Package: agentd-bin
    Filter: production_start_workflow_rejects_non_object_initial_context
  Level: daemon contract
  Test Double: real SqliteStore on tempfile
  Given a production host and a `draft` run start with a string context
  When `ProductionRunHost::start_workflow` validates the initial context
  Then it returns an error before a run snapshot exists

Scenario: agentctl posts context-file JSON
  Test:
    Package: agentctl
    Filter: run_start_live_posts_context_file_json
  Level: CLI contract
  Test Double: one-shot local TCP daemon
  Given `agentctl run start --context-file` points at a JSON object file
  When the live start path posts to the daemon
  Then the HTTP request body contains that object as `context`
  And the daemon success response still produces a success exit

Scenario: deployment checklist marks initial context gap closed
  Test:
    Package: agentd-bin
    Filter: deployment_checklist_marks_p137_initial_context_resolved
  Level: docs regression
  Test Double: source inspection
  Given docs/p0.9-deployment-checklist.md and the P137 spec
  When the known gaps section is inspected
  Then the `Initial run context` line names P137 as the production seeding bridge
  And it does not say `start_workflow` accepts but does not seed `context`
