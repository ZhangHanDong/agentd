# M2 Plan B (fleet inventory + native dispatch) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The worker fleet's capability/capacity inventory becomes real (workers declare a concurrent-task capacity and runtime capabilities at registration; the durable scheduler's `acquire` honors both, plus a minimum protocol-version floor and recorded/exposed network zone), and the daemon's production workflow dispatch path can route a task to native workers through the durable queue behind an explicit configuration switch — proven end-to-end without tmux.

**Architecture:** Registration gains a `capacity` (migration 0024) and a wire `protocol_version` the daemon floors against a constant. `SqliteDurableScheduler::acquire` (Plan A) grows two guards inside its existing `BEGIN IMMEDIATE` transaction: a capacity pre-check (never grant beyond an incarnation's open active leases) and a capability filter (a task whose execution spec declares a `provider` is only granted to a worker whose capabilities list that runtime). Two Plan A carry-overs land here: reconcile threads the lease's `terminal_reason` into the queue row so `explain_task` distinguishes success from failure, and `ExecutionSecurityScope` carries the target repository binding so worker evidence links stop reporting `"unspecified"`. Finally a config switch (`DaemonConfig.native_dispatch`) selects a native dispatch route that enqueues a task into the durable queue instead of composing tmux; tmux stays the default.

**Tech Stack:** Rust workspace (sqlx/SQLite, axum, tokio); no new external dependencies.

**Design reference:** `docs/superpowers/specs/2026-07-22-agent-chat-replacement-milestones-design.md` §M2 — item 2 (worker fleet capability/capacity inventory, zone, version negotiation) and the "Done when" exit-gate clause "the production workflow dispatch path can route work to native workers, so tmux is no longer the only production launch path". Builds directly on Plan A (`docs/superpowers/plans/2026-07-25-m2-durable-scheduler.md`).

## Global Constraints

- **Error classification (verbatim):** only `Unavailable` is retryable and maps to HTTP 503; `Invalid` → 400; `NotFound` → 404; `Conflict`/`LeaseRejected` → 409. Do not reclassify a version/capability/capacity rejection as retryable.
- **Transaction discipline:** every queue/lease mutation runs inside `BEGIN IMMEDIATE` and guards each conditional write with a `rows_affected()` check (mirror Plan A's `grant_and_transition` / `terminalize_closed_row`).
- **Idempotency (Plan A semantics):** the same `request_id` replays the original row/grant byte-for-byte; a different payload under the same `request_id` is a `Conflict`. New behavior must not weaken this.
- **Workers never open the daemon DB.** All new daemon-side reads/writes go through the pool the daemon already owns; worker-side code touches only its disposable scratch store.
- **Blocking IO under `spawn_blocking`.** No synchronous filesystem/process IO on an async runtime thread; keep sqlx calls `.await`ed as the existing code does.
- **Schema changes are a single new migration `0024`, bumping `schema_meta.version` from 23 to 24.** In the *same task* as the migration, update every version assertion: the nine `assert_eq!(version, "23")` sites in `crates/agentd-store/tests/migration.rs` (lines 80, 137, 228, 297, 349, 369, 479, 526, 590, 659, 852 — grep to confirm the current set) and `assert_eq!(report.schema_version, 23)` in `crates/agentd-store/tests/operational_doctor.rs` (line 23). `migration_backcompat.rs` asserts frozen historical versions (13/14/15) and must NOT change.
- **Wire compatibility:** every new field on a request struct deserialized from the network is `#[serde(default)]` (or defaulted via a named function) so an older peer keeps deserializing; construction sites in this repo are updated explicitly.
- **No new external dependencies** in any `Cargo.toml`.
- **Tests never run real Claude/Codex/tmux/Matrix.** The one known env-sensitive flake is `agentd-tmux::native native_runtime_can_terminate_a_running_child` — rerun it in isolation if it fails under full load.
- **Non-goals (explicitly OUT of Plan B):** requeue backoff, an outbox consumer, and `db.code()`-based SQLite constraint matching (keep Plan A's error-message matching). Do not add these.
- **Commits:** `type(scope): summary` + trailing `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`. Run `cargo fmt --all` and the task's `cargo clippy … -- -D warnings` before every commit.

## File Structure

| File | Role |
|---|---|
| `crates/agentd-store/migrations/0024_worker_capacity.sql` (new) | `capacity` column on `worker_incarnations`; version → 24 |
| `crates/agentd-core/src/ports/worker_fleet.rs` | `WorkerFleetRegisterRequest` gains `capacity` + `protocol_version`; `WORKER_PROTOCOL_VERSION` / `MIN_WORKER_PROTOCOL_VERSION` constants |
| `crates/agentd-store/src/worker_repo.rs` | persist/read `capacity`; `WorkerIncarnationRecord.capacity`; `list_current_incarnations` |
| `crates/agentd-store/src/worker_fleet.rs` | version-floor rejection; thread `capacity` into registration |
| `crates/agentd-store/src/durable_scheduler.rs` | capacity + capability guards in `acquire`; reconcile reason threading |
| `crates/agentd-core/src/ports/security.rs` | `ExecutionSecurityScope` gains `target_repository_id` / `target_base_commit` |
| `crates/agentd-store/src/capability_repo.rs` | `scope_for_snapshot` populates the repository binding |
| `crates/agentd-bin/src/worker_main.rs` | report repository binding from the scope instead of `"unspecified"` |
| `crates/agentd-bin/src/daemon.rs` | fleet inventory route; `DispatchRoute` + `dispatch_task_to_fleet`; resolve target repo on the daemon-local acknowledge path |
| `crates/agentd-bin/src/cli.rs` | `DaemonConfig.native_dispatch` from `AGENTD_NATIVE_DISPATCH` |
| Tests | `crates/agentd-store/tests/migration.rs`, `worker_fleet.rs`, `enterprise_scheduler.rs`, `crates/agentd-bin/tests/recovery_http.rs`, `worker_main.rs`, `docs/parity/agent-chat-capability-map.md` |

---

### Task 1: Schema + registration contract — `capacity` column, wire fields, protocol constants

**Files:**
- Create: `crates/agentd-store/migrations/0024_worker_capacity.sql`
- Modify: `crates/agentd-core/src/ports/worker_fleet.rs`, `crates/agentd-core/src/ports/mod.rs`
- Modify: `crates/agentd-store/src/worker_repo.rs`, `crates/agentd-store/src/worker_fleet.rs`
- Test: `crates/agentd-store/tests/migration.rs`, `crates/agentd-store/tests/worker_fleet.rs`

**Interfaces:**
- Consumes: Plan A migration 0023, `worker_incarnations` table (columns `capabilities_json`, `network_zone`), `WorkerRegistration`/`WorkerIncarnationRecord` in `worker_repo.rs`.
- Produces (later tasks rely on these exact names):
  - `pub const WORKER_PROTOCOL_VERSION: u32 = 1;` and `pub const MIN_WORKER_PROTOCOL_VERSION: u32 = 1;` in `crates/agentd-core/src/ports/worker_fleet.rs`, re-exported from `ports::mod`.
  - `WorkerFleetRegisterRequest` fields `pub capacity: u32` (serde default `default_worker_capacity` = 1) and `pub protocol_version: u32` (serde default = 0).
  - `WorkerRegistration.capacity: u32` and `WorkerIncarnationRecord.capacity: u32` in `worker_repo.rs`; `worker_incarnations.capacity` column (`INTEGER NOT NULL DEFAULT 1`).
  - `schema_meta.version = '24'`.

- [ ] **Step 1: Write the failing migration + capacity round-trip tests**

Append to `crates/agentd-store/tests/migration.rs` (mirror the existing `PRAGMA table_info` style):

```rust
#[tokio::test]
async fn migration_adds_worker_incarnation_capacity_column() {
    let (store, _dir) = open_temp().await;
    let rows = sqlx::query("PRAGMA table_info(worker_incarnations)")
        .fetch_all(store.pool())
        .await
        .expect("worker_incarnations columns");
    let columns: Vec<String> = rows.iter().map(|r| r.get::<String, _>("name")).collect();
    assert!(
        columns.contains(&"capacity".to_string()),
        "missing worker_incarnations.capacity; got {columns:?}"
    );
    let version: String =
        sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'version'")
            .fetch_one(store.pool())
            .await
            .expect("schema version row");
    assert_eq!(version, "24");
}
```

Append to `crates/agentd-store/tests/worker_fleet.rs` a repo-level capacity round-trip (reuse the file's existing worker/incarnation seeding helpers; if it registers through `worker_repo::register_incarnation`, set `capacity: 4`):

```rust
#[tokio::test]
async fn register_incarnation_persists_declared_capacity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let worker_id = WorkerId::new();
    worker_repo::create_worker(
        store.pool(),
        WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "corp-coding".to_string(),
            labels: serde_json::json!({}),
        },
    )
    .await
    .expect("worker");
    let incarnation_id = WorkerIncarnationId::new();
    worker_repo::register_incarnation(
        store.pool(),
        &worker_id,
        WorkerRegistration {
            id: incarnation_id.clone(),
            daemon_version: "0.0.0-test".to_string(),
            host_name: "host-a".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: serde_json::json!({"runtime": ["codex"]}),
            capacity: 4,
        },
    )
    .await
    .expect("incarnation");
    let record = worker_repo::get_incarnation(store.pool(), &incarnation_id)
        .await
        .expect("read")
        .expect("incarnation exists");
    assert_eq!(record.capacity, 4);
    assert_eq!(record.network_zone.as_deref(), Some("dev"));
}
```

(`worker_fleet.rs` tests connect a store inline — `SqliteStore::connect(&dir.path().join("agentd.db"))` under a `tempfile::tempdir()` guard — rather than via an `open_temp` helper; keep `dir` bound so the tempdir outlives the store. Add the `use` lines mirroring the file's other tests if missing: `agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration}`, `agentd_core::types::{WorkerId, WorkerIncarnationId}`, `agentd_store::SqliteStore`.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p agentd-store --test migration migration_adds_worker_incarnation_capacity_column`
Expected: FAIL (`missing worker_incarnations.capacity` and/or version `"23" != "24"`).
Run: `cargo test -p agentd-store --test worker_fleet register_incarnation_persists_declared_capacity`
Expected: FAIL to compile (`WorkerRegistration` has no field `capacity`).

- [ ] **Step 3: Implement the migration**

Create `crates/agentd-store/migrations/0024_worker_capacity.sql`:

```sql
-- M2 Plan B: declared concurrent-task capacity per worker incarnation. The
-- durable scheduler's acquire never grants beyond an incarnation's open
-- active leases (design doc §M2 item 2). Existing rows default to 1.
ALTER TABLE worker_incarnations ADD COLUMN capacity INTEGER NOT NULL DEFAULT 1;

UPDATE schema_meta SET value = '24' WHERE key = 'version';
```

- [ ] **Step 4: Implement the contract fields**

4a. `crates/agentd-core/src/ports/worker_fleet.rs` — add the constants and `default_worker_capacity`, and the two fields on `WorkerFleetRegisterRequest`:

```rust
/// Wire protocol version this daemon build speaks to the fleet. Bumped when a
/// registration/pull/heartbeat contract changes in a way workers must match.
pub const WORKER_PROTOCOL_VERSION: u32 = 1;

/// Lowest protocol version the daemon will accept a registration from. A
/// worker below this floor is rejected at registration (version negotiation).
pub const MIN_WORKER_PROTOCOL_VERSION: u32 = 1;

fn default_worker_capacity() -> u32 {
    1
}
```

Add to `WorkerFleetRegisterRequest` (after `capabilities`):

```rust
    /// Maximum concurrent leases the daemon may grant this incarnation.
    #[serde(default = "default_worker_capacity")]
    pub capacity: u32,
    /// Worker's wire protocol version. An older peer that omits it deserializes
    /// as 0 and is rejected against `MIN_WORKER_PROTOCOL_VERSION`.
    #[serde(default)]
    pub protocol_version: u32,
```

Re-export the constants from `crates/agentd-core/src/ports/mod.rs` (add to the `worker_fleet` re-export list): `MIN_WORKER_PROTOCOL_VERSION, WORKER_PROTOCOL_VERSION`.

4b. `crates/agentd-store/src/worker_repo.rs` — add `capacity` to both structs and thread it through SQL:

- In `WorkerRegistration` add `pub capacity: u32,` (after `capabilities`).
- In `WorkerIncarnationRecord` add `pub capacity: u32,` (after `capabilities`).
- In `register_incarnation`, extend the INSERT column list with `capacity` and bind it. Replace the incarnation INSERT with:

```rust
    sqlx::query(
        "INSERT INTO worker_incarnations \
         (id, worker_id, daemon_version, host_name, network_zone, capabilities_json, \
          capacity, is_current, registered_at, last_seen_at, superseded_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?, NULL)",
    )
    .bind(registration.id.as_str())
    .bind(worker_id.as_str())
    .bind(&registration.daemon_version)
    .bind(&registration.host_name)
    .bind(&registration.network_zone)
    .bind(capabilities_json)
    .bind(i64::from(registration.capacity.max(1)))
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
```

- In `get_incarnation` and `current_incarnation`, add `capacity` to the SELECT column list (both queries): `… capabilities_json, capacity, is_current, …`.
- In `row_to_incarnation`, read it:

```rust
        capacity: u32::try_from(row.get::<i64, _>("capacity")).unwrap_or(1).max(1),
```

4c. `crates/agentd-store/src/worker_fleet.rs` — thread `capacity` from the register request into `WorkerRegistration` (inside `register`, the `WorkerRegistration { … }` literal):

```rust
            WorkerRegistration {
                id: request.incarnation_id.clone(),
                daemon_version: request.daemon_version.clone(),
                host_name: request.host_name.clone(),
                network_zone: request.network_zone.clone(),
                capabilities: request.capabilities.clone(),
                capacity: request.capacity,
            },
```

4d. Fix every other construction site the compiler flags (mirror Plan A's sweep pattern):
- `WorkerRegistration { … }` literals — grep `WorkerRegistration {` across `crates/` (≈16 test files + `worker_repo.rs`/`worker_fleet.rs`). Each test literal gains `capacity: 1,` unless the test is capacity-specific.
- `WorkerFleetRegisterRequest { … }` literals — grep `WorkerFleetRegisterRequest {` (`worker_main.rs`, `crates/agentd-bin/tests/worker_fleet_http.rs`, `crates/agentd-store/tests/worker_fleet.rs`). Each gains `capacity: default,` and `protocol_version: agentd_core::ports::WORKER_PROTOCOL_VERSION,` — for tests use `capacity: 1`. In `crates/agentd-bin/src/worker_main.rs`'s `registration`, use `capacity: 1` and `protocol_version: agentd_core::ports::WORKER_PROTOCOL_VERSION`.

- [ ] **Step 5: Update the version assertions (same task as the migration)**

Change each `assert_eq!(version, "23")` in `crates/agentd-store/tests/migration.rs` to `"24"` (grep `assert_eq!(version, "23")` — update every hit; do NOT touch `migration_backcompat.rs`). Change `assert_eq!(report.schema_version, 23)` → `24` in `crates/agentd-store/tests/operational_doctor.rs`.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p agentd-store --test migration && cargo test -p agentd-store --test worker_fleet && cargo test -p agentd-store --test operational_doctor && cargo check -p agentd-core -p agentd-bin`
Expected: PASS.

- [ ] **Step 7: Gate and commit**

```bash
cargo fmt --all
cargo clippy -p agentd-core -p agentd-store -p agentd-bin --all-targets -- -D warnings
cargo nextest run -p agentd-store
git add crates/agentd-core crates/agentd-store crates/agentd-bin
git commit -m "feat(fleet): declare worker capacity and protocol version at registration

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Version negotiation + fleet inventory exposure

**Files:**
- Modify: `crates/agentd-store/src/worker_fleet.rs` (`register` floors protocol version)
- Modify: `crates/agentd-store/src/worker_repo.rs` (`list_current_incarnations`)
- Modify: `crates/agentd-bin/src/daemon.rs` (`GET /api/fleet/workers` on the recovery router)
- Test: `crates/agentd-store/tests/worker_fleet.rs`, `crates/agentd-bin/tests/recovery_http.rs`

**Interfaces:**
- Consumes: Task 1's `MIN_WORKER_PROTOCOL_VERSION`, `WorkerFleetRegisterRequest.protocol_version`, `WorkerIncarnationRecord.capacity`; the recovery router's `recovery_unauthorized(&state, &headers)` helper and `WorkerFleetService::store_pool()` (Plan A Task 6).
- Produces:
  - `worker_repo::list_current_incarnations(pool: &SqlitePool) -> Result<Vec<WorkerIncarnationRecord>, StoreError>` — all `is_current = 1` rows.
  - Route `GET /api/fleet/workers` → 401 unauthorized / 200 JSON array of `{ worker_id, incarnation_id, network_zone, capabilities, capacity, open_leases, daemon_version, host_name }`.
  - `register` returns `WorkerFleetError::Invalid` when `protocol_version < MIN_WORKER_PROTOCOL_VERSION`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/agentd-store/tests/worker_fleet.rs`:

```rust
#[tokio::test]
async fn register_rejects_worker_below_protocol_floor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let fleet = SqliteWorkerFleet::new(store.pool().clone());
    let request = WorkerFleetRegisterRequest {
        auth_proof: String::new(),
        worker_id: WorkerId::new(),
        trust_domain: "corp-coding".to_string(),
        labels: serde_json::json!({}),
        incarnation_id: WorkerIncarnationId::new(),
        daemon_version: "0.0.0-test".to_string(),
        host_name: "host-a".to_string(),
        network_zone: Some("dev".to_string()),
        capabilities: serde_json::json!({"runtime": ["codex"]}),
        capacity: 1,
        protocol_version: 0, // below the floor of 1
    };
    let error = fleet
        .register(&request)
        .await
        .expect_err("stale protocol must be rejected");
    assert!(matches!(error, WorkerFleetError::Invalid(_)));
}

#[tokio::test]
async fn list_current_incarnations_exposes_zone_and_capacity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let worker_id = WorkerId::new();
    worker_repo::create_worker(
        store.pool(),
        WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "corp-coding".to_string(),
            labels: serde_json::json!({}),
        },
    )
    .await
    .expect("worker");
    worker_repo::register_incarnation(
        store.pool(),
        &worker_id,
        WorkerRegistration {
            id: WorkerIncarnationId::new(),
            daemon_version: "0.0.0-test".to_string(),
            host_name: "host-a".to_string(),
            network_zone: Some("us-east".to_string()),
            capabilities: serde_json::json!({"runtime": ["codex"]}),
            capacity: 3,
        },
    )
    .await
    .expect("incarnation");
    let listed = worker_repo::list_current_incarnations(store.pool())
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].capacity, 3);
    assert_eq!(listed[0].network_zone.as_deref(), Some("us-east"));
}
```

(Ensure the test module imports `agentd_core::ports::{WorkerFleetRegisterRequest, WorkerFleetError}`, `SqliteWorkerFleet`, and the `worker_repo`/type `use`s already added in Task 1.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p agentd-store --test worker_fleet register_rejects_worker_below_protocol_floor list_current_incarnations_exposes_zone_and_capacity`
Expected: FAIL — register accepts protocol 0 (no floor yet); `list_current_incarnations` unresolved.

- [ ] **Step 3: Implement the version floor**

In `crates/agentd-store/src/worker_fleet.rs`, at the top of `register` (right after `self.authorize(&request.auth_proof)?;`), add:

```rust
        if request.protocol_version < agentd_core::ports::MIN_WORKER_PROTOCOL_VERSION {
            return Err(WorkerFleetError::Invalid(format!(
                "worker protocol version {} is below the minimum supported {}",
                request.protocol_version,
                agentd_core::ports::MIN_WORKER_PROTOCOL_VERSION
            )));
        }
```

- [ ] **Step 4: Implement the inventory read**

In `crates/agentd-store/src/worker_repo.rs`, add after `current_incarnation`:

```rust
/// List every current worker incarnation for fleet inventory/exposure.
///
/// # Errors
/// Returns [`StoreError`] if a row cannot be read or decoded.
pub async fn list_current_incarnations(
    pool: &SqlitePool,
) -> Result<Vec<WorkerIncarnationRecord>, StoreError> {
    let rows = sqlx::query(
        "SELECT id, worker_id, daemon_version, host_name, network_zone, capabilities_json, \
         capacity, is_current, registered_at, last_seen_at, superseded_at \
         FROM worker_incarnations WHERE is_current = 1 ORDER BY registered_at ASC",
    )
    .fetch_all(pool)
    .await?;
    rows.iter().map(row_to_incarnation).collect()
}
```

- [ ] **Step 5: Implement the inventory HTTP route**

In `crates/agentd-bin/src/daemon.rs`, add `.route("/api/fleet/workers", get(fleet_inventory))` to `recovery_router` (alongside the Plan A explain route), and:

```rust
async fn fleet_inventory(
    State(state): State<RecoveryApiState>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = recovery_unauthorized(&state, &headers) {
        return response;
    }
    let pool = state.service.store_pool();
    let incarnations = match agentd_store::worker_repo::list_current_incarnations(&pool).await {
        Ok(rows) => rows,
        Err(error) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": error.to_string() })),
            )
                .into_response();
        }
    };
    let mut workers = Vec::with_capacity(incarnations.len());
    for incarnation in incarnations {
        let open_leases: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution_task_leases \
             WHERE worker_incarnation_id = ? AND status = 'active'",
        )
        .bind(incarnation.id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        workers.push(json!({
            "worker_id": incarnation.worker_id.as_str(),
            "incarnation_id": incarnation.id.as_str(),
            "network_zone": incarnation.network_zone,
            "capabilities": incarnation.capabilities,
            "capacity": incarnation.capacity,
            "open_leases": open_leases,
            "daemon_version": incarnation.daemon_version,
            "host_name": incarnation.host_name,
        }));
    }
    (StatusCode::OK, Json(json!({ "workers": workers }))).into_response()
}
```

(`get`, `State`, `HeaderMap`, `Json`, `StatusCode`, `Response`, `json!` are already imported for the Plan A explain route; add `IntoResponse` to the `use` if the compiler asks.)

- [ ] **Step 6: Write the HTTP inventory test**

Append to `crates/agentd-bin/tests/recovery_http.rs` (reuse its service/app construction and worker-seeding from the acknowledge/explain tests):

```rust
#[tokio::test]
async fn recovery_http_exposes_fleet_inventory() {
    // Standard fixture: store + service + recovery_router("operator-secret"),
    // one worker registered + online with network_zone "us-east", capacity 2.
    let unauthorized = app
        .clone()
        .oneshot(
            Request::get("/api/fleet/workers")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let listed = app
        .clone()
        .oneshot(
            Request::get("/api/fleet/workers")
                .header("authorization", "Bearer operator-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(listed.status(), StatusCode::OK);
    let body = listed.into_body().collect().await.expect("body").to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let workers = json["workers"].as_array().expect("workers array");
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0]["network_zone"], "us-east");
    assert_eq!(workers[0]["capacity"], 2);
    assert_eq!(workers[0]["open_leases"], 0);
}
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p agentd-store --test worker_fleet && cargo test -p agentd-bin --test recovery_http`
Expected: PASS.

- [ ] **Step 8: Gate and commit**

```bash
cargo fmt --all
cargo clippy -p agentd-store -p agentd-bin --all-targets -- -D warnings
cargo nextest run -p agentd-store -p agentd-bin
git add crates/agentd-store crates/agentd-bin
git commit -m "feat(fleet): floor worker protocol version and expose fleet inventory

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Capacity- and capability-aware `acquire`

**Files:**
- Modify: `crates/agentd-store/src/durable_scheduler.rs`
- Test: `crates/agentd-store/tests/enterprise_scheduler.rs`

**Interfaces:**
- Consumes: Task 1's `worker_incarnations.capacity` + `capabilities_json`; Plan A's `acquire_in_transaction`, `task_is_open`, `terminalize_closed_row`, `grant_and_transition`; `task_runs.execution_spec_json` (`NativeExecutionSpec` serialized with a `provider` string field); the `enterprise_scheduler.rs` `fixture()` / `enqueue_request` / `acquire_request` / `scheduler_for` helpers (Plan A).
- Produces: `acquire` returns `Ok(None)` when the incarnation is at capacity or when no *capability-compatible* queued row is eligible; a task whose `execution_spec_json` declares `provider = P` is granted only to an incarnation whose `capabilities.runtime` contains `P`; a task with no execution spec (`provider` NULL) stays takeable by any worker (M1 legacy behavior preserved).

- [ ] **Step 1: Write the failing tests**

The `enterprise_scheduler.rs` fixture registers an incarnation with `capabilities: {"runtime": ["codex"]}` and default `capacity` 1. Add small helpers at the top of the file for this task:

```rust
async fn attach_spec(fixture: &Fixture, provider: &str) {
    let spec = agentd_core::types::NativeExecutionSpec {
        version: 1,
        provider: provider.to_string(),
        program: format!("/usr/bin/{provider}"),
        args: vec![],
        cwd: None,
        env: vec![],
    };
    fixture
        .store
        .set_task_execution_spec(&fixture.task_id, &spec)
        .await
        .expect("attach spec");
}

/// Register a second worker+incarnation with the given capabilities and
/// capacity, returning its incarnation id. Mirrors the fixture's seeding.
async fn register_worker(
    fixture: &Fixture,
    runtime: &str,
    capacity: u32,
) -> agentd_core::types::WorkerIncarnationId {
    let worker_id = agentd_core::types::WorkerId::new();
    agentd_store::worker_repo::create_worker(
        fixture.store.pool(),
        agentd_store::worker_repo::WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "corp-coding".to_string(),
            labels: serde_json::json!({}),
        },
    )
    .await
    .expect("worker");
    let incarnation_id = agentd_core::types::WorkerIncarnationId::new();
    agentd_store::worker_repo::register_incarnation(
        fixture.store.pool(),
        &worker_id,
        agentd_store::worker_repo::WorkerRegistration {
            id: incarnation_id.clone(),
            daemon_version: "0.0.0-test".to_string(),
            host_name: "host-b".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: serde_json::json!({ "runtime": [runtime] }),
            capacity,
        },
    )
    .await
    .expect("incarnation");
    incarnation_id
}
```

Then the tests:

```rust
#[tokio::test]
async fn acquire_skips_task_whose_runtime_the_worker_cannot_run() {
    let fixture = fixture().await;
    let scheduler = scheduler_for(&fixture);
    attach_spec(&fixture, "codex").await;
    scheduler
        .enqueue(&enqueue_request(&fixture, "rq-1", 10))
        .await
        .expect("enqueue");
    // A worker that only runs claude-code must not be granted the codex task.
    let claude_only = register_worker(&fixture, "claude-code", 4).await;
    let none = scheduler
        .acquire(&SchedulerAcquireRequest {
            request_id: "acq-claude".to_string(),
            worker_incarnation_id: claude_only,
            observed_at: 20,
            expires_at: 80,
        })
        .await
        .expect("acquire");
    assert!(none.is_none(), "capability mismatch must not grant");
    // The fixture's codex-capable incarnation still gets it.
    let grant = scheduler
        .acquire(&acquire_request(&fixture, "acq-codex", 20, 80))
        .await
        .expect("acquire")
        .expect("codex worker eligible");
    assert_eq!(grant.execution_task_id, fixture.task_id);
}

