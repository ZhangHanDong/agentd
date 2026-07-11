//! Backend configuration (design §4.6/§4.7): per-CLI readiness patterns and the
//! tunable delays. Held in memory in v0 — TOML loading is deferred until a
//! daemon consumer exists. All durations are public fields so tests construct a
//! zero-delay `Config`.

use std::time::Duration;

use agentd_core::types::CliKind;

/// Per-CLI substrings that mark a CLI's main prompt as visible (design §4.7).
/// Overridable because a CLI upgrade can change its startup banner.
#[derive(Debug, Clone)]
pub struct ReadyPatterns {
    pub claude_code: Vec<String>,
    pub codex: Vec<String>,
}

impl Default for ReadyPatterns {
    fn default() -> Self {
        // The claude-code / codex TUIs both render this hint on the idle prompt.
        // These are defaults, not law — operators override per §4.7.
        Self {
            claude_code: vec!["? for shortcuts".to_string(), "auto mode on".to_string()],
            codex: vec!["? for shortcuts".to_string(), "\u{203a} ".to_string()],
        }
    }
}

impl ReadyPatterns {
    /// The ready patterns for `cli`.
    #[must_use]
    pub fn for_cli(&self, cli: CliKind) -> &[String] {
        match cli {
            CliKind::ClaudeCode => &self.claude_code,
            CliKind::Codex => &self.codex,
        }
    }
}

/// Backend timing + readiness configuration. Constructed in memory in v0.
#[derive(Debug, Clone)]
pub struct Config {
    pub ready_patterns: ReadyPatterns,
    /// Settle delay after a bracketed paste, before sending Enter (§4.6).
    pub inject_delay: Duration,
    /// Total budget for `wait_for_ready` (§4.7).
    pub ready_deadline: Duration,
    /// First `wait_for_ready` probe interval; doubles up to `ready_probe_max`.
    pub ready_probe_initial: Duration,
    /// Ceiling for the `wait_for_ready` backoff interval.
    pub ready_probe_max: Duration,
    /// Gap between the two status captures whose diff means Busy vs Idle (§4.8).
    pub status_diff_gap: Duration,
    /// How long `shutdown` waits for a graceful `/exit` (§4.9).
    pub graceful_timeout: Duration,
    /// How long `shutdown` waits after C-c x2 before `kill-session` (§4.9).
    pub sigint_settle: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ready_patterns: ReadyPatterns::default(),
            inject_delay: Duration::from_millis(60),
            ready_deadline: Duration::from_secs(45),
            ready_probe_initial: Duration::from_millis(300),
            ready_probe_max: Duration::from_secs(2),
            status_diff_gap: Duration::from_millis(800),
            graceful_timeout: Duration::from_secs(8),
            sigint_settle: Duration::from_secs(3),
        }
    }
}

impl Config {
    /// True when any of `cli`'s ready patterns is a substring of `buffer` (§4.7).
    #[must_use]
    pub fn main_prompt_visible(&self, buffer: &str, cli: CliKind) -> bool {
        self.ready_patterns
            .for_cli(cli)
            .iter()
            .any(|pat| buffer.contains(pat.as_str()))
    }
}
