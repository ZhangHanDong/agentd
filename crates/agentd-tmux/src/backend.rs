//! `TmuxBackend` (design §4.1–§4.5): the real agent-spawning backend. It holds
//! an injected `Arc<dyn CommandRunner>` (P0.3 D3) so every flow is testable
//! against a `FakeRunner`. The agentd-core `AgentBackend` trait stays spawn-only
//! (D1); the other capabilities (§4.6–§4.10) are inherent methods landed across
//! P0.3 Tasks 2–5.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use agentd_core::ports::{CommandOutput, CommandRunner, RunOpts};

use crate::config::Config;
use crate::error::BackendError;

/// Launches and addresses agents inside tmux panes, all via the injected runner.
pub struct TmuxBackend {
    runner: Arc<dyn CommandRunner>,
    tmux_bin: PathBuf,
    cfg: Config,
}

impl fmt::Debug for TmuxBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `dyn CommandRunner` is not `Debug` and we cannot add a `Debug`
        // supertrait to the tagged core trait (D1), so the runner is elided.
        f.debug_struct("TmuxBackend")
            .field("tmux_bin", &self.tmux_bin)
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

impl TmuxBackend {
    /// Build a backend over `runner`, the resolved `tmux_bin`, and `cfg`.
    #[must_use]
    pub fn new(runner: Arc<dyn CommandRunner>, tmux_bin: PathBuf, cfg: Config) -> Self {
        Self {
            runner,
            tmux_bin,
            cfg,
        }
    }

    /// The backend configuration (timing + ready patterns).
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Run a tmux subcommand through the injected runner, prefixing the resolved
    /// binary. A process that runs to completion — even with a non-zero exit —
    /// returns `Ok(CommandOutput)`; only a launch/timeout failure becomes a
    /// [`BackendError`]. Callers (e.g. the `has-session` probe) interpret the
    /// exit code themselves.
    ///
    /// # Errors
    /// [`BackendError::Recoverable`] when the tmux process cannot be launched or
    /// times out.
    pub async fn tmux(&self, args: &[&str], opts: RunOpts) -> Result<CommandOutput, BackendError> {
        let owned_args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
        let program = self.tmux_bin.to_string_lossy();
        self.runner
            .run(program.as_ref(), &owned_args, opts)
            .await
            .map_err(|e| {
                let sub = args.first().copied().unwrap_or("tmux");
                BackendError::Recoverable(format!("tmux {sub} failed to run: {e}"))
            })
    }
}
