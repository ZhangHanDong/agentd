//! Durable local authority and process bindings for the native agent backend.

use agentd_core::types::{
    AgentProfileId, CapabilityAdmission, RunId, RuntimeAttemptId, RuntimeSessionId, TaskRunId,
    WorkerId, WorkerIncarnationId,
};
use serde_json::json;
use sqlx::{Row, SqlitePool};

use crate::agent_profile_repo::AgentProfileCreate;
use crate::util::now_unix;
use crate::{StoreError, agent_profile_repo, run_repo, task_repo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRuntimeAuthority {
    pub worker_id: WorkerId,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub host_instance_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeAgentRuntimeBinding {
    pub runtime_session_id: RuntimeSessionId,
    pub runtime_attempt_id: RuntimeAttemptId,
    pub agent_id: String,
    pub execution_task_id: TaskRunId,
    pub synthetic_task: bool,
    pub capability: CapabilityAdmission,
    pub worktree: String,
    pub status: String,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

pub async fn ensure_native_runtime_authority(
    pool: &SqlitePool,
    host_instance_id: &str,
) -> Result<NativeRuntimeAuthority, StoreError> {
    if host_instance_id.trim().is_empty() || host_instance_id.len() > 128 {
        return Err(StoreError::Invariant(
            "native runtime host instance id is invalid".to_string(),
        ));
    }
    let mut transaction = pool.begin().await?;
    let worker_id = match sqlx::query_scalar::<_, String>(
        "SELECT worker_id FROM native_runtime_authority WHERE singleton = 1",
    )
    .fetch_optional(&mut *transaction)
    .await?
    {
        Some(id) => WorkerId::from_string(id),
        None => {
            let id = WorkerId::new();
            let now = now_unix();
            sqlx::query(
                "INSERT INTO workers \
                 (id, status, trust_domain, labels_json, record_version, created_at, updated_at) \
                 VALUES (?, 'online', 'local.agentd', ?, 1, ?, ?)",
            )
            .bind(id.as_str())
            .bind(serde_json::to_string(
                &json!({"authority": "native_local"}),
            )?)
            .bind(now)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "INSERT INTO native_runtime_authority (singleton, worker_id, created_at) \
                 VALUES (1, ?, ?)",
            )
            .bind(id.as_str())
            .bind(now)
            .execute(&mut *transaction)
            .await?;
            id
        }
    };
    let current = sqlx::query_scalar::<_, String>(
        "SELECT id FROM worker_incarnations WHERE worker_id = ? AND is_current = 1",
    )
    .bind(worker_id.as_str())
    .fetch_optional(&mut *transaction)
    .await?;
    let worker_incarnation_id = if let Some(id) = current {
        WorkerIncarnationId::from_string(id)
    } else {
        let id = WorkerIncarnationId::new();
        let now = now_unix();
        sqlx::query(
            "INSERT INTO worker_incarnations \
             (id, worker_id, daemon_version, host_name, network_zone, capabilities_json, \
              is_current, registered_at, last_seen_at, superseded_at) \
             VALUES (?, ?, ?, ?, 'local', ?, 1, ?, ?, NULL)",
        )
        .bind(id.as_str())
        .bind(worker_id.as_str())
        .bind(env!("CARGO_PKG_VERSION"))
        .bind(host_instance_id)
        .bind(serde_json::to_string(&json!({"native_pty": true}))?)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        id
    };
    sqlx::query(
        "UPDATE workers SET status = 'online', record_version = record_version + 1, \
         updated_at = ?, retired_at = NULL WHERE id = ? AND status != 'retired'",
    )
    .bind(now_unix())
    .bind(worker_id.as_str())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(NativeRuntimeAuthority {
        worker_id,
        worker_incarnation_id,
        host_instance_id: host_instance_id.to_string(),
    })
}

pub async fn ensure_native_agent_profile(
    pool: &SqlitePool,
    role: &str,
    runtime: &str,
) -> Result<AgentProfileId, StoreError> {
    let existing = sqlx::query_scalar::<_, String>(
        "SELECT id FROM agent_profiles WHERE role = ? AND runtime = ? AND status = 'active' \
         ORDER BY created_at, id LIMIT 1",
    )
    .bind(role)
    .bind(runtime)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = existing {
        return Ok(AgentProfileId::from_string(id));
    }
    let id = AgentProfileId::new();
    agent_profile_repo::create_profile(
        pool,
        AgentProfileCreate {
            id: id.clone(),
            role: role.to_string(),
            capability: None,
            runtime: runtime.to_string(),
            model: None,
            prompt_profile: Some("native-runtime".to_string()),
        },
    )
    .await?;
    Ok(id)
}

pub async fn ensure_native_execution_task(
    pool: &SqlitePool,
    requested: Option<&TaskRunId>,
) -> Result<(TaskRunId, bool), StoreError> {
    if let Some(task_id) = requested {
        let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM task_runs WHERE id = ?")
            .bind(task_id.as_str())
            .fetch_one(pool)
            .await?;
        if exists != 1 {
            return Err(StoreError::NotFound);
        }
        return Ok((task_id.clone(), false));
    }
    let run_id = RunId::new();
    run_repo::insert_run(pool, &run_id, &"0".repeat(64)).await?;
    let task_id = task_repo::insert_task_run(
        pool,
        &run_id,
        &agentd_core::types::NodeId::parsed("native-agent-runtime"),
    )
    .await?;
    Ok((task_id, true))
}

pub async fn record_native_agent_binding(
    pool: &SqlitePool,
    binding: &NativeAgentRuntimeBinding,
) -> Result<NativeAgentRuntimeBinding, StoreError> {
    validate_binding(binding)?;
    sqlx::query(
        "INSERT OR IGNORE INTO native_agent_runtime_bindings \
         (runtime_session_id, runtime_attempt_id, agent_id, execution_task_id, synthetic_task, \
          capability_json, worktree, status, created_at, finished_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, NULL)",
    )
    .bind(binding.runtime_session_id.as_str())
    .bind(binding.runtime_attempt_id.as_str())
    .bind(&binding.agent_id)
    .bind(binding.execution_task_id.as_str())
    .bind(binding.synthetic_task)
    .bind(serde_json::to_string(&binding.capability)?)
    .bind(&binding.worktree)
    .bind(binding.created_at)
    .execute(pool)
    .await?;
    let stored = native_agent_binding(pool, &binding.runtime_session_id)
        .await?
        .ok_or(StoreError::NotFound)?;
    if stored.runtime_attempt_id != binding.runtime_attempt_id
        || stored.agent_id != binding.agent_id
        || stored.execution_task_id != binding.execution_task_id
        || stored.synthetic_task != binding.synthetic_task
        || stored.capability != binding.capability
        || stored.worktree != binding.worktree
    {
        return Err(StoreError::Conflict(
            "native agent runtime binding replay differs".to_string(),
        ));
    }
    Ok(stored)
}

pub async fn native_agent_binding(
    pool: &SqlitePool,
    session_id: &RuntimeSessionId,
) -> Result<Option<NativeAgentRuntimeBinding>, StoreError> {
    let row = sqlx::query(
        "SELECT runtime_session_id, runtime_attempt_id, agent_id, execution_task_id, \
         synthetic_task, capability_json, worktree, status, created_at, finished_at \
         FROM native_agent_runtime_bindings WHERE runtime_session_id = ?",
    )
    .bind(session_id.as_str())
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(binding_from_row).transpose()
}

pub async fn active_native_agent_binding(
    pool: &SqlitePool,
    agent_id: &str,
) -> Result<Option<NativeAgentRuntimeBinding>, StoreError> {
    let row = sqlx::query(
        "SELECT runtime_session_id, runtime_attempt_id, agent_id, execution_task_id, \
         synthetic_task, capability_json, worktree, status, created_at, finished_at \
         FROM native_agent_runtime_bindings WHERE agent_id = ? AND status = 'active' \
         ORDER BY created_at DESC, runtime_session_id DESC LIMIT 1",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(binding_from_row).transpose()
}

pub async fn finish_native_agent_binding(
    pool: &SqlitePool,
    session_id: &RuntimeSessionId,
    finished_at: i64,
) -> Result<NativeAgentRuntimeBinding, StoreError> {
    let binding = native_agent_binding(pool, session_id)
        .await?
        .ok_or(StoreError::NotFound)?;
    if binding.status == "active" {
        sqlx::query(
            "UPDATE native_agent_runtime_bindings SET status = 'finished', finished_at = ? \
             WHERE runtime_session_id = ? AND status = 'active'",
        )
        .bind(finished_at)
        .bind(session_id.as_str())
        .execute(pool)
        .await?;
        if binding.synthetic_task {
            task_repo::complete_task_run(pool, &binding.execution_task_id).await?;
        }
    }
    native_agent_binding(pool, session_id)
        .await?
        .ok_or(StoreError::NotFound)
}

fn validate_binding(binding: &NativeAgentRuntimeBinding) -> Result<(), StoreError> {
    if binding.agent_id.trim().is_empty()
        || binding.worktree.trim().is_empty()
        || binding.status != "active"
        || binding.finished_at.is_some()
    {
        return Err(StoreError::Invariant(
            "native agent runtime binding is invalid".to_string(),
        ));
    }
    Ok(())
}

fn binding_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<NativeAgentRuntimeBinding, StoreError> {
    let capability_json = row.get::<String, _>("capability_json");
    Ok(NativeAgentRuntimeBinding {
        runtime_session_id: RuntimeSessionId::from_string(
            row.get::<String, _>("runtime_session_id"),
        ),
        runtime_attempt_id: RuntimeAttemptId::from_string(
            row.get::<String, _>("runtime_attempt_id"),
        ),
        agent_id: row.get("agent_id"),
        execution_task_id: TaskRunId::from_string(row.get::<String, _>("execution_task_id")),
        synthetic_task: row.get("synthetic_task"),
        capability: serde_json::from_str(&capability_json).map_err(|error| {
            StoreError::Invariant(format!("native capability evidence is invalid: {error}"))
        })?,
        worktree: row.get("worktree"),
        status: row.get("status"),
        created_at: row.get("created_at"),
        finished_at: row.get("finished_at"),
    })
}
