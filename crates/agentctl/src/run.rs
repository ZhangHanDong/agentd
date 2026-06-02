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
/// The daemon could not be reached or rejected the live run (P0.9).
const EXIT_DAEMON: u8 = 3;

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

    // Live run: POST to the daemon, which owns the store + production RunHost.
    match post_run(&args.daemon_url, args.flow.name(), &args.id) {
        Ok((code, body)) if (200..300).contains(&code) => {
            println!("run started via {}: {body}", args.daemon_url);
            ExitCode::SUCCESS
        }
        Ok((code, body)) => {
            eprintln!("error: daemon rejected the run (HTTP {code}): {body}");
            ExitCode::from(EXIT_DAEMON)
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(EXIT_DAEMON)
        }
    }
}

/// Minimal dependency-free `POST /runs` to the daemon (a single request over a
/// blocking socket; the agent advances the run thereafter). Returns the HTTP
/// status code + response body, or a human-readable transport error.
fn post_run(daemon_url: &str, flow: &str, id: &str) -> Result<(u16, String), String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let addr = daemon_url
        .strip_prefix("http://")
        .unwrap_or(daemon_url)
        .trim_end_matches('/');
    let mut stream = TcpStream::connect(addr)
        .map_err(|e| format!("cannot reach daemon at {daemon_url}: {e}"))?;
    let body = format!(r#"{{"flow":"{flow}","run_id":"{id}","context":{{}}}}"#);
    let request = format!(
        "POST /runs HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("writing to daemon failed: {e}"))?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("reading from daemon failed: {e}"))?;
    let code = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or_else(|| "malformed daemon response".to_string())?;
    let body = response
        .split_once("\r\n\r\n")
        .map_or("", |(_, b)| b)
        .to_string();
    Ok((code, body))
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
