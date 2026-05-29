//! `TokioCommandRunner` — the production [`CommandRunner`] the daemon wires
//! (design D6). It spawns via `tokio::process::Command`, applies `RunOpts`
//! (cwd / env / stdin / timeout), and maps a launch failure or timeout to
//! [`CommandError`]. A process that runs to completion returns
//! [`CommandOutput`] with its exit code — even when that code is non-zero (a
//! non-zero `tmux has-session`, for instance, is a normal "no such session"
//! answer, not an error).

use std::process::Stdio;

use agentd_core::ports::{CommandError, CommandOutput, CommandRunner, RunOpts};
use tokio::io::AsyncWriteExt;

/// Production command runner backed by `tokio::process`.
#[derive(Debug, Default, Clone)]
pub struct TokioCommandRunner;

impl TokioCommandRunner {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl CommandRunner for TokioCommandRunner {
    async fn run(
        &self,
        program: &str,
        args: &[String],
        opts: RunOpts,
    ) -> Result<CommandOutput, CommandError> {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &opts.cwd {
            cmd.current_dir(cwd);
        }
        for (key, value) in &opts.env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|e| CommandError {
            message: format!("failed to launch `{program}`: {e}"),
            stderr: String::new(),
            status: None,
        })?;

        // Feed stdin (if any) and close the pipe so the child sees EOF. tmux
        // subcommands emit negligible stdout, so writing-then-waiting cannot
        // deadlock on a full stdout pipe here.
        match opts.stdin {
            Some(bytes) => {
                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(&bytes).await.map_err(|e| CommandError {
                        message: format!("failed writing stdin to `{program}`: {e}"),
                        stderr: String::new(),
                        status: None,
                    })?;
                    // `stdin` is dropped at the end of this block → pipe closes.
                }
            }
            None => {
                // Close the inherited stdin pipe so a child that reads stdin
                // does not block waiting for input we will never send.
                drop(child.stdin.take());
            }
        }

        let output = match tokio::time::timeout(opts.timeout, child.wait_with_output()).await {
            Ok(result) => result.map_err(|e| CommandError {
                message: format!("`{program}` failed while running: {e}"),
                stderr: String::new(),
                status: None,
            })?,
            Err(_elapsed) => {
                return Err(CommandError {
                    message: format!("`{program}` timed out after {:?}", opts.timeout),
                    stderr: String::new(),
                    status: None,
                });
            }
        };

        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status.code().unwrap_or(-1),
        })
    }
}
