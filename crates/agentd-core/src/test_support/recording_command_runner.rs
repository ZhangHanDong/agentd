//! A [`CommandRunner`] that records argv and replays scripted outputs. P0.3's
//! real tmux backend is tested against this exact shape.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::ports::{CommandError, CommandOutput, CommandRunner, RunOpts};

/// One recorded invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedCall {
    pub program: String,
    pub args: Vec<String>,
    /// The working directory the call requested (`RunOpts.cwd`).
    pub cwd: Option<PathBuf>,
}

/// Records every `run` call and returns scripted results in FIFO order. When
/// the script is exhausted it returns an empty, status-0 [`CommandOutput`].
#[derive(Debug, Default)]
pub struct RecordingCommandRunner {
    scripted: Mutex<VecDeque<Result<CommandOutput, CommandError>>>,
    calls: Mutex<Vec<RecordedCall>>,
}

impl RecordingCommandRunner {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue one scripted result (returned by a future `run` call, FIFO).
    pub fn push_output(&self, out: Result<CommandOutput, CommandError>) {
        self.scripted.lock().expect("scripted lock").push_back(out);
    }

    /// The argv of every `run` call so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().expect("calls lock").clone()
    }
}

#[async_trait::async_trait]
impl CommandRunner for RecordingCommandRunner {
    async fn run(
        &self,
        program: &str,
        args: &[String],
        opts: RunOpts,
    ) -> Result<CommandOutput, CommandError> {
        self.calls.lock().expect("calls lock").push(RecordedCall {
            program: program.to_string(),
            args: args.to_vec(),
            cwd: opts.cwd,
        });
        self.scripted
            .lock()
            .expect("scripted lock")
            .pop_front()
            .unwrap_or_else(|| {
                Ok(CommandOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    status: 0,
                })
            })
    }
}
