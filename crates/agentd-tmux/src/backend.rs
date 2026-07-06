//! `TmuxBackend` (design §4.1–§4.5): the real agent-spawning backend. It holds
//! an injected `Arc<dyn CommandRunner>` (P0.3 D3) so every flow is testable
//! against a `FakeRunner`. The agentd-core `AgentBackend` trait stays spawn-only
//! (D1); the other capabilities (§4.6–§4.10) are inherent methods landed across
//! P0.3 Tasks 2–5.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use agentd_core::CoreError;
use agentd_core::ports::{AgentBackend, CommandOutput, CommandRunner, RunOpts};
use agentd_core::types::{
    AgentHandle, AgentId, AgentStatus, BackendKind, CliKind, LaunchStrategy, SpawnRequest,
};

use crate::config::Config;
use crate::error::BackendError;

const AGENTD_MCP_STDIO_CMD_ENV: &str = "AGENTD_MCP_STDIO_CMD";
const AGENTD_CLAUDE_AUTO_TRUST_WORKSPACE_ENV: &str = "AGENTD_CLAUDE_AUTO_TRUST_WORKSPACE";
const LAUNCHER_GITIGNORE_PATTERN: &str = ".agentd-launcher-*.sh";
const MCP_CONFIG_GITIGNORE_PATTERN: &str = ".agentd-mcp-*.json";

/// Options for [`TmuxBackend::capture`] (design §4.8).
#[derive(Debug, Clone, Copy)]
pub struct CaptureOpts {
    /// Lines of scrollback to include; the pane is captured from `-<lines>`.
    pub lines: u32,
    /// Include ansi escape sequences (`-e`).
    pub ansi: bool,
}

/// How a [`TmuxBackend::shutdown`] actually ended the agent (design §4.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownMethod {
    /// The agent left on its own after a graceful `/exit`.
    Graceful,
    /// Two interrupts (C-c) brought it down.
    Interrupt,
    /// The session was killed outright.
    Kill,
}

/// Options for [`TmuxBackend::shutdown`].
#[derive(Debug, Clone)]
pub struct ShutdownOpts {
    /// Where the pre-kill transcript archive is written.
    pub archive_to: PathBuf,
}

