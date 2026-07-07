spec: task
name: "Content-addressed artifacts repository"
tags: [store, mvp, p0]
---

## Intent

Artifact *pointers* (kind + path + sha256 + byte length) are stored keyed by
their content hash. These are inherent `SqliteStore` methods, not part of the
engine-facing `Store` trait (the engine records artifacts inside `Outcome`; the
daemon persists pointers). Insert is idempotent on `sha256`.

## Decisions

- `insert_artifact(pool, &Artifact, run_id?, node_id?)` upserts `ON CONFLICT(sha256) DO NOTHING`; `get_artifact(pool, sha256) -> Option<Artifact>`.
- `ArtifactKind` round-trips as a kebab-case TEXT column (`context-pack`, etc.).
- agentd-store stores only a pointer, never artifact bytes (design §3.1).

## Boundaries

### Allowed Changes

- crates/agentd-store/src/artifact_repo.rs and lib.rs
- crates/agentd-store/tests/store_trait.rs

### Forbidden

- Do not store artifact bytes in the database (pointer only).

## Completion Criteria

Scenario: An artifact pointer round-trips and is idempotent on its hash
  Test: artifact_round_trips_content_addressed
  Given an artifact pointer
  When it is inserted twice and read back by sha256
  Then the second insert is a no-op and the kind, bytes, and path survive
