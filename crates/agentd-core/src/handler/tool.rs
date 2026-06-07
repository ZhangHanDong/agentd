//! The `tool` handler. Shells out via the `CommandRunner` port and maps the
//! result to an `Outcome`. Synchronous — never parks. agentd-core writes no
//! file here: when `artifact_path=` is set it records an `Artifact` *pointer*
//! (kind + path + sha256 + byte length of captured stdout) per design §3.1.

use std::time::Duration;

use crate::CoreError;
use crate::engine::HandlerStep;
use crate::handler::{Handler, HandlerCtx, sha256_hex};
use crate::ports::RunOpts;
use crate::types::{Artifact, ArtifactKind, Outcome, Status};

const DEFAULT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug)]
pub struct ToolHandler;

#[async_trait::async_trait]
impl Handler for ToolHandler {
    async fn run(&self, ctx: &mut HandlerCtx<'_>) -> Result<HandlerStep, CoreError> {
        // Parse `cmd` into program + args before any await so its borrow of
        // `ctx` ends before we touch `ctx.ports`.
        let (program, args) = {
            let cmd = ctx.node_attr("cmd").ok_or_else(|| {
                CoreError::Invariant(format!("tool node '{}' missing cmd attribute", ctx.node.id))
            })?;
            let mut parts = cmd.split_whitespace();
            let program = parts
                .next()
                .ok_or_else(|| {
                    CoreError::Invariant(format!("tool node '{}' has empty cmd", ctx.node.id))
                })?
                .to_string();
            let args: Vec<String> = parts.map(ToString::to_string).collect();
            (program, args)
        };
        let timeout = ctx
            .node_attr("timeout_secs")
            .and_then(|s| s.parse::<u64>().ok())
            .map_or_else(
                || Duration::from_secs(DEFAULT_TIMEOUT_SECS),
                Duration::from_secs,
            );
        let artifact_path = ctx.node_attr("artifact_path").map(ToString::to_string);

        let opts = RunOpts {
            timeout,
            // C1a: run the tool in the run's worktree if threaded; None → the
            // process cwd (today's behavior).
            cwd: ctx.worktree().map(std::path::Path::to_path_buf),
            ..RunOpts::default()
        };
        match ctx.ports.runner.run(&program, &args, opts).await {
            // The command ran. A clean exit is a deterministic outcome:
            // 0 → Success, anything else → Fail.
            Ok(output) if output.status == 0 => {
                let mut outcome = Outcome::success();
                if let Some(path) = artifact_path {
                    let bytes = output.stdout.as_bytes();
                    outcome.artifacts.push(Artifact {
                        kind: ArtifactKind::Transcript,
                        path: std::path::PathBuf::from(path),
                        sha256: sha256_hex(bytes),
                        bytes: bytes.len() as u64,
                    });
                }
                Ok(HandlerStep::Done(outcome))
            }
            Ok(_) => Ok(HandlerStep::Done(Outcome::fail())),
            // The command could not run / timed out / was killed — transient, so
            // Retry (the engine's D8c retry bound caps how often this re-runs).
            Err(_e) => Ok(HandlerStep::Done(Outcome {
                status: Status::Retry,
                ..Outcome::success()
            })),
        }
    }
}
