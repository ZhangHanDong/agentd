spec: task
name: "pre_tools reads are best-effort: timeout to error, never hang"
tags: [mempal, mvp, p0, client, best-effort]
---

## Intent

`pre_tools` reads (`mempal_search` / `mempal_fact_check`) must never hang a node
(design §3.4). `MempalMcpClient` wraps reads in a `Config` timeout and maps a
timeout to `CoreError::Mempal` — it does NOT swallow the error to empty. The
"proceed with empty results" fallback already lives at the call site in
`agentd-core` (`codergen.rs` does `.search(...).await.unwrap_or_default()`), so
this crate honors the port contract and lets the caller tolerate the error.

## Decisions

- `MempalConfig::pre_tools_timeout` defaults to 3s and is a public field, so tests construct a zero timeout. Reads (`search`, `fact_check`) run under `tokio::time::timeout`; writes (`ingest`, `kg_add`) are not wrapped (the drainer owns their retries).
- On timeout the client logs a warning and returns `Err(CoreError::Mempal)` (mapped from `MempalError::Timeout`). It does not return empty — empty substitution is the caller's responsibility (verified: `codergen.rs`).
- The fake caller's hang mode is a genuinely never-resolving future (`std::future::pending()`), so a zero timeout actually trips rather than racing an instant return.

## Boundaries

### Allowed Changes

- crates/agentd-mempal/**
- specs/mempal/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1) — the empty-fallback already exists there and must not be duplicated.
- Do not reference, open, or write mempal's `palace.db` (design §3.1).
- Do not swallow a read failure to an empty result inside the client (it breaks the port contract).

## Out of Scope

- The engine-side empty substitution (already in `agentd-core`); the outbox/drainer (Tasks 2–3).

## Completion Criteria

Scenario: search returns hits when mempal answers within the timeout
  Test: search_returns_hits_within_timeout
  Given a client with the default timeout and a caller that answers immediately with one hit
  When search runs
  Then it returns Ok with that hit

Scenario: a read that times out maps to an error and a caller's fallback is empty
  Test: test_pre_tools_search_falls_back_to_empty_on_timeout
  Given a client with a zero timeout and a caller whose response never resolves
  When search runs
  Then it returns Err(CoreError::Mempal) within the timeout without hanging, and a caller's unwrap_or_default yields an empty list

Scenario: a fact_check that times out maps to an error
  Test: fact_check_times_out_to_error
  Given a client with a zero timeout and a caller whose response never resolves
  When fact_check runs
  Then it returns Err(CoreError::Mempal)
