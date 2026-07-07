//! `SystemClock` — the production [`Clock`] (the daemon's real time source).
//! Tests use `agentd_core::test_support::FixedClock`; this is the ~5-line
//! production impl against the pub `Clock` trait (no agentd-core change, D1).

use std::time::{SystemTime, UNIX_EPOCH};

use agentd_core::ports::Clock;

/// The wall-clock time source for the running daemon.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_secs()).ok())
            .unwrap_or(0)
    }
}