#[tokio::test]
async fn acquire_grants_task_without_declared_runtime_to_any_worker() {
    let fixture = fixture().await;
    let scheduler = scheduler_for(&fixture);
    // No execution spec attached -> provider NULL -> unconstrained.
    scheduler
        .enqueue(&enqueue_request(&fixture, "rq-1", 10))
        .await
        .expect("enqueue");
    let any_worker = register_worker(&fixture, "claude-code", 4).await;
    let grant = scheduler
        .acquire(&SchedulerAcquireRequest {
            request_id: "acq-1".to_string(),
            worker_incarnation_id: any_worker,
            observed_at: 20,
            expires_at: 80,
        })
        .await
        .expect("acquire")
        .expect("unconstrained task grantable");
    assert_eq!(grant.execution_task_id, fixture.task_id);
}

#[tokio::test]
async fn acquire_refuses_to_exceed_worker_capacity() {
    let fixture = fixture().await;
    let scheduler = scheduler_for(&fixture);
    // capacity 1 incarnation (fixture default). Enqueue two tasks.
    let second_task = seed_second_task(&fixture).await; // see helper note below
    scheduler
        .enqueue(&enqueue_request(&fixture, "rq-1", 10))
        .await
        .expect("enqueue first");
    scheduler
        .enqueue(&SchedulerEnqueueRequest {
            request_id: "rq-2".to_string(),
            execution_task_id: second_task,
            max_attempts: 3,
            available_at: 10,
            enqueued_at: 11,
        })
        .await
        .expect("enqueue second");

    let first = scheduler
        .acquire(&acquire_request(&fixture, "acq-1", 20, 80))
        .await
        .expect("acquire")
        .expect("first grant");
    assert_eq!(first.execution_task_id, fixture.task_id);
    // The same incarnation now holds 1 active lease == capacity 1: no more.
    let second = scheduler
        .acquire(&acquire_request(&fixture, "acq-2", 21, 80))
        .await
        .expect("acquire");
    assert!(second.is_none(), "capacity 1 worker must not get a second lease");
}
```

Add a `seed_second_task(&fixture) -> TaskRunId` helper that inserts a second `task_runs` row under the fixture's run (mirror the fixture's `task_repo::insert_task_run(pool, &run_id, &NodeId::parsed("impl-2"))`). Import `SchedulerEnqueueRequest`, `SchedulerAcquireRequest` if not already in scope.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p agentd-store --test enterprise_scheduler acquire_skips_task_whose_runtime_the_worker_cannot_run acquire_refuses_to_exceed_worker_capacity acquire_grants_task_without_declared_runtime_to_any_worker`
Expected: FAIL — capability mismatch is granted; capacity is ignored (second grant succeeds).

