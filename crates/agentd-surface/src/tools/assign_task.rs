//! `assign_task` (design §4.12.1): hand an agent the open task it was assigned
//! for `(run, node)`. A task that belongs to a different agent — or no task at
//! all — is `not_assigned`.

use agentd_core::types::{NodeId, RunId};
use serde::{Deserialize, Serialize};

use crate::error::SurfaceError;
use crate::host::RunHost;

#[derive(Debug, Clone, Deserialize)]
pub struct AssignTaskInput {
    pub run_id: String,
    pub node_id: String,
    pub agent_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssignTaskOutput {
    pub task_run_id: String,
    pub worktree: Option<String>,
    pub spec_path: Option<String>,
    pub plan_path: Option<String>,
    pub context_pack: Option<String>,
}

/// Resolve the open task for `(run, node)` and verify it belongs to the caller.
///
/// # Errors
/// [`SurfaceError::NotAssigned`] when there is no open task for the agent, or
/// the task belongs to a different agent.
pub async fn assign_task(
    host: &dyn RunHost,
    input: AssignTaskInput,
) -> Result<AssignTaskOutput, SurfaceError> {
    let run_id = RunId::from_string(input.run_id.as_str());
    let node_id = NodeId::parsed(input.node_id.as_str());

    let task = host
        .open_task(&run_id, &node_id)
        .await?
        .ok_or(SurfaceError::NotAssigned)?;
    if task.agent_id != input.agent_id {
        // Don't hand an agent a task that belongs to a different agent.
        return Err(SurfaceError::NotAssigned);
    }

    Ok(AssignTaskOutput {
        task_run_id: task.task_run_id.to_string(),
        worktree: task.worktree,
        spec_path: task.spec_path,
        plan_path: task.plan_path,
        context_pack: task.context_pack,
    })
}
