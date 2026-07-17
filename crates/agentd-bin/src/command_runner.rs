//! Production async command runner for bounded non-interactive tools.

use std::process::Stdio;

use agentd_core::ports::{CommandError, CommandOutput, CommandRunner, RunOpts};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Default, Clone, Copy)]
pub struct TokioCommandRunner;

impl TokioCommandRunner {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl CommandRunner for TokioCommandRunner {
    async fn run(
        &self,
        program: &str,
        args: &[String],
        options: RunOpts,
    ) -> Result<CommandOutput, CommandError> {
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &options.cwd {
            command.current_dir(cwd);
        }
        command.envs(&options.env);
        let mut child = command.spawn().map_err(|error| CommandError {
            message: format!("failed to launch `{program}`: {error}"),
            stderr: String::new(),
            status: None,
        })?;
        if let Some(bytes) = options.stdin {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(&bytes).await.map_err(|error| CommandError {
                    message: format!("failed writing stdin to `{program}`: {error}"),
                    stderr: String::new(),
                    status: None,
                })?;
            }
        } else {
            drop(child.stdin.take());
        }
        let output = tokio::time::timeout(options.timeout, child.wait_with_output())
            .await
            .map_err(|_| CommandError {
                message: format!("`{program}` timed out after {:?}", options.timeout),
                stderr: String::new(),
                status: None,
            })?
            .map_err(|error| CommandError {
                message: format!("`{program}` failed while running: {error}"),
                stderr: String::new(),
                status: None,
            })?;
        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status.code().unwrap_or(-1),
        })
    }
}
