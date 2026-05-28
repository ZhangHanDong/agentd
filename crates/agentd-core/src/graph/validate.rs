//! Validation checks for [`super::NodeGraph`]. Each `check_*` appends to the
//! shared `violations` list so `from_ast` can report all problems at once.

use std::collections::{BTreeSet, VecDeque};

use crate::graph::node_graph::{HandlerKind, NodeGraph, NodeShape};

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

/// Run every validation check, accumulating violations.
pub fn run(g: &NodeGraph, violations: &mut Vec<String>) {
    check_start_terminal(g, violations);
    check_tools(g, violations);
    check_delphi(g, violations);
    check_reachability(g, violations);
    check_goal_gate_paths(g, violations);
    check_fan_out_pairing(g, violations);
}

fn check_start_terminal(g: &NodeGraph, violations: &mut Vec<String>) {
    if g.starts().is_empty() {
        violations.push("graph has no start node (shape=Mdiamond)".to_string());
    }
    if g.terminals().is_empty() {
        violations.push("graph has no terminal node (shape=Msquare)".to_string());
    }
}

/// Validate `pre_tools` / `post_action` token lists against the known-tool set,
/// and reject P1-only `converge_or_*` aggregators.
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
        if let Some(agg) = n.attr("aggregator") {
            if agg.starts_with("converge_or_") {
                violations.push(format!(
                    "node '{}': aggregator '{agg}' (converge_or_*) is reserved for P1 (design §2.5.1)",
                    n.id
                ));
            }
        }
    }
}

fn check_delphi(g: &NodeGraph, violations: &mut Vec<String>) {
    for n in &g.nodes {
        if n.attr("visibility") == Some("delphi") {
            violations.push(format!(
                "node '{}': visibility=delphi is reserved for P1 (design §2.5.1)",
                n.id
            ));
        }
    }
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
