//! Validation checks for [`super::NodeGraph`]. Each `check_*` appends to the
//! shared `violations` list so `from_ast` can report all problems at once.

use std::collections::{BTreeSet, VecDeque};

use crate::graph::node_graph::{HandlerKind, NodeDef, NodeGraph, NodeShape};

/// Closed set of tool names legal in `pre_tools` / `post_action` (design §4.12).
const KNOWN_TOOLS: &[&str] = &[
    // agentd-exposed MCP server tools (§4.12.1)
    "assign_task",
    "submit_review",
    "check_inbox",
    "submit_outcome",
    "query_run",
    // mempal MCP tools (§4.12.2)
    "mempal_search",
    "mempal_ingest",
    "mempal_kg",
    "mempal_fact_check",
    "mempal_peek_partner",
    "mempal_cowork_push",
    // post_action shorthands routed by the WorkflowExecutor directly
    "matrix.post",
    "github.status_push",
];

const KNOWN_VISIBILITIES: &[&str] = &["blind", "after_submit", "chain", "delphi"];
const BASE_AGGREGATORS: &[&str] = &[
    "any_fail",
    "majority_pass",
    "unanimous_pass",
    "first_blocker",
];

/// Run every validation check, accumulating violations.
pub fn run(g: &NodeGraph, violations: &mut Vec<String>) {
    check_node_ids(g, violations);
    check_edge_endpoints(g, violations);
    check_start_terminal(g, violations);
    check_tools(g, violations);
    check_aggregators(g, violations);
    check_delphi(g, violations);
    check_reachability(g, violations);
    check_goal_gate_paths(g, violations);
    check_fan_out_pairing(g, violations);
}

/// Reject duplicate node ids: a repeated declaration would silently corrupt
/// shape/handler classification (e.g. the same id as both start and terminal),
/// and `node()` would resolve only the first declaration-order match.
fn check_node_ids(g: &NodeGraph, violations: &mut Vec<String>) {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for n in &g.nodes {
        if !seen.insert(n.id.as_str()) {
            violations.push(format!("duplicate node id '{}'", n.id));
        }
    }
}

/// Reject an edge whose endpoint is not a declared node. The parser tolerates
/// such an id (DOT permits implicit endpoints), but a workflow node needs a
/// shape or handler, and the engine would otherwise step into a node missing
/// from the graph (a runtime invariant error on an already-"valid" graph).
fn check_edge_endpoints(g: &NodeGraph, violations: &mut Vec<String>) {
    let ids: BTreeSet<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
    let mut missing: BTreeSet<&str> = BTreeSet::new();
    for e in &g.edges {
        if !ids.contains(e.from.as_str()) {
            missing.insert(e.from.as_str());
        }
        if !ids.contains(e.to.as_str()) {
            missing.insert(e.to.as_str());
        }
    }
    for id in missing {
        violations.push(format!("edge endpoint '{id}' is not a declared node"));
    }
}

fn check_start_terminal(g: &NodeGraph, violations: &mut Vec<String>) {
    let starts = g.starts();
    if starts.is_empty() {
        violations.push("graph has no start node (shape=Mdiamond)".to_string());
    } else if starts.len() > 1 {
        // The engine drives a run from a single entry point (starts().next());
        // more than one start would silently execute only the first component.
        let ids: Vec<&str> = starts.iter().map(|n| n.id.as_str()).collect();
        violations.push(format!(
            "graph has {} start nodes (shape=Mdiamond), expected exactly one: {ids:?}",
            starts.len()
        ));
    }
    if g.terminals().is_empty() {
        violations.push("graph has no terminal node (shape=Msquare)".to_string());
    }
}

/// Validate `pre_tools` / `post_action` token lists against the known-tool set.
fn check_tools(g: &NodeGraph, violations: &mut Vec<String>) {
    for n in &g.nodes {
        for attr in ["pre_tools", "post_action"] {
            if let Some(list) = n.attr(attr) {
                for token in split_top_level_commas(list) {
                    let name = tool_name(&token);
                    if !name.is_empty() && !KNOWN_TOOLS.contains(&name.as_str()) {
                        violations.push(format!(
                            "node '{}': {attr} references unknown tool '{name}'",
                            n.id
                        ));
                    }
                }
            }
        }
    }
}

