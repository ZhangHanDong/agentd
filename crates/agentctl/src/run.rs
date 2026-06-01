//! `agentctl run start` — the standalone Path-B run trigger. It resolves the
//! `--flow` workflow under `--workflows-dir`, parses + validates it (the same
//! path as `flow validate`), and with `--dry-run` prints the resolved plan.
//!
//! Live execution (driving the run to completion) needs the production `RunHost` +
//! daemon that reconstruct the engine from checkpoint to service cross-process
//! agent events — deferred to P0.9. Without `--dry-run` this returns a clear
//! deferred error rather than hanging.

use std::process::ExitCode;

use agentd_core::dot::parser;
use agentd_core::graph::{NodeDef, NodeGraph, NodeShape};

use crate::cli::{RunCmd, RunStartArgs};

/// A parse/validation/IO failure (mirrors `flow`'s convention).
const EXIT_INVALID: u8 = 2;
/// The live run path is not wired in P0.8 (deferred to P0.9).
const EXIT_DEFERRED: u8 = 3;

#[must_use]
pub fn run(cmd: &RunCmd) -> ExitCode {
    match cmd {
        RunCmd::Start(args) => start(args),
    }
}

fn start(args: &RunStartArgs) -> ExitCode {
    let path = args.workflows_dir.join(args.flow.file_name());

    let src = match std::fs::read_to_string(&path) {
        Ok(src) => src,
        Err(err) => {
            eprintln!("error: cannot read {}: {err}", path.display());
            return ExitCode::from(EXIT_INVALID);
        }
    };
    let ast = match parser::parse(&src) {
        Ok(ast) => ast,
        Err(err) => {
            eprintln!("error: {} failed to parse: {err}", path.display());
            return ExitCode::from(EXIT_INVALID);
        }
    };
    let graph = match NodeGraph::from_ast(&ast) {
        Ok(graph) => graph,
        Err(err) => {
            eprintln!("error: {} failed validation: {err}", path.display());
            return ExitCode::from(EXIT_INVALID);
        }
    };

    if args.dry_run {
        print_plan(args, &path.display().to_string(), &graph);
        return ExitCode::SUCCESS;
    }

    // Live execution drives the run to completion; that requires the production
    // RunHost + daemon (cross-process agent unpark from checkpoint), deferred to
    // P0.9. Fail clearly instead of hanging.
    eprintln!(
        "error: live execution of run '{}' is deferred to P0.9 (no daemon / production RunHost wired). \
         Re-run with --dry-run to validate the flow and print the plan.",
        args.id
    );
    ExitCode::from(EXIT_DEFERRED)
}

fn print_plan(args: &RunStartArgs, path: &str, graph: &NodeGraph) {
    println!(
        "run start (dry-run): flow={:?} id={} workflow={path}",
        args.flow, args.id
    );
    println!(
        "plan: {} — {} nodes, {} edges",
        graph.name,
        graph.nodes.len(),
        graph.edges.len()
    );
    for node in &graph.nodes {
        let kind = node_kind(node);
        let gate = if node.goal_gate { " goal_gate" } else { "" };
        println!("  {} [{kind}]{gate}", node.id);
    }
}

/// A short label for a node in the printed plan: its shape for start/terminal,
/// else its handler kind.
fn node_kind(node: &NodeDef) -> String {
    match node.shape {
        NodeShape::Start => "start".to_string(),
        NodeShape::Terminal => "terminal".to_string(),
        NodeShape::Regular => node
            .handler
            .map_or_else(|| "?".to_string(), |h| format!("{h:?}")),
    }
}