- [ ] **Step 3: Implement the guards in `acquire_in_transaction`**

3a. Add a capability parser near the top of `durable_scheduler.rs` (after `queue_record`):

```rust
/// Extract the worker's runnable runtime kinds from its capabilities JSON,
/// e.g. `{"runtime": ["codex", "claude-code"]}` -> `["codex", "claude-code"]`.
fn worker_runtime_capabilities(capabilities_json: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(capabilities_json)
        .ok()
        .and_then(|value| {
            value
                .get("runtime")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
        })
        .unwrap_or_default()
}
```

3b. In `acquire_in_transaction`, after the replay-return block and before the selection loop, insert the capacity + capability preamble:

```rust
    // Capacity + capability preamble. Read the acquiring incarnation once.
    let Some((capacity, capabilities_json)) = sqlx::query_as::<_, (i64, String)>(
        "SELECT capacity, capabilities_json FROM worker_incarnations WHERE id = ?",
    )
    .bind(request.worker_incarnation_id.as_str())
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?
    else {
        return Err(DurableSchedulerError::NotFound(
            "worker incarnation not found".into(),
        ));
    };

    // Capacity: never grant beyond the incarnation's open active leases.
    let open_leases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_task_leases \
         WHERE worker_incarnation_id = ? AND status = 'active'",
    )
    .bind(request.worker_incarnation_id.as_str())
    .fetch_one(&mut *connection)
    .await
    .map_err(storage_error)?;
    if open_leases >= capacity {
        return Ok(None);
    }

    // Capability filter, applied in SQL so an incompatible row is never
    // selected (and thus never spins the terminalize loop). A task with no
    // execution spec declares no provider and stays unconstrained.
    let runtimes = worker_runtime_capabilities(&capabilities_json);
    let provider_expr = "json_extract(t.execution_spec_json, '$.provider')";
    let capability_clause = if runtimes.is_empty() {
        format!("{provider_expr} IS NULL")
    } else {
        let placeholders = std::iter::repeat("?")
            .take(runtimes.len())
            .collect::<Vec<_>>()
            .join(", ");
        format!("({provider_expr} IS NULL OR {provider_expr} IN ({placeholders}))")
    };
    let select_sql = format!(
        "SELECT q.id, q.execution_task_id FROM execution_task_queue q \
         JOIN task_runs t ON t.id = q.execution_task_id \
         WHERE q.status = 'queued' AND q.available_at <= ? AND {capability_clause} \
         ORDER BY q.enqueued_at ASC, q.id ASC LIMIT 1"
    );
```