fn check_aggregators(g: &NodeGraph, violations: &mut Vec<String>) {
    for n in &g.nodes {
        if n.handler != Some(HandlerKind::ParallelFanIn) {
            continue;
        }
        let Some(agg) = n.attr("aggregator") else {
            continue;
        };
        if let Some(fallback) = agg.strip_prefix("converge_or_") {
            if !BASE_AGGREGATORS.contains(&fallback) {
                violations.push(format!(
                    "node '{}': aggregator '{agg}' uses unknown converge fallback '{fallback}'",
                    n.id
                ));
                continue;
            }
            if !has_upstream_delphi_fan_out(g, n) {
                violations.push(format!(
                    "node '{}': aggregator '{agg}' requires a paired fan_out with visibility=delphi",
                    n.id
                ));
            }
        } else if !BASE_AGGREGATORS.contains(&agg) {
            violations.push(format!("node '{}': unknown aggregator '{agg}'", n.id));
        }
    }
}

fn check_delphi(g: &NodeGraph, violations: &mut Vec<String>) {
    for n in &g.nodes {
        if n.handler != Some(HandlerKind::ParallelFanOut) {
            continue;
        }
        let visibility = n.attr("visibility").unwrap_or("blind");
        if !KNOWN_VISIBILITIES.contains(&visibility) {
            violations.push(format!(
                "node '{}': unknown visibility '{visibility}'",
                n.id
            ));
            continue;
        }

        let max_rounds = parse_max_rounds(n, violations);
        if visibility == "delphi" {
            let convergence = n.attr("convergence").unwrap_or("verdict_stable");
            if !is_supported_delphi_convergence(convergence) {
                violations.push(format!(
                    "node '{}': convergence='{convergence}' is not supported for Delphi (expected verdict_stable or findings_diff<N> with 0.0 <= N <= 1.0)",
                    n.id
                ));
            }
            if !matches!(max_rounds, Some(rounds) if rounds >= 2) {
                violations.push(format!(
                    "node '{}': visibility=delphi requires max_rounds >= 2",
                    n.id
                ));
            }
            let fan_ins = downstream_fan_ins(g, n);
            match fan_ins.as_slice() {
                [] => violations.push(format!(
                    "node '{}': visibility=delphi requires a reachable parallel.fan_in partner",
                    n.id
                )),
                [fan_in] => {
                    let agg = fan_in.attr("aggregator").unwrap_or("any_fail");
                    if !is_supported_converge_aggregator(agg) {
                        violations.push(format!(
                            "node '{}': visibility=delphi requires paired fan_in '{}' to use aggregator=converge_or_<fallback> (got '{agg}')",
                            n.id, fan_in.id
                        ));
                    }
                }
                _ => violations.push(format!(
                    "node '{}': visibility=delphi reaches {} fan_in nodes; this slice requires exactly one reachable parallel.fan_in partner",
                    n.id,
                    fan_ins.len()
                )),
            }
        } else if max_rounds.is_some_and(|rounds| rounds > 1) {
            violations.push(format!(
                "node '{}': max_rounds > 1 requires visibility=delphi",
                n.id
            ));
        }
    }
}

fn is_supported_delphi_convergence(convergence: &str) -> bool {
    convergence == "verdict_stable" || parse_findings_diff_threshold(convergence).is_some()
}

fn parse_findings_diff_threshold(convergence: &str) -> Option<f64> {
    let inner = convergence
        .strip_prefix("findings_diff<")?
        .strip_suffix('>')?;
    let threshold = inner.parse::<f64>().ok()?;
    threshold
        .is_finite()
        .then_some(threshold)
        .filter(|n| (0.0..=1.0).contains(n))
}

fn parse_max_rounds(n: &NodeDef, violations: &mut Vec<String>) -> Option<u32> {
    let raw = n.attr("max_rounds")?;
    match raw.parse::<u32>() {
        Ok(0) => {
            violations.push(format!("node '{}': max_rounds must be >= 1", n.id));
            None
        }
        Ok(rounds) => Some(rounds),
        Err(_) => {
            violations.push(format!(
                "node '{}': max_rounds must be an integer >= 1 (got '{raw}')",
                n.id
            ));
            None
        }
    }
}

fn downstream_fan_ins<'g>(g: &'g NodeGraph, n: &NodeDef) -> Vec<&'g NodeDef> {
    let reachable = forward_reachable(g, std::slice::from_ref(&n.id));
    g.nodes
        .iter()
        .filter(|candidate| {
            candidate.handler == Some(HandlerKind::ParallelFanIn)
                && reachable.contains(candidate.id.as_str())
        })
        .collect()
}