/// The outcome of a [`TmuxBackend::shutdown`] (design §4.9).
#[derive(Debug, Clone)]
pub struct ShutdownReport {
    /// Which escalation step ended the agent.
    pub method: ShutdownMethod,
    /// SHA-256 of the archived transcript, recorded before any kill.
    pub final_capture_sha: String,
}

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

    /// Deliver `prompt` to the agent through tmux's paste buffer (design §4.6).
    /// The payload goes into the buffer — argv for prompts up to 64 KiB, stdin
    /// otherwise — and is pasted; it is NEVER typed as keystrokes (the final
    /// keystroke is a bare Enter, never a literal-key send of the payload).
    ///
    /// # Errors
    /// [`BackendError`] when any of the four tmux stages fails to run.
    pub async fn send_prompt(
        &self,
        handle: &AgentHandle,
        prompt: &str,
    ) -> Result<(), BackendError> {
        const MAX_ARGV_BYTES: usize = 64 * 1024;
        let via_stdin = prompt.len() > MAX_ARGV_BYTES;

        // Stage 1: load the paste buffer.
        if via_stdin {
            let opts = RunOpts {
                stdin: Some(prompt.as_bytes().to_vec()),
                ..RunOpts::default()
            };
            self.tmux(&["set-buffer", "-"], opts).await?;
        } else {
            self.tmux(&["set-buffer", prompt], RunOpts::default())
                .await?;
        }

        // Stage 2: bracketed paste into the pane, deleting the buffer afterwards.
        self.tmux(
            &["paste-buffer", "-p", "-t", handle.address.as_str(), "-d"],
            RunOpts::default(),
        )
        .await?;

        // Stage 3: let the paste settle before the newline.
        tokio::time::sleep(self.cfg.inject_delay).await;

        // Stage 4: a single bare Enter — the payload is never typed as keys.
        self.tmux(
            &["send-keys", "-t", handle.address.as_str(), "Enter"],
            RunOpts::default(),
        )
        .await?;
        Ok(())
    }

    /// Block until the CLI's main prompt is visible (design §4.7), polling the
    /// pane with exponential backoff (`ready_probe_initial` doubling to
    /// `ready_probe_max`) until `ready_deadline`.
    ///
    /// # Errors
    /// [`BackendError::Recoverable`] if the deadline passes before the main
    /// prompt appears.
    pub async fn wait_for_ready(
        &self,
        handle: &AgentHandle,
        cli: CliKind,
    ) -> Result<(), BackendError> {
        self.wait_for_ready_inner(handle, cli, false).await
    }

    async fn wait_for_ready_inner(
        &self,
        handle: &AgentHandle,
        cli: CliKind,
        auto_trust_workspace: bool,
    ) -> Result<(), BackendError> {
        let deadline = Instant::now() + self.cfg.ready_deadline;
        let mut probe = self.cfg.ready_probe_initial;
        let mut trust_confirmed = false;
        while Instant::now() < deadline {
            let buf = self
                .capture_pane(handle.address.as_str(), 50, false)
                .await?;
            if self.cfg.main_prompt_visible(&buf, cli) {
                return Ok(());
            }
            if auto_trust_workspace
                && !trust_confirmed
                && cli == CliKind::ClaudeCode
                && claude_workspace_trust_prompt_visible(&buf)
            {
                self.tmux(
                    &["send-keys", "-t", handle.address.as_str(), "Enter"],
                    RunOpts::default(),
                )
                .await?;
                trust_confirmed = true;
            }
            tokio::time::sleep(probe).await;
            probe = (probe * 2).min(self.cfg.ready_probe_max);
        }
        Err(BackendError::Recoverable(format!(
            "agent did not reach its main prompt within {:?}",
            self.cfg.ready_deadline
        )))
    }

    /// Capture the pane's visible buffer plus `lines` of scrollback (design
    /// §4.8). `ansi` includes escape sequences (`-e`). The public `capture`
    /// surface (with `CaptureOpts`) is layered on this in Task 4.
    async fn capture_pane(
        &self,
        address: &str,
        lines: u32,
        ansi: bool,
    ) -> Result<String, BackendError> {
        let start = format!("-{lines}");
        let mut args = vec!["capture-pane", "-p", "-t", address];
        if ansi {
            args.push("-e");
        }
        args.push("-S");
        args.push(start.as_str());
        let out = self.tmux(&args, RunOpts::default()).await?;
        Ok(out.stdout)
    }

    /// Capture a pane's buffer (design §4.8). The public surface over
    /// [`Self::capture_pane`]; gets its first trait-level caller in a later phase.
    ///
    /// # Errors
    /// [`BackendError`] when the capture command fails to run.
    pub async fn capture(
        &self,
        handle: &AgentHandle,
        opts: CaptureOpts,
    ) -> Result<String, BackendError> {
        self.capture_pane(handle.address.as_str(), opts.lines, opts.ansi)
            .await
    }

    /// Detect the agent's status (design §4.8): read `pane_current_command`,
    /// and when a CLI is running, diff two captures over `status_diff_gap` to
    /// tell Idle from Busy.
    ///
    /// # Errors
    /// [`BackendError`] when a probe command fails to run.
    pub async fn status(&self, handle: &AgentHandle) -> Result<AgentStatus, BackendError> {
        let addr = handle.address.as_str();

        // Step 1: what is the pane running? No addressable pane → Gone.
        let probe = self
            .tmux(
                &[
                    "display-message",
                    "-p",
                    "-t",
                    addr,
                    "#{pane_current_command}",
                ],
                RunOpts::default(),
            )
            .await?;
        let cmd = probe.stdout.trim();
        if probe.status != 0 || cmd.is_empty() {
            return Ok(AgentStatus::Gone);
        }

        // Step 2: a shell means the CLI is still booting (Starting) or has
        // exited back to a shell that shows prior output (Gone).
        if matches!(cmd.trim_start_matches('-'), "bash" | "zsh" | "sh") {
            let buf = self.capture_pane(addr, 50, false).await?;
            return Ok(if buf.trim().is_empty() {
                AgentStatus::Starting
            } else {
                AgentStatus::Gone
            });
        }

        // Step 3: the CLI is running — diff two captures over the settle gap.
        let first = self.capture_pane(addr, 50, false).await?;
        tokio::time::sleep(self.cfg.status_diff_gap).await;
        let second = self.capture_pane(addr, 50, false).await?;
        if first == second {
            Ok(AgentStatus::Idle {
                last_output_age: self.cfg.status_diff_gap,
            })
        } else {
            Ok(AgentStatus::Busy {
                last_output_age: Duration::ZERO,
            })
        }
    }

    /// Tear an agent down (design §4.9): archive the transcript BEFORE any kill,
    /// then escalate graceful `/exit` → interrupt (C-c x2) → `kill-session`.
    ///
    /// # Errors
    /// [`BackendError::Recoverable`] when the session is already gone;
    /// [`BackendError`] when a tmux stage or the archive write fails.
    pub async fn shutdown(
        &self,
        handle: &AgentHandle,
        opts: ShutdownOpts,
    ) -> Result<ShutdownReport, BackendError> {
        const ARCHIVE_LINES: u32 = 5000;
        let session = handle.session_name.as_str();
        let addr = handle.address.as_str();

        // A missing session has nothing to tear down (case 5).
        if !self.session_alive(session).await? {
            return Err(BackendError::Recoverable(format!(
                "session {session} is already gone; nothing to shut down"
            )));
        }

        // Archive the transcript BEFORE any kill action (case 7).
        let transcript = self.capture_pane(addr, ARCHIVE_LINES, false).await?;
        write_file(&opts.archive_to, &transcript)?;
        let final_capture_sha = sha256_hex(transcript.as_bytes());

        // Escalation 1: graceful `/exit`, then wait and re-probe.
        self.send_prompt(handle, "/exit").await?;
        tokio::time::sleep(self.cfg.graceful_timeout).await;
        if !self.session_alive(session).await? {
            return Ok(ShutdownReport {
                method: ShutdownMethod::Graceful,
                final_capture_sha,
            });
        }

        // Escalation 2: two interrupts (named C-c key, never the -l flag).
        self.tmux(&["send-keys", "-t", addr, "C-c"], RunOpts::default())
            .await?;
        self.tmux(&["send-keys", "-t", addr, "C-c"], RunOpts::default())
            .await?;
        tokio::time::sleep(self.cfg.sigint_settle).await;
        if !self.session_alive(session).await? {
            return Ok(ShutdownReport {
                method: ShutdownMethod::Interrupt,
                final_capture_sha,
            });
        }

        // Escalation 3: kill the session outright.
        self.tmux(&["kill-session", "-t", session], RunOpts::default())
            .await?;
        Ok(ShutdownReport {
            method: ShutdownMethod::Kill,
            final_capture_sha,
        })
    }

    /// Re-attach to a surviving session on daemon restart (design §4.10).
    /// Returns `Ok(None)` when `target` no longer exists, else a rebuilt handle.
    ///
    /// # Errors
    /// [`BackendError`] when the pane re-probe fails to run or parse.
    pub async fn rebind(&self, target: &str) -> Result<Option<AgentHandle>, BackendError> {
        if !self.session_alive(target).await? {
            return Ok(None);
        }
        let address = format!("{target}:0.0");
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
        let agent_id = target.strip_prefix("agentd-").unwrap_or(target);
        Ok(Some(AgentHandle {
            agent_id: AgentId::parsed(agent_id),
            backend: BackendKind::Tmux,
            address,
            pane_id: Some(pane_id),
            pid,
            // Approximate; the DB holds the authoritative start time (§4.10).
            session_name: target.to_string(),
            spawned_at: SystemTime::now(),
        }))
    }

    /// True when `tmux has-session -t <session>` exits zero.
    async fn session_alive(&self, session: &str) -> Result<bool, BackendError> {
        let probe = self
            .tmux(&["has-session", "-t", session], RunOpts::default())
            .await?;
        Ok(probe.status == 0)
    }
}

