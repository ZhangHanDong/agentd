//! The `conditional` branch handler. Thin by design (§2.4): edge selection
//! already routes by `condition=` (tier 1) and the highest-weight unconditional
//! edge (tier 4), so this handler's only non-redundant job is to emit `Fail`
//! when no branch matches and there is no default. It evaluates each outgoing
//! edge's `condition` against the run context (Task 4 `eval_condition`, with a
//! synthetic `Outcome::success()` — so conditions here use `kv("k")=="v"`, never
//! `outcome=`/`answer=`, which are meaningless on a branch node).

use crate::CoreError;
use crate::engine::HandlerStep;
use crate::graph::EdgeDef;
use crate::graph::edge_select::eval_condition;
use crate::handler::{Handler, HandlerCtx};
use crate::types::Outcome;

#[derive(Debug)]
pub struct ConditionalHandler;

#[async_trait::async_trait]
impl Handler for ConditionalHandler {
    async fn run(&self, ctx: &mut HandlerCtx<'_>) -> Result<HandlerStep, CoreError> {
        let probe = Outcome::success();
        let mut default_label: Option<String> = None;
        for edge in ctx.outgoing_edges() {
            if let Some(cond) = edge.attr("condition") {
                if eval_condition(cond, &probe, ctx.context) {
                    return Ok(HandlerStep::Done(success_with_label(edge_label(edge))));
                }
            } else if default_label.is_none() {
                default_label = Some(edge_label(edge));
            }
        }
        if let Some(label) = default_label {
            return Ok(HandlerStep::Done(success_with_label(label)));
        }
        Ok(HandlerStep::Done(Outcome::fail()))
    }
}

/// The branch's routing label: its `label=` attribute, falling back to the
/// target node id (which edge selection can also match on).
fn edge_label(edge: &EdgeDef) -> String {
    edge.attr("label")
        .map_or_else(|| edge.to.clone(), ToString::to_string)
}

fn success_with_label(label: String) -> Outcome {
    Outcome {
        preferred_label: Some(label),
        ..Outcome::success()
    }
}