fn has_upstream_delphi_fan_out(g: &NodeGraph, n: &NodeDef) -> bool {
    let upstream = backward_reachable(g, &n.id);
    g.nodes.iter().any(|candidate| {
        candidate.handler == Some(HandlerKind::ParallelFanOut)
            && candidate.attr("visibility") == Some("delphi")
            && upstream.contains(candidate.id.as_str())
    })
}

fn is_supported_converge_aggregator(agg: &str) -> bool {
    agg.strip_prefix("converge_or_")
        .is_some_and(|fallback| BASE_AGGREGATORS.contains(&fallback))
}

fn check_reachability(g: &NodeGraph, violations: &mut Vec<String>) {
    let reachable = forward_reachable(
        g,
        &g.starts().iter().map(|n| n.id.clone()).collect::<Vec<_>>(),
    );
    for n in &g.nodes {
        if n.shape != NodeShape::Start && !reachable.contains(&n.id) {
            violations.push(format!("node '{}' is unreachable from any start", n.id));
        }
    }
}

fn check_goal_gate_paths(g: &NodeGraph, violations: &mut Vec<String>) {
    let terminals: BTreeSet<String> = g.terminals().iter().map(|n| n.id.clone()).collect();
    for n in &g.nodes {
        if n.goal_gate {
            let from_gate = forward_reachable(g, std::slice::from_ref(&n.id));
            let reaches_terminal = from_gate.iter().any(|id| terminals.contains(id));
            if !reaches_terminal {
                violations.push(format!(
                    "goal_gate node '{}' has no path to any terminal",
                    n.id
                ));
            }
        }
    }
}

/// A `parallel.fan_in` reachable from ≥2 `parallel.fan_out` nodes (none carrying
/// `pair_with`) is rejected in P0.1 (boundary D8d; pairing is P1).
fn check_fan_out_pairing(g: &NodeGraph, violations: &mut Vec<String>) {
    let fan_outs: BTreeSet<String> = g
        .nodes
        .iter()
        .filter(|n| n.handler == Some(HandlerKind::ParallelFanOut))
        .map(|n| n.id.clone())
        .collect();
    for n in &g.nodes {
        if n.handler == Some(HandlerKind::ParallelFanIn) && n.attr("pair_with").is_none() {
            let preds = backward_reachable(g, &n.id);
            let upstream = preds.intersection(&fan_outs).count();
            if upstream >= 2 {
                violations.push(format!(
                    "fan_in node '{}' is reached by {upstream} fan_out nodes; P0.1 supports only one fan_out per fan_in (set pair_with in a later phase)",
                    n.id
                ));
            }
        }
    }
}

// ─── graph traversal helpers ────────────────────────────────────────

fn forward_reachable(g: &NodeGraph, seeds: &[String]) -> BTreeSet<String> {
    let mut seen: BTreeSet<String> = seeds.iter().cloned().collect();
    let mut queue: VecDeque<String> = seeds.iter().cloned().collect();
    while let Some(cur) = queue.pop_front() {
        for e in &g.edges {
            if e.from == cur && seen.insert(e.to.clone()) {
                queue.push_back(e.to.clone());
            }
        }
    }
    seen
}

fn backward_reachable(g: &NodeGraph, target: &str) -> BTreeSet<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(target.to_string());
    while let Some(cur) = queue.pop_front() {
        for e in &g.edges {
            if e.to == cur && seen.insert(e.from.clone()) {
                queue.push_back(e.from.clone());
            }
        }
    }
    seen
}

// ─── pre_tools / post_action token parsing ──────────────────────────

/// Split a comma-separated list, treating commas inside `(...)` as non-separators.
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0_i32;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                let t = cur.trim().to_string();
                if !t.is_empty() {
                    out.push(t);
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let t = cur.trim().to_string();
    if !t.is_empty() {
        out.push(t);
    }
    out
}

/// The leading tool name of a token: chars up to the first `(` or whitespace.
/// e.g. `mempal_search(wing=$x)` → `mempal_search`; `mempal_kg query(...)` → `mempal_kg`.
fn tool_name(token: &str) -> String {
    token
        .chars()
        .take_while(|c| *c != '(' && !c.is_whitespace())
        .collect()
}
