//! The `wait.human` handler (D1 park). `run` opens a human-wait row and parks;
//! `resume` closes the wait (so a replayed event is a no-op), stages the answer
//! and feedback into the context, and returns `Done` with `preferred_label`
//! set to the answer so an edge `condition="answer=approve"` routes.

use crate::CoreError;
use crate::engine::{EngineEvent, HandlerStep, ParkReason};
use crate::handler::{Handler, HandlerCtx};
use crate::types::{NodeId, Outcome};

#[derive(Debug)]
pub struct WaitHumanHandler;

#[async_trait::async_trait]
impl Handler for WaitHumanHandler {
    async fn run(&self, ctx: &mut HandlerCtx<'_>) -> Result<HandlerStep, CoreError> {
        let prompt = ctx
            .node_attr("prompt")
            .unwrap_or("awaiting human decision")
            .to_string();
        let run_id = ctx.run_id.clone();
        let node_id = NodeId::parsed(&ctx.node.id);
        let wait_id = ctx
            .ports
            .store
            .open_human_wait(&run_id, &node_id, &prompt)
            .await?;
        Ok(HandlerStep::Park(ParkReason::HumanAnswer { wait_id }))
    }

    async fn resume(
        &self,
        ctx: &mut HandlerCtx<'_>,
        event: EngineEvent,
    ) -> Result<HandlerStep, CoreError> {
        let EngineEvent::HumanAnswered {
            wait_id,
            answer,
            feedback,
        } = event
        else {
            return Err(CoreError::Invariant(
                "wait.human resumed with a non-HumanAnswered event".to_string(),
            ));
        };
        // Close the wait first so a replayed event resolves to None downstream.
        ctx.ports
            .store
            .answer_human_wait(&wait_id, &answer, feedback.as_deref())
            .await?;
        ctx.stage("answer", serde_json::Value::String(answer.clone()));
        if let Some(fb) = feedback {
            ctx.stage("human_feedback", serde_json::Value::String(fb));
        }
        Ok(HandlerStep::Done(Outcome {
            preferred_label: Some(answer),
            ..Outcome::success()
        }))
    }
}
