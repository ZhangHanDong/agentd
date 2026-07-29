//! Turn accepted Matrix commands into runs, idempotently.
//!
//! Run creation deliberately sits outside the inbound transaction: creating a
//! task graph advances it, which dispatches messages and enqueues execution
//! rows, and none of that belongs inside the request that accepts a Matrix
//! event. Instead the durable command row is the handoff, and this sweep — the
//! same shape and error discipline as the other maintenance-tick sweeps —
//! drives it.
//!
//! Idempotency has two independent guards, which is what makes restart/replay
//! produce zero duplicate accepted executions: the graph id is derived from
//! the canonical `command_id`, so a replayed sweep hits `create_graph`'s
//! duplicate-id `Conflict` rather than creating a second graph; and the
//! command→run bind is a compare-and-set that only fires on an `accepted` row
//! with no `run_id`.
//!
//! The reverse edge lives here too: `settle_running_commands` retires a
//! command once its run reaches a terminal state. Without it a command stays
//! `running` forever and keeps holding its room's open-dedup slot, which makes
//! re-sending the same command text in that room permanently impossible.

use std::collections::BTreeMap;

use sqlx::SqlitePool;

use crate::agent_chat_task_graph_repo;
use crate::error::StoreError;
use crate::matrix_bridge_repo::{
    self, MatrixCommandRecord, MatrixCommandRunPlan, matrix_command_graph_id,
};

/// Create the run for every accepted command that has none yet.
///
/// Returns how many commands were bound to a run.
///
/// # Errors
/// [`StoreError`] only if the listing itself fails. One command's failure is
/// isolated and logged: this runs on the maintenance tick, where a single bad
/// command must not stop the sweep or the loop.
pub async fn dispatch_accepted_commands(pool: &SqlitePool) -> Result<u64, StoreError> {
    let commands = matrix_bridge_repo::list_accepted_commands(pool).await?;
    let mut dispatched = 0_u64;
    for command in commands {
        match dispatch_one(pool, &command).await {
            Ok(true) => dispatched += 1,
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(
                    command_id = command.command_id.as_str(),
                    %error,
                    "dispatching accepted Matrix command failed this tick"
                );
            }
        }
    }
    Ok(dispatched)
}

async fn dispatch_one(
    pool: &SqlitePool,
    command: &MatrixCommandRecord,
) -> Result<bool, StoreError> {
    let Some(plan_json) = command.run_request_json.as_deref() else {
        // Accepted with no run plan: nothing to create. Settle it so it stops
        // holding the open-dedup slot for its room and project.
        settle_without_run(pool, command).await?;
        return Ok(false);
    };
    let plan: MatrixCommandRunPlan = serde_json::from_str(plan_json)?;
    let graph_id = matrix_command_graph_id(&command.command_id);

    let mut nodes = BTreeMap::new();
    nodes.insert(
        "run".to_string(),
        agent_chat_task_graph_repo::AgentChatTaskGraphNodeInput {
            id: None,
            assignee: Some(plan.assignee.clone()),
            role: None,
            capability: None,
            description: plan.description.clone(),
            depends_on: Vec::new(),
            condition: None,
            execution: None,
        },
    );

    match agent_chat_task_graph_repo::create_graph(
        pool,
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some(graph_id.clone()),
            owner: plan.owner.clone(),
            label: plan.label.clone(),
            nodes,
        },
    )
    .await
    {
        Ok(_) => {}
        // The graph already exists: a previous sweep created it and crashed
        // before binding. Proceed to the bind; do not create a second graph.
        Err(StoreError::Conflict(message)) if message.starts_with("task graph already exists") => {}
        Err(error) => return Err(error),
    }

    // The graph is deliberately left for `advance_active_graphs` on the same
    // tick to dispatch, which is the one place graph advance lives.
    matrix_bridge_repo::bind_command_run(
        pool,
        &command.command_id,
        &graph_id,
        command.record_version,
    )
    .await?;
    Ok(true)
}

async fn settle_without_run(
    pool: &SqlitePool,
    command: &MatrixCommandRecord,
) -> Result<(), StoreError> {
    let updated = sqlx::query(
        "UPDATE matrix_commands \
         SET status = 'settled', record_version = record_version + 1, updated_at = ? \
         WHERE command_id = ? AND record_version = ? AND status = 'accepted'",
    )
    .bind(crate::util::now_unix())
    .bind(&command.command_id)
    .bind(command.record_version)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "matrix command '{}' record version mismatch",
            command.command_id
        )));
    }
    Ok(())
}

/// Settle every running command whose run has reached a terminal state.
///
/// This is a separate sweep rather than a hook inside `settle_node_executions`
/// on purpose. Two reasons: that function lives in the generic task-graph
/// repo, which must not learn about `matrix_commands`; and a crash between its
/// node update and a command update would lose the settle with nothing left to
/// repair it. As an independent sweep over durable state, the next tick simply
/// finds the row again.
///
/// Returns how many commands were settled.
///
/// # Errors
/// [`StoreError`] only if the listing itself fails. One command's failure is
/// isolated and logged, the same as the other maintenance-tick sweeps.
pub async fn settle_running_commands(pool: &SqlitePool) -> Result<u64, StoreError> {
    // The LEFT JOIN is what closes the last leak: a command whose graph row is
    // gone will never show a terminal status, so without the `g.id IS NULL`
    // arm it would hold its room's open-dedup slot forever.
    let command_ids: Vec<String> = sqlx::query_scalar(
        "SELECT c.command_id \
         FROM matrix_commands c \
         LEFT JOIN agent_chat_task_graphs g ON g.id = c.run_id \
         WHERE c.status = 'running' \
           AND (g.id IS NULL OR g.status IN ('complete', 'failed', 'cancelled')) \
         ORDER BY c.created_at ASC, c.command_id ASC",
    )
    .fetch_all(pool)
    .await?;

    let mut settled = 0_u64;
    for command_id in command_ids {
        match settle_one(pool, &command_id).await {
            Ok(true) => settled += 1,
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(
                    command_id = command_id.as_str(),
                    %error,
                    "settling a finished Matrix command failed this tick"
                );
            }
        }
    }
    Ok(settled)
}

/// Move one command from `running` to `settled`.
///
/// The predicate deliberately guards on `status` alone and not on the version
/// read by the listing: a concurrent bump is an ordinary race here, not a
/// caller error, and folding it into the predicate would log a conflict every
/// tick. Zero rows is therefore a benign skip — a replayed settle is a no-op —
/// and keeping `status = 'running'` in the `WHERE` is what makes that no-op
/// safe while still preserving the optimistic-locking invariant for later
/// writers. No compare-and-set against a concurrent re-dispatch is needed:
/// `list_accepted_commands` only ever returns `accepted` rows with no
/// `run_id`, so a `running` row is unreachable from the dispatch path.
async fn settle_one(pool: &SqlitePool, command_id: &str) -> Result<bool, StoreError> {
    let updated = sqlx::query(
        "UPDATE matrix_commands \
         SET status = 'settled', record_version = record_version + 1, updated_at = ? \
         WHERE command_id = ? AND status = 'running'",
    )
    .bind(crate::util::now_unix())
    .bind(command_id)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() == 1)
}
