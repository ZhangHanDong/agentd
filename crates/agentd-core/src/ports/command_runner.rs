//! The command-execution seam (design §4, D6). Runtime adapters and the
//! `tool` handler (Task 7) both shell out through this one trait, so the fake
//! (`RecordingCommandRunner`) is the exact shape the real runner is tested against.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Options for a single command invocation.
#[derive(Debug, Clone)]
pub struct RunOpts {
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Duration,
}

impl Default for RunOpts {
    fn default() -> Self {
        Self {
            cwd: None,
            env: HashMap::new(),
            stdin: None,
            timeout: Duration::from_secs(30),
        }
    }
}

/// The captured result of a finished command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub status: i32,
}

/// A command that failed to launch, timed out, or was killed (distinct from a
/// command that ran and returned a non-zero exit — that is a successful
/// `CommandOutput` with a non-zero `status`).
#[derive(Debug, Clone, thiserror::Error)]
#[error("command error: {message} (status {status:?})")]
pub struct CommandError {
    pub message: String,
    pub stderr: String,
    pub status: Option<i32>,
}

/// Run an external process and capture its output.
#[async_trait::async_trait]
pub trait CommandRunner: Send + Sync {
    /// Execute `program` with `args` under `opts`, returning captured output.
    ///
    /// # Errors
    /// Returns [`CommandError`] when the process cannot be launched, times out,
    /// or is killed. A process that runs to completion — even with a non-zero
    /// exit code — returns `Ok(CommandOutput)` with that code in `status`.
    async fn run(
        &self,
        program: &str,
        args: &[String],
        opts: RunOpts,
    ) -> Result<CommandOutput, CommandError>;
}
