//! The `parallel.fan_in` handler. Synchronous (`fan_out` already gated on all N
//! verdicts, so they are present): it reads the paired review run's verdicts
//! from the context-staged `review_run_id`, applies the node's `aggregator`, and
//! returns `Done` with the mapped status. Delphi round orchestration is P1.4+,
//! but `converge_or_*` fallback aggregators are executable once graph
//! validation accepts a well-formed Delphi pair.

use std::collections::{BTreeSet, VecDeque};

use crate::CoreError;
use crate::engine::HandlerStep;
use crate::graph::{HandlerKind, NodeDef};
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
        if aggregator.starts_with("converge_or_") {
            let round = ctx
                .ports
                .store
                .review_round(&review_run_id)
                .await?
                .ok_or_else(|| {
                    CoreError::Invariant(format!(
                        "fan_in node '{}' found no round for review_run_id {}",
                        ctx.node.id,
                        review_run_id.as_str()
                    ))
                })?;
            let fan_out = paired_delphi_fan_out(ctx)?;
            let max_rounds = parse_delphi_max_rounds(fan_out)?;
            let convergence = fan_out.attr("convergence").unwrap_or("verdict_stable");
            let signature = verdict_signature(&verdicts);
            let verdicts_stable = ctx
                .context
                .get("delphi_previous_verdicts")
                .and_then(serde_json::Value::as_str)
                == Some(signature.as_str());
            let findings_signature = findings_signature(&verdicts);
            let findings_stable = findings_diff_threshold(convergence).is_some_and(|threshold| {
                ctx.context
                    .get("delphi_previous_findings")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|previous| {
                        normalized_text_diff(previous, &findings_signature) <= threshold
                    })
            });
            if !(verdicts_stable || findings_stable) && round < max_rounds {
                let mut outcome = Outcome {
                    status: Status::PartialSuccess,
                    ..Outcome::success()
                };
                outcome.context_updates.insert(
                    "delphi_next_round".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(round + 1)),
                );
                outcome.context_updates.insert(
                    "delphi_previous_verdicts".to_string(),
                    serde_json::Value::String(signature),
                );
                outcome.context_updates.insert(
                    "delphi_previous_findings".to_string(),
                    serde_json::Value::String(findings_signature),
                );
                return Ok(HandlerStep::Done(outcome));
            }
        }
        let status = aggregate(&aggregator, &verdicts);
        Ok(HandlerStep::Done(Outcome {
            status,
            ..Outcome::success()
        }))
    }
}

/// Map a set of verdicts to a pass/fail status per the chosen aggregator.
fn aggregate(aggregator: &str, verdicts: &[ReviewVerdict]) -> Status {
    let aggregator = aggregator
        .strip_prefix("converge_or_")
        .unwrap_or(aggregator);
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

fn verdict_signature(verdicts: &[ReviewVerdict]) -> String {
    let mut parts: Vec<String> = verdicts
        .iter()
        .map(|verdict| {
            format!(
                "{}={}",
                verdict.reviewer_id.as_str(),
                verdict_value_name(verdict.value)
            )
        })
        .collect();
    parts.sort();
    parts.join(";")
}

fn findings_signature(verdicts: &[ReviewVerdict]) -> String {
    let mut parts: Vec<String> = verdicts
        .iter()
        .map(|verdict| {
            format!(
                "{}={}",
                verdict.reviewer_id.as_str(),
                normalize_findings(&verdict.findings)
            )
        })
        .collect();
    parts.sort();
    parts.join(";")
}

fn normalize_findings(findings: &str) -> String {
    findings.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalized_text_diff(previous: &str, current: &str) -> f64 {
    let previous = normalize_findings(previous);
    let current = normalize_findings(current);
    let max_len = previous.chars().count().max(current.chars().count());
    if max_len == 0 {
        return 0.0;
    }
    levenshtein_chars(&previous, &current) as f64 / max_len as f64
}

fn levenshtein_chars(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (cur[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn findings_diff_threshold(convergence: &str) -> Option<f64> {
    let inner = convergence
        .strip_prefix("findings_diff<")?
        .strip_suffix('>')?;
    let threshold = inner.parse::<f64>().ok()?;
    threshold
        .is_finite()
        .then_some(threshold)
        .filter(|n| (0.0..=1.0).contains(n))
}

fn verdict_value_name(value: VerdictValue) -> &'static str {
    match value {
        VerdictValue::Pass => "pass",
        VerdictValue::Fail => "fail",
        VerdictValue::Block => "block",
    }
}

fn paired_delphi_fan_out<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a NodeDef, CoreError> {
    let upstream = backward_reachable(ctx, &ctx.node.id);
    let fan_outs: Vec<_> = ctx
        .graph
        .nodes
        .iter()
        .filter(|node| {
            node.handler == Some(HandlerKind::ParallelFanOut)
                && node.attr("visibility") == Some("delphi")
                && upstream.contains(node.id.as_str())
        })
        .collect();
    let [fan_out] = fan_outs.as_slice() else {
        return Err(CoreError::Invariant(format!(
            "fan_in node '{}' expected exactly one upstream Delphi fan_out, found {}",
            ctx.node.id,
            fan_outs.len()
        )));
    };
    Ok(fan_out)
}

fn parse_delphi_max_rounds(fan_out: &NodeDef) -> Result<u32, CoreError> {
    fan_out
        .attr("max_rounds")
        .unwrap_or("1")
        .parse::<u32>()
        .map_err(|err| {
            CoreError::Invariant(format!(
                "fan_out node '{}' has invalid max_rounds: {err}",
                fan_out.id
            ))
        })
}

fn backward_reachable(ctx: &HandlerCtx<'_>, target: &str) -> BTreeSet<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(target.to_string());
    while let Some(cur) = queue.pop_front() {
        for edge in &ctx.graph.edges {
            if edge.to == cur && seen.insert(edge.from.clone()) {
                queue.push_back(edge.from.clone());
            }
        }
    }
    seen
}
