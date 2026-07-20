//! `SQLite` control-plane adapter for durable task dispatch and fencing.

use agentd_core::ports::{
    TaskLeaseCloseRequest, TaskLeaseDispatchRequest, TaskLeaseError, TaskLeasePort,
    TaskLeaseRejectionReason, TaskLeaseRenewRequest,
};
use agentd_core::types::{
    FencingToken, LeaseId, LeaseStatus, NativeExecutionSpec, TaskLeaseClaim, TaskLeaseGrant,
    TaskRunId, WorkerIncarnationId,
};
use sqlx::pool::PoolConnection;
use sqlx::{Row, Sqlite, SqliteConnection, SqlitePool};

#[derive(Debug, Clone)]
pub struct SqliteTaskLeaseControlPlane {
    pool: SqlitePool,
}

impl SqliteTaskLeaseControlPlane {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl TaskLeasePort for SqliteTaskLeaseControlPlane {
    async fn dispatch(
        &self,
        request: &TaskLeaseDispatchRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        validate_dispatch(request)?;
        let mut connection = self.pool.acquire().await.map_err(storage_error)?;
        begin_immediate(&mut connection).await?;
        let result = dispatch_in_transaction(&mut connection, request).await;
        finish_transaction(&mut connection, result).await
    }

    async fn renew(
        &self,
        request: &TaskLeaseRenewRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        validate_claim_input(&request.claim, request.observed_at)?;
        if request.expires_at <= request.observed_at {
            return Err(TaskLeaseError::Invalid(
                "expires_at must be greater than observed_at".to_string(),
            ));
        }
        let mut connection = self.pool.acquire().await.map_err(storage_error)?;
        begin_immediate(&mut connection).await?;
        let result = renew_in_transaction(&mut connection, request).await;
        finish_decision(&mut connection, result).await
    }

    async fn release(
        &self,
        request: &TaskLeaseCloseRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        self.close(request, LeaseStatus::Released).await
    }

    async fn cancel(
        &self,
        request: &TaskLeaseCloseRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        self.close(request, LeaseStatus::Cancelled).await
    }

    async fn validate_claim(
        &self,
        claim: &TaskLeaseClaim,
        observed_at: i64,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        validate_claim_input(claim, observed_at)?;
        let mut connection = self.pool.acquire().await.map_err(storage_error)?;
        begin_immediate(&mut connection).await?;
        let result = match authorize_claim(&mut connection, claim, observed_at).await {
            Ok(ClaimAuthorization::Authorized(grant)) => Ok(Decision::Return(*grant)),
            Ok(ClaimAuthorization::Rejected(error)) => Ok(Decision::Reject(error)),
            Err(error) => Err(error),
        };
        finish_decision(&mut connection, result).await
    }

    async fn expire_due(&self, observed_at: i64) -> Result<u64, TaskLeaseError> {
        validate_observed_at(observed_at)?;
        let mut connection = self.pool.acquire().await.map_err(storage_error)?;
        begin_immediate(&mut connection).await?;
        let result = expire_due_in_transaction(&mut connection, observed_at).await;
        finish_transaction(&mut connection, result).await
    }
}

impl SqliteTaskLeaseControlPlane {
    async fn close(
        &self,
        request: &TaskLeaseCloseRequest,
        status: LeaseStatus,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        validate_claim_input(&request.claim, request.observed_at)?;
        if request.reason.trim().is_empty() {
            return Err(TaskLeaseError::Invalid(
                "terminal reason must not be empty".to_string(),
            ));
        }
        let mut connection = self.pool.acquire().await.map_err(storage_error)?;
        begin_immediate(&mut connection).await?;
        let result = close_in_transaction(&mut connection, request, status).await;
        finish_decision(&mut connection, result).await
    }
}

enum Decision<T> {
    Return(T),
    Reject(TaskLeaseError),
}

enum ClaimAuthorization {
    Authorized(Box<TaskLeaseGrant>),
    Rejected(TaskLeaseError),
}

async fn begin_immediate(connection: &mut PoolConnection<Sqlite>) -> Result<(), TaskLeaseError> {
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut **connection)
        .await
        .map_err(storage_error)?;
    Ok(())
}

