//! The injected clock the cadence engine reads time from.
//!
//! Spec (#255, Testing Decisions): "Cadence logic takes an injected
//! clock so quiescence and debounce are simulated, never slept." A
//! 30-minute quiescence window is not something a test suite can afford
//! to wait out, and a test that *does* sleep buys flakiness with its
//! wall-clock time. So every time the engine asks what time it is, it
//! asks this — [`SystemClock`] in production, [`TestClock`] in tests,
//! where "advance 31 minutes" is one function call.

use std::sync::Mutex;

use chrono::{DateTime, TimeDelta, Utc};

/// Wall-clock source for the cadence engine.
pub trait Clock: Send + Sync + std::fmt::Debug + 'static {
    fn now(&self) -> DateTime<Utc>;
}

/// The real clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock that only moves when a test moves it.
#[derive(Debug)]
pub struct TestClock {
    now: Mutex<DateTime<Utc>>,
}

impl TestClock {
    /// Start at `start`.
    #[must_use]
    pub fn new(start: DateTime<Utc>) -> Self {
        Self {
            now: Mutex::new(start),
        }
    }

    /// Move time forward by `delta`.
    pub fn advance(&self, delta: TimeDelta) {
        let mut now = self.now.lock().expect("test clock poisoned");
        *now += delta;
    }

    /// Move time forward by `minutes`.
    pub fn advance_minutes(&self, minutes: i64) {
        self.advance(TimeDelta::minutes(minutes));
    }
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new(DateTime::UNIX_EPOCH)
    }
}

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().expect("test clock poisoned")
    }
}