3c. Replace the selection query inside the existing `loop { … }` so it uses `select_sql` with the capability binds (keep the `task_is_open` / `terminalize_closed_row` / `grant_and_transition` body unchanged):

```rust
    loop {
        let mut query =
            sqlx::query_as::<_, (String, String)>(&select_sql).bind(request.observed_at);
        for runtime in &runtimes {
            query = query.bind(runtime);
        }
        let row = query
            .fetch_optional(&mut *connection)
            .await
            .map_err(storage_error)?;
        let Some((queue_id, task_id)) = row else {
            return Ok(None);
        };

        if !task_is_open(connection, &task_id).await? {
            terminalize_closed_row(connection, &queue_id, &task_id, request.observed_at).await?;
            continue;
        }

        let grant = grant_and_transition(connection, request, &queue_id, &task_id).await?;
        return Ok(Some(grant));
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p agentd-store --test enterprise_scheduler`
Expected: all PASS — the new three plus every Plan A scheduler test (unconstrained tasks and the single-task concurrency test are unaffected).

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all
cargo clippy -p agentd-store --all-targets -- -D warnings
cargo nextest run -p agentd-store
git add crates/agentd-store
git commit -m "feat(scheduler): honor worker capacity and runtime capability on acquire

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Explain/evidence fidelity — reconcile reason + repository binding

