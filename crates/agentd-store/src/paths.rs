//! Filesystem paths for agentd's local state (design §3.2 — `~/.agentd`).

use std::path::PathBuf;

/// The agentd home directory. Honors `AGENTD_HOME` if set, else `~/.agentd`,
/// falling back to a relative `.agentd` when no home directory is resolvable.
#[must_use]
pub fn agentd_home() -> PathBuf {
    if let Ok(dir) = std::env::var("AGENTD_HOME") {
        return PathBuf::from(dir);
    }
    directories::BaseDirs::new().map_or_else(
        || PathBuf::from(".agentd"),
        |b| b.home_dir().join(".agentd"),
    )
}

/// Default database path (`<agentd_home>/agentd.db`).
#[must_use]
pub fn default_db_path() -> PathBuf {
    agentd_home().join("agentd.db")
}
