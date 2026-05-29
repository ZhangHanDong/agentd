//! The `parallel.fan_out` handler (D1 park, D7 no disk bundle). `run` computes
//! an in-memory `context_sha` over the serialized context + node id (so every
//! reviewer pins one snapshot), records a review-run row, spawns N reviewer
//! agents, stages the review-run id (for the paired `fan_in`), and parks.
//! `resume` records each verdict and re-parks until all N have arrived.

use crate::CoreError;
use crate::engine::{EngineEvent, HandlerStep, ParkReason};
use crate::handler::{Handler, HandlerCtx, sha256_hex, spawn_request};
use crate::types::{NodeId, Outcome, ReviewVerdict};

#[derive(Debug)]
pub struct FanOutHandler;

/// The reviewer roles declared by the node's `reviewers` comma-list.
fn reviewer_roles(ctx: &HandlerCtx<'_>) -> Vec<String> {
    ctx.node_attr("reviewers")
        .map(|s| {
            s.split(',')
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

#[async_trait::async_trait]
impl Handler for FanOutHandler {
    async fn run(&self, ctx: &mut HandlerCtx<'_>) -> Result<HandlerStep, CoreError> {
        let roles = reviewer_roles(ctx);
        if roles.is_empty() {
            return Err(CoreError::Invariant(format!(
                "fan_out node '{}' declares no reviewers",
                ctx.node.id
            )));
        }
        let expected = roles.len();
        let run_id = ctx.run_id.clone();
        let node_id = NodeId::parsed(&ctx.node.id);

        // Deterministic context_sha: serialized context bytes + the node id.
        let mut to_hash = serde_json::to_vec(ctx.context)?;
        to_hash.extend_from_slice(node_id.as_str().as_bytes());
        let context_sha = sha256_hex(&to_hash);

        let review_run_id = ctx
            .ports
            .store
            .insert_review_run(&run_id, &node_id, expected, &context_sha)
            .await?;

        for role in &roles {
            let prompt = format!("adversarial review (context_sha={context_sha})");
            ctx.ports
                .backend
                .spawn(spawn_request(role, Some(prompt)))
                .await?;
        }

        ctx.stage(
            "review_run_id",
            serde_json::Value::String(review_run_id.as_str().to_string()),
        );
        ctx.stage("context_sha", serde_json::Value::String(context_sha));
        Ok(HandlerStep::Park(ParkReason::ReviewVerdicts {
            review_run_id,
            expected,
        }))
    }

    async fn resume(
        &self,
        ctx: &mut HandlerCtx<'_>,
        event: EngineEvent,
    ) -> Result<HandlerStep, CoreError> {
        let EngineEvent::ReviewVerdictSubmitted {
            review_run_id,
            reviewer_id,
            verdict,
        } = event
        else {
            return Err(CoreError::Invariant(
                "fan_out resumed with a non-ReviewVerdictSubmitted event".to_string(),
            ));
        };
        ctx.ports
            .store
            .insert_review_verdict(
                &review_run_id,
                ReviewVerdict {
                    reviewer_id,
                    value: verdict,
                },
            )
            .await?;
        let collected = ctx.ports.store.count_verdicts(&review_run_id).await?;
        let expected = reviewer_roles(ctx).len();
        if collected >= expected {
            Ok(HandlerStep::Done(Outcome::success()))
        } else {
            Ok(HandlerStep::Park(ParkReason::ReviewVerdicts {
                review_run_id,
                expected,
            }))
        }
    }
}