**Files:**
- Modify: `crates/agentd-store/src/durable_scheduler.rs` (`reconcile_in_transaction` threads the lease reason)
- Modify: `crates/agentd-core/src/ports/security.rs` (`ExecutionSecurityScope` fields)
- Modify: `crates/agentd-store/src/capability_repo.rs` (`scope_for_snapshot`)
- Modify: `crates/agentd-bin/src/worker_main.rs`, `crates/agentd-bin/src/daemon.rs` (report the repository binding)
- Test: `crates/agentd-store/tests/enterprise_scheduler.rs`, `crates/agentd-bin/tests/worker_main.rs`

**Interfaces:**
- Consumes: Plan A `reconcile_in_transaction`; `execution_task_leases.terminal_reason`; `ProjectExecutionSnapshot::target_repository()` → `&RepositoryBinding { repository_ref, base_commit, … }` with `RepositoryRef::resource_id()`; `capability_repo::scope_for_snapshot(&ProjectExecutionSnapshot, TaskLeaseClaim) -> ExecutionSecurityScope`; `project_authority_repo::get_snapshot(pool, snapshot_ref) -> Result<ProjectExecutionSnapshot, StoreError>`.
- Produces:
  - `SchedulerQueueRecord.last_reason` (via reconcile) contains the lease's `terminal_reason`, so `explain_task` distinguishes a successful release ("worker execution complete") from a failed one ("worker execution failed: …").
  - `ExecutionSecurityScope` gains `pub target_repository_id: String` and `pub target_base_commit: String`, populated by `scope_for_snapshot` from the snapshot's target repository (falling back to `"unspecified"` when the snapshot declares no target). Worker + daemon-local evidence links report these instead of the hard-coded `"unspecified"`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/agentd-store/tests/enterprise_scheduler.rs`:

```rust
#[tokio::test]
async fn reconcile_threads_release_reason_into_last_reason() {
    let fixture = fixture().await;
    let scheduler = scheduler_for(&fixture);
    scheduler
        .enqueue(&enqueue_request(&fixture, "rq-1", 10))
        .await
        .expect("enqueue");
    let grant = scheduler
        .acquire(&acquire_request(&fixture, "acq-1", 20, 80))
        .await
        .expect("acquire")
        .expect("grant");
    let lease_plane = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());
    lease_plane
        .release(&TaskLeaseCloseRequest {
            claim: grant.claim(),
            observed_at: 30,
            reason: "worker execution failed: boom".to_string(),
        })
        .await
        .expect("release");

    scheduler.reconcile(31).await.expect("reconcile");
    let explanation = scheduler
        .explain_task(&fixture.task_id)
        .await
        .expect("explain")
        .expect("row");
    assert_eq!(explanation.queue.status, SchedulerQueueStatus::Completed);
    assert!(
        explanation
            .queue
            .last_reason
            .as_deref()
            .unwrap_or("")
            .contains("worker execution failed: boom"),
        "explain must carry the worker's release reason, got {:?}",
        explanation.queue.last_reason
    );
}
```

Change the worker e2e assertion in `crates/agentd-bin/tests/worker_main.rs` (`worker_once_executes_a_dispatched_task_end_to_end`, currently asserting `"unspecified"` at lines ~296-298) to expect the authority snapshot's target repository (`authority_snapshot()` declares `RepositoryRef … "repo-1"` and `base_commit = "0123…4567"`):

```rust
    // The security scope now carries the target repository binding, so the
    // worker reports the real repository id and base commit from the
    // project-authority snapshot instead of the "unspecified" sentinel.
    for record in &artifacts.records {
        assert_eq!(record.publish.links.target_repository_id, "repo-1");
        assert_eq!(
            record.publish.links.target_base_commit,
            "0123456789abcdef0123456789abcdef01234567"
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p agentd-store --test enterprise_scheduler reconcile_threads_release_reason_into_last_reason`
Expected: FAIL — `last_reason` is `"lease … released"` with no worker reason.
Run: `cargo test -p agentd-bin --test worker_main worker_once_executes_a_dispatched_task_end_to_end`
Expected: FAIL — worker still reports `"unspecified"`.

