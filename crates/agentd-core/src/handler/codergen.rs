//! The `codergen` handler (D1 park). `run` assembles the initial prompt
//! (context vars named by `initial_prompt_includes` + best-effort `pre_tools`
//! mempal results), spawns the agent, records a task-run row, stages its id
//! (ctx-staged, so the pre-park checkpoint captures it), and parks. `resume`
//! returns the agent-reported outcome verbatim — there is no blocking wait.

use std::fmt::Write as _;

use crate::CoreError;
use crate::engine::{EngineEvent, HandlerStep, ParkReason};
use crate::handler::{Handler, HandlerCtx, spawn_request};
use crate::types::NodeId;

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

        let mut prompt = String::new();
        for key in &includes {
            if let Some(v) = ctx.context.get(key).and_then(serde_json::Value::as_str) {
                let _ = writeln!(prompt, "{key}: {v}");
            }
        }
        if wants_mempal {
            // pre_tools are best-effort: a mempal failure must not abort the node.
            let hits = ctx
                .ports
                .mempal
                .search(&role, "project", "")
                .await
                .unwrap_or_default();
            for hit in hits {
                let _ = writeln!(prompt, "memory: {}", hit.body);
            }
        }

        let run_id = ctx.run_id.clone();
        let node_id = NodeId::parsed(&ctx.node.id);
        ctx.ports
            .backend
            .spawn(spawn_request(&role, Some(prompt)))
            .await?;
        let task_run_id = ctx.ports.store.insert_task_run(&run_id, &node_id).await?;
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
        Ok(HandlerStep::Done(outcome))
    }
}
