# M2 Durable Scheduler (Plan A: scheduler core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Task dispatch becomes a durable queue authority: queue selection, lease grant, and outbox event commit in one `BEGIN IMMEDIATE` transaction; expired work retries with attempt limits and dead-letters when exhausted; worker pull is request-idempotent; an operator can explain any task's scheduling state.

**Architecture:** A new `execution_task_queue` + `scheduler_acquisitions` + `execution_scheduler_outbox` trio (migration 0023) becomes the scheduling authority. `SqliteDurableScheduler` composes the existing crate-private lease-grant transaction (`dispatch_in_transaction`) inside its own acquire transaction. `SqliteWorkerFleet::pull` re-routes through the queue (auto-enqueueing legacy open tasks) so the M1 worker keeps working unchanged. A reaper reconciles expired leases back into the queue. This plan deliberately stops before routing production workflow dispatch to native workers — that is M2 Plan B, a separate plan.

**Tech Stack:** Rust workspace (sqlx/SQLite, axum, tokio); no new external dependencies.

**Design reference:** `docs/superpowers/specs/2026-07-22-agent-chat-replacement-milestones-design.md` §M2. Gap analysis: Codex AD-E2 matrix (mempal `drawer_agentd_default_*`, 2026-07-16).

## Global Constraints

- No new external dependencies in any `Cargo.toml`.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace` pass after every task (known env-sensitive flake: `agentd-tmux::native native_runtime_can_terminate_a_running_child` — rerun in isolation if it fails under full load).
- The legacy `agent_scheduler_*` (agent-chat compatibility scheduler) is NOT touched and NOT the authority; the new queue lives in its own tables.
- Migrations are additive only; `schema_meta` version advances 22 → 23.
- Wire compatibility: `WorkerFleetPullRequest` gains only an optional `#[serde(default)]` field; existing clients keep working.
- Tests never run real Claude/Codex/tmux/Matrix.
- Commits: `type(scope): summary` + `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

## File Structure

| File | Role |
|---|---|
| `crates/agentd-core/src/types/ids.rs` | `SchedulerQueueId` (`sq_`), `SchedulerEventId` (`se_`) |
| `crates/agentd-core/src/types/enterprise.rs` | `SchedulerQueueStatus` contract enum |
| `crates/agentd-core/src/ports/durable_scheduler.rs` (new) | `DurableSchedulerPort`: enqueue / acquire / explain DTOs |
| `crates/agentd-core/src/ports/mod.rs`, `types/mod.rs` | registrations/re-exports |
| `crates/agentd-store/migrations/0023_enterprise_scheduler.sql` (new) | queue + acquisitions + outbox tables |
| `crates/agentd-store/src/durable_scheduler.rs` (new) | `SqliteDurableScheduler` |
| `crates/agentd-store/src/task_lease_control_plane.rs` | expose crate-private grant primitive |
| `crates/agentd-store/src/worker_fleet.rs` | `pull` routes through queue acquire |
| `crates/agentd-store/src/lib.rs` | module registration |
| `crates/agentd-bin/src/daemon.rs` | reaper into `worker_fleet_tick`; explain route on recovery router |
| Tests | `crates/agentd-store/tests/enterprise_scheduler.rs` (new), `migration.rs`, `worker_fleet.rs`, `crates/agentd-bin/tests/recovery_http.rs`, `worker_main.rs` (regression) |

---

### Task 1: Contract layer — IDs, queue status, port, migration

**Files:**
- Modify: `crates/agentd-core/src/types/ids.rs`, `crates/agentd-core/src/types/enterprise.rs`, `crates/agentd-core/src/types/mod.rs`
- Create: `crates/agentd-core/src/ports/durable_scheduler.rs`
- Modify: `crates/agentd-core/src/ports/mod.rs`
- Create: `crates/agentd-store/migrations/0023_enterprise_scheduler.sql`
- Test: `crates/agentd-store/tests/migration.rs`

**Interfaces:**
- Consumes: `id_newtype!` macro (ids.rs), `contract_status!` macro (enterprise.rs), existing `TaskRunId`/`LeaseId`/`WorkerIncarnationId`/`TaskLeaseGrant`.
- Produces (later tasks rely on these exact names):
  - `SchedulerQueueId` (prefix `sq_`), `SchedulerEventId` (prefix `se_`).
  - `SchedulerQueueStatus { Queued, Leased, Completed, DeadLetter, Cancelled }` (terminal: Completed, DeadLetter, Cancelled), snake_case wire strings.
  - Port DTOs and trait (full definition in Step 3).
  - Tables `execution_task_queue`, `scheduler_acquisitions`, `execution_scheduler_outbox`; `schema_meta.version = '23'`.

- [ ] **Step 1: Write the failing migration test**

Append to `crates/agentd-store/tests/migration.rs` (follow the existing table-assert style at the top of the file):

```rust
#[tokio::test]
async fn migration_creates_enterprise_scheduler_tables() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    for table in [
        "execution_task_queue",
        "scheduler_acquisitions",
        "execution_scheduler_outbox",
    ] {
        let found: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?",
        )
        .bind(table)
        .fetch_optional(store.pool())
        .await
        .expect("table query");
        assert_eq!(found.as_deref(), Some(table), "missing table {table}");
    }
    let version: String =
        sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'version'")
            .fetch_one(store.pool())
            .await
            .expect("schema version row");
    assert_eq!(version, "23");
}
```

Also update the existing assertion `assert_eq!(version, "22")` in `migration_is_idempotent_on_reopen` to `"23"`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p agentd-store --test migration`
Expected: new test FAILS (`missing table execution_task_queue`); the reopen test fails on `"22" != "23"` only after Step 3's migration lands (initially it still passes — that's fine, the new test is the red).

- [ ] **Step 3: Implement the contract layer**

3a. `crates/agentd-core/src/types/ids.rs` — add alongside the existing `id_newtype!` invocations (copy the exact invocation style used by `LeaseId`):

```rust
id_newtype!(SchedulerQueueId, "sq_");
id_newtype!(SchedulerEventId, "se_");
```