- [ ] **Step 3: Implement reconcile reason threading**

In `crates/agentd-store/src/durable_scheduler.rs`, extend the `reconcile_in_transaction` SELECT to fetch `l.terminal_reason` and fold it into the queue `last_reason`. Replace the query + the `match` reason strings:

```rust
    let rows: Vec<(String, String, String, i64, i64, String, Option<String>)> = sqlx::query_as(
        "SELECT q.id, q.execution_task_id, q.current_lease_id, q.attempts, q.max_attempts, \
         l.status, l.terminal_reason \
         FROM execution_task_queue q \
         JOIN execution_task_leases l ON l.id = q.current_lease_id \
         WHERE q.status = 'leased' AND l.status != 'active'",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(storage_error)?;
    let mut changed = 0_u64;
    for (queue_id, task_id, lease_id, attempts, max_attempts, lease_status, terminal_reason) in rows
    {
        let lease_reason = terminal_reason.unwrap_or_default();
        let (new_status, reason, kind) = match lease_status.as_str() {
            "released" => (
                "completed",
                format!("lease {lease_id} released: {lease_reason}"),
                "task_completed",
            ),
            "cancelled" => (
                "cancelled",
                format!("lease {lease_id} cancelled: {lease_reason}"),
                "task_cancelled",
            ),
            // expired / superseded: retry or dead-letter.
            other => {
                if attempts >= max_attempts {
                    (
                        "dead_letter",
                        format!("lease {lease_id} {other}: {lease_reason}; attempts exhausted"),
                        "task_dead_lettered",
                    )
                } else {
                    (
                        "queued",
                        format!("lease {lease_id} {other}: {lease_reason}; requeued"),
                        "task_requeued",
                    )
                }
            }
        };
```

(The rest of the loop body — the guarded `UPDATE execution_task_queue` and the outbox `INSERT` — is unchanged.)

- [ ] **Step 4: Implement repository binding in the scope**

4a. `crates/agentd-core/src/ports/security.rs` — add to `ExecutionSecurityScope` (after `egress_profile`):

```rust
    pub target_repository_id: String,
    pub target_base_commit: String,
```

4b. `crates/agentd-store/src/capability_repo.rs` — populate them in `scope_for_snapshot`. Add before the struct literal:

```rust
    let (target_repository_id, target_base_commit) = snapshot
        .target_repository()
        .map(|binding| {
            (
                binding.repository_ref.resource_id().to_string(),
                binding.base_commit.clone(),
            )
        })
        .unwrap_or_else(|_| ("unspecified".to_string(), "unspecified".to_string()));
```

and add `target_repository_id,` and `target_base_commit,` to the `ExecutionSecurityScope { … }` literal.

4c. `crates/agentd-bin/src/worker_main.rs` — in `execute_grant`, replace the hard-coded `target_repository_id` / `target_base_commit` (lines ~189-190) with values read from the grant's security scope:

```rust
                    target_repository_id: grant
                        .security_scope
                        .as_ref()
                        .map(|scope| scope.target_repository_id.clone())
                        .unwrap_or_else(|| "unspecified".to_string()),
                    target_base_commit: grant
                        .security_scope
                        .as_ref()
                        .map(|scope| scope.target_base_commit.clone())
                        .unwrap_or_else(|| "unspecified".to_string()),
```

Delete the stale "not yet transmitted … tracked for M2" comment block above those two fields.

4d. `crates/agentd-bin/src/daemon.rs` — the daemon-local acknowledge path (lines ~1020-1021) resolves the same snapshot the session references; replace the two `"unspecified"` literals by resolving the target repository from the session's project-authority snapshot. Just before building `links`, add:

```rust
        let snapshot_ref = format!(
            "{}:{}:{}:{}",
            session.snapshot.authority_key,
            session.snapshot.resource_kind,
            session.snapshot.resource_id,
            session.snapshot.resource_version
        );
        let (target_repository_id, target_base_commit) =
            match agentd_store::project_authority_repo::get_snapshot(
                self.native_worker.store().pool(),
                &snapshot_ref,
            )
            .await
            {
                Ok(snapshot) => snapshot
                    .target_repository()
                    .map(|binding| {
                        (
                            binding.repository_ref.resource_id().to_string(),
                            binding.base_commit.clone(),
                        )
                    })
                    .unwrap_or_else(|_| {
                        ("unspecified".to_string(), "unspecified".to_string())
                    }),
                Err(_) => ("unspecified".to_string(), "unspecified".to_string()),
            };
```

and set `target_repository_id,` / `target_base_commit,` in the `ExecutionEvidenceLinks { … }` literal (note `session.snapshot` is consumed later into `ExecutionSnapshotLink`; read the ref fields for `snapshot_ref` *before* that move, or clone the four fields — keep the borrow order correct).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p agentd-store --test enterprise_scheduler && cargo test -p agentd-bin --test worker_main && cargo check -p agentd-core --all-targets`
Expected: PASS. Also run `cargo test -p agentd-store --test enterprise_execution_artifacts` in case a scope-equality fixture needs the two new fields (add `target_repository_id`/`target_base_commit` to any `ExecutionSecurityScope { … }` literal the compiler flags).

- [ ] **Step 6: Gate and commit**

```bash
cargo fmt --all
cargo clippy -p agentd-core -p agentd-store -p agentd-bin --all-targets -- -D warnings
cargo nextest run -p agentd-core -p agentd-store -p agentd-bin
git add crates/agentd-core crates/agentd-store crates/agentd-bin
git commit -m "feat(scheduler): thread release reason and repository binding into evidence

Reconcile carries the lease terminal_reason into the queue row so explain
distinguishes a successful release from a failed one, and the execution
security scope now carries the target repository binding so worker and
daemon evidence links report the real repository id and base commit.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Native dispatch route + configuration switch

**Files:**
- Modify: `crates/agentd-bin/src/cli.rs` (`DaemonConfig.native_dispatch`)
- Modify: `crates/agentd-bin/src/daemon.rs` (`DispatchRoute`, `production_dispatch_route`, `dispatch_task_to_fleet`)
- Test: `crates/agentd-bin/tests/recovery_http.rs` (or a new `crates/agentd-bin/tests/native_dispatch.rs`)

**Interfaces:**
- Consumes: `DaemonConfig` (`crates/agentd-bin/src/cli.rs:274`), `SqliteStore::set_task_execution_spec`, Plan A `SqliteDurableScheduler::{new, enqueue}`, `SchedulerEnqueueRequest`, `NativeExecutionSpec`, `TaskRunId`.
- Produces:
  - `DaemonConfig.native_dispatch: bool` (default `false`; parsed from `AGENTD_NATIVE_DISPATCH` where the config is built).
  - `pub enum DispatchRoute { Tmux, NativeQueue }` (derive `Debug, Clone, Copy, PartialEq, Eq`).
  - `pub fn production_dispatch_route(config: &DaemonConfig) -> DispatchRoute` — `NativeQueue` iff `config.native_dispatch`, else `Tmux`.
  - `pub async fn dispatch_task_to_fleet(store: &SqliteStore, task_id: &TaskRunId, spec: &NativeExecutionSpec, observed_at: i64) -> Result<(), CoreError>` — attaches the execution spec to the task and enqueues it into the durable queue (request_id `format!("dispatch-{task_id}")`, `max_attempts = 3`, available/enqueued at `observed_at`), so native workers pull it. This is the native launch primitive the switch selects; tmux stays the default in `build_production_host`.

- [ ] **Step 1: Write the failing tests**

