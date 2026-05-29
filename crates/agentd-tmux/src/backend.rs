//! `TmuxBackend` (design §4.1–§4.5): the real agent-spawning backend. It holds
//! an injected `Arc<dyn CommandRunner>` (P0.3 D3) so every flow is testable
//! against a `FakeRunner`. The agentd-core `AgentBackend` trait stays spawn-only
//! (D1); the other capabilities (§4.6–§4.10) are inherent methods landed across
//! P0.3 Tasks 2–5.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use agentd_core::CoreError;
use agentd_core::ports::{AgentBackend, CommandOutput, CommandRunner, RunOpts};
use agentd_core::types::{AgentHandle, BackendKind, CliKind, LaunchStrategy, SpawnRequest};

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

#[async_trait::async_trait]
impl AgentBackend for TmuxBackend {
    /// Spawn an agent into a fresh tmux session (design §4.5). Maps the internal
    /// [`BackendError`] to [`CoreError::Backend`] at the trait boundary (D2).
    async fn spawn(&self, req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        let session = format!("agentd-{}", req.agent_id.as_str());
        let address = format!("{session}:0.0");

        // §4.5 step 1: liveness probe FIRST. A zero exit means the session is
        // already live — the caller should rebind, not spawn a second one. We
        // return before writing any launcher or running new-session.
        let probe = self
            .tmux(&["has-session", "-t", session.as_str()], RunOpts::default())
            .await?;
        if probe.status == 0 {
            return Err(BackendError::Recoverable(format!(
                "session {session} already exists; rebind instead of spawning"
            ))
            .into());
        }

        // §4.5 step 2: write the launcher into the worktree, then exclude it.
        let launcher_path = req
            .worktree
            .join(format!(".agentd-launcher-{}.sh", req.agent_id.as_str()));
        write_file(&launcher_path, &build_launcher_script(&req)?)?;
        amend_gitignore(&req.worktree)?;

        // §4.5 step 3: launch — directly, or wrapped in a transient systemd scope.
        let worktree_str = req.worktree.to_string_lossy().into_owned();
        let launcher_str = launcher_path.to_string_lossy().into_owned();
        let launch_out = match &req.launch_strategy {
            LaunchStrategy::Direct => {
                self.tmux(
                    &[
                        "new-session",
                        "-d",
                        "-s",
                        session.as_str(),
                        "-c",
                        worktree_str.as_str(),
                        "bash",
                        launcher_str.as_str(),
                    ],
                    RunOpts::default(),
                )
                .await?
            }
            LaunchStrategy::Systemd { scope_name } => {
                let unit = format!("--unit={scope_name}");
                let tmux_bin_str = self.tmux_bin.to_string_lossy().into_owned();
                let argv: Vec<String> = [
                    "--user",
                    "--scope",
                    unit.as_str(),
                    "--collect",
                    "--quiet",
                    tmux_bin_str.as_str(),
                    "new-session",
                    "-d",
                    "-s",
                    session.as_str(),
                    "-c",
                    worktree_str.as_str(),
                    "bash",
                    launcher_str.as_str(),
                ]
                .iter()
                .map(|a| (*a).to_string())
                .collect();
                self.runner
                    .run("systemd-run", &argv, RunOpts::default())
                    .await
                    .map_err(|e| {
                        BackendError::Recoverable(format!("systemd-run failed to launch: {e}"))
                    })?
            }
        };
        if launch_out.status != 0 {
            return Err(BackendError::Recoverable(format!(
                "tmux new-session failed (status {}): {}",
                launch_out.status,
                launch_out.stderr.trim()
            ))
            .into());
        }

        // §4.5 step 4: probe the new pane's id and pid.
        let probe = self
            .tmux(
                &[
                    "display-message",
                    "-p",
                    "-t",
                    address.as_str(),
                    "#{pane_id} #{pane_pid}",
                ],
                RunOpts::default(),
            )
            .await?;
        let (pane_id, pid) = parse_pane_info(&probe.stdout)?;

        // §4.5 step 5: when `initial_prompt` is set, `wait_for_ready` +
        // `send_prompt` are wired in here in Task 3 (buffer-path injection).

        Ok(AgentHandle {
            agent_id: req.agent_id.clone(),
            backend: BackendKind::Tmux,
            address,
            pane_id: Some(pane_id),
            pid,
            session_name: session,
            spawned_at: SystemTime::now(),
        })
    }
}

