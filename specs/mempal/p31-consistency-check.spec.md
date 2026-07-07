spec: task
name: "Mempal consistency check (read-only drift report)"
tags: [mempal, mvp, p0, consistency]
---

## Intent

After draining, agentd may want to confirm that drawers it believes it ingested
are actually searchable in mempal (design §3.5). `check_against` searches mempal
for each expected drawer and reports the ones mempal does not return. It is
strictly read-only — it reports drift and never re-ingests (on a git↔mempal
drift, git wins and re-ingest is a separate operator action).

## Decisions

- `ExpectedDrawer { wing, kind, query }` describes a drawer agentd expects (e.g. a drained Ingest write). `check_against(expected, client)` returns the subset mempal does not surface.
- A drawer is present when `MempalClient::search(query, wing, kind)` returns at least one hit; it is reported missing when the search returns no hits.
- A search failure (mempal unreachable) conservatively reports the drawer as missing and logs a warning — the check makes no writes and propagates no error (it is a best-effort report).

## Boundaries

### Allowed Changes

- crates/agentd-mempal/**
- specs/mempal/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not reference, open, or write mempal's on-disk database (MCP-only, §3.1).
- Do not re-ingest or write anything during the check — it is read-only (§3.5).

## Out of Scope

- Auto-repair / re-ingest of missing drawers (a separate operator action); sourcing the expected set from the store (the caller supplies it).

## Completion Criteria

Scenario: the check reports the drawers mempal does not return
  Test: test_consistency_check_reports_missing_drawers
  Given two expected drawers and a client scripted to return a hit for the first and no hits for the second
  When check_against runs
  Then it returns exactly the second drawer

Scenario: the check is empty when every drawer is present
  Test: consistency_check_empty_when_all_present
  Given one expected drawer and a client scripted to return a hit
  When check_against runs
  Then it returns an empty report

Scenario: a search failure conservatively reports the drawer missing
  Test: consistency_search_failure_reports_missing
  Given one expected drawer and a client whose search fails
  When check_against runs
  Then it returns that drawer as missing without erroring