Create `crates/agentd-bin/tests/native_dispatch.rs`:

```rust
use agentd_bin::daemon::{DispatchRoute, dispatch_task_to_fleet, production_dispatch_route};
use agentd_bin::cli::DaemonConfig;
use agentd_core::types::{NativeExecutionSpec, NodeId, RunId};
use agentd_store::{SqliteStore, run_repo, task_repo};

fn config_with_native_dispatch(native: bool) -> DaemonConfig {
    // Build a DaemonConfig with defaults and flip native_dispatch. If
    // DaemonConfig has no Default, mirror the smallest constructor the crate
    // exposes for tests and set native_dispatch = `native`.
    let mut config = DaemonConfig::for_test();
    config.native_dispatch = native;
    config
}

#[tokio::test]
async fn default_route_is_tmux_and_switch_selects_native_queue() {
    assert_eq!(
        production_dispatch_route(&config_with_native_dispatch(false)),
        DispatchRoute::Tmux
    );
    assert_eq!(
        production_dispatch_route(&config_with_native_dispatch(true)),
        DispatchRoute::NativeQueue
    );
}

#[tokio::test]
async fn dispatch_task_to_fleet_enqueues_a_queued_row_with_spec() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let run_id = RunId::new();
    run_repo::insert_run(store.pool(), &run_id, "workflow-sha")
        .await
        .expect("run");
    let task_id = task_repo::insert_task_run(store.pool(), &run_id, &NodeId::parsed("impl"))
        .await
        .expect("task");
    let spec = NativeExecutionSpec {
        version: 1,
        provider: "codex".into(),
        program: "/usr/bin/codex".into(),
        args: vec![],
        cwd: None,
        env: vec![],
    };

    dispatch_task_to_fleet(&store, &task_id, &spec, 100)
        .await
        .expect("dispatch");

    let (status, provider): (String, Option<String>) = sqlx::query_as(
        "SELECT q.status, json_extract(t.execution_spec_json, '$.provider') \
         FROM execution_task_queue q JOIN task_runs t ON t.id = q.execution_task_id \
         WHERE q.execution_task_id = ?",
    )
    .bind(task_id.as_str())
    .fetch_one(store.pool())
    .await
    .expect("queue row");
    assert_eq!(status, "queued");
    assert_eq!(provider.as_deref(), Some("codex"));
}
```

If `DaemonConfig` has no test constructor, add a small `#[cfg(any(test, feature = "test-util"))]`-free `pub fn for_test() -> Self` in `cli.rs` that fills every field with harmless defaults (temp paths, `accept_workflow_change: false`, `native_dispatch: false`); reuse whatever the existing daemon tests already use to build a `DaemonConfig` if such a helper exists (grep `DaemonConfig {` in tests first and mirror it instead of adding one).

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p agentd-bin --test native_dispatch`
Expected: FAIL to compile (`native_dispatch`, `DispatchRoute`, `production_dispatch_route`, `dispatch_task_to_fleet` unresolved).

- [ ] **Step 3: Implement the config switch**

In `crates/agentd-bin/src/cli.rs`, add to `DaemonConfig` (after `accept_workflow_change`):

```rust
    /// Route production workflow dispatch to native workers through the durable
    /// queue instead of composing tmux. Off by default; tmux stays the fallback.
    pub native_dispatch: bool,
```

Set it where `DaemonConfig` is constructed from CLI/env (mirror how `accept_workflow_change` is sourced): `native_dispatch: std::env::var("AGENTD_NATIVE_DISPATCH").map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false),`. Update every other `DaemonConfig { … }` construction site the compiler flags (grep `DaemonConfig {`) with `native_dispatch: false,`.

- [ ] **Step 4: Implement the route + dispatch primitive**

In `crates/agentd-bin/src/daemon.rs`, add:

```rust
/// Which launch path production workflow dispatch uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchRoute {
    /// Compose tmux (the default, legacy production launch path).
    Tmux,
    /// Enqueue into the durable scheduler queue for native workers to pull.
    NativeQueue,
}

/// Select the production dispatch route from configuration. `NativeQueue` only
/// when the operator opts in; otherwise tmux remains the launch path.
#[must_use]
pub fn production_dispatch_route(config: &DaemonConfig) -> DispatchRoute {
    if config.native_dispatch {
        DispatchRoute::NativeQueue
    } else {
        DispatchRoute::Tmux
    }
}

/// Native launch primitive: attach the versioned execution spec to the task and
/// enqueue it into the durable scheduler queue so an online native worker pulls
/// and executes it — no tmux. This is the `NativeQueue` route's dispatch action.
///
/// # Errors
/// [`CoreError`] if the spec cannot be persisted or the enqueue fails.
pub async fn dispatch_task_to_fleet(
    store: &SqliteStore,
    task_id: &agentd_core::types::TaskRunId,
    spec: &agentd_core::types::NativeExecutionSpec,
    observed_at: i64,
) -> Result<(), CoreError> {
    use agentd_core::ports::Store as _;
    store.set_task_execution_spec(task_id, spec).await?;
    let scheduler =
        agentd_store::durable_scheduler::SqliteDurableScheduler::new(store.pool().clone());
    use agentd_core::ports::DurableSchedulerPort as _;
    scheduler
        .enqueue(&agentd_core::ports::SchedulerEnqueueRequest {
            request_id: format!("dispatch-{}", task_id.as_str()),
            execution_task_id: task_id.clone(),
            max_attempts: 3,
            available_at: observed_at,
            enqueued_at: observed_at,
        })
        .await
        .map_err(|error| CoreError::Backend(error.to_string()))?;
    Ok(())
}
```

(`agentd_core::ports::Store::set_task_execution_spec` returns `Result<(), CoreError>`, so the `?` propagates directly with the `use … Store as _;` in scope. `CoreError::Backend(String)` exists — it is the right variant for wrapping the scheduler's stringly error.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p agentd-bin --test native_dispatch`
Expected: PASS.

- [ ] **Step 6: Gate and commit**