async fn finish_transaction<T>(
    connection: &mut PoolConnection<Sqlite>,
    result: Result<T, TaskLeaseError>,
) -> Result<T, TaskLeaseError> {
    match result {
        Ok(value) => {
            sqlx::query("COMMIT")
                .execute(&mut **connection)
                .await
                .map_err(storage_error)?;
            Ok(value)
        }
        Err(error) => {
            sqlx::query("ROLLBACK")
                .execute(&mut **connection)
                .await
                .map_err(storage_error)?;
            Err(error)
        }
    }
}

async fn finish_decision<T>(
    connection: &mut PoolConnection<Sqlite>,
    result: Result<Decision<T>, TaskLeaseError>,
) -> Result<T, TaskLeaseError> {
    match result {
        Ok(Decision::Return(value)) => {
            sqlx::query("COMMIT")
                .execute(&mut **connection)
                .await
                .map_err(storage_error)?;
            Ok(value)
        }
        Ok(Decision::Reject(error)) => {
            sqlx::query("COMMIT")
                .execute(&mut **connection)
                .await
                .map_err(storage_error)?;
            Err(error)
        }
        Err(error) => {
            sqlx::query("ROLLBACK")
                .execute(&mut **connection)
                .await
                .map_err(storage_error)?;
            Err(error)
        }
    }
}

async fn renew_in_transaction(
    connection: &mut SqliteConnection,
    request: &TaskLeaseRenewRequest,
) -> Result<Decision<TaskLeaseGrant>, TaskLeaseError> {
    let grant = match authorize_claim(connection, &request.claim, request.observed_at).await? {
        ClaimAuthorization::Authorized(grant) => *grant,
        ClaimAuthorization::Rejected(error) => return Ok(Decision::Reject(error)),
    };
    if request.expires_at <= grant.expires_at {
        return Ok(Decision::Reject(TaskLeaseError::Invalid(
            "renewal expires_at must extend the current expiry".to_string(),
        )));
    }
    let updated = sqlx::query(
        "UPDATE execution_task_leases \
         SET expires_at = ?, renewed_at = ?, record_version = record_version + 1 \
         WHERE id = ? AND status = 'active' AND record_version = ?",
    )
    .bind(request.expires_at)
    .bind(request.observed_at)
    .bind(request.claim.lease_id.as_str())
    .bind(version_to_i64(grant.record_version)?)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    if updated.rows_affected() != 1 {
        return Ok(Decision::Reject(TaskLeaseError::Conflict(
            "active lease changed during renewal".to_string(),
        )));
    }
    Ok(Decision::Return(
        get_grant(connection, request.claim.lease_id.as_str()).await?,
    ))
}

async fn close_in_transaction(
    connection: &mut SqliteConnection,
    request: &TaskLeaseCloseRequest,
    status: LeaseStatus,
) -> Result<Decision<TaskLeaseGrant>, TaskLeaseError> {
    match authorize_claim(connection, &request.claim, request.observed_at).await? {
        ClaimAuthorization::Authorized(_) => {}
        ClaimAuthorization::Rejected(error) => return Ok(Decision::Reject(error)),
    }
    terminalize_active(
        connection,
        request.claim.execution_task_id.as_str(),
        request.claim.lease_id.as_str(),
        status,
        request.observed_at,
        request.reason.trim(),
    )
    .await?;
    Ok(Decision::Return(
        get_grant(connection, request.claim.lease_id.as_str()).await?,
    ))
}

async fn expire_due_in_transaction(
    connection: &mut SqliteConnection,
    observed_at: i64,
) -> Result<u64, TaskLeaseError> {
    let rows = sqlx::query(
        "SELECT id, execution_task_id FROM execution_task_leases \
         WHERE status = 'active' AND expires_at <= ? \
         ORDER BY execution_task_id, fencing_token",
    )
    .bind(observed_at)
    .fetch_all(&mut *connection)
    .await
    .map_err(storage_error)?;
    for row in &rows {
        let lease_id: String = row.get("id");
        let task_id: String = row.get("execution_task_id");
        terminalize_active(
            connection,
            &task_id,
            &lease_id,
            LeaseStatus::Expired,
            observed_at,
            "ttl_elapsed",
        )
        .await?;
    }
    u64::try_from(rows.len())
        .map_err(|_| TaskLeaseError::Unavailable("expiry result exceeds u64".to_string()))
}

