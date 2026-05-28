//! A [`Clock`] whose time is settable, for deterministic timeout/age tests.

use std::sync::atomic::{AtomicI64, Ordering};

use crate::ports::Clock;

/// A clock that returns a fixed, settable unix time.
#[derive(Debug)]
pub struct FixedClock {
    now: AtomicI64,
}

impl FixedClock {
    #[must_use]
    pub fn new(now_unix: i64) -> Self {
        Self {
            now: AtomicI64::new(now_unix),
        }
    }

    /// Advance/reset the clock to `now_unix`.
    pub fn set(&self, now_unix: i64) {
        self.now.store(now_unix, Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now_unix(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }
}