```bash
cargo fmt --all
cargo clippy -p agentd-bin --all-targets -- -D warnings
cargo nextest run -p agentd-bin
git add crates/agentd-bin
git commit -m "feat(dispatch): add native-queue production dispatch route behind a switch

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Exit-gate e2e — production dispatch executed by a native worker, no tmux

**Files:**
- Test: `crates/agentd-bin/tests/native_dispatch.rs`
- Modify: `docs/parity/agent-chat-capability-map.md`

**Interfaces:**
- Consumes: Task 5's `dispatch_task_to_fleet`; the M1 worker e2e harness pattern from `crates/agentd-bin/tests/worker_main.rs` (`serve_daemon`, `run_worker_once`, `authority_snapshot`, `runtime_session_repo::create_session`, `project_authority_repo::record_snapshot`); Task 4's repository-binding evidence.
- Produces: an end-to-end proof that a task dispatched through the native production route (`dispatch_task_to_fleet`) is pulled and executed by a native worker with no `TmuxBackend` anywhere in the flow.

- [ ] **Step 1: Write the failing e2e test**

Append to `crates/agentd-bin/tests/native_dispatch.rs`. This mirrors `worker_once_executes_a_dispatched_task_end_to_end` but the dispatch is the production `dispatch_task_to_fleet` call (not a hand-written `enqueue`), and the test never constructs a tmux backend:

```rust
#[tokio::test]
async fn production_native_dispatch_is_executed_by_a_worker_without_tmux() {
    use std::os::unix::fs::PermissionsExt;

    // Seed run + task + agent profile + worker + runtime session + authority
    // snapshot exactly as crates/agentd-bin/tests/worker_main.rs::fixture() and
    // its e2e do (copy that seeding here; keep the session's snapshot ref
    // "specify:execution_snapshot:spec-1:v1" so pull can resolve a scope).
    // Bind `store`, `run_id`, `task_id`, `session_id`.

    // A codex shim that exits immediately (basename must equal the provider).
    let shim_dir = tempfile::tempdir().expect("shim dir");
    let shim = shim_dir.path().join("codex");
    std::fs::write(&shim, "#!/bin/sh\nexit 0\n").expect("write shim");
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let spec = NativeExecutionSpec {
        version: 1,
        provider: "codex".into(),
        program: shim.to_string_lossy().into_owned(),
        args: vec![],
        cwd: Some(shim_dir.path().to_string_lossy().into_owned()),
        env: vec![],
    };

    // Production dispatch decision: the switch routes to the native queue, and
    // the native launch primitive enqueues the task. No tmux is involved.
    assert_eq!(
        production_dispatch_route(&config_with_native_dispatch(true)),
        DispatchRoute::NativeQueue
    );
    dispatch_task_to_fleet(&store, &task_id, &spec, 100)
        .await
        .expect("native dispatch");

    // A real native worker pulls the dispatched task over HTTP and executes it.
    let base_url = serve_daemon(store.clone(), "worker-secret").await;
    let worker_state = tempfile::tempdir().expect("worker state");
    let report = agentd_bin::worker_main::run_worker_once(
        &base_url,
        "worker-secret",
        worker_state.path(),
        std::time::Duration::from_millis(100),
        std::time::Duration::from_secs(30),
    )
    .await
    .expect("worker run");
    assert_eq!(report.executed, 1);
    assert_eq!(report.released, 1);

    // Daemon-side: the queue row completed and the session is Completed.
    let (status,): (String,) = sqlx::query_as(
        "SELECT status FROM execution_task_queue WHERE execution_task_id = ?",
    )
    .bind(task_id.as_str())
    .fetch_one(store.pool())
    .await
    .expect("queue row");
    assert_eq!(status, "completed");
    let session = runtime_session_repo::get_session(store.pool(), &session_id)
        .await
        .expect("session lookup")
        .expect("session");
    assert_eq!(
        session.status,
        agentd_core::types::RuntimeSessionStatus::Completed
    );
}
```

Copy the `serve_daemon` helper and the fixture seeding verbatim from `crates/agentd-bin/tests/worker_main.rs` (or factor a shared `mod` — but duplication is acceptable and keeps the test self-contained). Add the same `use` set that file uses (`runtime_session_repo`, `project_authority_repo`, the `ProjectExecutionSnapshot` builder types, `AgentProfileCreate`, `agent_profile_repo`, `worker_repo`, etc.).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p agentd-bin --test native_dispatch production_native_dispatch_is_executed_by_a_worker_without_tmux`
Expected: FAIL first on missing seeding/imports, then (once seeding compiles) it should pass — if it does not reach `executed == 1`, debug the seeding (most likely the runtime session snapshot ref or the authority snapshot record), not the dispatch path.

- [ ] **Step 3: Make it pass**

Complete the fixture seeding until the assertions hold. No production code changes should be needed — Tasks 1-5 already provide the dispatch route, the queue, and the worker pull. The proof is behavioral: the only launch path exercised is `dispatch_task_to_fleet` → durable queue → `run_worker_once`; `TmuxBackend`/`ProductionRunHost` are never constructed in this test.

- [ ] **Step 4: Update the capability map**

In `docs/parity/agent-chat-capability-map.md`:
- `pool_scheduler` note — append: "M2 Plan B adds capacity- and capability-honoring acquisition, a worker protocol-version floor, exposed fleet inventory (zone/capabilities/capacity/open-leases), and a native production dispatch route (`dispatch_task_to_fleet`) behind `AGENTD_NATIVE_DISPATCH` proven to run a task on a native worker with no tmux; row is now full for the M2 dispatch scope."
- `worker_fleet_protocol` note — append: "M2 Plan B completes capability/capacity inventory, zone exposure, and version negotiation (min protocol floor at registration)."
- `durable_task_leases` note — append: "M2 Plan B threads the lease terminal_reason into scheduler explain and the repository binding into the execution security scope so evidence links report the real target repository."

- [ ] **Step 5: Full-workspace gate and commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
git add crates/agentd-bin docs/parity/agent-chat-capability-map.md
git commit -m "test(dispatch): prove native production dispatch executes on a worker without tmux

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

(If `native_runtime_can_terminate_a_running_child` fails under full load, rerun it in isolation: `cargo nextest run -p agentd-tmux native_runtime_can_terminate_a_running_child`.)

---

## Self-Review Notes

- **Design §M2 item 2 coverage:** capability inventory → Task 3 (capability filter in `acquire`); capacity → Tasks 1+3 (`capacity` column, capacity pre-check); zone → already recorded (Plan A migration), now *exposed* in Task 2's `GET /api/fleet/workers`; version negotiation → Tasks 1+2 (`protocol_version` field + `MIN_WORKER_PROTOCOL_VERSION` floor). Drain/offline pre-existed and are untouched.
- **Design §M2 exit-gate ("production workflow dispatch can route to native workers, tmux no longer the only launch path"):** Task 5 (config switch + `DispatchRoute` + `dispatch_task_to_fleet`) and Task 6 (e2e proof). Tmux stays the default route.
- **Carry-overs (from `.superpowers/sdd/progress.md`):** release-reason/terminal-state fidelity → Task 4 (reconcile threads `terminal_reason` into `last_reason`, so `explain_task` tells success from failure); repository binding through the control plane → Task 4 (`ExecutionSecurityScope.target_repository_id/target_base_commit`, worker + daemon evidence links). Requeue backoff, outbox consumer, and `db.code()` matching are declared non-goals in Global Constraints and left OUT.
- **Schema/version discipline:** migration 0024 and every version assertion (`migration.rs` `"23"`→`"24"`, `operational_doctor.rs` `23`→`24`) land together in Task 1; `migration_backcompat.rs`'s frozen 13/14/15 asserts are deliberately untouched.
- **Placeholder scan:** every code step carries complete code; the "grep-and-mirror" instructions (construction-site sweeps for `WorkerRegistration`/`WorkerFleetRegisterRequest`/`DaemonConfig`; the `CoreError` variant name; the `DaemonConfig` test constructor) are locate-and-match instructions against existing code, matching Plan A's accepted convention — not deferred logic.
- **Type consistency:** `capacity: u32` is the field type in `WorkerFleetRegisterRequest`, `WorkerRegistration`, and `WorkerIncarnationRecord`; the DB column is `INTEGER` read back via `u32::try_from(...).unwrap_or(1).max(1)`. `WORKER_PROTOCOL_VERSION`/`MIN_WORKER_PROTOCOL_VERSION` are `u32` and compared against `WorkerFleetRegisterRequest.protocol_version: u32`. `DispatchRoute`, `production_dispatch_route`, and `dispatch_task_to_fleet` signatures in Task 5's Interfaces match their Task 6 call sites. `scope_for_snapshot` gains no parameter (reads the snapshot it already receives), so its Plan A callers in `worker_fleet.rs` are unchanged.
- **Known open shapes for implementers (locate-and-mirror, not placeholders):** the smallest existing `DaemonConfig` test constructor (or an added `for_test()`); the exact current set of `assert_eq!(version, "23")` lines (grep before editing); the daemon-local acknowledge path's borrow order when reading `session.snapshot` fields for the repository resolution before they are moved into `ExecutionSnapshotLink`. Each is an explicit instruction to read the neighbor and match it. (`CoreError::Backend`, the `Store` trait path, and `set_task_execution_spec`'s return type are all confirmed against the tree.)