async fn authorize_claim(
    connection: &mut SqliteConnection,
    claim: &TaskLeaseClaim,
    observed_at: i64,
) -> Result<ClaimAuthorization, TaskLeaseError> {
    let Some(grant) = get_optional_grant(connection, claim.lease_id.as_str()).await? else {
        return Ok(ClaimAuthorization::Rejected(TaskLeaseError::NotFound(
            format!("lease {}", claim.lease_id),
        )));
    };
    if grant.execution_task_id != claim.execution_task_id
        || grant.worker_incarnation_id != claim.worker_incarnation_id
    {
        return Ok(ClaimAuthorization::Rejected(rejected(
            TaskLeaseRejectionReason::ClaimMismatch,
            format!("claim tuple does not match lease {}", claim.lease_id),
        )));
    }

    let head = sqlx::query(
        "SELECT last_fencing_token, current_lease_id \
         FROM execution_task_lease_heads WHERE execution_task_id = ?",
    )
    .bind(claim.execution_task_id.as_str())
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?;
    let Some(head) = head else {
        return Ok(ClaimAuthorization::Rejected(rejected(
            TaskLeaseRejectionReason::NotCurrentLease,
            format!(
                "execution task {} has no lease head",
                claim.execution_task_id
            ),
        )));
    };
    let last_token = head.get::<i64, _>("last_fencing_token");
    let claim_token = token_to_i64(claim.fencing_token)?;
    if grant.fencing_token != claim.fencing_token || claim_token < last_token {
        return Ok(ClaimAuthorization::Rejected(rejected(
            TaskLeaseRejectionReason::StaleFencingToken,
            format!(
                "claim token {} is not current for execution task {}",
                claim.fencing_token, claim.execution_task_id
            ),
        )));
    }
    if grant.status.is_terminal() {
        return Ok(ClaimAuthorization::Rejected(rejected(
            TaskLeaseRejectionReason::TerminalLease,
            format!("lease {} is {}", claim.lease_id, grant.status),
        )));
    }
    let current_lease_id = head.get::<Option<String>, _>("current_lease_id");
    if current_lease_id.as_deref() != Some(claim.lease_id.as_str()) {
        return Ok(ClaimAuthorization::Rejected(rejected(
            TaskLeaseRejectionReason::NotCurrentLease,
            format!("lease {} is not the task head", claim.lease_id),
        )));
    }

    if !worker_can_report(connection, claim.worker_incarnation_id.as_str()).await? {
        return Ok(ClaimAuthorization::Rejected(rejected(
            TaskLeaseRejectionReason::StaleWorkerIncarnation,
            format!(
                "worker incarnation {} is no longer eligible",
                claim.worker_incarnation_id
            ),
        )));
    }
    if grant.expires_at <= observed_at {
        terminalize_active(
            connection,
            claim.execution_task_id.as_str(),
            claim.lease_id.as_str(),
            LeaseStatus::Expired,
            observed_at,
            "ttl_elapsed",
        )
        .await?;
        return Ok(ClaimAuthorization::Rejected(rejected(
            TaskLeaseRejectionReason::LeaseExpired,
            format!("lease {} elapsed at {}", claim.lease_id, grant.expires_at),
        )));
    }
    Ok(ClaimAuthorization::Authorized(Box::new(grant)))
}