/// Build the launcher script body (design §4.5): shebang, `cd` into the
/// worktree, exported env, then `exec` the CLI. Values are single-quoted so a
/// path or env value with shell metacharacters cannot break out, and keys are
/// validated as shell identifiers so a key cannot inject (or corrupt the script).
///
/// # Errors
/// [`BackendError::Invariant`] when an `env_overrides` key is not a shell
/// identifier (`[A-Za-z_][A-Za-z0-9_]*`).
fn build_launcher_script(req: &SpawnRequest) -> Result<String, BackendError> {
    let mut script = String::from("#!/usr/bin/env bash\nset -euo pipefail\n");
    script.push_str("cd ");
    script.push_str(&sh_quote(&req.worktree.to_string_lossy()));
    script.push('\n');
    if let Some(mxid) = &req.mxid {
        script.push_str("export AGENTD_MXID=");
        script.push_str(&sh_quote(mxid));
        script.push('\n');
    }
    // Sort env keys so the launcher (and its tests) are deterministic.
    let mut envs: Vec<(&String, &String)> = req.env_overrides.iter().collect();
    envs.sort_by(|a, b| a.0.cmp(b.0));
    for (key, value) in envs {
        if !is_shell_identifier(key) {
            return Err(BackendError::Invariant(format!(
                "env override key {key:?} is not a shell identifier"
            )));
        }
        script.push_str("export ");
        script.push_str(key);
        script.push('=');
        script.push_str(&sh_quote(value));
        script.push('\n');
    }
    script.push_str("exec ");
    script.push_str(cli_command(req.cli));
    script.push('\n');
    Ok(script)
}

/// True when `key` is a POSIX shell identifier: a non-empty `[A-Za-z_]` head
/// followed by `[A-Za-z0-9_]`.
fn is_shell_identifier(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The CLI binary launched per [`CliKind`] (v0 defaults; configurable later).
fn cli_command(cli: CliKind) -> &'static str {
    match cli {
        CliKind::ClaudeCode => "claude",
        CliKind::Codex => "codex",
    }
}

/// POSIX single-quote a value: wrap in `'…'`, rewriting any `'` as `'\''`.
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Parse `display-message`'s `"#{pane_id} #{pane_pid}"` output. The first
/// whitespace token is the pane id (required); the second, if numeric, is the
/// pid. Output with no pane id is an [`BackendError::Invariant`].
fn parse_pane_info(stdout: &str) -> Result<(String, Option<u32>), BackendError> {
    let line = stdout.trim();
    let mut parts = line.split_whitespace();
    let pane_id = parts
        .next()
        .ok_or_else(|| {
            BackendError::Invariant(format!(
                "display-message returned no pane_id (output: {line:?})"
            ))
        })?
        .to_string();
    let pid = parts.next().and_then(|tok| tok.parse::<u32>().ok());
    Ok((pane_id, pid))
}

/// Write `contents` to `path`, mapping an IO failure to a recoverable error.
fn write_file(path: &Path, contents: &str) -> Result<(), BackendError> {
    std::fs::write(path, contents)
        .map_err(|e| BackendError::Recoverable(format!("failed to write {}: {e}", path.display())))
}

/// Append `.agentd-launcher-*.sh` to the worktree `.gitignore` if not already
/// present (idempotent). A missing `.gitignore` is created.
fn amend_gitignore(worktree: &Path) -> Result<(), BackendError> {
    const PATTERN: &str = ".agentd-launcher-*.sh";
    let path = worktree.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == PATTERN) {
        return Ok(());
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(PATTERN);
    content.push('\n');
    write_file(&path, &content)
}
