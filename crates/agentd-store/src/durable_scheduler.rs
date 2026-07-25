//! `SQLite` authority for the durable scheduler port (M2 Plan A).

use agentd_core::ports::{
    DurableSchedulerError, DurableSchedulerPort, SchedulerAcquireRequest, SchedulerEnqueueRequest,
    SchedulerQueueRecord, SchedulerTaskExplanation, TaskLeaseDispatchRequest,
};
use agentd_core::types::{
    LeaseId, SchedulerEventId, SchedulerQueueId, SchedulerQueueStatus, TaskLeaseGrant, TaskRunId,
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

fn queue_record(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<SchedulerQueueRecord, DurableSchedulerError> {
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
        if let Some((record, original)) = get_by_request_id(&self.pool, &request.request_id).await?
        {
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
            Err(sqlx::Error::Database(db))
                if db.message().contains(
                    "UNIQUE constraint failed: execution_task_queue.execution_task_id",
                ) =>
            {
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

    async fn reconcile(&self, observed_at: i64) -> Result<u64, DurableSchedulerError> {
        let mut connection = self.pool.acquire().await.map_err(storage_error)?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await
            .map_err(storage_error)?;
        let result = reconcile_in_transaction(&mut connection, observed_at).await;
        match result {
            Ok(count) => {
                sqlx::query("COMMIT")
                    .execute(&mut *connection)
                    .await
                    .map_err(storage_error)?;
                Ok(count)
            }
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                Err(error)
            }
        }
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
        Ok(Some(SchedulerTaskExplanation {
            queue,
            active_lease,
        }))
    }
}

async fn acquire_in_transaction(
    connection: &mut sqlx::SqliteConnection,
    request: &SchedulerAcquireRequest,
) -> Result<Option<TaskLeaseGrant>, DurableSchedulerError> {
    // Idempotent replay: a completed acquisition returns its original grant.
    let existing: Option<String> =
        sqlx::query_scalar("SELECT lease_id FROM scheduler_acquisitions WHERE request_id = ?")
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
        let placeholders = std::iter::repeat_n("?", runtimes.len())
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

    // Select the oldest eligible queue row. A row whose task closed while it
    // sat queued (finished_at set / status no longer 'running') is
    // terminalized in place and skipped rather than retried: leaving it
    // 'queued' would let it keep winning selection forever and wedge the
    // scheduler, since reconcile() only handles 'leased' rows. Every skipped
    // row is terminalized before the next iteration, so the eligible set
    // strictly shrinks and the loop is bounded. A capability-incompatible row
    // is excluded by the SELECT itself (never terminalized) so another worker
    // can still take it.
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

        // A Conflict from dispatch here is worker-related (stale incarnation,
        // worker offline) since the task-open check above just passed;
        // abort the whole acquire rather than terminalizing the row.
        let grant = grant_and_transition(connection, request, &queue_id, &task_id).await?;
        return Ok(Some(grant));
    }
}

async fn task_is_open(
    connection: &mut sqlx::SqliteConnection,
    task_id: &str,
) -> Result<bool, DurableSchedulerError> {
    let open: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_runs WHERE id = ? AND finished_at IS NULL \
         AND status = 'running'",
    )
    .bind(task_id)
    .fetch_one(&mut *connection)
    .await
    .map_err(storage_error)?;
    Ok(open != 0)
}

async fn terminalize_closed_row(
    connection: &mut sqlx::SqliteConnection,
    queue_id: &str,
    task_id: &str,
    observed_at: i64,
) -> Result<(), DurableSchedulerError> {
    let cancelled = sqlx::query(
        "UPDATE execution_task_queue SET status = 'cancelled', current_lease_id = NULL, \
         last_reason = ?, updated_at = ? WHERE id = ? AND status = 'queued'",
    )
    .bind(format!("task {task_id} closed before lease"))
    .bind(observed_at)
    .bind(queue_id)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    if cancelled.rows_affected() != 1 {
        return Err(DurableSchedulerError::Conflict(
            "queue row changed during acquisition".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO execution_scheduler_outbox \
         (event_id, kind, queue_id, task_id, lease_id, payload, created_at) \
         VALUES (?, 'task_cancelled', ?, ?, ?, '{}', ?)",
    )
    .bind(SchedulerEventId::new().as_str())
    .bind(queue_id)
    .bind(task_id)
    .bind(Option::<String>::None)
    .bind(observed_at)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    Ok(())
}

/// Grant the lease through the existing fenced primitive (validates the open
/// task, the current online incarnation, and allocates the token), then
/// transition the queue row and record the acquisition/outbox event.
async fn grant_and_transition(
    connection: &mut sqlx::SqliteConnection,
    request: &SchedulerAcquireRequest,
    queue_id: &str,
    task_id: &str,
) -> Result<TaskLeaseGrant, DurableSchedulerError> {
    let grant = crate::task_lease_control_plane::dispatch_in_transaction(
        connection,
        &TaskLeaseDispatchRequest {
            execution_task_id: TaskRunId::from_string(task_id.to_string()),
            worker_incarnation_id: request.worker_incarnation_id.clone(),
            observed_at: request.observed_at,
            expires_at: request.expires_at,
        },
    )
    .await
    .map_err(|error| DurableSchedulerError::Conflict(error.to_string()))?;

    // Transition the queue row. The guard is defensive: BEGIN IMMEDIATE
    // serializes writers, so a raced row is unreachable today, but a zero-row
    // update must never let the acquisition/outbox writes proceed against an
    // un-transitioned queue row.
    let transitioned = sqlx::query(
        "UPDATE execution_task_queue SET status = 'leased', attempts = attempts + 1, \
         current_lease_id = ?, updated_at = ? WHERE id = ? AND status = 'queued'",
    )
    .bind(grant.lease_id.as_str())
    .bind(request.observed_at)
    .bind(queue_id)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    if transitioned.rows_affected() != 1 {
        return Err(DurableSchedulerError::Conflict(
            "queue row changed during acquisition".into(),
        ));
    }

    // Record the acquisition for replay and append the outbox event.
    sqlx::query(
        "INSERT INTO scheduler_acquisitions \
         (request_id, queue_id, lease_id, worker_incarnation_id, acquired_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&request.request_id)
    .bind(queue_id)
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
    .bind(queue_id)
    .bind(task_id)
    .bind(grant.lease_id.as_str())
    .bind(request.observed_at)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;

    Ok(grant)
}

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
            "released" => (
                "completed",
                format!("lease {lease_id} released"),
                "task_completed",
            ),
            "cancelled" => (
                "cancelled",
                format!("lease {lease_id} cancelled"),
                "task_cancelled",
            ),
            // expired / superseded: retry or dead-letter.
            other => {
                if attempts >= max_attempts {
                    (
                        "dead_letter",
                        format!("lease {lease_id} {other}; attempts exhausted"),
                        "task_dead_lettered",
                    )
                } else {
                    (
                        "queued",
                        format!("lease {lease_id} {other}; requeued"),
                        "task_requeued",
                    )
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
