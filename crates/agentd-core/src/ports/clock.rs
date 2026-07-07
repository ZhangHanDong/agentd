//! The time seam. Injecting a clock keeps timeout/age logic deterministic in
//! tests (`FixedClock`) while production uses the system clock. Sync — no
//! `#[async_trait]` needed.

/// A source of the current unix time (seconds).
pub trait Clock: Send + Sync {
    /// Current unix time in seconds.
    fn now_unix(&self) -> i64;
}