(Adjust to the macro's actual invocation shape — open the file and mirror how `LeaseId` is declared, including doc comments.)

3b. `crates/agentd-core/src/types/enterprise.rs` — add after `LeaseStatus`:

```rust
contract_status!(
    SchedulerQueueStatus {
        Queued => "queued",
        Leased => "leased",
        Completed => "completed",
        DeadLetter => "dead_letter",
        Cancelled => "cancelled",
    }
    terminal { Completed, DeadLetter, Cancelled }
);
```

3c. Export both from `crates/agentd-core/src/types/mod.rs` (mirror existing export lists).

3d. Create `crates/agentd-core/src/ports/durable_scheduler.rs`:

```rust
//! Durable scheduler port (M2): queue selection, lease grant, and outbox
//! event commit as one authority. Implementations own the transaction
//! boundary; callers never compose queue and lease writes separately.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{
    LeaseId, SchedulerQueueId, SchedulerQueueStatus, TaskLeaseGrant, TaskRunId,
    WorkerIncarnationId,
};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DurableSchedulerError {
    #[error("invalid scheduler input: {0}")]
    Invalid(String),
    #[error("scheduler resource not found: {0}")]
    NotFound(String),
    #[error("scheduler conflict: {0}")]
    Conflict(String),
    #[error("scheduler unavailable: {0}")]
    Unavailable(String),
}

/// Idempotent enqueue: the same `request_id` for the same task replays the
/// original row; a different payload under the same `request_id` conflicts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerEnqueueRequest {
    pub request_id: String,
    pub execution_task_id: TaskRunId,
    pub max_attempts: u32,
    pub available_at: i64,
    pub enqueued_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerQueueRecord {
    pub id: SchedulerQueueId,
    pub execution_task_id: TaskRunId,
    pub status: SchedulerQueueStatus,
    pub attempts: u32,
    pub max_attempts: u32,
    pub available_at: i64,
    pub current_lease_id: Option<LeaseId>,
    pub last_reason: Option<String>,
    pub enqueued_at: i64,
    pub updated_at: i64,
}

/// Idempotent acquire: the same `request_id` replays the original grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerAcquireRequest {
    pub request_id: String,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub observed_at: i64,
    pub expires_at: i64,
}

/// One task's full scheduling explanation for operators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerTaskExplanation {
    pub queue: SchedulerQueueRecord,
    pub active_lease: Option<TaskLeaseGrant>,
}

#[async_trait::async_trait]
pub trait DurableSchedulerPort: Send + Sync {
    async fn enqueue(
        &self,
        request: &SchedulerEnqueueRequest,
    ) -> Result<SchedulerQueueRecord, DurableSchedulerError>;

    /// Select eligible work, verify the online incarnation, grant the lease,
    /// transition the queue row, and append the outbox event — atomically.
    /// Returns `None` when no work is eligible.
    async fn acquire(
        &self,
        request: &SchedulerAcquireRequest,
    ) -> Result<Option<TaskLeaseGrant>, DurableSchedulerError>;

    /// Reconcile terminal/expired lease state back into the queue: released
    /// leases complete their row; expired leases requeue with attempts+1 or
    /// dead-letter at the limit. Returns rows changed.
    async fn reconcile(&self, observed_at: i64) -> Result<u64, DurableSchedulerError>;

    async fn explain_task(
        &self,
        task_id: &TaskRunId,
    ) -> Result<Option<SchedulerTaskExplanation>, DurableSchedulerError>;
}
```

3e. Register in `crates/agentd-core/src/ports/mod.rs`: `pub mod durable_scheduler;` and re-export `DurableSchedulerError, DurableSchedulerPort, SchedulerAcquireRequest, SchedulerEnqueueRequest, SchedulerQueueRecord, SchedulerTaskExplanation`.

3f. Create `crates/agentd-store/migrations/0023_enterprise_scheduler.sql`:

```sql
-- M2 Plan A: durable scheduler authority. Queue selection, lease grant, and
-- outbox append commit in one transaction (design doc §M2; AD-E2 matrix).

CREATE TABLE execution_task_queue (
    id                 TEXT PRIMARY KEY,
    execution_task_id  TEXT NOT NULL REFERENCES task_runs(id),
    status             TEXT NOT NULL DEFAULT 'queued'
                       CHECK (status IN ('queued','leased','completed','dead_letter','cancelled')),
    attempts           INTEGER NOT NULL DEFAULT 0,
    max_attempts       INTEGER NOT NULL DEFAULT 3,
    available_at       INTEGER NOT NULL,
    current_lease_id   TEXT,
    last_reason        TEXT,
    request_id         TEXT NOT NULL,
    enqueued_at        INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL
);
-- One open queue row per task; terminal rows do not block a re-enqueue.
CREATE UNIQUE INDEX idx_queue_open_task ON execution_task_queue(execution_task_id)
    WHERE status IN ('queued','leased');
CREATE UNIQUE INDEX idx_queue_request ON execution_task_queue(request_id);
CREATE INDEX idx_queue_eligible ON execution_task_queue(status, available_at, enqueued_at);

CREATE TABLE scheduler_acquisitions (
    request_id             TEXT PRIMARY KEY,
    queue_id               TEXT NOT NULL REFERENCES execution_task_queue(id),
    lease_id               TEXT NOT NULL,
    worker_incarnation_id  TEXT NOT NULL,
    acquired_at            INTEGER NOT NULL
);

CREATE TABLE execution_scheduler_outbox (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id   TEXT NOT NULL UNIQUE,
    kind       TEXT NOT NULL,
    queue_id   TEXT NOT NULL,
    task_id    TEXT NOT NULL,
    lease_id   TEXT,
    payload    TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    drained_at INTEGER
);
CREATE INDEX idx_scheduler_outbox_pending ON execution_scheduler_outbox(drained_at, seq)
    WHERE drained_at IS NULL;

UPDATE schema_meta SET value = '23' WHERE key = 'version';
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p agentd-store --test migration && cargo check -p agentd-core`
Expected: PASS (both the new table test and the updated reopen version assert).

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all
cargo clippy -p agentd-core -p agentd-store --all-targets -- -D warnings
cargo test -p agentd-store --test migration --test migration_backcompat
git add crates/agentd-core crates/agentd-store/migrations crates/agentd-store/tests/migration.rs
git commit -m "feat(scheduler): add durable scheduler contract and schema

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

(If `migration_backcompat` asserts the version literal, update it the same way.)

---

### Task 2: `SqliteDurableScheduler::enqueue` with exact-replay idempotency

**Files:**
- Create: `crates/agentd-store/src/durable_scheduler.rs`
- Modify: `crates/agentd-store/src/lib.rs` (`pub mod durable_scheduler;`)
- Test: `crates/agentd-store/tests/enterprise_scheduler.rs` (new)

**Interfaces:**
- Consumes: Task 1's port/types/migration; fixture seeding style from `crates/agentd-store/tests/enterprise_task_leases.rs` (run/task/worker/incarnation).
- Produces: `SqliteDurableScheduler::new(pool: SqlitePool)` implementing `DurableSchedulerPort` (this task: `enqueue` + `explain_task` real; `acquire`/`reconcile` return `Unavailable("not implemented")` until Tasks 3-4).

- [ ] **Step 1: Write the failing tests**

Create `crates/agentd-store/tests/enterprise_scheduler.rs` with a `fixture()` (copy the run/task/worker/incarnation seeding from `enterprise_task_leases.rs`'s fixture — bind `store`, `task_id`, `incarnation_id`) and:

```rust
#[tokio::test]
async fn enqueue_creates_a_queued_row_and_replays_identically() {
    let fixture = fixture().await;
    let scheduler = SqliteDurableScheduler::new(fixture.store.pool().clone());
    let request = SchedulerEnqueueRequest {
        request_id: "rq-1".to_string(),
        execution_task_id: fixture.task_id.clone(),
        max_attempts: 3,
        available_at: 10,
        enqueued_at: 10,
    };

    let first = scheduler.enqueue(&request).await.expect("enqueue");
    assert_eq!(first.status, SchedulerQueueStatus::Queued);
    assert_eq!(first.attempts, 0);
    assert_eq!(first.execution_task_id, fixture.task_id);

    let replay = scheduler.enqueue(&request).await.expect("replay");
    assert_eq!(replay, first, "same request_id replays the identical row");
}

#[tokio::test]
async fn enqueue_conflicts_on_same_request_with_different_payload() {
    let fixture = fixture().await;
    let scheduler = SqliteDurableScheduler::new(fixture.store.pool().clone());
    let request = SchedulerEnqueueRequest {
        request_id: "rq-1".to_string(),
        execution_task_id: fixture.task_id.clone(),
        max_attempts: 3,
        available_at: 10,
        enqueued_at: 10,
    };
    scheduler.enqueue(&request).await.expect("enqueue");

    let mut changed = request.clone();
    changed.max_attempts = 5;
    let error = scheduler
        .enqueue(&changed)
        .await
        .expect_err("changed payload must conflict");
    assert!(matches!(error, DurableSchedulerError::Conflict(_)));
}

#[tokio::test]
async fn enqueue_rejects_second_open_row_for_same_task() {
    let fixture = fixture().await;
    let scheduler = SqliteDurableScheduler::new(fixture.store.pool().clone());
    scheduler
        .enqueue(&SchedulerEnqueueRequest {
            request_id: "rq-1".to_string(),
            execution_task_id: fixture.task_id.clone(),
            max_attempts: 3,
            available_at: 10,
            enqueued_at: 10,
        })
        .await
        .expect("first enqueue");
    let error = scheduler
        .enqueue(&SchedulerEnqueueRequest {
            request_id: "rq-2".to_string(),
            execution_task_id: fixture.task_id.clone(),
            max_attempts: 3,
            available_at: 11,
            enqueued_at: 11,
        })
        .await
        .expect_err("second open row for the same task must conflict");
    assert!(matches!(error, DurableSchedulerError::Conflict(_)));
}

#[tokio::test]
async fn explain_reports_queue_row_and_absent_lease() {
    let fixture = fixture().await;
    let scheduler = SqliteDurableScheduler::new(fixture.store.pool().clone());
    assert!(scheduler
        .explain_task(&fixture.task_id)
        .await
        .expect("explain")
        .is_none());
    scheduler
        .enqueue(&SchedulerEnqueueRequest {
            request_id: "rq-1".to_string(),
            execution_task_id: fixture.task_id.clone(),
            max_attempts: 3,
            available_at: 10,
            enqueued_at: 10,
        })
        .await
        .expect("enqueue");
    let explanation = scheduler
        .explain_task(&fixture.task_id)
        .await
        .expect("explain")
        .expect("queued task explains");
    assert_eq!(explanation.queue.status, SchedulerQueueStatus::Queued);
    assert!(explanation.active_lease.is_none());
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p agentd-store --test enterprise_scheduler`
Expected: FAIL to compile (`SqliteDurableScheduler` unresolved).

- [ ] **Step 3: Implement**

Create `crates/agentd-store/src/durable_scheduler.rs`:

```rust
//! SQLite authority for the durable scheduler port (M2 Plan A).

use agentd_core::ports::{
    DurableSchedulerError, DurableSchedulerPort, SchedulerAcquireRequest,
    SchedulerEnqueueRequest, SchedulerQueueRecord, SchedulerTaskExplanation,
};
use agentd_core::types::{
    LeaseId, SchedulerQueueId, SchedulerQueueStatus, TaskLeaseGrant, TaskRunId,
};
use async_trait::async_trait;
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone)]
pub struct SqliteDurableScheduler {
    pool: SqlitePool,
}

impl SqliteDurableScheduler {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn storage_error(error: impl std::fmt::Display) -> DurableSchedulerError {
    DurableSchedulerError::Unavailable(error.to_string())
}

fn queue_record(row: &sqlx::sqlite::SqliteRow) -> Result<SchedulerQueueRecord, DurableSchedulerError> {
    let status_text: String = row.get("status");
    let status = SchedulerQueueStatus::try_from(status_text.as_str())
        .map_err(|error| DurableSchedulerError::Unavailable(error.to_string()))?;
    Ok(SchedulerQueueRecord {
        id: SchedulerQueueId::from_string(row.get::<String, _>("id")),
        execution_task_id: TaskRunId::from_string(row.get::<String, _>("execution_task_id")),
        status,
        attempts: u32::try_from(row.get::<i64, _>("attempts")).unwrap_or(u32::MAX),
        max_attempts: u32::try_from(row.get::<i64, _>("max_attempts")).unwrap_or(u32::MAX),
        available_at: row.get("available_at"),
        current_lease_id: row
            .get::<Option<String>, _>("current_lease_id")
            .map(LeaseId::from_string),
        last_reason: row.get("last_reason"),
        enqueued_at: row.get("enqueued_at"),
        updated_at: row.get("updated_at"),
    })
}

const QUEUE_COLUMNS: &str = "id, execution_task_id, status, attempts, max_attempts, \
     available_at, current_lease_id, last_reason, request_id, enqueued_at, updated_at";

async fn get_by_request_id(
    pool: &SqlitePool,
    request_id: &str,
) -> Result<Option<(SchedulerQueueRecord, SchedulerEnqueueRequest)>, DurableSchedulerError> {
    let row = sqlx::query(&format!(
        "SELECT {QUEUE_COLUMNS} FROM execution_task_queue WHERE request_id = ?"
    ))
    .bind(request_id)
    .fetch_optional(pool)
    .await
    .map_err(storage_error)?;
    let Some(row) = row else { return Ok(None) };
    let record = queue_record(&row)?;
    let original = SchedulerEnqueueRequest {
        request_id: row.get("request_id"),
        execution_task_id: record.execution_task_id.clone(),
        max_attempts: record.max_attempts,
        available_at: record.available_at,
        enqueued_at: record.enqueued_at,
    };
    Ok(Some((record, original)))
}

#[async_trait]
impl DurableSchedulerPort for SqliteDurableScheduler {
    async fn enqueue(
        &self,
        request: &SchedulerEnqueueRequest,
    ) -> Result<SchedulerQueueRecord, DurableSchedulerError> {
        if request.request_id.trim().is_empty() {
            return Err(DurableSchedulerError::Invalid(
                "enqueue request_id is required".into(),
            ));
        }
        if request.max_attempts == 0 {
            return Err(DurableSchedulerError::Invalid(
                "max_attempts must be at least 1".into(),
            ));
        }
        // Exact replay: same request_id + same payload returns the row.
        if let Some((record, original)) = get_by_request_id(&self.pool, &request.request_id).await? {
            if original == *request {
                return Ok(record);
            }
            return Err(DurableSchedulerError::Conflict(
                "request_id replayed with a different payload".into(),
            ));
        }
        let id = SchedulerQueueId::new();
        let inserted = sqlx::query(
            "INSERT INTO execution_task_queue \
             (id, execution_task_id, status, attempts, max_attempts, available_at, \
              request_id, enqueued_at, updated_at) \
             VALUES (?, ?, 'queued', 0, ?, ?, ?, ?, ?)",
        )
        .bind(id.as_str())
        .bind(request.execution_task_id.as_str())
        .bind(i64::from(request.max_attempts))
        .bind(request.available_at)
        .bind(&request.request_id)
        .bind(request.enqueued_at)
        .bind(request.enqueued_at)
        .execute(&self.pool)
        .await;
        match inserted {
            Ok(_) => {}
            Err(sqlx::Error::Database(db)) if db.message().contains("idx_queue_open_task") => {
                return Err(DurableSchedulerError::Conflict(
                    "task already has an open queue row".into(),
                ));
            }
            Err(sqlx::Error::Database(db)) if db.message().contains("FOREIGN KEY") => {
                return Err(DurableSchedulerError::NotFound(
                    "execution task does not exist".into(),
                ));
            }
            Err(error) => return Err(storage_error(error)),
        }
        get_by_request_id(&self.pool, &request.request_id)
            .await?
            .map(|(record, _)| record)
            .ok_or_else(|| DurableSchedulerError::Unavailable("enqueue readback failed".into()))
    }

    async fn acquire(
        &self,
        _request: &SchedulerAcquireRequest,
    ) -> Result<Option<TaskLeaseGrant>, DurableSchedulerError> {
        Err(DurableSchedulerError::Unavailable(
            "acquire lands in the next task".into(),
        ))
    }

    async fn reconcile(&self, _observed_at: i64) -> Result<u64, DurableSchedulerError> {
        Err(DurableSchedulerError::Unavailable(
            "reconcile lands in a later task".into(),
        ))
    }

    async fn explain_task(
        &self,
        task_id: &TaskRunId,
    ) -> Result<Option<SchedulerTaskExplanation>, DurableSchedulerError> {
        let row = sqlx::query(&format!(
            "SELECT {QUEUE_COLUMNS} FROM execution_task_queue \
             WHERE execution_task_id = ? ORDER BY enqueued_at DESC, id DESC LIMIT 1"
        ))
        .bind(task_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        let Some(row) = row else { return Ok(None) };
        let queue = queue_record(&row)?;
        let active_lease = match &queue.current_lease_id {
            None => None,
            Some(lease_id) => {
                crate::task_lease_control_plane::get_grant_by_id(&self.pool, lease_id.as_str())
                    .await
                    .map_err(storage_error)?
            }
        };
        Ok(Some(SchedulerTaskExplanation { queue, active_lease }))
    }
}
```

Also add to `crates/agentd-store/src/task_lease_control_plane.rs` a small pub(crate) pool-level reader (place near the existing helpers; reuse the file's row→grant mapping function — find the function `dispatch_in_transaction` uses to map a lease row and call it):

```rust
/// Read one lease grant by id outside any transaction (explain/reporting).
pub(crate) async fn get_grant_by_id(
    pool: &SqlitePool,
    lease_id: &str,
) -> Result<Option<TaskLeaseGrant>, TaskLeaseError> {
    let mut connection = pool.acquire().await.map_err(storage_error)?;
    match get_grant(&mut connection, lease_id).await {
        Ok(grant) => Ok(Some(grant)),
        Err(TaskLeaseError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}
```

(`get_grant(connection, lease_id)` — locate the existing in-transaction grant reader in this file; it exists because renew/close re-read the grant. Match its actual name/signature; if it takes `&mut SqliteConnection`, pass `&mut *connection`.)

Register `pub mod durable_scheduler;` in `crates/agentd-store/src/lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p agentd-store --test enterprise_scheduler`
Expected: 4/4 PASS.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all
cargo clippy -p agentd-store --all-targets -- -D warnings
cargo test -p agentd-store
git add crates/agentd-store
git commit -m "feat(scheduler): idempotent durable enqueue and task explanation

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Atomic `acquire` — queue + lease + outbox in one transaction

**Files:**
- Modify: `crates/agentd-store/src/task_lease_control_plane.rs` (expose the grant primitive)
- Modify: `crates/agentd-store/src/durable_scheduler.rs`
- Test: `crates/agentd-store/tests/enterprise_scheduler.rs`

**Interfaces:**
- Consumes: `dispatch_in_transaction(&mut SqliteConnection, &TaskLeaseDispatchRequest) -> Result<TaskLeaseGrant, TaskLeaseError>` — currently private in `task_lease_control_plane.rs`; make it `pub(crate)`.
- Produces: working `SqliteDurableScheduler::acquire` per the port contract; outbox rows with `kind = "lease_granted"`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/agentd-store/tests/enterprise_scheduler.rs`:

```rust
#[tokio::test]
async fn acquire_grants_lease_transitions_queue_and_appends_outbox() {
    let fixture = fixture().await;
    let scheduler = SqliteDurableScheduler::new(fixture.store.pool().clone());
    scheduler.enqueue(&enqueue_request(&fixture, "rq-1", 10)).await.expect("enqueue");

    let grant = scheduler
        .acquire(&SchedulerAcquireRequest {
            request_id: "acq-1".to_string(),
            worker_incarnation_id: fixture.incarnation_id.clone(),
            observed_at: 20,
            expires_at: 80,
        })
        .await
        .expect("acquire")
        .expect("eligible work");
    assert_eq!(grant.execution_task_id, fixture.task_id);

    let explanation = scheduler
        .explain_task(&fixture.task_id)
        .await
        .expect("explain")
        .expect("row");
    assert_eq!(explanation.queue.status, SchedulerQueueStatus::Leased);
    assert_eq!(
        explanation.queue.current_lease_id.as_ref().map(|l| l.as_str().to_owned()),
        Some(grant.lease_id.as_str().to_owned())
    );
    assert!(explanation.active_lease.is_some());

    let (kind, task_id): (String, String) = sqlx::query_as(
        "SELECT kind, task_id FROM execution_scheduler_outbox ORDER BY seq DESC LIMIT 1",
    )
    .fetch_one(fixture.store.pool())
    .await
    .expect("outbox row");
    assert_eq!(kind, "lease_granted");
    assert_eq!(task_id, fixture.task_id.as_str());
}

#[tokio::test]
async fn acquire_replays_identical_grant_for_same_request_id() {
    let fixture = fixture().await;
    let scheduler = SqliteDurableScheduler::new(fixture.store.pool().clone());
    scheduler.enqueue(&enqueue_request(&fixture, "rq-1", 10)).await.expect("enqueue");
    let request = SchedulerAcquireRequest {
        request_id: "acq-1".to_string(),
        worker_incarnation_id: fixture.incarnation_id.clone(),
        observed_at: 20,
        expires_at: 80,
    };
    let first = scheduler.acquire(&request).await.expect("acquire").expect("grant");
    let replay = scheduler.acquire(&request).await.expect("replay").expect("grant");
    assert_eq!(first.lease_id, replay.lease_id, "replay returns the same lease");
    let outbox_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution_scheduler_outbox")
            .fetch_one(fixture.store.pool())
            .await
            .expect("count");
    assert_eq!(outbox_count, 1, "replay must not append a second event");
}

#[tokio::test]
async fn acquire_returns_none_when_nothing_eligible() {
    let fixture = fixture().await;
    let scheduler = SqliteDurableScheduler::new(fixture.store.pool().clone());
    // Nothing enqueued.
    assert!(scheduler
        .acquire(&SchedulerAcquireRequest {
            request_id: "acq-none".to_string(),
            worker_incarnation_id: fixture.incarnation_id.clone(),
            observed_at: 20,
            expires_at: 80,
        })
        .await
        .expect("acquire")
        .is_none());
    // Enqueued but not yet available.
    scheduler.enqueue(&enqueue_request(&fixture, "rq-1", 1_000)).await.expect("enqueue");
    assert!(scheduler
        .acquire(&SchedulerAcquireRequest {
            request_id: "acq-early".to_string(),
            worker_incarnation_id: fixture.incarnation_id.clone(),
            observed_at: 20,
            expires_at: 80,
        })
        .await
        .expect("acquire")
        .is_none());
}

#[tokio::test]
async fn concurrent_acquire_grants_exactly_one_winner() {
    let fixture = fixture().await;
    scheduler_for(&fixture).enqueue(&enqueue_request(&fixture, "rq-1", 10)).await.expect("enqueue");
    let mut handles = Vec::new();
    for index in 0..4 {
        let pool = fixture.store.pool().clone();
        let incarnation = fixture.incarnation_id.clone();
        handles.push(tokio::spawn(async move {
            SqliteDurableScheduler::new(pool)
                .acquire(&SchedulerAcquireRequest {
                    request_id: format!("acq-{index}"),
                    worker_incarnation_id: incarnation,
                    observed_at: 20,
                    expires_at: 80,
                })
                .await
        }));
    }
    let mut grants = 0;
    for handle in handles {
        if handle.await.expect("join").expect("acquire").is_some() {
            grants += 1;
        }
    }
    assert_eq!(grants, 1, "exactly one concurrent acquirer wins");
}
```

Add the small helpers `enqueue_request(fixture, request_id, available_at) -> SchedulerEnqueueRequest` and `scheduler_for(&fixture) -> SqliteDurableScheduler` at the top of the test file.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p agentd-store --test enterprise_scheduler`
Expected: new tests FAIL with `Unavailable("acquire lands in the next task")`.

- [ ] **Step 3: Implement**

3a. In `task_lease_control_plane.rs`, change `async fn dispatch_in_transaction` to `pub(crate) async fn dispatch_in_transaction` (no body change).

3b. Replace the `acquire` stub in `durable_scheduler.rs`:

```rust
async fn acquire(
    &self,
    request: &SchedulerAcquireRequest,
) -> Result<Option<TaskLeaseGrant>, DurableSchedulerError> {
    if request.request_id.trim().is_empty() {
        return Err(DurableSchedulerError::Invalid(
            "acquire request_id is required".into(),
        ));
    }
    let mut connection = self.pool.acquire().await.map_err(storage_error)?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
    let result = acquire_in_transaction(&mut connection, request).await;
    match result {
        Ok(value) => {
            sqlx::query("COMMIT")
                .execute(&mut *connection)
                .await
                .map_err(storage_error)?;
            Ok(value)
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}
```

with the transaction body as a free function in the same file:

```rust
async fn acquire_in_transaction(
    connection: &mut sqlx::SqliteConnection,
    request: &SchedulerAcquireRequest,
) -> Result<Option<TaskLeaseGrant>, DurableSchedulerError> {
    // Idempotent replay: a completed acquisition returns its original grant.
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT lease_id FROM scheduler_acquisitions WHERE request_id = ?",
    )
    .bind(&request.request_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?;
    if let Some(lease_id) = existing {
        let grant = crate::task_lease_control_plane::get_grant_in_tx(connection, &lease_id)
            .await
            .map_err(storage_error)?;
        return Ok(Some(grant));
    }

    // Select the oldest eligible queue row.
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT id, execution_task_id FROM execution_task_queue \
         WHERE status = 'queued' AND available_at <= ? \
         ORDER BY enqueued_at ASC, id ASC LIMIT 1",
    )
    .bind(request.observed_at)
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?;
    let Some((queue_id, task_id)) = row else {
        return Ok(None);
    };

    // Grant the lease through the existing fenced primitive (validates the
    // open task, the current online incarnation, and allocates the token).
    let grant = crate::task_lease_control_plane::dispatch_in_transaction(
        connection,
        &agentd_core::ports::TaskLeaseDispatchRequest {
            execution_task_id: TaskRunId::from_string(task_id.clone()),
            worker_incarnation_id: request.worker_incarnation_id.clone(),
            observed_at: request.observed_at,
            expires_at: request.expires_at,
        },
    )
    .await
    .map_err(|error| DurableSchedulerError::Conflict(error.to_string()))?;

    // Transition the queue row.
    sqlx::query(
        "UPDATE execution_task_queue SET status = 'leased', attempts = attempts + 1, \
         current_lease_id = ?, updated_at = ? WHERE id = ? AND status = 'queued'",
    )
    .bind(grant.lease_id.as_str())
    .bind(request.observed_at)
    .bind(&queue_id)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;

    // Record the acquisition for replay and append the outbox event.
    sqlx::query(
        "INSERT INTO scheduler_acquisitions \
         (request_id, queue_id, lease_id, worker_incarnation_id, acquired_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&request.request_id)
    .bind(&queue_id)
    .bind(grant.lease_id.as_str())
    .bind(request.worker_incarnation_id.as_str())
    .bind(request.observed_at)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    sqlx::query(
        "INSERT INTO execution_scheduler_outbox \
         (event_id, kind, queue_id, task_id, lease_id, payload, created_at) \
         VALUES (?, 'lease_granted', ?, ?, ?, '{}', ?)",
    )
    .bind(SchedulerEventId::new().as_str())
    .bind(&queue_id)
    .bind(&task_id)
    .bind(grant.lease_id.as_str())
    .bind(request.observed_at)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;

    Ok(Some(grant))
}
```

3c. In `task_lease_control_plane.rs`, expose the in-transaction grant reader as `pub(crate) async fn get_grant_in_tx(connection: &mut SqliteConnection, lease_id: &str) -> Result<TaskLeaseGrant, TaskLeaseError>` — this is a rename/visibility change of the existing internal reader found in Task 2 Step 3 (keep one function; `get_grant_by_id` from Task 2 calls it).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p agentd-store --test enterprise_scheduler && cargo test -p agentd-store --test enterprise_task_leases`
Expected: all PASS (lease suite must not regress).

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all
cargo clippy -p agentd-store --all-targets -- -D warnings
cargo test -p agentd-store
git add crates/agentd-store
git commit -m "feat(scheduler): atomic queue+lease+outbox acquisition

Queue selection, fenced lease grant, queue transition, acquisition
record, and outbox append commit in one BEGIN IMMEDIATE transaction;
replays of the same acquire request return the original grant without
a second event.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: `reconcile` — retry with attempt limits, dead-letter, completion

**Files:**
- Modify: `crates/agentd-store/src/durable_scheduler.rs`
- Modify: `crates/agentd-bin/src/daemon.rs` (`worker_fleet_tick` calls reconcile)
- Test: `crates/agentd-store/tests/enterprise_scheduler.rs`

**Interfaces:**
- Consumes: lease terminal states (`LeaseStatus::{Released,Expired,Cancelled}` on `execution_task_leases.status`); `SqliteTaskLeaseControlPlane::expire_due` already flips overdue actives to `expired` — reconcile runs AFTER it in the tick.
- Produces: `reconcile(observed_at) -> u64` (rows changed); outbox kinds `"task_completed"`, `"task_requeued"`, `"task_dead_lettered"`; `worker_fleet_tick` gains a `scheduler: &SqliteDurableScheduler` parameter (update its callers in daemon.rs and any test that invokes it).

- [ ] **Step 1: Write the failing tests**

Append to `enterprise_scheduler.rs`:

```rust
#[tokio::test]
async fn reconcile_completes_row_when_lease_released() {
    let fixture = fixture().await;
    let scheduler = scheduler_for(&fixture);
    scheduler.enqueue(&enqueue_request(&fixture, "rq-1", 10)).await.expect("enqueue");
    let grant = scheduler
        .acquire(&acquire_request(&fixture, "acq-1", 20, 80))
        .await.expect("acquire").expect("grant");
    let lease_plane = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());
    lease_plane
        .release(&TaskLeaseCloseRequest {
            claim: grant.claim(),
            observed_at: 30,
            reason: "done".to_string(),
        })
        .await
        .expect("release");

    let changed = scheduler.reconcile(31).await.expect("reconcile");
    assert_eq!(changed, 1);
    let explanation = scheduler.explain_task(&fixture.task_id).await.expect("explain").expect("row");
    assert_eq!(explanation.queue.status, SchedulerQueueStatus::Completed);
}

#[tokio::test]
async fn reconcile_requeues_expired_lease_until_dead_letter() {
    let fixture = fixture().await;
    let scheduler = scheduler_for(&fixture);
    // max_attempts = 2: first expiry requeues, second dead-letters.
    let mut enqueue = enqueue_request(&fixture, "rq-1", 10);
    enqueue.max_attempts = 2;
    scheduler.enqueue(&enqueue).await.expect("enqueue");
    let lease_plane = SqliteTaskLeaseControlPlane::new(fixture.store.pool().clone());

    // Attempt 1: acquire then let it expire.
    scheduler.acquire(&acquire_request(&fixture, "acq-1", 20, 25)).await.expect("acquire").expect("grant");
    lease_plane.expire_due(30).await.expect("expire");
    let changed = scheduler.reconcile(30).await.expect("reconcile");
    assert_eq!(changed, 1);
    let explanation = scheduler.explain_task(&fixture.task_id).await.expect("explain").expect("row");
    assert_eq!(explanation.queue.status, SchedulerQueueStatus::Queued, "first expiry requeues");
    assert_eq!(explanation.queue.attempts, 1);

    // Attempt 2: acquire again, expire again -> dead letter.
    scheduler.acquire(&acquire_request(&fixture, "acq-2", 40, 45)).await.expect("acquire").expect("grant");
    lease_plane.expire_due(50).await.expect("expire");
    scheduler.reconcile(50).await.expect("reconcile");
    let explanation = scheduler.explain_task(&fixture.task_id).await.expect("explain").expect("row");
    assert_eq!(explanation.queue.status, SchedulerQueueStatus::DeadLetter);
    assert!(explanation.queue.last_reason.as_deref().unwrap_or("").contains("expired"));
}
```

Add helper `acquire_request(fixture, id, observed_at, expires_at)`. Import `SqliteTaskLeaseControlPlane`, `TaskLeaseCloseRequest`, `TaskLeasePort` as needed.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p agentd-store --test enterprise_scheduler`
Expected: the two new tests FAIL with `Unavailable("reconcile lands in a later task")`.

- [ ] **Step 3: Implement `reconcile`**

Replace the stub. One transaction; walk all `leased` queue rows whose lease is terminal:

```rust
async fn reconcile(&self, observed_at: i64) -> Result<u64, DurableSchedulerError> {
    let mut connection = self.pool.acquire().await.map_err(storage_error)?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
    let result = reconcile_in_transaction(&mut connection, observed_at).await;
    match result {
        Ok(count) => {
            sqlx::query("COMMIT").execute(&mut *connection).await.map_err(storage_error)?;
            Ok(count)
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}
```

```rust
async fn reconcile_in_transaction(
    connection: &mut sqlx::SqliteConnection,
    observed_at: i64,
) -> Result<u64, DurableSchedulerError> {
    let rows: Vec<(String, String, String, i64, i64, String)> = sqlx::query_as(
        "SELECT q.id, q.execution_task_id, q.current_lease_id, q.attempts, q.max_attempts, l.status \
         FROM execution_task_queue q \
         JOIN execution_task_leases l ON l.id = q.current_lease_id \
         WHERE q.status = 'leased' AND l.status != 'active'",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(storage_error)?;
    let mut changed = 0_u64;
    for (queue_id, task_id, lease_id, attempts, max_attempts, lease_status) in rows {
        let (new_status, reason, kind) = match lease_status.as_str() {
            "released" => ("completed", format!("lease {lease_id} released"), "task_completed"),
            "cancelled" => ("cancelled", format!("lease {lease_id} cancelled"), "task_cancelled"),
            // expired / superseded: retry or dead-letter.
            other => {
                if attempts >= max_attempts {
                    (
                        "dead_letter",
                        format!("lease {lease_id} {other}; attempts exhausted"),
                        "task_dead_lettered",
                    )
                } else {
                    ("queued", format!("lease {lease_id} {other}; requeued"), "task_requeued")
                }
            }
        };
        sqlx::query(
            "UPDATE execution_task_queue SET status = ?, current_lease_id = NULL, \
             last_reason = ?, available_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(new_status)
        .bind(&reason)
        .bind(observed_at)
        .bind(observed_at)
        .bind(&queue_id)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "INSERT INTO execution_scheduler_outbox \
             (event_id, kind, queue_id, task_id, lease_id, payload, created_at) \
             VALUES (?, ?, ?, ?, ?, '{}', ?)",
        )
        .bind(SchedulerEventId::new().as_str())
        .bind(kind)
        .bind(&queue_id)
        .bind(&task_id)
        .bind(&lease_id)
        .bind(observed_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        changed += 1;
    }
    Ok(changed)
}
```

Note: keep `current_lease_id = NULL` for completed/cancelled/dead-letter too — `last_reason` retains the lease id for the audit trail, and the unique open-task index only spans queued/leased.

3b. In `crates/agentd-bin/src/daemon.rs`, extend `worker_fleet_tick`:

```rust
pub async fn worker_fleet_tick(
    fleet: &dyn WorkerFleetPort,
    recovery_registry: &NativeRecoveryRegistry,
    native_worker: &AgentdWorker,
    scheduler: &agentd_store::durable_scheduler::SqliteDurableScheduler,
    observed_at: i64,
) {
    let _ = fleet.recover_offline(observed_at - 30).await;
    let _ = fleet.expire_due(observed_at).await;
    let _ = scheduler.reconcile(observed_at).await;
    let _ = recovery_registry.recover_one(native_worker).await;
}
```

Update its call site inside `WorkerFleetService::start` (search `worker_fleet_tick(`) to construct/hold a `SqliteDurableScheduler::new(pool)` and pass it; update any test invoking `worker_fleet_tick` the same way. Import `DurableSchedulerPort` where `reconcile` is called.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p agentd-store --test enterprise_scheduler && cargo test -p agentd-bin`
Expected: PASS.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all
cargo clippy -p agentd-store -p agentd-bin --all-targets -- -D warnings
cargo test -p agentd-store -p agentd-bin
git add crates/agentd-store crates/agentd-bin
git commit -m "feat(scheduler): reconcile terminal leases into retry, completion, and dead-letter

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Route `SqliteWorkerFleet::pull` through the durable queue

**Files:**
- Modify: `crates/agentd-core/src/ports/worker_fleet.rs` (`WorkerFleetPullRequest` gains `#[serde(default)] pub request_id: Option<String>`)
- Modify: `crates/agentd-store/src/worker_fleet.rs`
- Test: `crates/agentd-store/tests/worker_fleet.rs`, regression `crates/agentd-bin/tests/worker_main.rs`

**Interfaces:**
- Consumes: Tasks 2-4. Existing pull construction sites (grep `WorkerFleetPullRequest {` across crates — each gains `request_id: None` or a real id).
- Produces: `pull` behavior — worker/incarnation authorization unchanged; then (a) auto-enqueue any open unleased `task_runs` rows missing an open queue row (bridge for legacy tasks; `request_id = format!("auto-{task_id}")`, `max_attempts = 3`, available immediately), (b) `acquire` via the durable scheduler with `request_id` = the request's id or `format!("pull-{incarnation}-{observed_at}")`, (c) unchanged native-grant enrichment (spec/scope/session) after acquire. Wire shape backward compatible.

- [ ] **Step 1: Write the failing test**

Append to `crates/agentd-store/tests/worker_fleet.rs` (reuse its existing fixture style):

```rust
#[tokio::test]
async fn pull_routes_through_durable_queue_and_replays_by_request_id() {
    // Seed the standard fleet fixture (worker registered + online, one open
    // task) exactly as the existing pull tests in this file do.
    // ... fixture setup copied from the nearest existing pull test ...

    let request = WorkerFleetPullRequest {
        auth_proof: proof.clone(),
        worker_incarnation_id: incarnation_id.clone(),
        observed_at: 20,
        expires_at: 80,
        request_id: Some("pull-1".to_string()),
    };
    let first = fleet.pull(&request).await.expect("pull").expect("grant");

    // The queue row is the authority now.
    let (status, lease): (String, Option<String>) = sqlx::query_as(
        "SELECT status, current_lease_id FROM execution_task_queue WHERE execution_task_id = ?",
    )
    .bind(first.execution_task_id.as_str())
    .fetch_one(store.pool())
    .await
    .expect("queue row");
    assert_eq!(status, "leased");
    assert_eq!(lease.as_deref(), Some(first.lease_id.as_str()));

    // Same request_id replays the same grant instead of erroring.
    let replay = fleet.pull(&request).await.expect("replay").expect("grant");
    assert_eq!(replay.lease_id, first.lease_id);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p agentd-store --test worker_fleet`
Expected: FAIL to compile (`request_id` field missing) — that is the red.

- [ ] **Step 3: Implement**

3a. Add to `WorkerFleetPullRequest` in `crates/agentd-core/src/ports/worker_fleet.rs`:

```rust
    /// Optional idempotency key: replaying the same id returns the original
    /// grant. `None` derives a per-call key (no replay protection).
    #[serde(default)]
    pub request_id: Option<String>,
```

Fix every construction site (grep `WorkerFleetPullRequest {`): tests and `worker_fleet_client.rs::incarnation_request` add `request_id: None`; `worker_main.rs`'s pull loop passes `request_id: Some(format!("pull-{}-{}", incarnation_id.as_str(), observed_at))`.

3b. In `SqliteWorkerFleet::pull` (worker_fleet.rs), keep the authorization/incarnation/online checks, then REPLACE the ad-hoc `SELECT t.id ... LIMIT 1` + `dispatch` block with:

```rust
        // Bridge: give every open, unleased task an open queue row so the
        // durable queue is the single dispatch authority even for tasks
        // created before the queue existed.
        let open_tasks: Vec<String> = sqlx::query_scalar(
            "SELECT t.id FROM task_runs t \
             WHERE t.finished_at IS NULL AND t.status = 'running' \
             AND NOT EXISTS (SELECT 1 FROM execution_task_queue q \
                 WHERE q.execution_task_id = t.id AND q.status IN ('queued','leased'))",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| WorkerFleetError::Unavailable(error.to_string()))?;
        let scheduler = crate::durable_scheduler::SqliteDurableScheduler::new(self.pool.clone());
        for task_id in open_tasks {
            let enqueue = agentd_core::ports::SchedulerEnqueueRequest {
                request_id: format!("auto-{task_id}"),
                execution_task_id: TaskRunId::from_string(task_id),
                max_attempts: 3,
                available_at: request.observed_at,
                enqueued_at: request.observed_at,
            };
            match scheduler.enqueue(&enqueue).await {
                Ok(_) => {}
                // A racing pull created the row first — fine.
                Err(agentd_core::ports::DurableSchedulerError::Conflict(_)) => {}
                Err(error) => {
                    return Err(WorkerFleetError::Unavailable(error.to_string()));
                }
            }
        }

        let acquire_id = request
            .request_id
            .clone()
            .unwrap_or_else(|| {
                format!(
                    "pull-{}-{}",
                    request.worker_incarnation_id.as_str(),
                    request.observed_at
                )
            });
        let grant = scheduler
            .acquire(&agentd_core::ports::SchedulerAcquireRequest {
                request_id: acquire_id,
                worker_incarnation_id: request.worker_incarnation_id.clone(),
                observed_at: request.observed_at,
                expires_at: request.expires_at,
            })
            .await
            .map_err(|error| match error {
                agentd_core::ports::DurableSchedulerError::Conflict(message) => {
                    WorkerFleetError::Conflict(message)
                }
                other => WorkerFleetError::Unavailable(other.to_string()),
            })?;
        let Some(grant) = grant else {
            return Ok(None);
        };
```

Keep everything after this point (the native-spec/security-scope/session enrichment that today follows the `dispatch` call) unchanged, operating on `grant`. Import `DurableSchedulerPort` for method resolution.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p agentd-store --test worker_fleet && cargo test -p agentd-bin --test worker_main && cargo test -p agentd-bin --test recovery_http`
Expected: PASS — including the M1 e2e (`worker_once_executes_a_dispatched_task_end_to_end`), which now flows through the queue transparently.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p agentd-core -p agentd-store -p agentd-bin
git add crates/agentd-core crates/agentd-store crates/agentd-bin
git commit -m "feat(fleet): route worker pull through the durable scheduler queue

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Operator explain route + docs

**Files:**
- Modify: `crates/agentd-bin/src/daemon.rs` (route on recovery router)
- Modify: `docs/parity/agent-chat-capability-map.md`
- Test: `crates/agentd-bin/tests/recovery_http.rs`

**Interfaces:**
- Consumes: `SqliteDurableScheduler::explain_task` (Task 2), recovery router auth helper (`recovery_unauthorized`).
- Produces: `GET /api/scheduler/tasks/:task_id/explain` → 401 unauthorized / 404 unknown task / 200 `SchedulerTaskExplanation` JSON.

- [ ] **Step 1: Write the failing test**

Append to `crates/agentd-bin/tests/recovery_http.rs` (reuse the file's service/app construction and, for a queued task, the lease-fixture seeding from its acknowledge test):

```rust
#[tokio::test]
async fn recovery_http_explains_scheduler_task_state() {
    // Standard fixture: store + service + recovery_router("operator-secret"),
    // one open task seeded, then enqueue it:
    // SqliteDurableScheduler::new(pool).enqueue(...) with request_id "rq-explain".
    let unauthorized = app
        .clone()
        .oneshot(
            Request::get(format!("/api/scheduler/tasks/{}/explain", task_id.as_str()))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let missing = app
        .clone()
        .oneshot(
            Request::get("/api/scheduler/tasks/tr_unknown/explain")
                .header("authorization", "Bearer operator-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let explained = app
        .clone()
        .oneshot(
            Request::get(format!("/api/scheduler/tasks/{}/explain", task_id.as_str()))
                .header("authorization", "Bearer operator-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(explained.status(), StatusCode::OK);
    let body = explained.into_body().collect().await.expect("body").to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json["queue"]["status"], "queued");
    assert!(json["active_lease"].is_null());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p agentd-bin --test recovery_http`
Expected: new test FAILS (404/405 on the explain route for the queued task — route missing).

- [ ] **Step 3: Implement**

In `daemon.rs`: add `.route("/api/scheduler/tasks/:task_id/explain", get(explain_scheduler_task))` to `recovery_router` and:

```rust
async fn explain_scheduler_task(
    State(state): State<RecoveryApiState>,
    headers: HeaderMap,
    AxumPath(task_id): AxumPath<String>,
) -> Response {
    if let Some(response) = recovery_unauthorized(&state, &headers) {
        return response;
    }
    let scheduler = agentd_store::durable_scheduler::SqliteDurableScheduler::new(
        state.service.store_pool(),
    );
    use agentd_core::ports::DurableSchedulerPort as _;
    match scheduler
        .explain_task(&agentd_core::types::TaskRunId::from_string(task_id))
        .await
    {
        Ok(Some(explanation)) => (StatusCode::OK, Json(explanation)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "task has no scheduler state" })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}
```

Add a small accessor on `WorkerFleetService` if none exists: `pub(crate) fn store_pool(&self) -> sqlx::SqlitePool { self.native_worker.store().pool().clone() }` (or reuse an existing equivalent — check first).

Update `docs/parity/agent-chat-capability-map.md`: `pool_scheduler` note — append: "M2 Plan A adds the durable queue authority (execution_task_queue + acquisitions + scheduler outbox, single-transaction acquire), retry/dead-letter reconciliation in the fleet tick, request-idempotent pull, and the operator explain API; row stays partial until production workflow dispatch routes through it (M2 Plan B)." `durable_task_leases` note — append: "M2 Plan A composes lease grants inside the scheduler's atomic acquire."

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p agentd-bin --test recovery_http`
Expected: PASS.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
git add crates/agentd-bin docs/parity/agent-chat-capability-map.md
git commit -m "feat(scheduler): operator explain API for task scheduling state

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Self-Review Notes

- **Design-doc §M2 coverage:** item 1 (canonical single-transaction queue+lease+outbox) → Tasks 1-3; item 3 (pull idempotency, retry, dead-letter) → Tasks 3-5; item 4 (reaper) → Task 4 (reconcile in `worker_fleet_tick`, after `expire_due`); item 5 (explain API) → Task 6. Item 2 (fleet capability/capacity inventory, zone, version negotiation) and the "production dispatch routes to native workers" exit-gate item are **deliberately deferred to M2 Plan B** — this plan is the transactional authority those need first (same sequencing as Codex's AD-E2 "smallest plan").
- **Known open shapes for implementers:** the exact name/signature of the in-transaction grant reader in `task_lease_control_plane.rs` (Task 2/3 instruct locating it); `id_newtype!` invocation style (Task 1 instructs mirroring `LeaseId`); `worker_fleet_tick` call sites (Task 4 instructs grepping). These are locate-and-mirror instructions, not placeholders.
- **M1 regression guard:** Task 5 Step 4 runs the M1 worker e2e; the queue bridge (auto-enqueue) keeps the pulled-task behavior identical from the worker's perspective.
