//! The transport-agnostic MCP tool dispatcher (design §4.12.1): registers the
//! five agentd tools and routes a `tools/call` to its handler. The rmcp stdio
//! transport that hosts this dispatcher is wired into the daemon in P0.9 — it
//! needs a real MCP client (an agent) to exercise, so binding it now would be
//! untestable; the dispatcher here is the full, tested agent-facing contract.

use serde_json::Value;

use crate::error::SurfaceError;
use crate::host::RunHost;
use crate::tools::{assign_task, check_inbox, query_run, submit_outcome, submit_review};

/// One registered MCP tool.
#[derive(Debug, Clone, Copy)]
pub struct ToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
}

/// The five agentd MCP tools (design §4.12.1).
#[must_use]
pub fn tool_descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "assign_task",
            description: "Claim the open task assigned to this agent.",
        },
        ToolDescriptor {
            name: "submit_outcome",
            description: "Submit a node outcome (append-once per run/node/attempt).",
        },
        ToolDescriptor {
            name: "submit_review",
            description: "Submit a reviewer verdict for a review run.",
        },
        ToolDescriptor {
            name: "check_inbox",
            description: "Pull cowork-bus messages for this agent.",
        },
        ToolDescriptor {
            name: "query_run",
            description: "Read a run's status, current node, completed nodes, and context.",
        },
    ]
}

// Owned by value because it is a `.map_err(bad_args)` callback target.
#[allow(clippy::needless_pass_by_value)]
fn bad_args(e: serde_json::Error) -> SurfaceError {
    SurfaceError::Internal(format!("bad args: {e}"))
}

fn encode<T: serde::Serialize>(value: T) -> Result<Value, SurfaceError> {
    serde_json::to_value(value).map_err(|e| SurfaceError::Internal(format!("encode: {e}")))
}

/// Route an MCP `tools/call` (`name` + JSON `args`) to its handler and return
/// the handler's JSON output.
///
/// # Errors
/// The tool's [`SurfaceError`]; or `Internal` on malformed args or an unknown
/// tool name.
pub async fn dispatch(host: &dyn RunHost, name: &str, args: Value) -> Result<Value, SurfaceError> {
    match name {
        "assign_task" => encode(
            assign_task::assign_task(host, serde_json::from_value(args).map_err(bad_args)?).await?,
        ),
        "submit_outcome" => encode(
            submit_outcome::submit_outcome(host, serde_json::from_value(args).map_err(bad_args)?)
                .await?,
        ),
        "submit_review" => encode(
            submit_review::submit_review(host, serde_json::from_value(args).map_err(bad_args)?)
                .await?,
        ),
        "check_inbox" => encode(
            check_inbox::check_inbox(host, serde_json::from_value(args).map_err(bad_args)?).await?,
        ),
        "query_run" => encode(
            query_run::query_run(host, serde_json::from_value(args).map_err(bad_args)?).await?,
        ),
        other => Err(SurfaceError::Internal(format!("unknown tool: {other}"))),
    }
}
