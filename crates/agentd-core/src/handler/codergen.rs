//! The `codergen` handler (D1 park). `run` assembles the initial prompt
//! (context vars named by `initial_prompt_includes` + best-effort `pre_tools`
//! mempal results), spawns the agent, records a task-run row, stages its id
//! (ctx-staged, so the pre-park checkpoint captures it), and parks. `resume`
//! returns the agent-reported outcome verbatim — there is no blocking wait.

use std::fmt::Write as _;
use std::path::Path;

use crate::CoreError;
use crate::engine::{EngineEvent, HandlerStep, ParkReason};
use crate::handler::{
    Handler, HandlerCtx, append_agent_allocation_prompt_context, current_node_allocation_agent_ids,
    spawn_request, stage_agent_allocation,
};
use crate::ports::{AgentAllocationRequest, AgentAllocationStatus};
use crate::types::{NodeId, RunId, TaskRunId};

#[derive(Debug)]
pub struct CodergenHandler;

#[async_trait::async_trait]
impl Handler for CodergenHandler {
    async fn run(&self, ctx: &mut HandlerCtx<'_>) -> Result<HandlerStep, CoreError> {
        let role = ctx.node_attr("role").unwrap_or("implementer").to_string();
        let includes: Vec<String> = ctx
            .node_attr("initial_prompt_includes")
            .map(|s| {
                s.split(',')
                    .map(|v| v.trim().trim_start_matches('$').to_string())
                    .filter(|v| !v.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let wants_mempal = ctx
            .node_attr("pre_tools")
            .is_some_and(|p| p.contains("mempal_search"));

        let mut prompt = initial_codergen_prompt(ctx, &role, &includes, wants_mempal).await;

        let run_id = ctx.run_id.clone();
        let node_id = NodeId::parsed(&ctx.node.id);
        let task_run_id = ctx.ports.store.insert_task_run(&run_id, &node_id).await?;
        let allocated_worktree = if let Some(allocator) = ctx.worktree_allocator() {
            Some(allocator.allocate(task_run_id.as_str()).await?)
        } else {
            None
        };
        if let Some(worktree) = allocated_worktree.as_deref() {
            ctx.ports
                .store
                .set_task_run_worktree(&task_run_id, worktree)
                .await?;
        }
        let worktree = allocated_worktree
            .as_deref()
            .unwrap_or_else(|| Path::new("."));
        let allocation = ctx
            .ports
            .agent_allocator
            .allocate(AgentAllocationRequest {
                run_id: run_id.clone(),
                node_id: node_id.clone(),
                role: role.clone(),
                capability: ctx.node_attr("capability").map(ToString::to_string),
                task: serde_json::json!({
                    "kind": "workflow_codergen",
                    "handler": "codergen",
                    "runId": run_id.as_str(),
                    "nodeId": node_id.as_str(),
                    "taskRunId": task_run_id.as_str(),
                    "requestedRole": role,
                    "worktree": worktree.to_string_lossy(),
                }),
            })
            .await?;
        if allocation.status == AgentAllocationStatus::Queued {
            stage_agent_allocation(ctx, &allocation);
            stage_queued_codergen_dispatch(ctx, &task_run_id, &prompt, worktree);
            if let Some(worktree) = allocated_worktree {
                ctx.stage(
                    "worktree",
                    serde_json::Value::String(worktree.to_string_lossy().into_owned()),
                );
            }
            ctx.stage(
                "task_run_id",
                serde_json::Value::String(task_run_id.as_str().to_string()),
            );
            return Ok(HandlerStep::Park(ParkReason::AgentOutcome { task_run_id }));
        }
        let agent_id = allocation.agent_id.clone();
        ctx.ports
            .store
            .set_task_run_agent(&task_run_id, &agent_id)
            .await?;
        append_agent_allocation_prompt_context(&mut prompt, &allocation);
        append_outcome_submission_context(
            &mut prompt,
            &run_id,
            &node_id,
            agent_id.as_str(),
            &task_run_id,
        );
        let request = spawn_request(
            agent_id.as_str(),
            Some(task_run_id.clone()),
            Some(prompt),
            worktree,
        );
        ctx.ports
            .backend
            .dispatch_allocated(request, &allocation)
            .await?;
        stage_agent_allocation(ctx, &allocation);
        if let Some(worktree) = allocated_worktree {
            ctx.stage(
                "worktree",
                serde_json::Value::String(worktree.to_string_lossy().into_owned()),
            );
        }
        ctx.stage(
            "task_run_id",
            serde_json::Value::String(task_run_id.as_str().to_string()),
        );
        Ok(HandlerStep::Park(ParkReason::AgentOutcome { task_run_id }))
    }

    async fn resume(
        &self,
        ctx: &mut HandlerCtx<'_>,
        event: EngineEvent,
    ) -> Result<HandlerStep, CoreError> {
        let EngineEvent::AgentOutcomeSubmitted {
            task_run_id,
            outcome,
        } = event
        else {
            return Err(CoreError::Invariant(
                "codergen resumed with a non-AgentOutcomeSubmitted event".to_string(),
            ));
        };
        // Close the task run so a replayed AgentOutcomeSubmitted resolves to None
        // downstream (no double-advance) — mirrors wait.human's close-on-answer.
        ctx.ports.store.complete_task_run(&task_run_id).await?;
        for agent_id in current_node_allocation_agent_ids(ctx) {
            if let Some(drained) = ctx.ports.agent_allocator.release(&agent_id).await? {
                stage_agent_allocation(ctx, &drained);
            }
        }
        Ok(HandlerStep::Done(outcome))
    }
}

async fn initial_codergen_prompt(
    ctx: &HandlerCtx<'_>,
    role: &str,
    includes: &[String],
    wants_mempal: bool,
) -> String {
    let mut prompt = String::new();
    for key in includes {
        if let Some(v) = ctx.context.get(key).and_then(serde_json::Value::as_str) {
            let _ = writeln!(prompt, "{key}: {v}");
        }
    }
    append_runtime_path_context(&mut prompt);
    let _ = writeln!(
        prompt,
        "agentd_role_task: read the listed inputs, complete this node's role in the current worktree, then submit outcome through agentd_submit_outcome."
    );
    if wants_mempal {
        // pre_tools are best-effort: a mempal failure must not abort the node.
        let hits = ctx
            .ports
            .mempal
            .search(role, "project", "")
            .await
            .unwrap_or_default();
        for hit in hits {
            let _ = writeln!(prompt, "memory: {}", hit.body);
        }
    }
    prompt
}

fn stage_queued_codergen_dispatch(
    ctx: &mut HandlerCtx<'_>,
    task_run_id: &TaskRunId,
    base_prompt: &str,
    worktree: &Path,
) {
    let mut root = ctx
        .staged_updates()
        .get("agentd_queued_workflow_dispatches")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    root.insert(
        ctx.node.id.clone(),
        serde_json::json!({
            "handler": "codergen",
            "taskRunId": task_run_id.as_str(),
            "basePrompt": base_prompt,
            "worktree": worktree.to_string_lossy(),
        }),
    );
    ctx.stage(
        "agentd_queued_workflow_dispatches",
        serde_json::Value::Object(root),
    );
}

fn append_runtime_path_context(prompt: &mut String) {
    if !prompt.is_empty() && !prompt.ends_with('\n') {
        prompt.push('\n');
    }
    let cwd = std::env::current_dir().map_or_else(
        |_| "<unknown>".to_string(),
        |path| path.display().to_string(),
    );
    let _ = writeln!(prompt, "agentd_daemon_cwd: {cwd}");
    let _ = writeln!(
        prompt,
        "agentd_runtime_path_rule: relative paths in this prompt resolve from agentd_daemon_cwd; make code changes in the current worktree."
    );
}

fn append_outcome_submission_context(
    prompt: &mut String,
    run_id: &RunId,
    node_id: &NodeId,
    agent_id: &str,
    task_run_id: &TaskRunId,
) {
    if !prompt.is_empty() && !prompt.ends_with('\n') {
        prompt.push('\n');
    }
    let _ = writeln!(prompt, "agentd_run_id: {}", run_id.as_str());
    let _ = writeln!(prompt, "agentd_node_id: {}", node_id.as_str());
    let _ = writeln!(prompt, "agentd_agent_id: {agent_id}");
    let _ = writeln!(prompt, "agentd_task_run_id: {}", task_run_id.as_str());
    let _ = writeln!(
        prompt,
        "agentd_submit_outcome: use JSON-RPC tools/call name=submit_outcome arguments={{run_id:\"{}\",node_id:\"{}\",attempt:1,status:\"success|fail|retry|partial_success\",context_updates:{{}}}}",
        run_id.as_str(),
        node_id.as_str()
    );
}