async fn worker_can_report(
    connection: &mut SqliteConnection,
    incarnation_id: &str,
) -> Result<bool, TaskLeaseError> {
    let row = sqlx::query(
        "SELECT incarnation.is_current, worker.status \
         FROM worker_incarnations AS incarnation \
         JOIN workers AS worker ON worker.id = incarnation.worker_id \
         WHERE incarnation.id = ?",
    )
    .bind(incarnation_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?;
    let Some(row) = row else {
        return Ok(false);
    };
    let status: String = row.get("status");
    Ok(row.get::<i64, _>("is_current") == 1 && matches!(status.as_str(), "online" | "draining"))
}

async fn get_grant(
    connection: &mut SqliteConnection,
    lease_id: &str,
) -> Result<TaskLeaseGrant, TaskLeaseError> {
    get_optional_grant(connection, lease_id)
        .await?
        .ok_or_else(|| TaskLeaseError::NotFound(format!("lease {lease_id}")))
}

async fn get_optional_grant(
    connection: &mut SqliteConnection,
    lease_id: &str,
) -> Result<Option<TaskLeaseGrant>, TaskLeaseError> {
    let row = sqlx::query(
        "SELECT l.id, l.execution_task_id, l.worker_incarnation_id, l.fencing_token, l.status, \
                acquired_at, expires_at, renewed_at, terminal_at, terminal_reason, \
                record_version, t.execution_spec_json \
         FROM execution_task_leases l JOIN task_runs t ON t.id = l.execution_task_id \
         WHERE l.id = ?",
    )
    .bind(lease_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?;
    row.as_ref().map(row_to_grant).transpose()
}

fn row_to_grant(row: &sqlx::sqlite::SqliteRow) -> Result<TaskLeaseGrant, TaskLeaseError> {
    let token = row.get::<i64, _>("fencing_token");
    let status_text = row.get::<String, _>("status");
    let version = row.get::<i64, _>("record_version");
    Ok(TaskLeaseGrant {
        lease_id: LeaseId::from_string(row.get::<String, _>("id")),
        execution_task_id: TaskRunId::from_string(row.get::<String, _>("execution_task_id")),
        worker_incarnation_id: WorkerIncarnationId::from_string(
            row.get::<String, _>("worker_incarnation_id"),
        ),
        fencing_token: FencingToken::new(u64::try_from(token).map_err(|_| {
            TaskLeaseError::Unavailable(format!("invalid durable fencing token {token}"))
        })?)
        .map_err(|error| TaskLeaseError::Unavailable(error.to_string()))?,
        status: LeaseStatus::try_from(status_text.as_str()).map_err(|_| {
            TaskLeaseError::Unavailable(format!("invalid durable lease status {status_text}"))
        })?,
        acquired_at: row.get("acquired_at"),
        expires_at: row.get("expires_at"),
        renewed_at: row.get("renewed_at"),
        terminal_at: row.get("terminal_at"),
        terminal_reason: row.get("terminal_reason"),
        record_version: u64::try_from(version).map_err(|_| {
            TaskLeaseError::Unavailable(format!("invalid lease record version {version}"))
        })?,
        security_scope: None,
        runtime_session_id: None,
        execution_spec: parse_execution_spec(row.get("execution_spec_json"))?,
    })
}

fn parse_execution_spec(
    encoded: Option<String>,
) -> Result<Option<NativeExecutionSpec>, TaskLeaseError> {
    encoded
        .map(|value| {
            let spec = serde_json::from_str::<NativeExecutionSpec>(&value).map_err(|error| {
                TaskLeaseError::Unavailable(format!("invalid execution spec: {error}"))
            })?;
            spec.validate().map_err(|error| {
                TaskLeaseError::Unavailable(format!("invalid execution spec: {error}"))
            })?;
            Ok(spec)
        })
        .transpose()
}

async fn dispatch_in_transaction(
    connection: &mut SqliteConnection,
    request: &TaskLeaseDispatchRequest,
) -> Result<TaskLeaseGrant, TaskLeaseError> {
    validate_open_task(connection, request.execution_task_id.as_str()).await?;
    validate_dispatch_worker(connection, request.worker_incarnation_id.as_str()).await?;
    reconcile_active_lease(
        connection,
        request.execution_task_id.as_str(),
        request.observed_at,
    )
    .await?;

    let fencing_token = allocate_fencing_token(
        connection,
        request.execution_task_id.as_str(),
        request.observed_at,
    )
    .await?;
    let lease_id = LeaseId::new();
    sqlx::query(
        "INSERT INTO execution_task_leases \
         (id, execution_task_id, worker_incarnation_id, fencing_token, status, \
          acquired_at, expires_at, record_version) \
         VALUES (?, ?, ?, ?, 'active', ?, ?, 1)",
    )
    .bind(lease_id.as_str())
    .bind(request.execution_task_id.as_str())
    .bind(request.worker_incarnation_id.as_str())
    .bind(token_to_i64(fencing_token)?)
    .bind(request.observed_at)
    .bind(request.expires_at)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    let updated = sqlx::query(
        "UPDATE execution_task_lease_heads \
         SET current_lease_id = ?, updated_at = ? \
         WHERE execution_task_id = ? AND last_fencing_token = ?",
    )
    .bind(lease_id.as_str())
    .bind(request.observed_at)
    .bind(request.execution_task_id.as_str())
    .bind(token_to_i64(fencing_token)?)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(TaskLeaseError::Conflict(
            "task lease head changed during dispatch".to_string(),
        ));
    }

    let execution_spec = sqlx::query_scalar::<_, Option<String>>(
        "SELECT execution_spec_json FROM task_runs WHERE id = ?",
    )
    .bind(request.execution_task_id.as_str())
    .fetch_one(&mut *connection)
    .await
    .map_err(storage_error)
    .and_then(parse_execution_spec)?;
    Ok(TaskLeaseGrant {
        lease_id,
        execution_task_id: request.execution_task_id.clone(),
        worker_incarnation_id: request.worker_incarnation_id.clone(),
        fencing_token,
        status: LeaseStatus::Active,
        acquired_at: request.observed_at,
        expires_at: request.expires_at,
        renewed_at: None,
        terminal_at: None,
        terminal_reason: None,
        record_version: 1,
        execution_spec,
        security_scope: None,
        runtime_session_id: None,
    })
}

fn validate_dispatch(request: &TaskLeaseDispatchRequest) -> Result<(), TaskLeaseError> {
    validate_id(request.execution_task_id.as_str(), "tr_", "TaskRunId")?;
    validate_id(
        request.worker_incarnation_id.as_str(),
        "wi_",
        "WorkerIncarnationId",
    )?;
    validate_observed_at(request.observed_at)?;
    if request.expires_at <= request.observed_at {
        return Err(TaskLeaseError::Invalid(
            "expires_at must be greater than observed_at".to_string(),
        ));
    }
    Ok(())
}

fn validate_claim_input(claim: &TaskLeaseClaim, observed_at: i64) -> Result<(), TaskLeaseError> {
    validate_id(claim.execution_task_id.as_str(), "tr_", "TaskRunId")?;
    validate_id(
        claim.worker_incarnation_id.as_str(),
        "wi_",
        "WorkerIncarnationId",
    )?;
    validate_id(claim.lease_id.as_str(), "ls_", "LeaseId")?;
    validate_observed_at(observed_at)
}

async fn validate_open_task(
    connection: &mut SqliteConnection,
    task_id: &str,
) -> Result<(), TaskLeaseError> {
    let row = sqlx::query("SELECT finished_at FROM task_runs WHERE id = ?")
        .bind(task_id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| TaskLeaseError::NotFound(format!("execution task {task_id}")))?;
    if row.get::<Option<i64>, _>("finished_at").is_some() {
        return Err(TaskLeaseError::Conflict(format!(
            "execution task {task_id} is finished"
        )));
    }
    Ok(())
}

async fn validate_dispatch_worker(
    connection: &mut SqliteConnection,
    incarnation_id: &str,
) -> Result<(), TaskLeaseError> {
    let row = sqlx::query(
        "SELECT incarnation.is_current, worker.status \
         FROM worker_incarnations AS incarnation \
         JOIN workers AS worker ON worker.id = incarnation.worker_id \
         WHERE incarnation.id = ?",
    )
    .bind(incarnation_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?
    .ok_or_else(|| TaskLeaseError::NotFound(format!("worker incarnation {incarnation_id}")))?;
    if row.get::<i64, _>("is_current") != 1 {
        return Err(rejected(
            TaskLeaseRejectionReason::StaleWorkerIncarnation,
            format!("worker incarnation {incarnation_id} is superseded"),
        ));
    }
    let status: String = row.get("status");
    if status != "online" {
        return Err(TaskLeaseError::Conflict(format!(
            "worker incarnation {incarnation_id} cannot dispatch while worker is {status}"
        )));
    }
    Ok(())
}

async fn reconcile_active_lease(
    connection: &mut SqliteConnection,
    task_id: &str,
    observed_at: i64,
) -> Result<(), TaskLeaseError> {
    let row = sqlx::query(
        "SELECT lease.id, lease.worker_incarnation_id, lease.expires_at, \
                incarnation.is_current \
         FROM execution_task_leases AS lease \
         JOIN worker_incarnations AS incarnation \
           ON incarnation.id = lease.worker_incarnation_id \
         WHERE lease.execution_task_id = ? AND lease.status = 'active'",
    )
    .bind(task_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?;
    let Some(row) = row else {
        return Ok(());
    };
    let lease_id: String = row.get("id");
    let expires_at: i64 = row.get("expires_at");
    let is_current: i64 = row.get("is_current");
    if expires_at <= observed_at {
        terminalize_active(
            connection,
            task_id,
            &lease_id,
            LeaseStatus::Expired,
            observed_at,
            "ttl_elapsed",
        )
        .await?;
        return Ok(());
    }
    if is_current != 1 {
        terminalize_active(
            connection,
            task_id,
            &lease_id,
            LeaseStatus::Superseded,
            observed_at,
            "worker_incarnation_superseded",
        )
        .await?;
        return Ok(());
    }
    Err(TaskLeaseError::Conflict(format!(
        "execution task {task_id} already has active lease {lease_id}"
    )))
}

async fn terminalize_active(
    connection: &mut SqliteConnection,
    task_id: &str,
    lease_id: &str,
    status: LeaseStatus,
    terminal_at: i64,
    reason: &str,
) -> Result<(), TaskLeaseError> {
    let updated = sqlx::query(
        "UPDATE execution_task_leases \
         SET status = ?, terminal_at = ?, terminal_reason = ?, \
             record_version = record_version + 1 \
         WHERE id = ? AND execution_task_id = ? AND status = 'active'",
    )
    .bind(status.as_str())
    .bind(terminal_at)
    .bind(reason)
    .bind(lease_id)
    .bind(task_id)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(TaskLeaseError::Conflict(format!(
            "active lease {lease_id} changed during reconciliation"
        )));
    }
    sqlx::query(
        "UPDATE execution_task_lease_heads \
         SET current_lease_id = NULL, updated_at = ? \
         WHERE execution_task_id = ? AND current_lease_id = ?",
    )
    .bind(terminal_at)
    .bind(task_id)
    .bind(lease_id)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    Ok(())
}

async fn allocate_fencing_token(
    connection: &mut SqliteConnection,
    task_id: &str,
    observed_at: i64,
) -> Result<FencingToken, TaskLeaseError> {
    let current: Option<i64> = sqlx::query_scalar(
        "SELECT last_fencing_token FROM execution_task_lease_heads \
         WHERE execution_task_id = ?",
    )
    .bind(task_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?;
    let next = match current {
        None => {
            sqlx::query(
                "INSERT INTO execution_task_lease_heads \
                 (execution_task_id, last_fencing_token, current_lease_id, updated_at) \
                 VALUES (?, 1, NULL, ?)",
            )
            .bind(task_id)
            .bind(observed_at)
            .execute(&mut *connection)
            .await
            .map_err(storage_error)?;
            1
        }
        Some(current) => {
            let next = current.checked_add(1).ok_or_else(|| {
                TaskLeaseError::Conflict(format!(
                    "fencing token exhausted for execution task {task_id}"
                ))
            })?;
            sqlx::query(
                "UPDATE execution_task_lease_heads \
                 SET last_fencing_token = ?, current_lease_id = NULL, updated_at = ? \
                 WHERE execution_task_id = ? AND last_fencing_token = ?",
            )
            .bind(next)
            .bind(observed_at)
            .bind(task_id)
            .bind(current)
            .execute(&mut *connection)
            .await
            .map_err(storage_error)?;
            next
        }
    };
    FencingToken::new(
        u64::try_from(next).map_err(|_| {
            TaskLeaseError::Invalid(format!("invalid durable fencing token {next}"))
        })?,
    )
    .map_err(|error| TaskLeaseError::Invalid(error.to_string()))
}

fn validate_id(value: &str, prefix: &str, label: &str) -> Result<(), TaskLeaseError> {
    let payload = value
        .strip_prefix(prefix)
        .ok_or_else(|| TaskLeaseError::Invalid(format!("invalid {label}: {value}")))?;
    if payload.len() != 26 || payload.parse::<ulid::Ulid>().is_err() {
        return Err(TaskLeaseError::Invalid(format!("invalid {label}: {value}")));
    }
    Ok(())
}

fn validate_observed_at(observed_at: i64) -> Result<(), TaskLeaseError> {
    if observed_at < 0 {
        return Err(TaskLeaseError::Invalid(
            "observed_at must be non-negative".to_string(),
        ));
    }
    Ok(())
}

fn token_to_i64(token: FencingToken) -> Result<i64, TaskLeaseError> {
    i64::try_from(token.value()).map_err(|_| {
        TaskLeaseError::Conflict("fencing token exceeds SQLite INTEGER range".to_string())
    })
}

fn version_to_i64(version: u64) -> Result<i64, TaskLeaseError> {
    i64::try_from(version).map_err(|_| {
        TaskLeaseError::Unavailable("lease record version exceeds SQLite INTEGER".to_string())
    })
}

fn rejected(reason: TaskLeaseRejectionReason, message: String) -> TaskLeaseError {
    TaskLeaseError::Rejected { reason, message }
}

fn storage_error(error: impl std::fmt::Display) -> TaskLeaseError {
    TaskLeaseError::Unavailable(format!("{error}"))
}
