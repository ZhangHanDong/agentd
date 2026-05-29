//! The `parallel.fan_in` handler. Synchronous (`fan_out` already gated on all N
//! verdicts, so they are present): it reads the paired review run's verdicts
//! from the context-staged `review_run_id`, applies the node's `aggregator`, and
//! returns `Done` with the mapped status. Delphi / `converge_or_*` modes are
//! P1+ (rejected by `flow validate` until then).

use crate::CoreError;
use crate::engine::HandlerStep;
use crate::handler::{Handler, HandlerCtx};
use crate::types::{Outcome, ReviewRunId, ReviewVerdict, Status, VerdictValue};

#[derive(Debug)]
pub struct FanInHandler;

#[async_trait::async_trait]
impl Handler for FanInHandler {
    async fn run(&self, ctx: &mut HandlerCtx<'_>) -> Result<HandlerStep, CoreError> {
        let review_run_id = {
            let raw = ctx
                .context
                .get("review_run_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    CoreError::Invariant(format!(
                        "fan_in node '{}' found no review_run_id in context",
                        ctx.node.id
                    ))
                })?;
            ReviewRunId::from_string(raw)
        };
        let aggregator = ctx
            .node_attr("aggregator")
            .unwrap_or("any_fail")
            .to_string();
        let verdicts = ctx.ports.store.list_verdicts(&review_run_id).await?;
        let status = aggregate(&aggregator, &verdicts);
        Ok(HandlerStep::Done(Outcome {
            status,
            ..Outcome::success()
        }))
    }
}

/// Map a set of verdicts to a pass/fail status per the chosen aggregator.
fn aggregate(aggregator: &str, verdicts: &[ReviewVerdict]) -> Status {
    let total = verdicts.len();
    let passes = verdicts
        .iter()
        .filter(|v| v.value == VerdictValue::Pass)
        .count();
    let any_block = verdicts.iter().any(|v| v.value == VerdictValue::Block);
    let any_fail_or_block = verdicts
        .iter()
        .any(|v| matches!(v.value, VerdictValue::Fail | VerdictValue::Block));
    let ok = match aggregator {
        // strict majority of Pass
        "majority_pass" => passes * 2 > total,
        // every reviewer must Pass
        "unanimous_pass" => total > 0 && passes == total,
        // only a hard Block sinks it; plain Fails are advisory
        "first_blocker" => !any_block,
        // "any_fail" (the default): any Fail or Block sinks it. Unknown
        // aggregators are rejected by `flow validate`, so this default is safe.
        _ => !any_fail_or_block,
    };
    if ok { Status::Success } else { Status::Fail }
}
