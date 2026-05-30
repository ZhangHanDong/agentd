spec: task
name: "MempalMcpClient: port methods over the MCP tool seam"
tags: [mempal, mvp, p0, client]
---

## Intent

`agentd-mempal` implements the `agentd_core::ports::MempalClient` trait by mapping
each method to a mempal MCP tool call (design §4.12.2) through an injected
`McpToolCaller` seam — so every test runs against a fake caller, with no real
rmcp or mempal server. The client never touches mempal's `palace.db`; the MCP
tool channel is the only path (design §3.1).

## Decisions

- `McpToolCaller` is an object-safe trait: `call_tool(tool: &str, args: Value) -> Result<Value, MempalError>`. The production rmcp-backed impl is deferred to P0.7; v0 ships the seam and a `RecordingToolCaller` fake.
- `MempalMcpClient` holds `Arc<dyn McpToolCaller>`. Method → tool/args mapping: `search` → `mempal_search` `{query, wing, kind}`; `ingest` → `mempal_ingest` `{wing, kind, body}`; `kg_add` → `mempal_kg` `{op: "add", subject, predicate, object}`; `fact_check` → `mempal_fact_check` `{text}`.
- Read results parse an array of `{drawer_id, body, score}` objects — `search` from the `hits` field, `fact_check` from the `issues` field — into `Vec<DrawerHit>`. A hit element missing `drawer_id` is `MempalError::Decode`.
- `agentd-mempal` keeps a private `MempalError` (Transport / Timeout / Decode) and maps it to `CoreError::Mempal(String)` at the `MempalClient` boundary (D5). `agentd-core` is not modified — the port, `DrawerHit`, and `MempalWrite` already exist there.

## Boundaries

### Allowed Changes

- crates/agentd-mempal/**
- specs/mempal/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not reference, open, or write mempal's `palace.db` — MCP is the only channel (design §3.1).

## Out of Scope

- The outbox enqueue/drainer (Tasks 2–3) and consistency check (Task 4); the real rmcp transport (P0.7).

## Completion Criteria

Scenario: search issues mempal_search and parses hits
  Test: search_issues_mempal_search_and_parses_hits
  Given a client whose caller is scripted to return one hit with drawer_id "d1"
  When search runs with query "q", wing "proj", and kind "spec"
  Then the recorded call is mempal_search with args query "q", wing "proj", kind "spec", and the returned hit has drawer_id "d1"

Scenario: ingest issues mempal_ingest
  Test: ingest_issues_mempal_ingest
  Given a client with a recording caller
  When ingest runs with wing "proj", kind "spec", body "hello"
  Then the recorded call is mempal_ingest with args wing "proj", kind "spec", body "hello"

Scenario: kg_add issues mempal_kg with the add op
  Test: kg_add_issues_mempal_kg_add
  Given a client with a recording caller
  When kg_add runs with subject "s", predicate "p", object "o"
  Then the recorded call is mempal_kg with args op "add", subject "s", predicate "p", object "o"

Scenario: fact_check issues mempal_fact_check and parses issues as hits
  Test: fact_check_issues_mempal_fact_check
  Given a client whose caller is scripted to return one issue with drawer_id "i1" under the issues field
  When fact_check runs with claim "the sky is green"
  Then the recorded call is mempal_fact_check with args text "the sky is green", and the returned hit has drawer_id "i1"

Scenario: a transport failure on a write maps to CoreError::Mempal
  Test: ingest_transport_failure_maps_to_core_mempal
  Given a client whose caller is scripted to fail with a transport error
  When ingest runs
  Then it returns Err(CoreError::Mempal)

Scenario: an undecodable hits payload is an error
  Test: search_undecodable_payload_is_error
  Given a client whose caller returns a hit object with no drawer_id field
  When search runs
  Then it returns Err(CoreError::Mempal)
