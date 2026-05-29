//! `agentctl flow` — workflow `.dot` operations. `validate` parses a file and
//! runs the agentd-core validation pass, exiting 0 on success or 2 on any
//! parse/validation failure (listing every violation on stderr).

use std::path::Path;
use std::process::ExitCode;

use agentd_core::dot::parser;
use agentd_core::graph::NodeGraph;

use crate::cli::FlowCmd;

/// Exit code for a validation/parse failure (distinct from clap's usage exit 2
/// — both are 2 by convention; a successful validate is 0).
const EXIT_INVALID: u8 = 2;

#[must_use]
pub fn run(cmd: &FlowCmd) -> ExitCode {
    match cmd {
        FlowCmd::Validate(args) => validate(&args.path),
    }
}

fn validate(path: &Path) -> ExitCode {
    let src = match std::fs::read_to_string(path) {
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
    match NodeGraph::from_ast(&ast) {
        Ok(graph) => {
            println!(
                "ok: {} validated ({} nodes, {} edges)",
                path.display(),
                graph.nodes.len(),
                graph.edges.len()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            // CoreError::GraphValidate carries every violation, joined by "; ".
            eprintln!("error: {} failed validation: {err}", path.display());
            ExitCode::from(EXIT_INVALID)
        }
    }
}
