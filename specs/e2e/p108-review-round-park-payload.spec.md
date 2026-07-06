spec: task
name: "Review park payload carries Delphi round"
tags: [e2e, core, store, daemon, p2, delphi]
---

## Intent

Prepare the P1.4 Delphi loop by making review parks round-aware before the loop
exists. Consecutive Delphi rounds can park at the same review node, so the
daemon event payload must distinguish round 1 from round 2 instead of relying
only on the node id.

## Decisions

- `review_runs` stores `round` as an `INTEGER NOT NULL DEFAULT 1`; existing
  deployed rows become round 1 through a real migration.
- `ParkReason::ReviewVerdicts` carries `round`; current non-Delphi fan_out
  starts at round 1.
- On resume, fan_out reads the stored review run round instead of deriving it
  from the live workflow graph.
- The daemon keeps the existing payload-equality dedup rule. A review park emits
  compact JSON with both `node` and `round`, while non-review park payloads keep
  the existing node-only shape.

## Boundaries

### Allowed Changes

- crates/agentd-core/src/engine/**
- crates/agentd-core/src/handler/fan_out.rs
- crates/agentd-core/src/ports/**
- crates/agentd-core/src/test_support/**
- crates/agentd-core/tests/**
- crates/agentd-store/src/**
- crates/agentd-store/migrations/**
- crates/agentd-store/tests/**
- crates/agentd-bin/**
- crates/agentd-bin/tests/**
- crates/agentd-surface/tests/**
- crates/agentctl/tests/**
- specs/e2e/**
- specs/store/**

### Forbidden

- Do not change `event_repo::last` or the durable event dedup algorithm.
- Do not implement the Delphi N-round loop, reviewer stance packs, or
  `converge_or_*` aggregators in this slice.
- Do not persist `ParkReason` in checkpoints or add a checkpoint migration.
- Do not change non-review park payloads from `{"node":"..."}`.

## Out of Scope

- Enabling `visibility=delphi` in workflow validation.
- Creating later-round review runs from an aggregator loop.
- Changing MCP or HTTP response schemas beyond the event payload already exposed
  by `events_from` and SSE replay.

## Completion Criteria

Scenario: initial review park emits the default round
  Test: production_runhost_review_park_payload_includes_default_round
  Level: daemon integration
  Test Double: real SqliteStore with fake backend ports
  Given an `execute.dot` run has advanced from `implement` to the review fan_out
  When the daemon persists the review `run_parked` event
  Then the compact payload is exactly `{"node":"review","round":1}`

Scenario: different review rounds are not deduped
  Test: production_runhost_emits_review_reparks_when_round_differs
  Level: daemon unit for crates/agentd-bin/src/host.rs
  Test Double: synthetic RunProgress over a real SqliteStore
  Given two consecutive review parks for the same run and node
  When the first park carries round 1 and the second park carries round 2
  Then two durable `run_parked` rows exist because their payloads differ

Scenario: duplicate review re-park in the same round is still deduped
  Test: production_runhost_dedupes_same_node_reparks
  Level: daemon integration
  Test Double: real SqliteStore with fake backend ports
  Given an `execute.dot` review fan_out is parked at review round 1
  When one non-final reviewer verdict re-parks the same node in round 1
  Then exactly one durable review `run_parked` row exists for that round

Scenario: non-review park rejects a round field
  Test: production_runhost_non_review_park_payload_stays_node_only
  Level: daemon integration
  Test Double: real SqliteStore with fake backend ports
  Given a `draft.dot` run parks at a non-review node
  When the daemon persists the first `run_parked` event
  Then the payload is exactly `{"node":"propose_spec"}` with no round field

Scenario: fan_out resume reconstructs the stored round
  Test: fan_out_resume_reparks_with_stored_round
  Level: core unit
  Test Double: InMemoryStore
  Given an open review run stored with round 2 and expected reviewer count 3
  When one reviewer verdict is submitted
  Then fan_out re-parks with `ParkReason::ReviewVerdicts.round` equal to 2

Scenario: review run round survives deployed-store migration
  Test: review_runs_round_migration_preserves_existing_rows
  Level: store migration
  Test Double: raw SQL migration harness
  Given a database migrated only through `0001_init.sql` contains a review run
  When the real round migration is applied
  Then the existing review run still exists and reads back with round 1