#[async_trait::async_trait]
impl AgentBackend for TmuxBackend {
    /// Spawn an agent into a fresh tmux session (design §4.5). Maps the internal
    /// [`BackendError`] to [`CoreError::Backend`] at the trait boundary (D2).
    async fn spawn(&self, req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        let session = format!("agentd-{}", req.agent_id.as_str());
        let address = format!("{session}:0.0");

        // §4.5 step 1: liveness probe FIRST. A live session means the caller
        // should rebind, not spawn a second one. We return before writing any
        // launcher or running new-session.
        if self.session_alive(session.as_str()).await? {
            return Err(BackendError::Recoverable(format!(
                "session {session} already exists; rebind instead of spawning"
            ))
            .into());
        }

        // §4.5 step 2: write the launcher into the worktree, then exclude it.
        let launcher_path = req
            .worktree
            .join(format!(".agentd-launcher-{}.sh", req.agent_id.as_str()));
        let mcp_config = build_claude_mcp_config(&req)?;
        let launcher =
            build_launcher_script(&req, mcp_config.as_ref().map(|config| config.0.as_path()))?;
        if let Some((path, contents)) = &mcp_config {
            write_file(path, contents)?;
        }
        write_file(&launcher_path, &launcher)?;
        amend_gitignore(&req.worktree, mcp_config.is_some())?;

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

        let handle = AgentHandle {
            agent_id: req.agent_id.clone(),
            backend: BackendKind::Tmux,
            address,
            pane_id: Some(pane_id),
            pid,
            session_name: session,
            spawned_at: SystemTime::now(),
        };

        // §4.5 step 5: deliver the initial prompt once the agent is ready.
        if let Some(prompt) = &req.initial_prompt {
            self.wait_for_ready_inner(&handle, req.cli, auto_trust_workspace_enabled(&req))
                .await?;
            self.send_prompt(&handle, prompt).await?;
        }

        Ok(handle)
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
fn build_launcher_script(
    req: &SpawnRequest,
    mcp_config_path: Option<&Path>,
) -> Result<String, BackendError> {
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
    push_cli_exec(&mut script, req.cli, mcp_config_path);
    script.push('\n');
    Ok(script)
}

fn build_claude_mcp_config(req: &SpawnRequest) -> Result<Option<(PathBuf, String)>, BackendError> {
    let Some(command) = req.env_overrides.get(AGENTD_MCP_STDIO_CMD_ENV) else {
        return Ok(None);
    };
    if req.cli != CliKind::ClaudeCode {
        return Ok(None);
    }
    if command.trim().is_empty() {
        return Err(BackendError::Invariant(format!(
            "{AGENTD_MCP_STDIO_CMD_ENV} cannot be empty"
        )));
    }

    let path = req
        .worktree
        .join(format!(".agentd-mcp-{}.json", req.agent_id.as_str()));
    let config = serde_json::json!({
        "mcpServers": {
            "agentd": {
                "type": "stdio",
                "command": "sh",
                "args": ["-lc", command],
            }
        }
    });
    let contents = serde_json::to_string_pretty(&config)
        .map_err(|e| BackendError::Invariant(format!("failed to encode Claude MCP config: {e}")))?;
    Ok(Some((path, format!("{contents}\n"))))
}

fn auto_trust_workspace_enabled(req: &SpawnRequest) -> bool {
    req.env_overrides
        .get(AGENTD_CLAUDE_AUTO_TRUST_WORKSPACE_ENV)
        .is_some_and(|value| env_truthy(value))
        || std::env::var(AGENTD_CLAUDE_AUTO_TRUST_WORKSPACE_ENV)
            .is_ok_and(|value| env_truthy(&value))
}

fn env_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn claude_workspace_trust_prompt_visible(buffer: &str) -> bool {
    buffer.contains("Quick safety check")
        && buffer.contains("Yes, I trust this folder")
        && buffer.contains("Enter to confirm")
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

/// Push the CLI command line launched per [`CliKind`] (v0 defaults; configurable later).
fn push_cli_exec(script: &mut String, cli: CliKind, mcp_config_path: Option<&Path>) {
    match (cli, mcp_config_path) {
        (CliKind::ClaudeCode, Some(path)) => {
            script.push_str("claude --mcp-config ");
            script.push_str(&sh_quote(&path.to_string_lossy()));
            script.push_str(" --strict-mcp-config");
        }
        (CliKind::ClaudeCode, None) => script.push_str("claude"),
        (CliKind::Codex, _) => script.push_str("codex"),
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

/// Lowercase hex SHA-256 of `bytes` (the shutdown transcript fingerprint).
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Write `contents` to `path`, mapping an IO failure to a recoverable error.
fn write_file(path: &Path, contents: &str) -> Result<(), BackendError> {
    std::fs::write(path, contents)
        .map_err(|e| BackendError::Recoverable(format!("failed to write {}: {e}", path.display())))
}

/// Append `.agentd-launcher-*.sh` to the worktree `.gitignore` if not already
/// present (idempotent). A missing `.gitignore` is created.
fn amend_gitignore(worktree: &Path, include_mcp_config: bool) -> Result<(), BackendError> {
    let path = worktree.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut content = existing;
    for pattern in [LAUNCHER_GITIGNORE_PATTERN, MCP_CONFIG_GITIGNORE_PATTERN] {
        if pattern == MCP_CONFIG_GITIGNORE_PATTERN && !include_mcp_config {
            continue;
        }
        if content.lines().any(|line| line.trim() == pattern) {
            continue;
        }
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(pattern);
        content.push('\n');
    }
    write_file(&path, &content)
}
