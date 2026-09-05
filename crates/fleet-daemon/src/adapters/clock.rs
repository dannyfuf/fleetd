//! Injectable wall-clock access.

use chrono::{DateTime, Utc};

/// Wall-clock access used by persistence and job metadata.
pub trait Clock: Send + Sync {
    /// Returns the current UTC instant.
    fn now(&self) -> DateTime<Utc>;

    /// Returns the current Unix epoch timestamp in milliseconds.
    fn epoch_millis(&self) -> i64 {
        self.now().timestamp_millis()
    }
}

/// The system wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
