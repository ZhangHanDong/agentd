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

/// Upper bound on the daemon-response bytes `post_run` will buffer (P1). The real
/// reply is a tiny JSON (`{run_id, status}`); this is orders of magnitude above
/// it, small enough to bound memory against a hostile/buggy peer. Bounds MEMORY,
/// not time — there is no read deadline (see p83 Out of Scope).
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

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
    use std::io::Write;
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
    read_response(stream)
}

/// Read + parse the daemon's HTTP response from `stream`, bounding the buffered
/// bytes at [`MAX_RESPONSE_BYTES`] so a peer that streams forever can't OOM the
/// client (P1, herdr borrow). Reads at most `MAX + 1` bytes via [`Read::take`];
/// if the buffer exceeds the cap the response is REJECTED (parsing a truncated
/// body could accept half a JSON object as complete). Returns `(status_code,
/// body)`, or a human-readable error. A seam: takes `impl Read` so tests drive
/// it with an in-memory `Cursor`, no socket.
fn read_response(stream: impl std::io::Read) -> Result<(u16, String), String> {
    use std::io::Read;
    let mut buf = Vec::new();
    stream
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut buf)
        .map_err(|e| format!("reading from daemon failed: {e}"))?;
    if buf.len() > MAX_RESPONSE_BYTES {
        return Err(format!(
            "daemon response exceeds {MAX_RESPONSE_BYTES} bytes"
        ));
    }
    let response = String::from_utf8_lossy(&buf);
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

#[cfg(test)]
mod tests {
    //! Unit tests for the bounded-read seam (P1, `p83-post-run-bounded-read`).
    //! `read_response` takes an `impl Read`, so a `Cursor` drives it with no
    //! socket — the project's seam + fake model, here over a crate-internal fn.
    use super::{MAX_RESPONSE_BYTES, read_response};
    use std::io::Cursor;

    #[test]
    fn read_response_parses_status_and_body() {
        let raw = "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\n\
                   Connection: close\r\n\r\n{\"run_id\":\"r1\",\"status\":\"parked\"}";
        let (code, body) = read_response(Cursor::new(raw.as_bytes())).expect("parse");
        assert_eq!(code, 201);
        assert_eq!(body, "{\"run_id\":\"r1\",\"status\":\"parked\"}");
    }

    #[test]
    fn read_response_rejects_oversized() {
        // A VALID status line + a body larger than the cap: the only thing that
        // can fail is the memory bound, so a green test proves the bound fired
        // (not the malformed-status path).
        let mut raw = String::from("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
        raw.push_str(&"x".repeat(MAX_RESPONSE_BYTES + 1));
        let err = read_response(Cursor::new(raw.into_bytes())).expect_err("over cap");
        assert!(err.contains("exceeds"), "bound fired, not parser: {err}");
    }

    #[test]
    fn read_response_malformed_status_is_error() {
        let raw = "garbage with no http status line\r\n\r\nbody";
        let err = read_response(Cursor::new(raw.as_bytes())).expect_err("malformed");
        assert!(err.contains("malformed"), "{err}");
    }
}
