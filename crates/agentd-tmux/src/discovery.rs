//! tmux binary discovery (design §4.4). [`resolve_tmux_bin`] is a pure core that
//! takes its environment as parameters so it is deterministic in tests;
//! [`tmux_bin`] wraps it with the real env, the standard candidates, and a
//! `PATH` scan.

use std::path::{Path, PathBuf};

use crate::error::BackendError;

/// Standard install locations probed when `AGENTD_TMUX_BIN` is unset, in
/// priority order (design §4.4).
const CANDIDATES: &[&str] = &[
    "/opt/homebrew/bin/tmux",
    "/usr/local/bin/tmux",
    "/usr/bin/tmux",
];

/// Resolve the tmux binary path. Pure core: the caller injects the environment.
///
/// Order (§4.4): `env_override` is returned verbatim with **no** existence
/// check (the operator is trusted) > the first `candidates` entry for which
/// `exists` is true > `which()`. When nothing is found this is
/// [`BackendError::Fatal`] with an install hint.
///
/// # Errors
/// [`BackendError::Fatal`] when no tmux binary can be located.
pub fn resolve_tmux_bin(
    env_override: Option<PathBuf>,
    candidates: &[PathBuf],
    exists: impl Fn(&Path) -> bool,
    which: impl FnOnce() -> Option<PathBuf>,
) -> Result<PathBuf, BackendError> {
    if let Some(path) = env_override {
        return Ok(path);
    }
    for candidate in candidates {
        if exists(candidate) {
            return Ok(candidate.clone());
        }
    }
    if let Some(found) = which() {
        return Ok(found);
    }
    Err(BackendError::Fatal(
        "tmux not found. Install tmux or set AGENTD_TMUX_BIN".to_string(),
    ))
}

/// Production entry point: reads `AGENTD_TMUX_BIN`, probes [`CANDIDATES`], then
/// falls back to a `PATH` scan for `tmux`.
///
/// # Errors
/// [`BackendError::Fatal`] when no tmux binary can be located.
pub fn tmux_bin() -> Result<PathBuf, BackendError> {
    let env_override = std::env::var_os("AGENTD_TMUX_BIN").map(PathBuf::from);
    let candidates: Vec<PathBuf> = CANDIDATES.iter().map(|s| PathBuf::from(*s)).collect();
    resolve_tmux_bin(env_override, &candidates, Path::exists, which_tmux)
}

/// Scan `PATH` for an executable named `tmux`.
fn which_tmux() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("tmux"))
        .find(|candidate| candidate.is_file())
}
